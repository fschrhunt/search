//! Stamp release identity and embed bounded assets with the installation validator.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#[path = "src/engines/manifest.rs"]
mod manifest;
use std::{env, fs, path::Path};
#[path = "src/engines/catalog_build.rs"]
mod catalog_build;
use catalog_build::asset;

/// Generate immutable catalog tuples; schema, path and size errors fail the build.
fn embed_catalog() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("engines");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut dirs = Vec::new();
    for entry in fs::read_dir(&root)? {
        if dirs.len() >= 512 {
            return Err("catalog exceeds directory limit".into());
        }
        dirs.push(entry?);
    }
    dirs.sort_by_key(|entry| entry.file_name());
    let mut generated = String::from("static ASSETS: &[EmbeddedPackage] = &[\n");
    for dir in dirs {
        let kind = dir.file_type()?;
        if kind.is_symlink() {
            return Err("catalog package directories must not be symlinks".into());
        }
        if !kind.is_dir() {
            continue;
        }
        let path = dir.path().join("engine.json");
        if !path.try_exists()? && fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let m = manifest::manifest(&asset(&path)?)?;
        if dir.file_name().to_str() != Some(&m.id) {
            return Err("package directory must match id".into());
        }
        let mut names = m.files.clone();
        if !names.iter().any(|n| n == "engine.json") {
            names.push("engine.json".into());
        }
        names.sort();
        let mut total = 0usize;
        generated.push_str(&format!("({:?}, &[\n", m.id));
        for name in names {
            let path = dir.path().join(&name);
            let bytes = asset(&path)?;
            total = total.saturating_add(bytes.len());
            if total > manifest::MAX_PACKAGE {
                return Err("catalog package exceeds size limit".into());
            }
            generated.push_str(&format!("({name:?}, include_bytes!({path:?})),\n"));
        }
        generated.push_str("]),\n");
    }
    generated.push_str("];\n");
    fs::write(
        Path::new(&env::var("OUT_DIR")?).join("catalog.rs"),
        generated,
    )?;
    Ok(())
}

/// Stamp release identity and validate and embed the declared engine catalog.
fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    embed_catalog()
}
