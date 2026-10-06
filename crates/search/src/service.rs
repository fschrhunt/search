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
    store: Arc<Store>,
    config: Config,
}

impl Search {
    /// Open the engine and its local index from the supplied configuration.
    pub fn open(config: Config) -> Result<Self, SearchError> {
        let store = Arc::new(
            Store::open(&config.data_dir, config.index.clone()).map_err(SearchError::Store)?,
        );
        let fetcher = Fetcher::new(config.fetch.clone(), Arc::clone(&store), &config.user_agent)
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
    /// The corpus lookup and the provider fan-out run concurrently, so consulting
    /// the index costs no wall-clock time: whichever finishes first contributes
    /// what it has. When `use_index` is off, this is the plain fan-out.
    pub async fn search(&self, query: Query) -> Response {
        if !self.config.search.should_use_index() {
            return self.registry.search(query).await;
        }
        let result_limit = self.config.search.result_limit(query.limit);
        let local_limit = result_limit;
        let mut query = query;
        query.limit = result_limit;
        // SQLite is synchronous, so do its bounded FTS lookup on the blocking
        // pool while provider requests are in flight.
        let store = Arc::clone(&self.store);
        let text = query.text.clone();
        let local_lookup = tokio::task::spawn_blocking(move || store.search(&text, local_limit));
        let (mut response, local) = tokio::join!(self.registry.search(query), local_lookup);
        let local = local.ok().and_then(Result::ok).unwrap_or_default();
        discovery::blend(
            &mut response,
            &local,
            self.config.search.index_weight,
            result_limit,
        );
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
        self.store.search(query, limit)
    }

    /// Re-fetch the seeded hosts' stale documents, so a corpus a user has chosen
    /// to keep fresh stays true. Returns how many were refreshed. A no-op when
    /// no hosts are configured — nothing is fetched but what a caller asks for.
    pub async fn refresh_seeded(&self) -> usize {
        let hosts = &self.config.index.refresh_hosts;
        if hosts.is_empty() {
            return 0;
        }
        let stale = self
            .store
            .stale_seeded(hosts, self.config.index.refresh_after());
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
        self.store.stats()
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
    Store(StoreError),
    Client(reqwest::Error),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Store(error) => write!(f, "open index: {error}"),
            SearchError::Client(error) => write!(f, "build guarded HTTP client: {error}"),
        }
    }
}

impl std::error::Error for SearchError {}
