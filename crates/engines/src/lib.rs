//! Offline, declarative engine packages. Inspection never executes package code.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

#[cfg(test)]
mod catalog_build;
mod store;
use std::{path::PathBuf, sync::Arc};
mod manifest;
pub use manifest::Manifest;
use manifest::{manifest, valid_file, valid_id, MAX_FILE, MAX_FILES, MAX_PACKAGE};

pub use store::{install_catalog, install_local, list, remove, resolve, update};
/// The only engine available without installation or filesystem writes.
pub const DEFAULT_ENGINE: &str = "mwmbl";

/// Resolved installed package metadata and an optional lifetime execution lease.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub path: PathBuf,
    pub source: String,
    pub digest: String,
    /// Keep this shared lease alive for the entire lifetime of a running engine host.
    pub lease: Option<Arc<std::fs::File>>,
}

type Assets = Vec<(String, Vec<u8>)>;
type EmbeddedFile = (&'static str, &'static [u8]);
type EmbeddedPackage = (&'static str, &'static [EmbeddedFile]);
include!(concat!(env!("OUT_DIR"), "/catalog.rs"));

/// Return embedded catalog metadata in stable ID order without disk access.
pub fn catalog() -> Result<Vec<Manifest>, String> {
    ASSETS
        .iter()
        .map(|(id, files)| {
            let bytes = files
                .iter()
                .find(|(name, _)| *name == "engine.json")
                .map(|(_, bytes)| *bytes)
                .ok_or("catalog manifest missing")?;
            let m = manifest(bytes)?;
            if m.id != *id {
                return Err("catalog identity mismatch".into());
            }
            Ok(m)
        })
        .collect()
}

/// Read only assets declared by an embedded package.
fn embedded(id: &str) -> Result<(Manifest, Assets), String> {
    valid_id(id)?;
    let files = ASSETS
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, files)| *files)
        .ok_or("engine is not in the catalog")?;
    let assets: Assets = files
        .iter()
        .map(|(name, bytes)| ((*name).into(), bytes.to_vec()))
        .collect();
    let m = manifest(
        assets
            .iter()
            .find(|(name, _)| name == "engine.json")
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or("catalog manifest missing")?,
    )?;
    Ok((m, assets))
}
