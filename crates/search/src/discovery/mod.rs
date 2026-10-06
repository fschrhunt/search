//! Discovery: turn a query into ranked, deduplicated results by fanning out to
//! several independent providers in parallel.
//!
//! Each provider is keyless unless a key is configured. This module owns no
//! index of its own; `crate::index` is the separate freshness layer.

mod parse;
mod registry;
mod web;

pub use registry::{blend, Registry};

/// One discovered page, normalized across providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Unix timestamp when the local indexed copy was fetched, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// Every provider that returned this URL, for transparency. A list, not a
    /// joined string: a display concern must not decide a dedup comparison.
    pub providers: Vec<String>,
    /// Reciprocal-rank-fusion score; higher is better.
    pub score: f64,
}

/// Which providers to use and how many results to ask each for.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Query {
    pub text: String,
    pub limit: usize,
    pub per_provider: usize,
    /// Restrict to these provider names; empty means every enabled provider.
    pub providers: Vec<String>,
}

/// The normalized answer the caller receives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub query: String,
    pub results: Vec<Finding>,
    pub providers: Vec<ProviderState>,
    pub duration_ms: u64,
}

/// How one provider fared, so an agent can tell a genuinely empty result set
/// from a provider that failed or was skipped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderState {
    pub name: String,
    pub status: ProviderStatus,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

/// The outcome of one provider's query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Ok,
    Error,
    Timeout,
    Cancelled,
}

/// A finding paired with the rank its provider gave it, and a weight scaling
/// its vote in the fusion. Internal to the merge: neither reaches the caller.
#[derive(Debug, Clone)]
pub(crate) struct Ranked {
    pub finding: Finding,
    pub rank: usize,
    pub weight: f64,
}

impl Ranked {
    /// A provider finding with the default weight.
    pub(crate) fn provider(finding: Finding, rank: usize) -> Self {
        Ranked {
            finding,
            rank,
            weight: 1.0,
        }
    }
}

/// A boxed future, so the `Provider` trait stays object-safe without an
/// async-trait macro: the registry holds `Box<dyn Provider>` and fans out. The
/// future is `'static` because it is spawned onto the runtime.
pub type ProviderFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<Finding>, ProviderError>> + Send>,
>;

/// A provider that discovers pages for a query. Implementations must return on
/// cancellation and never block past the deadline they are given. `search`
/// returns an owned, `'static` future so the registry can spawn it.
pub trait Provider: Send + Sync + 'static {
    /// The provider's stable identifier, used in configuration and output.
    fn name(&self) -> &'static str;

    /// Run one query. Results are in the provider's own ranking order.
    fn search(&self, query: String, limit: usize) -> ProviderFuture;

    /// Whether this provider is missing a key it needs.
    fn missing_key(&self) -> bool {
        false
    }
}

/// A provider failure, classified so the registry can report it without
/// leaking a response body into the provider state.
#[derive(Debug, Clone)]
pub struct ProviderError {
    pub message: String,
    pub cause: FailureCause,
}

/// Why a provider query failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCause {
    /// The request never left, or the answer could not be read.
    Network,
    /// The provider answered with a status that is not success.
    Rejected,
    /// The provider's answer could not be parsed.
    Malformed,
    /// The deadline passed.
    Timeout,
}

impl ProviderError {
    pub fn network(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            cause: FailureCause::Network,
        }
    }
    pub fn rejected(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            cause: FailureCause::Rejected,
        }
    }
    pub fn malformed(message: impl Into<String>) -> Self {
        ProviderError {
            message: message.into(),
            cause: FailureCause::Malformed,
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

/// The provider families, one constructor per file section in `web.rs` and its
/// siblings. Kept here so the registry has one list to build from.
pub(super) fn default_providers(
    settings: &crate::config::ProviderSettings,
) -> Vec<Box<dyn Provider>> {
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(web::Brave),
        Box::new(web::Marginalia),
        Box::new(web::Mwmbl),
        Box::new(web::Wikipedia),
        Box::new(web::HackerNews),
        Box::new(web::StackExchange),
        Box::new(web::Arxiv),
    ];
    providers
        .into_iter()
        .filter(|provider| enabled(settings, provider.name()))
        .collect()
}

fn enabled(settings: &crate::config::ProviderSettings, name: &str) -> bool {
    settings.enabled && (settings.only.is_empty() || settings.only.iter().any(|e| e == name))
}
