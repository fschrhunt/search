//! The service's settings: its package home and listener, how long a
//! query may run, and which search engines are enabled.
//!
//! Omitted fields use safe defaults; explicit values are validated, not replaced.
//! Credential values stay in the environment or frontend storage; settings
//! reference only the variable names needed by engine adapters.

mod defaults;
pub(crate) mod engines;
mod load;
mod settings;

pub use engines::{resolve_adapter, valid_engine_id};
pub use settings::{
    Adapter, Command, Config, EngineSettings, FetchSettings, Http, RemoteSettings, SearchSettings,
    DEFAULT_ADDRESS,
};

/// Load and validate configuration from `path`, falling back to `CONFIG`
/// and then home/settings.json. Environment overrides are applied last.
pub use load::load;
pub use load::{home, settings_path, ConfigError};
