//! Shared build-time and runtime manifest schema and bounded validation.
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
pub(crate) const MAX_FILE: u64 = 2 * 1024 * 1024;
pub(crate) const MAX_PACKAGE: usize = 8 * 1024 * 1024;
pub(crate) const MAX_FILES: usize = 64;

/// Declarative adapter metadata; `required` names direct adapter fields.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub description: String,
    pub adapter: serde_json::Value,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    /// Declared assets allowed to receive owner execute permission.
    #[serde(default)]
    pub executables: Vec<String>,
    /// Executable names needed at runtime; metadata only, never installed automatically.
    #[serde(default)]
    pub requires: Vec<String>,
}

/// Validate caller IDs before using them in any filesystem operation.
pub(crate) fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || id == "index"
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err("invalid engine id".into());
    }
    Ok(())
}

/// Accept portable relative asset names, excluding store control files.
pub(crate) fn valid_file(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 240
        || name.contains('\\')
        || name.split('/').any(|s| s.is_empty() || s.starts_with('.'))
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("invalid package file path".into());
    }
    Ok(())
}

/// Validate schema without resolving credentials or executing commands.
pub(crate) fn manifest(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.len() > MAX_FILE as usize {
        return Err("manifest exceeds size limit".into());
    }
    let m: Manifest = serde_json::from_slice(bytes).map_err(|_| "invalid engine manifest")?;
    valid_id(&m.id)?;
    if m.schema_version != 1
        || m.version.trim().is_empty()
        || m.version.len() > 128
        || m.description.len() > 8192
        || m.files.len() >= MAX_FILES
        || m.required.len() > 64
        || m.executables.len() > MAX_FILES
        || m.requires.len() > MAX_FILES
    {
        return Err("unsupported or invalid engine manifest".into());
    }
    let adapter = m.adapter.as_object().ok_or("adapter must be an object")?;
    match adapter.get("type").and_then(|v| v.as_str()) {
        Some("http") => {
            let url = adapter
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("HTTP adapter requires url field")?;
            if url.trim().is_empty() && !m.required.iter().any(|s| s == "url") {
                return Err("HTTP adapter requires url".into());
            }
        }
        Some("command") => {
            if adapter
                .get("command")
                .and_then(|v| v.as_str())
                .is_none_or(|v| v.trim().is_empty())
            {
                return Err("command adapter requires command".into());
            }
        }
        _ => return Err("unsupported adapter type".into()),
    }
    let mut names = std::collections::BTreeSet::new();
    for name in &m.files {
        valid_file(name)?;
        if !names.insert(name) {
            return Err("duplicate package file".into());
        }
    }
    for name in &m.files {
        if m.files
            .iter()
            .any(|other| other.starts_with(&format!("{name}/")))
            || name.starts_with("engine.json/")
        {
            return Err("package file collides with a directory".into());
        }
    }
    let mut executables = std::collections::BTreeSet::new();
    for name in &m.executables {
        valid_file(name)?;
        if name == "engine.json" || !m.files.contains(name) || !executables.insert(name) {
            return Err("executable must name a unique declared package file".into());
        }
    }
    let mut requires = std::collections::BTreeSet::new();
    for name in &m.requires {
        if name.is_empty()
            || name.len() > 64
            || name.starts_with('.')
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_+.".contains(&b))
            || !requires.insert(name)
        {
            return Err("runtime requirements must be executable names".into());
        }
    }
    let mut required = std::collections::BTreeSet::new();
    for name in &m.required {
        if name.is_empty()
            || name.len() > 64
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !required.insert(name)
        {
            return Err("invalid required adapter field".into());
        }
    }
    Ok(m)
}
