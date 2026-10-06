//! MCP tools and transports for the Search engine.
//!
//! Two tools, `web_search` and `web_fetch`, matching the shapes models already
//! know. This crate owns tool handlers and both stdio and streamable HTTP
//! transports; the CLI decides where to mount them.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::stdio;
use rmcp::{
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt,
};

use search::text::{select as passages, Passage, DEFAULT_BUDGET};
use search::{Fetched, Query, Response, Search};

pub mod backend;
mod http;
use backend::Backend;

pub use http::mount;

/// The MCP server over the in-process search engine.
#[derive(Clone)]
pub struct McpServer {
    search: Backend,
    /// Read by the `#[tool_handler]` macro's generated dispatch, which clippy's
    /// field-usage analysis cannot see. Scoped allow, proof: macro-generated use.
    #[allow(dead_code)]
    tool_router: ToolRouter<McpServer>,
}

/// Arguments for `web_search`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// One to five concise keyword queries (512 bytes maximum each). Each is
    /// searched independently and reported in input order.
    pub queries: Vec<String>,
    /// Maximum results per query (default 10, max 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to specific providers, such as "brave" or "wikipedia".
    #[serde(default)]
    pub providers: Option<Vec<String>>,
}

/// Arguments for `web_fetch`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FetchArgs {
    /// One to ten http or https URLs to read.
    pub urls: Vec<String>,
    /// What you are looking for in these pages. When given, only the passages
    /// that match are returned, which is far cheaper than the whole page.
    #[serde(default)]
    pub query: Option<String>,
    /// The most characters to return per page (default 6000, max 40000).
    #[serde(default)]
    pub max_characters: Option<usize>,
}

/// The JSON shape a search tool returns: one response per query, in order.
#[derive(serde::Serialize)]
struct SearchOutput {
    queries: Vec<Response>,
}

/// One fetched page, either whole or reduced to the passages a query matched.
#[derive(serde::Serialize)]
struct FocusedPage {
    #[serde(flatten)]
    page: Fetched,
    /// Present only when a query was given: the matching passages, replacing
    /// `text` as the thing to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    passages: Option<Vec<Passage>>,
}

impl FocusedPage {
    /// Keep the text when no query narrowed it, subject to the character budget.
    fn whole(mut page: Fetched, budget: usize) -> Self {
        page.text = page.text.chars().take(budget).collect();
        FocusedPage {
            page,
            passages: None,
        }
    }

    /// Reduce to the passages matching `query`; the whole text is dropped so the
    /// model is not tempted to read past the answer.
    fn build(mut page: Fetched, query: &str, budget: usize) -> Self {
        if query.trim().is_empty() || page.text.is_empty() {
            return Self::whole(page, budget);
        }
        let mut used = 0;
        let mut found = Vec::new();
        for mut passage in passages(&page.text, query, budget) {
            let remaining = budget.saturating_sub(used);
            if remaining == 0 {
                break;
            }
            passage.text = passage.text.chars().take(remaining).collect();
            used += passage.text.chars().count();
            found.push(passage);
        }
        if found.is_empty() {
            return Self::whole(page, budget);
        }
        // The full text stays in the index; the tool answer does not carry it.
        page.text = String::new();
        FocusedPage {
            page,
            passages: Some(found),
        }
    }
}

/// The combined answer the fetch tool returns.
#[derive(serde::Serialize)]
struct FetchOutput {
    pages: Vec<FocusedPage>,
}

#[tool_router]
impl McpServer {
    /// Build the MCP server for an in-process search engine.
    pub fn new(search: Arc<Search>) -> Self {
        McpServer {
            search: Backend::Local(search),
            tool_router: Self::tool_router(),
        }
    }

    /// Build the same tool surface over an exclusive selected backend.
    pub fn with_backend(search: Backend) -> Self {
        Self {
            search,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "web_search",
        description = "Search the private local index and live web across several independent providers; return ranked results with title, URL, and snippet. Use it when you need current information, source discovery, or facts you are not confident about; do not use it for a page you already have a URL for — fetch that instead. The results are a starting point: read the few that matter with web_fetch before relying on them. Every answer names which providers responded, so an empty result is never mistaken for a broken one."
    )]
    async fn web_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let queries = validate_queries(args.queries)
            .map_err(|message| McpError::invalid_params(message, None))?;
        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let providers = args.providers.unwrap_or_default();
        let available = self.search.providers().await.map_err(remote_error)?;
        if let Some(unknown) = providers
            .iter()
            .find(|provider| !available.contains(provider))
        {
            return Err(McpError::invalid_params(
                format!("provider is not enabled: {unknown}"),
                None,
            ));
        }

        let responses = futures::future::join_all(queries.into_iter().map(|text| {
            self.search.search(Query {
                text,
                limit,
                per_provider: limit,
                providers: providers.clone(),
            })
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(remote_error)?;
        json_result(SearchOutput { queries: responses })
    }

    #[tool(
        name = "web_fetch",
        description = "Read one or more URLs as clean, readable text. When indexing is enabled, fetched pages join the local index. Pass a query to get only the passages that match it instead of the whole page — this is almost always what you want, and it is far cheaper. Public internet only: private and link-local addresses are refused."
    )]
    async fn web_fetch(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.urls.is_empty() || args.urls.len() > 10 {
            return Err(McpError::invalid_params(
                "provide between one and ten URLs",
                None,
            ));
        }
        if args.urls.iter().any(|url| url.trim().is_empty()) {
            return Err(McpError::invalid_params("URLs must not be empty", None));
        }
        let budget = args
            .max_characters
            .unwrap_or(DEFAULT_BUDGET)
            .clamp(500, 40_000);
        let pages = self.search.fetch(&args.urls).await.map_err(remote_error)?;

        // With a query, return only the matching passages; without one, the text.
        let query = args.query.unwrap_or_default();
        let focused: Vec<FocusedPage> = pages
            .into_iter()
            .map(|page| FocusedPage::build(page, &query, budget))
            .collect();
        json_result(FetchOutput { pages: focused })
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("search", search::VERSION))
            .with_instructions(
                "Search the live web and read pages. Use web_search to find sources, then \
                 web_fetch to read the ones that matter. Results report which providers \
                 answered, so an empty answer is never mistaken for a broken one.",
            )
    }
}

/// Surface routing failures as MCP errors without substituting local answers.
fn remote_error(message: String) -> McpError {
    McpError::internal_error(message, None)
}

/// Return both structured JSON and readable JSON text for MCP clients.
fn json_result<T: serde::Serialize>(value: T) -> Result<CallToolResult, McpError> {
    let value = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::structured(value))
}

/// Validate and trim the bounded set of independent search queries.
fn validate_queries(queries: Vec<String>) -> Result<Vec<String>, String> {
    if !(1..=5).contains(&queries.len()) {
        return Err("provide between one and five queries".into());
    }
    queries
        .into_iter()
        .map(|query| {
            let query = query.trim().to_string();
            if query.is_empty() {
                Err("queries must not be empty".into())
            } else if query.len() > 512 {
                Err("queries must be 512 bytes or fewer".into())
            } else {
                Ok(query)
            }
        })
        .collect()
}

/// Serve MCP over stdin/stdout until the client disconnects.
pub async fn serve_stdio(
    search: Arc<Search>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let running = McpServer::new(search).serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

/// Serve the regular stdio tools over the selected execution backend.
pub async fn serve_backend(
    search: Backend,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let running = McpServer::with_backend(search).serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(text: &str) -> Fetched {
        Fetched {
            url: "https://example.com".into(),
            fetched_at: None,
            final_url: None,
            status: 200,
            content_type: "text/html".into(),
            title: None,
            byline: None,
            published: None,
            site: None,
            text: text.into(),
            truncated: None,
            indexed: None,
            redirect: None,
            error: None,
        }
    }

    #[test]
    fn unfiltered_fetch_respects_the_character_budget() {
        let page = FocusedPage::build(fetched("abcdef"), "", 3);
        assert_eq!(page.page.text, "abc");
    }

    #[test]
    fn search_queries_are_bounded_and_trimmed_without_dropping_inputs() {
        assert_eq!(
            validate_queries(vec![" rust ".into(), "async".into()]).unwrap_or_default(),
            vec!["rust", "async"]
        );
        assert!(validate_queries(Vec::new()).is_err());
        assert!(validate_queries(vec!["rust".into(), " ".into()]).is_err());
        assert!(validate_queries(vec!["x".repeat(513)]).is_err());
    }

    #[test]
    fn focused_fetch_does_not_exceed_the_character_budget() {
        let page = FocusedPage::build(fetched("keyword and more text"), "keyword", 3);
        let used: usize = page
            .passages
            .unwrap_or_default()
            .iter()
            .map(|passage| passage.text.chars().count())
            .sum();
        assert!(used <= 3);
    }

    #[test]
    fn tool_results_include_structured_content() {
        let structured = json_result(serde_json::json!({"ok": true}))
            .ok()
            .and_then(|result| result.structured_content);
        assert_eq!(structured, Some(serde_json::json!({"ok": true})));
    }
}
