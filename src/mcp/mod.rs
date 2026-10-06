//! MCP tools and transports for the Search engine.
//!
//! Two tools, `web_search` and `web_fetch`, matching the shapes models already
//! know. This module owns tool handlers and both stdio and streamable HTTP
//! transports; the CLI decides where to mount them.

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::stdio;
use rmcp::{
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt,
};

use crate::core::text::{select as passages, Passage, DEFAULT_BUDGET};
use crate::core::{Answer, Page, Query, Search};

mod http;
use crate::client::Client;

pub use http::mount;

/// The MCP server over the in-process search engine.
#[derive(Clone)]
pub struct Server {
    search: Client,
    /// Read by the `#[tool_handler]` macro's generated dispatch, which clippy's
    /// field-usage analysis cannot see. Scoped allow, proof: macro-generated use.
    #[allow(dead_code)]
    tool_router: ToolRouter<Server>,
}

/// Arguments for `web_search`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// One to five concise keyword queries (512 bytes maximum each). Each is
    /// searched independently and reported in input order.
    pub queries: Vec<String>,
    /// Maximum results per query (default 10, max 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to enabled engine package IDs, such as "mwmbl" or "searxng".
    #[serde(default)]
    pub engines: Option<Vec<String>>,
}

/// Arguments for `web_fetch`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    /// One to ten http or https URLs to read.
    pub urls: Vec<String>,
    /// What you are looking for in these pages. When given, only the passages
    /// that match are returned instead of the whole page.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional character limit per page (max 40000). Without a query or limit,
    /// return the clean page; focused reads default to 6000 characters.
    #[serde(default)]
    pub max_characters: Option<usize>,
}

/// The JSON shape a search tool returns: one answer per query, in order.
#[derive(serde::Serialize)]
struct SearchOutput {
    queries: Vec<Answer>,
}

/// One fetched page, either whole or reduced to the passages a query matched.
#[derive(serde::Serialize)]
struct FocusedPage {
    #[serde(flatten)]
    page: Page,
    /// Present only when a query was given: the matching passages, replacing
    /// `text` as the thing to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    passages: Option<Vec<Passage>>,
}

impl FocusedPage {
    /// Keep the text when no query narrowed it, subject to the character budget.
    fn whole(mut page: Page, budget: usize) -> Self {
        if page.text.chars().count() > budget {
            page.truncated = Some(true);
        }
        page.text = page.text.chars().take(budget).collect();
        FocusedPage {
            page,
            passages: None,
        }
    }

    /// Return matching excerpts, or bounded page text when no passage matches.
    fn build(mut page: Page, query: &str, budget: usize) -> Self {
        if query.trim().is_empty() || page.text.is_empty() {
            return Self::whole(page, budget);
        }
        let found = passages(&page.text, query, budget);
        if !found.iter().any(|passage| passage.score > 0.0) {
            return Self::whole(page, budget);
        }
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
impl Server {
    /// Build the MCP server for an in-process search engine.
    pub fn new(search: Arc<Search>) -> Self {
        Server {
            search: Client::Local(search),
            tool_router: Self::tool_router(),
        }
    }

    /// Build the same tool surface over an exclusive selected client.
    pub fn with_client(search: Client) -> Self {
        Self {
            search,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "web_search",
        description = "Search the live web using the selected engines. Returns ranked titles, URLs, snippets, and engine status. Read relevant pages with web_fetch; use web_fetch directly when you already have a URL."
    )]
    async fn web_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let queries = validate_queries(args.queries)
            .map_err(|message| McpError::invalid_params(message, None))?;
        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let engines = args.engines.unwrap_or_default();
        let responses = futures::future::join_all(queries.into_iter().map(|text| {
            self.search.search(Query {
                text,
                limit,
                engines: engines.clone(),
            })
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(client_error)?;
        json_result(SearchOutput { queries: responses })
    }

    #[tool(
        name = "web_fetch",
        description = "Read one or more public URLs as clean text. Optionally pass a query for relevant excerpts or max_characters to limit the output. Private and link-local addresses are refused."
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
        if args
            .urls
            .iter()
            .any(|url| url.trim().is_empty() || url.len() > 8192)
        {
            return Err(McpError::invalid_params(
                "URLs must be nonempty and at most 8192 bytes",
                None,
            ));
        }
        if args
            .max_characters
            .is_some_and(|limit| limit == 0 || limit > 40_000)
        {
            return Err(McpError::invalid_params(
                "max_characters must be between 1 and 40000",
                None,
            ));
        }
        let focused = args
            .query
            .as_deref()
            .is_some_and(|query| !query.trim().is_empty());
        let budget =
            args.max_characters
                .unwrap_or(if focused { DEFAULT_BUDGET } else { usize::MAX });
        let pages = self.search.fetch(&args.urls).await.map_err(client_error)?;

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
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("search", crate::VERSION))
            .with_instructions(
                "Search the live web and read pages. Use web_search to find sources, then \
                 web_fetch to read the ones that matter. Results report which engines \
                 answered, so an empty answer is never mistaken for a broken one.",
            )
    }
}

/// Surface routing failures as MCP errors without substituting local answers.
fn client_error(message: String) -> McpError {
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
    let running = Server::new(search).serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

/// Serve the regular stdio tools over the selected execution client.
pub async fn serve_client(search: Client) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let running = Server::with_client(search).serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(text: &str) -> Page {
        Page {
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
            redirect: None,
            error: None,
        }
    }

    #[test]
    fn unfiltered_fetch_respects_the_character_budget() {
        let page = FocusedPage::build(fetched("abcdef"), "", 3);
        assert_eq!(page.page.text, "abc");
        assert_eq!(page.page.truncated, Some(true));
    }

    #[test]
    fn an_unmatched_focus_returns_bounded_text_not_a_matching_passage() {
        let page = FocusedPage::build(fetched("Readable page text."), "unmatched", 8);
        assert_eq!(page.page.text, "Readable");
        assert_eq!(page.page.truncated, Some(true));
        assert!(page.passages.is_none());
    }

    #[tokio::test]
    async fn a_default_fetch_returns_the_clean_page_without_a_character_cutoff(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/", listener.local_addr()?);
        let body = "Readable text. ".repeat(700);
        let expected = body.clone();
        let host = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await?;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
            Ok::<_, std::io::Error>(())
        });
        let mut config: crate::core::Config = serde_json::from_value(serde_json::json!({
            "engines": {"use": []}, "fetch": {"allow_private_networks": true}
        }))?;
        config.home = std::env::temp_dir().join("search-full-fetch");
        let server = Server::new(Arc::new(Search::open(config)?));
        let answer = server
            .web_fetch(Parameters(FetchArgs {
                urls: vec![url],
                query: None,
                max_characters: None,
            }))
            .await?
            .structured_content
            .ok_or("missing content")?;
        assert_eq!(
            answer
                .pointer("/pages/0/text")
                .and_then(serde_json::Value::as_str),
            Some(expected.as_str())
        );
        host.await??;
        Ok(())
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

    /// Tool validation must use enabled installed packages, not a fixed name list.
    #[tokio::test]
    async fn web_search_accepts_custom_engine_selection() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!("search-mcp-engine-{}", uuid_for_test()));
        let source = root.join("source");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&source)?;
        std::fs::write(
            source.join("engine.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "id": "custom", "version": "1.0.0", "description": "MCP fixture",
                "adapter": {
                    "type": "command", "command": "python3", "args": ["-c",
                        "import json,sys; r=json.load(sys.stdin); json.dump({'results':[{'title':r['query'],'url':'https://example.com/'}]},sys.stdout)"]
                }
            }))?,
        )?;
        let home = root.join("home");
        crate::engines::install_local(&home, &source)?;
        let mut config: crate::core::Config = serde_json::from_value(serde_json::json!({
            "engines": {"use": ["custom"]}
        }))?;
        config.home = home;
        let server = Server::new(Arc::new(Search::open(config)?));
        let answer = server
            .web_search(Parameters(SearchArgs {
                queries: vec!["custom query".into()],
                limit: Some(1),
                engines: Some(vec!["custom".into()]),
            }))
            .await?
            .structured_content
            .ok_or("missing structured content")?;
        assert_eq!(
            answer.pointer("/queries/0/results/0/title"),
            Some(&serde_json::json!("custom query"))
        );
        assert_eq!(
            answer.pointer("/queries/0/results/0/engines"),
            Some(&serde_json::json!(["custom"]))
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// A per-process nanosecond suffix isolates this one filesystem fixture without another dependency.
    fn uuid_for_test() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    }
}
