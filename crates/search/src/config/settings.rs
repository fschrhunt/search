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
    pub engines: EngineSettings,
    pub remote: RemoteSettings,
    pub index: IndexSettings,
}

/// Bounds on one discovery query. Milliseconds on disk; durations in memory.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    /// Results returned at most.
    pub max_results: usize,
    /// Milliseconds one search engine may take before it is abandoned.
    pub engine_timeout: u64,
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
    pub fn engine_timeout(&self) -> Duration {
        Duration::from_millis(self.engine_timeout)
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

/// Which built-in and configured search engines run.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSettings {
    /// Enable live web engines. False leaves local-index search available.
    pub enabled: bool,
    /// Restrict to these engine names; an empty list means all engines when enabled.
    pub only: Vec<String>,
    /// Named HTTP or executable adapters, selected and ranked like built-ins.
    pub custom: BTreeMap<String, AdapterSettings>,
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

/// A configured discovery adapter. Credentials are resolved only at query time.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterSettings {
    Http(HttpAdapterSettings),
    Command(CommandAdapterSettings),
}

/// GET-only JSON discovery, with independent private-network permission.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpAdapterSettings {
    pub url: String,
    #[serde(default = "default_query_param")]
    pub query_param: String,
    #[serde(default)]
    pub limit_param: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Environment variables supply whole header values, without interpolation.
    #[serde(default)]
    pub header_env: BTreeMap<String, String>,
    #[serde(default = "default_results_pointer")]
    pub results_pointer: String,
    #[serde(default = "default_title_pointer")]
    pub title_pointer: String,
    #[serde(default = "default_url_pointer")]
    pub url_pointer: String,
    #[serde(default = "default_snippet_pointer")]
    pub snippet_pointer: String,
    #[serde(default = "default_adapter_cap")]
    pub max_response_bytes: u64,
    #[serde(default)]
    pub allow_private_networks: bool,
}

/// A directly spawned executable speaking one JSON request and response per query.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandAdapterSettings {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_adapter_cap")]
    pub max_response_bytes: u64,
}

/// The default bound on adapter output, independent of page-fetch limits.
fn default_adapter_cap() -> u64 {
    1 << 20
}
/// Default query parameter and JSON locations match the command response shape.
fn default_query_param() -> String {
    "q".into()
}
/// Locate the array in the response object.
fn default_results_pointer() -> String {
    "/results".into()
}
/// Locate a row's title.
fn default_title_pointer() -> String {
    "/title".into()
}
/// Locate a row's URL.
fn default_url_pointer() -> String {
    "/url".into()
}
/// Locate a row's optional snippet.
fn default_snippet_pointer() -> String {
    "/snippet".into()
}
