//! The Search command line and HTTP API. Engine and MCP behavior live in their
//! own crates and are composed here into the `search` executable.

pub mod args;
pub mod http;
pub mod render;
pub mod run;
pub mod stdio;

/// The product version, for `search version` and the MCP server info.
pub use search::VERSION;

pub mod remote;
pub mod trust;
