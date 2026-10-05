//! The configuration shape and its invariants.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// The default listener: loopback only. Exposing search beyond loopback is a
/// deliberate act, and the token must be set to do it.
pub const DEFAULT_ADDR: &str = "127.0.0.1:8642";

/// The environment variable a token is read from unless the config names another.
pub const DEFAULT_TOKEN_ENV: &str = "SEARCH_TOKEN";

/// Everything the service can be told. Durations are milliseconds in the file,
/// because a number is what an operator can diff and a comment can explain.
///
/// Every optional behaviour is off unless named here, so a default install does
/// the one thing it promises — search — and nothing else.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Listen address, `host:port`.
    pub addr: String,
    /// Directory the index lives in.
    pub data_dir: PathBuf,
    /// A literal token. Prefer `token_env`.
    pub token: Option<String>,
    /// The environment variable holding the token.
    pub token_env: String,
    /// Log verbosity name.
    pub log: LogLevel,
    /// The user agent the fetcher sends.
    pub user_agent: String,
    pub search: SearchSettings,
    pub fetch: FetchSettings,
    pub engines: EngineSettings,
    pub index: IndexSettings,
}

/// Bounds on one discovery query. Milliseconds on disk; durations in memory.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    /// Results returned at most.
    pub max_results: usize,
    /// How long any single provider may take before it is abandoned.
    #[serde(rename = "maxProviderTimeMs")]
    pub max_provider_time_ms: u64,
    /// The ceiling for the whole fan-out.
    #[serde(rename = "overallTimeoutMs")]
    pub overall_timeout_ms: u64,
    /// How long a query answer is reused.
    #[serde(rename = "cacheTtlMs")]
    pub cache_ttl_ms: u64,
    /// Consult the local corpus on every search and blend its hits with the
    /// borrowed ones. On by default: it is the reason the corpus exists, and it
    /// only ever adds local results, never removes remote ones.
    pub use_index: Option<bool>,
    /// Blend weight for a local hit against a borrowed one. Higher favors what
    /// you have already read.
    pub index_weight: f64,
}

/// Bounds on the fetcher.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FetchSettings {
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
    /// The most bytes read from any one response.
    pub max_bytes: u64,
    /// The most redirects followed, each re-checked.
    pub max_redirects: usize,
    #[serde(rename = "cacheTtlMs")]
    pub cache_ttl_ms: u64,
    /// Refuse only for tests and air-gapped mirrors: disabling the guard makes
    /// the fetcher able to reach private addresses.
    pub allow_private: bool,
    /// Whether a fetched page joins the private index. Defaults on.
    pub index_fetched: Option<bool>,
    /// How many fetches run at once.
    pub max_concurrency: usize,
    /// The most characters of a page stored in the index. A page longer than
    /// this is stored from its opening; the corpus is a finder, not an archive.
    #[serde(rename = "indexTextChars")]
    pub index_text_chars: usize,
}

/// How the private corpus is kept small, fresh, and useful.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexSettings {
    /// The corpus's size ceiling in megabytes. Past it, the least recently
    /// touched documents are evicted, so a long-lived service cannot grow
    /// without bound. Zero means no ceiling.
    #[serde(rename = "maxSizeMb")]
    pub max_size_mb: u64,
    /// Documents touched less recently than this are pruned on startup and after
    /// a write, so stale pages do not linger. Zero disables age pruning.
    #[serde(rename = "maxAgeDays")]
    pub max_age_days: u64,
    /// Hosts the corpus keeps fresh by itself: a search's hits on these hosts are
    /// re-fetched when older than the freshness window, so a seeded corpus stays
    /// true. Empty by default — nothing is fetched but what a caller asks for.
    pub refresh_hosts: Vec<String>,
    /// A `refresh_hosts` document is considered fresh for this long.
    #[serde(rename = "refreshAfterDays")]
    pub refresh_after_days: u64,
}

impl SearchSettings {
    pub fn max_results_or_default(&self) -> usize {
        if self.max_results == 0 {
            10
        } else {
            self.max_results
        }
    }

    /// Use the configured maximum for a zero request and cap every explicit request.
    pub fn result_limit(&self, requested: usize) -> usize {
        let maximum = self.max_results_or_default();
        if requested == 0 {
            maximum
        } else {
            requested.min(maximum)
        }
    }
    pub fn max_provider_time(&self) -> Duration {
        Duration::from_millis(self.max_provider_time_ms)
    }
    pub fn overall_timeout(&self) -> Duration {
        Duration::from_millis(self.overall_timeout_ms)
    }
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_millis(self.cache_ttl_ms)
    }
    /// Whether a search should consult the local corpus (`use_index`, default on).
    pub fn should_use_index(&self) -> bool {
        self.use_index.unwrap_or(true)
    }
}

impl FetchSettings {
    /// Whether fetched pages join the index (`index_fetched` defaults on).
    pub fn should_index(&self) -> bool {
        self.index_fetched.unwrap_or(true)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_millis(self.cache_ttl_ms)
    }
}

impl IndexSettings {
    pub fn max_size_bytes(&self) -> u64 {
        self.max_size_mb.saturating_mul(1024 * 1024)
    }
    pub fn max_age(&self) -> Option<Duration> {
        if self.max_age_days == 0 {
            None
        } else {
            Some(Duration::from_secs(
                self.max_age_days.saturating_mul(86_400),
            ))
        }
    }
    pub fn refresh_after(&self) -> Duration {
        // A zero window would re-fetch on every search; treat it as a day.
        let days = self.refresh_after_days.max(1);
        Duration::from_secs(days.saturating_mul(86_400))
    }
    /// Whether `host` is one the corpus keeps fresh.
    pub fn is_refresh_host(&self, host: &str) -> bool {
        self.refresh_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
    }
}

/// Which providers run, and the keys any of them need.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSettings {
    /// Restrict to these provider names; empty means every keyless provider.
    pub enabled: Vec<String>,
    /// Provider name to the environment variable holding its key.
    pub key_envs: BTreeMap<String, String>,
}

/// Log verbosity, parsed from a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}
