//! The Search command line and HTTP API. Engine and MCP behavior live in their
//! own modules and are composed here into the `search` executable.

pub mod args;
pub mod engines;
pub mod http;
pub mod render;
pub mod run;
pub mod stdio;

/// The product version, for `search version` and the MCP server info.
pub use crate::VERSION;

pub mod auth;
pub mod remote;
