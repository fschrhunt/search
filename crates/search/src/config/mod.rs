//! The service's settings: its home, listener and corpus location, how long a
//! query may run, and which search engines are enabled.
//!
//! Omitted fields use safe defaults; explicit values are validated, not replaced.
//! Credential values stay in the environment or frontend storage; settings
//! reference only the variable names needed by discovery adapters.

pub(crate) mod adapters;
mod defaults;
mod load;
mod settings;

pub use adapters::{resolve_adapter, resolve_engines, valid_engine_id};
pub use settings::{
    AdapterSettings, CommandAdapterSettings, Config, EngineSettings, FetchSettings,
    HttpAdapterSettings, IndexSettings, RemoteSettings, SearchSettings, DEFAULT_ADDRESS,
};

/// Load and validate configuration from `path`, falling back to `CONFIG`
/// and then home/settings.json. Environment overrides are applied last.
pub use load::load;
pub use load::{home, settings_path, ConfigError};
