//! Pin the native public module paths for minimal, MCP and CLI builds.

use search::client::{Client, Remote};
use search::core::config::{Adapter, EngineSettings, Preset};
use search::core::engines::{Engine, EngineError, EngineState, EngineStatus, Found, Pool};
use search::core::{Answer, Config, Link, Page, Query, Search};

/// Consumers can name the service, engines, settings and execution contract in one package.
#[test]
fn public_module_contract_compiles() {
    let _ = std::mem::size_of::<(Search, Config, Query, Answer, Page, Link)>();
    let _ = std::mem::size_of::<(Client, Remote, Pool, EngineState, EngineStatus)>();
    let _ = std::mem::size_of::<(EngineError, Found, EngineSettings, Adapter)>();
    let _: Option<&dyn Engine> = None;
    let _ = Preset::find;
    #[cfg(feature = "mcp")]
    let _ = std::mem::size_of::<search::mcp::Server>();
    #[cfg(feature = "cli")]
    let _ = search::cli::run::execute;
}
