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
    /// Resolve selected packages before opening the optional corpus.
    /// Disabled indexing and the embedded default require no filesystem writes.
    pub fn open(mut config: Config) -> Result<Self, SearchError> {
        crate::config::resolve_engines(&mut config)
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
    pub fn provider_names(&self) -> Vec<String> {
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
            home: dir.join("home"),
            dir: dir.join("data"),
            ..Default::default()
        };
        config.index.enabled = false;
        let service = Search::open(config).unwrap();
        assert_eq!(service.provider_names(), [search_engines::DEFAULT_ENGINE]);
        assert!(service.index_search("anything", 1).is_err());
        assert!(service.index_stats().is_err());
        assert_eq!(service.refresh_seeded().await, 0);
        assert!(!dir.exists());
    }
    /// Package resolution must precede database access when local setup is incomplete.
    #[test]
    fn invalid_selected_package_does_not_create_the_corpus() {
        let root = std::env::temp_dir().join(format!("search-missing-{}", uuid::Uuid::new_v4()));
        let mut config = Config {
            home: root.join("home"),
            dir: root.join("data"),
            ..Default::default()
        };
        config.engines.use_engines = vec!["missing-fixture".into()];
        let error = Search::open(config).err().unwrap().to_string();
        assert!(error.contains("missing-fixture"));
        assert!(!root.exists());
    }

    /// Search uses resolved package assets and overrides, discarding injected adapters.
    #[tokio::test]
    async fn installed_package_runs_from_its_root_without_opening_the_index() {
        let root = std::env::temp_dir().join(format!("search-installed-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        let home = root.join("home");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&source).unwrap();
        let manifest = serde_json::json!({
            "schema_version":1,"id":"fixture","version":"1","description":"offline fixture",
            "adapter":{"type":"command","command":"python3","args":["runner.py"],"config":{"title":"default"}},
            "files":["runner.py","asset.txt"]
        });
        std::fs::write(
            source.join("engine.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(source.join("asset.txt"), "package asset").unwrap();
        std::fs::write(
            source.join("runner.py"),
            r#"
import sys,json
request=json.load(sys.stdin)
assert open('asset.txt').read() == 'package asset'
print(json.dumps({'results':[{'title':request['config']['title'],'url':'https://example.com/'}]}))
"#,
        )
        .unwrap();
        let installed = search_engines::install_local(&home, &source).unwrap();
        let mut config = Config {
            home: home.clone(),
            dir: root.join("data"),
            ..Default::default()
        };
        config.index.enabled = false;
        config.search.engine_timeout = 5000;
        config.engines.use_engines = vec!["fixture".into()];
        config.engines.config.insert(
            "fixture".into(),
            serde_json::json!({"config":{"title":"configured"}}),
        );
        config
            .engines
            .config
            .insert("unselected".into(), serde_json::json!({"type":"invalid"}));
        config.engines.adapters.insert(
            "fixture".into(),
            serde_json::from_value(
                serde_json::json!({"type":"command","command":"must-not-execute"}),
            )
            .unwrap(),
        );
        let service = Search::open(config).unwrap();
        let crate::config::AdapterSettings::Command(adapter) =
            &service.config().engines.adapters["fixture"]
        else {
            panic!("command package")
        };
        assert_eq!(adapter.cwd, installed.path);
        let response = service
            .search(Query {
                text: "query".into(),
                ..Default::default()
            })
            .await;
        assert_eq!(response.providers[0].status, discovery::ProviderStatus::Ok);
        assert_eq!(response.results[0].title, "configured");
        assert_eq!(response.results[0].providers, ["fixture"]);
        assert!(!root.join("data").exists());
        assert!(search_engines::remove(&home, "fixture").is_err());
        drop(service);
        drop(installed);
        search_engines::remove(&home, "fixture").unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
