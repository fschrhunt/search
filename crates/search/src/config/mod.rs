//! The service's settings: where it listens, where its data lives, how long a
//! query may run, and which search engines are enabled.
//!
//! Omitted fields use safe defaults; explicit values are validated, not replaced.
//! Credential values stay in the environment or frontend storage; settings
//! reference only the variable names needed by discovery adapters.

pub(crate) mod adapters;
mod defaults;
mod load;
mod settings;

pub use adapters::BUILTIN_NAMES;
pub use settings::{
    AdapterSettings, CommandAdapterSettings, Config, EngineSettings, FetchSettings,
    HttpAdapterSettings, IndexSettings, RemoteSettings, SearchSettings, DEFAULT_ADDRESS,
};

/// Load and validate configuration from `path`, falling back to `CONFIG`
/// and then the standard location. Environment overrides are applied last.
pub use load::load;
