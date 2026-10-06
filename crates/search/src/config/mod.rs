//! The service's settings: where it listens, where its data lives, how long a
//! query may run, and which search engines are enabled.
//!
//! Omitted fields use safe defaults; explicit values are validated, not replaced.
//! Credentials belong to the frontend, separate from shareable settings.

mod defaults;
mod load;
mod settings;

pub use settings::{
    Config, EngineSettings, FetchSettings, IndexSettings, RemoteSettings, SearchSettings,
    DEFAULT_ADDRESS,
};

/// Load and validate configuration from `path`, falling back to `CONFIG`
/// and then the standard location. Environment overrides are applied last.
pub use load::load;
