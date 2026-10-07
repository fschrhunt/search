//! Pin the native public module paths for minimal, MCP and CLI builds.

use search::client::{Client, Operation, Remote};
use search::core::{Answer, Config, Link, Page, Query, Search};
use search::engines::{Engine, Installed, Manifest, Pool};

/// Consumers can name the service, execution contract and package APIs in one package.
#[test]
fn public_module_contract_compiles() {
    let _ = std::mem::size_of::<(Search, Config, Query, Answer, Page, Link)>();
    let _ = std::mem::size_of::<(Client, Operation, Remote, Pool, Installed, Manifest)>();
    let _: Option<&dyn Engine> = None;
    let _ = search::engines::catalog;
    let _ = search::engines::install_catalog;
    let _ = search::engines::install_local;
    let _ = search::engines::resolve;
    let _ = search::engines::list;
    let _ = search::engines::update;
    let _ = search::engines::remove;
    #[cfg(feature = "mcp")]
    let _ = std::mem::size_of::<search::mcp::Server>();
    #[cfg(feature = "cli")]
    let _ = search::cli::run::execute;
}
