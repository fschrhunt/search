//! Built-in defaults used by Serde for omitted fields, never explicit zero values.

use std::time::Duration;

use super::{
    Config, FetchSettings, IndexSettings, ProviderSettings, RemoteSettings, SearchSettings,
    DEFAULT_ADDRESS,
};

/// Seconds a provider may take before it is abandoned.
pub const DEFAULT_PROVIDER_SECS: u64 = 2;
/// Seconds the whole fan-out may take.
pub const DEFAULT_OVERALL_SECS: u64 = 8;
/// Seconds a fetched page is reused from memory.
pub const DEFAULT_FETCH_CACHE_SECS: u64 = 600;
/// The most bytes read from one response.
pub const DEFAULT_MAX_BYTES: u64 = 4 << 20;
/// Redirects followed.
pub const DEFAULT_MAX_REDIRECTS: usize = 5;
/// Fetches in flight at once.
pub const DEFAULT_MAX_CONCURRENCY: usize = 8;
/// The most characters of a page stored in the index.
pub const DEFAULT_INDEX_TEXT_CHARS: usize = 40_000;
/// The corpus's size ceiling, in megabytes.
pub const DEFAULT_INDEX_MAX_SIZE_MB: u64 = 512;
/// How long a corpus document lives before it is pruned.
pub const DEFAULT_INDEX_MAX_AGE_DAYS: u64 = 180;
/// How long a seeded document stays fresh.
pub const DEFAULT_REFRESH_AFTER_DAYS: u64 = 7;
/// How much a local hit counts against a borrowed one.
pub const DEFAULT_INDEX_WEIGHT: f64 = 1.5;

impl Default for Config {
    fn default() -> Self {
        Config {
            notes: Default::default(),
            address: DEFAULT_ADDRESS.into(),
            dir: default_dir(),
            user_agent: format!(
                "search/{} (+https://github.com/fschrhunt/search)",
                crate::VERSION
            ),
            search: SearchSettings::default(),
            fetch: FetchSettings::default(),
            providers: ProviderSettings::default(),
            remote: RemoteSettings::default(),
            index: IndexSettings::default(),
        }
    }
}

impl Default for SearchSettings {
    fn default() -> Self {
        SearchSettings {
            max_results: 10,
            provider_timeout: secs(DEFAULT_PROVIDER_SECS),
            timeout: secs(DEFAULT_OVERALL_SECS),
            local_weight: DEFAULT_INDEX_WEIGHT,
        }
    }
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            only: Vec::new(),
            api: Default::default(),
        }
    }
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self { timeout: secs(120) }
    }
}

impl Default for FetchSettings {
    fn default() -> Self {
        FetchSettings {
            timeout: secs(15),
            max_response_bytes: DEFAULT_MAX_BYTES,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            cache_ttl: secs(DEFAULT_FETCH_CACHE_SECS),
            allow_private_networks: false,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_stored_chars: DEFAULT_INDEX_TEXT_CHARS,
        }
    }
}

impl Default for IndexSettings {
    fn default() -> Self {
        IndexSettings {
            enabled: true,
            save_fetched_pages: None,
            include_in_search: None,
            max_size_mb: DEFAULT_INDEX_MAX_SIZE_MB,
            retention_days: DEFAULT_INDEX_MAX_AGE_DAYS,
            refresh_hosts: Vec::new(),
            refresh_interval_days: DEFAULT_REFRESH_AFTER_DAYS,
        }
    }
}

fn secs(n: u64) -> u64 {
    Duration::from_secs(n).as_millis() as u64
}

/// `~/.local/share/search`, or the working dir when there is no home.
fn default_dir() -> std::path::PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => std::path::PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("search"),
        None => std::path::PathBuf::from("data"),
    }
}
