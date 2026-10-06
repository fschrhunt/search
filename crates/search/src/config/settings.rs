//! The configuration shape and its invariants.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// The default listener: loopback only. Exposing search beyond loopback is a
/// deliberate act; the frontend provides paired HTTPS authentication.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:8642";

/// Everything the service can be told. Duration values are milliseconds.
/// Saving and blending local pages default on; `index.enabled` disables the corpus.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Optional human explanations, ignored by the engine.
    pub notes: BTreeMap<String, String>,
    /// Listen address, `host:port`.
    pub address: String,
    /// Directory the index lives in.
    pub dir: PathBuf,
    /// The user agent the fetcher sends.
    pub user_agent: String,
    pub search: SearchSettings,
    pub fetch: FetchSettings,
    pub providers: ProviderSettings,
    pub remote: RemoteSettings,
    pub index: IndexSettings,
}

/// Bounds on one discovery query. Milliseconds on disk; durations in memory.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    /// Results returned at most.
    pub max_results: usize,
    /// Milliseconds one provider may take before it is abandoned.
    pub provider_timeout: u64,
    /// Milliseconds allowed for the whole query.
    pub timeout: u64,
    /// Blend weight for a local hit against a borrowed one. Higher favors what
    /// you have already read.
    pub local_weight: f64,
}

/// Bounds on the fetcher.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FetchSettings {
    pub timeout: u64,
    /// Maximum bytes read from any one response.
    pub max_response_bytes: u64,
    /// The most redirects followed, each re-checked.
    pub max_redirects: usize,
    pub cache_ttl: u64,
    /// Refuse only for tests and air-gapped mirrors: disabling the guard makes
    /// the fetcher able to reach private addresses.
    pub allow_private_networks: bool,
    /// How many fetches run at once.
    pub max_concurrency: usize,
    /// The most characters of a page stored in the index. A page longer than
    /// this is stored from its opening; the corpus is a finder, not an archive.
    pub max_stored_chars: usize,
}

/// How the server-local corpus is enabled, bounded and explicitly refreshed.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexSettings {
    /// Open the server-local corpus. False means no database access at all.
    pub enabled: bool,
    /// Whether pages fetched by Search should be stored locally.
    pub save_fetched_pages: Option<bool>,
    /// Whether local matches should be included in web search results.
    pub include_in_search: Option<bool>,
    /// The corpus's size ceiling in megabytes. Past it, the least recently
    /// touched documents are evicted, so a long-lived service cannot grow
    /// without bound. Zero means no ceiling.
    pub max_size_mb: u64,
    /// Documents touched less recently than this are pruned on startup and after
    /// a write, so stale pages do not linger. Zero disables age pruning.
    pub retention_days: u64,
    /// Hosts whose saved pages are refreshed by the explicit refresh command.
    /// Empty by default; there is no automatic background crawl.
    pub refresh_hosts: Vec<String>,
    /// A `refresh_hosts` document is considered fresh for this long.
    pub refresh_interval_days: u64,
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
    pub fn provider_timeout(&self) -> Duration {
        Duration::from_millis(self.provider_timeout)
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout)
    }
}

impl FetchSettings {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout)
    }
    pub fn cache_ttl(&self) -> Duration {
        Duration::from_millis(self.cache_ttl)
    }
}

impl IndexSettings {
    /// Whether pages fetched by Search should be stored locally.
    pub fn should_save_fetched_pages(&self) -> bool {
        self.enabled && self.save_fetched_pages.unwrap_or(true)
    }
    /// Whether local matches should be included in web search results.
    pub fn should_include_in_search(&self) -> bool {
        self.enabled && self.include_in_search.unwrap_or(true)
    }
    pub fn max_size_bytes(&self) -> u64 {
        self.max_size_mb.saturating_mul(1024 * 1024)
    }
    pub fn max_age(&self) -> Option<Duration> {
        if self.retention_days == 0 {
            None
        } else {
            Some(Duration::from_secs(
                self.retention_days.saturating_mul(86_400),
            ))
        }
    }
    pub fn refresh_after(&self) -> Duration {
        Duration::from_secs(self.refresh_interval_days.saturating_mul(86_400))
    }
    /// Whether `host` is one the corpus keeps fresh.
    pub fn is_refresh_host(&self, host: &str) -> bool {
        self.refresh_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
    }
}

/// Which providers run, and API keys any of them need.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderSettings {
    /// Enable live web providers. False leaves local-index search available.
    pub enabled: bool,
    /// Restrict to these provider names; an empty list means all providers when enabled.
    pub only: Vec<String>,
    /// Provider name to the environment variable holding its API key.
    pub api: BTreeMap<String, String>,
}

/// Limits on calls made to the selected paired host.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSettings {
    /// Overall request deadline in milliseconds.
    pub timeout: u64,
}

impl RemoteSettings {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout)
    }
}
