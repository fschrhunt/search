//! Stamp release identity into the build.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

/// Export release version, channel and commit, defaulting to local values.
fn main() {
    let version = std::env::var("SEARCH_BUILD_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    let channel = std::env::var("SEARCH_BUILD_CHANNEL").unwrap_or_else(|_| "local".into());
    let commit = std::env::var("SEARCH_BUILD_COMMIT").unwrap_or_else(|_| "local".into());
    println!("cargo:rustc-env=SEARCH_BUILD_VERSION={version}");
    println!("cargo:rustc-env=SEARCH_BUILD_CHANNEL={channel}");
    println!("cargo:rustc-env=SEARCH_BUILD_COMMIT={commit}");
    println!("cargo:rerun-if-env-changed=SEARCH_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=SEARCH_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=SEARCH_BUILD_COMMIT");
}
