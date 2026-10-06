//! The in-process search engine over discovery, fetching, and the local index.

use std::sync::Arc;

use crate::config::Config;
use crate::discovery::{self, Query, Response};
use crate::fetch::{Fetched, Fetcher};
use crate::index::{Store, StoreError};

/// The configured search engine used in-process by applications and adapters.
pub struct Search {
    registry: discovery::Registry,
    fetcher: Fetcher,
    store: Option<Arc<Store>>,
    config: Config,
}

impl Search {
    /// Open the engine, touching the database only when the index is enabled.
    pub fn open(config: Config) -> Result<Self, SearchError> {
        config
            .validate()
            .map_err(|error| SearchError::Settings(error.to_string()))?;
        let store = if config.index.enabled {
            Some(Arc::new(
                Store::open(&config.dir, config.index.clone()).map_err(SearchError::Store)?,
            ))
        } else {
            None
        };
        let fetcher = Fetcher::new(
            config.fetch.clone(),
            config.index.should_save_fetched_pages(),
            store.clone(),
            &config.user_agent,
        )
        .map_err(SearchError::Client)?;
        let registry = discovery::Registry::new(&config.engines, config.search.clone());
        Ok(Search {
            registry,
            fetcher,
            store,
            config,
        })
    }

    /// Discover results across providers, blended with the local corpus.
    ///
    /// Corpus lookup and provider fan-out share a deadline. A busy corpus cannot
    /// delay available web results beyond the configured query timeout.
    pub async fn search(&self, query: Query) -> Response {
        if !self.config.index.should_include_in_search() {
            return self.registry.search(query).await;
        }
        let result_limit = self.config.search.result_limit(query.limit);
        let started = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.config.search.timeout();
        let local_limit = result_limit;
        let mut query = query;
        query.limit = result_limit;
        // SQLite is synchronous, so do its bounded FTS lookup on the blocking
        // pool while provider requests are in flight.
        let Some(store) = self.store.clone() else {
            return self.registry.search(query).await;
        };
        let text = query.text.clone();
        let local_lookup = tokio::task::spawn_blocking(move || store.search(&text, local_limit));
        let (mut response, local) = tokio::join!(
            self.registry.search(query),
            tokio::time::timeout_at(deadline, local_lookup)
        );
        let local = local
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .unwrap_or_default();
        discovery::blend(
            &mut response,
            &local,
            self.config.search.local_weight,
            result_limit,
        );
        response.duration_ms = started.elapsed().as_millis() as u64;
        response
    }

    /// Fetch and index one or more URLs, preserving order.
    pub async fn fetch(&self, urls: &[String]) -> Vec<Fetched> {
        self.fetcher.fetch_many(urls).await
    }

    /// Search only what has already been fetched.
    pub fn index_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::index::Hit>, StoreError> {
        self.store
            .as_ref()
            .ok_or_else(StoreError::disabled)?
            .search(query, limit)
    }

    /// Re-fetch the seeded hosts' stale documents, so a corpus a user has chosen
    /// to keep fresh stays true. Returns how many were refreshed. A no-op when
    /// no hosts are configured — nothing is fetched but what a caller asks for.
    pub async fn refresh_seeded(&self) -> usize {
        let Some(store) = &self.store else {
            return 0;
        };
        let hosts = &self.config.index.refresh_hosts;
        if hosts.is_empty() {
            return 0;
        }
        let stale = store.stale_seeded(hosts, self.config.index.refresh_after());
        if stale.is_empty() {
            return 0;
        }
        let results = self.fetcher.fetch_many(&stale).await;
        results.iter().filter(|r| r.error.is_none()).count()
    }

    /// The enabled provider names, for status output.
    pub fn provider_names(&self) -> Vec<&'static str> {
        self.registry.names()
    }

    /// Corpus counts.
    pub fn index_stats(&self) -> Result<crate::index::Stats, StoreError> {
        self.store
            .as_ref()
            .ok_or_else(StoreError::disabled)?
            .stats()
    }

    /// The running configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configured deadline for a complete provider query.
    pub fn overall_timeout(&self) -> std::time::Duration {
        self.registry.overall_timeout()
    }
}

/// Why the search engine could not start.
#[derive(Debug)]
pub enum SearchError {
    Settings(String),
    Store(StoreError),
    Client(reqwest::Error),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Settings(error) => write!(f, "settings: {error}"),
            SearchError::Store(error) => write!(f, "open index: {error}"),
            SearchError::Client(error) => write!(f, "build guarded HTTP client: {error}"),
        }
    }
}

impl std::error::Error for SearchError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disabled corpus must not create or open anything on disk.
    #[tokio::test]
    async fn disabling_the_index_leaves_disk_untouched() {
        let dir = std::env::temp_dir().join(format!("search-disabled-{}", uuid::Uuid::new_v4()));
        let mut config = Config {
            dir: dir.clone(),
            ..Default::default()
        };
        config.index.enabled = false;
        let service = Search::open(config).unwrap();
        assert!(service.index_search("anything", 1).is_err());
        assert!(service.index_stats().is_err());
        assert_eq!(service.refresh_seeded().await, 0);
        assert!(!dir.exists());
    }
}
