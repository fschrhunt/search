//! Engines: turn a query into ranked, deduplicated results by fanning out to
//! several independent engines in parallel.
//!
//! Selected packages provide adapters that resolve credentials from the
//! environment. Engine results come directly from those live engines.
//! Catalog inspection is declarative; installation retains private receipts and leases.

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

mod adapter;
mod pool;

pub use pool::Pool;

/// One discovered page, normalized across engines.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Every engine that returned this URL, for transparency. A list, not a
    /// joined string: a display concern must not decide a dedup comparison.
    pub engines: Vec<String>,
    /// Reciprocal-rank-fusion score; higher is better.
    pub score: f64,
}

/// The search text, result limit, and optional engine selection.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    pub text: String,
    pub limit: usize,
    /// Restrict to these engine names; empty means every enabled engine.
    pub engines: Vec<String>,
}

/// The normalized answer the caller receives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Answer {
    pub query: String,
    pub results: Vec<Link>,
    pub engines: Vec<EngineState>,
    pub duration_ms: u64,
}

/// How one engine fared, so an agent can tell a genuinely empty result set
/// from an engine that failed or was skipped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineState {
    pub name: String,
    pub status: EngineStatus,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

/// The outcome of one engine's query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    Ok,
    Error,
    Timeout,
}

/// A link paired with its engine rank for reciprocal-rank fusion.
/// The rank is internal to the merge and does not reach the caller.
#[derive(Debug, Clone)]
pub(crate) struct Ranked {
    pub link: Link,
    pub rank: usize,
}

impl Ranked {
    /// Preserve a link's position in its engine's result ordering.
    pub(crate) fn engine(link: Link, rank: usize) -> Self {
        Ranked { link, rank }
    }
}

/// A boxed future, so the `Engine` trait stays object-safe without an
/// async-trait macro: the pool holds `Arc<dyn Engine>` and fans out. The
/// future is `'static` because it is spawned onto the runtime.
pub type EngineFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Link>, EngineError>> + Send>>;

/// Discover pages in upstream rank order. The pool drops each future at its
/// deadline; implementations must not block the runtime and must release work on
/// cancellation. An owned, `'static` future lets the pool spawn each query.
pub trait Engine: Send + Sync + 'static {
    /// The engine's stable identifier, used in configuration and output.
    fn name(&self) -> &str;

    /// Run one query. Results are in the engine's own ranking order.
    fn search(&self, query: String, limit: usize) -> EngineFuture;
}

/// An engine failure, classified so the pool can report it without
/// leaking a response body into the engine state.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub message: String,
    pub cause: FailureCause,
}

/// Why an engine query failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCause {
    /// The request never left, or the answer could not be read.
    Network,
    /// The engine answered with a status that is not success.
    Rejected,
    /// The engine's answer could not be parsed.
    Malformed,
    /// The deadline passed.
    Timeout,
}

impl EngineError {
    pub fn network(message: impl Into<String>) -> Self {
        EngineError {
            message: message.into(),
            cause: FailureCause::Network,
        }
    }
    pub fn rejected(message: impl Into<String>) -> Self {
        EngineError {
            message: message.into(),
            cause: FailureCause::Rejected,
        }
    }
    pub fn malformed(message: impl Into<String>) -> Self {
        EngineError {
            message: message.into(),
            cause: FailureCause::Malformed,
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod catalog_build;

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire names track engines while rankings and status values remain stable.
    #[test]
    fn query_and_answer_use_engine_fields() {
        let query: Query = serde_json::from_value(serde_json::json!({
            "text": "query", "limit": 3, "engines": ["fixture"]
        }))
        .unwrap();
        assert_eq!(query.limit, 3);
        assert_eq!(
            serde_json::to_value(query).unwrap(),
            serde_json::json!({
                "text": "query", "limit": 3, "engines": ["fixture"]
            })
        );
        let answer = Answer {
            query: "query".into(),
            results: vec![Link {
                title: "Title".into(),
                url: "https://example.com/".into(),
                snippet: None,
                engines: vec!["fixture".into()],
                score: 1.0 / 61.0,
            }],
            engines: vec![EngineState {
                name: "fixture".into(),
                status: EngineStatus::Ok,
                count: 1,
                error: None,
                elapsed_ms: 1,
            }],
            duration_ms: 1,
        };
        assert_eq!(
            serde_json::to_value(answer).unwrap(),
            serde_json::json!({
                "query": "query", "results": [{"title":"Title", "url":"https://example.com/", "engines":["fixture"], "score":1.0/61.0}],
                "engines":[{"name":"fixture", "status":"ok", "count":1, "elapsed_ms":1}], "duration_ms":1
            })
        );
    }
}
