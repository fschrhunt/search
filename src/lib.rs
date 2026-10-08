//! Search's live engine, shared execution and optional frontends.
//! Core (including its engines) stays protocol-free; MCP is optional and the
//! CLI feature includes MCP.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

#[cfg(feature = "cli")]
pub mod cli;
pub mod client;
pub mod core;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(windows)]
mod private_fs;

/// Release version stamped by the build; local builds use the manifest version.
pub const VERSION: &str = env!("SEARCH_BUILD_VERSION");
/// Release channel; local builds never follow a published channel.
pub const CHANNEL: &str = env!("SEARCH_BUILD_CHANNEL");
/// Exact published source revision, or `local` for ordinary builds.
pub const COMMIT: &str = env!("SEARCH_BUILD_COMMIT");
