//! The in-process search engine over live engines and guarded fetching.

use crate::core::config::Config;
use crate::core::fetch::{Fetcher, Page};
use crate::engines::{self, Answer, Query};

/// The configured search engine used in-process by applications and adapters.
pub struct Search {
    pool: engines::Pool,
    fetcher: Fetcher,
    config: Config,
}

impl Search {
    /// Resolve selected packages and build live engines and fetching.
    /// The embedded default requires no filesystem writes.
    pub fn open(config: Config) -> Result<Self, SearchError> {
        let pool = engines::Pool::new(&config)
            .map_err(|error| SearchError::Settings(error.to_string()))?;
        let fetcher =
            Fetcher::new(config.fetch.clone(), &config.user_agent).map_err(SearchError::Client)?;
        Ok(Search {
            pool,
            fetcher,
            config,
        })
    }

    /// Discover live results across the selected engines.
    pub async fn search(&self, query: Query) -> Answer {
        self.pool.search(query).await
    }

    /// Fetch one or more URLs, preserving order and caching recent answers in memory.
    pub async fn fetch(&self, urls: &[String]) -> Vec<Page> {
        self.fetcher.fetch_many(urls).await
    }

    /// The enabled engine names, for status output.
    pub fn engine_names(&self) -> Vec<String> {
        self.pool.names()
    }

    /// The running configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configured deadline for a complete engine query.
    pub fn overall_timeout(&self) -> std::time::Duration {
        self.pool.overall_timeout()
    }
}

/// Why the search engine could not start.
#[derive(Debug)]
pub enum SearchError {
    Settings(String),
    Client(reqwest::Error),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::Settings(error) => write!(f, "settings: {error}"),
            SearchError::Client(error) => write!(f, "build guarded HTTP client: {error}"),
        }
    }
}

impl std::error::Error for SearchError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening the default engines must not create anything on disk.
    #[test]
    fn default_open_leaves_disk_untouched() {
        let dir = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-diskless-{}", uuid::Uuid::new_v4()));
        let config = Config {
            home: dir.join("home"),
            ..Default::default()
        };
        let service = Search::open(config).unwrap();
        assert_eq!(service.engine_names(), [crate::engines::DEFAULT_ENGINE]);
        assert!(!dir.exists());
    }
    /// Missing selected packages fail without creating the package home.
    #[test]
    fn invalid_selected_package_does_not_create_home() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-missing-{}", uuid::Uuid::new_v4()));
        let mut config = Config {
            home: root.join("home"),
            ..Default::default()
        };
        config.engines.use_engines = vec!["missing-fixture".into()];
        let error = Search::open(config).err().unwrap().to_string();
        assert!(error.contains("missing-fixture"));
        assert!(!root.exists());
    }

    /// Search uses trusted package assets and overrides, retaining leases only while running.
    #[tokio::test]
    async fn installed_package_runs_from_its_root() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-installed-{}", uuid::Uuid::new_v4()));
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
            "adapter":{"type":"command","command": if cfg!(windows) { "python.exe" } else { "python3" },"args":["runner.py"],"config":{"title":"default"}},
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
        let installed = crate::engines::install_local(&home, &source).unwrap();
        let mut config = Config {
            home: home.clone(),
            ..Default::default()
        };
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
        drop(installed);
        let service = Search::open(config.clone()).unwrap();
        assert_eq!(service.config().engines.config, config.engines.config);
        let response = service
            .search(Query {
                text: "query".into(),
                ..Default::default()
            })
            .await;
        assert_eq!(response.engines[0].status, engines::EngineStatus::Ok);
        assert_eq!(response.results[0].title, "configured");
        assert_eq!(response.results[0].engines, ["fixture"]);
        assert!(crate::engines::remove(&home, "fixture").is_err());
        drop(service);
        // Parallel process tests can briefly inherit the lease between fork and exec.
        // Keep the release assertion bounded so a real retained lease still fails.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let result = crate::engines::remove(&home, "fixture");
            if result.is_ok() {
                break;
            }
            assert!(
                result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.contains("package is busy"))
                    && std::time::Instant::now() < deadline,
                "lease was not released: {result:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
