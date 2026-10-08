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

use crate::core::text::{focus, Focus, FocusedPage};
use crate::core::{Answer, Query, Search};

mod http;
use crate::client::{validate_query, validate_urls, Client};

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
    /// Maximum results per query, at most 50. When omitted, the host's
    /// configured maximum (`search.max_results`, normally 10), which also caps
    /// any larger request.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Restrict to selected engine IDs, such as "mwmbl" or "searxng".
    #[serde(default)]
    pub engines: Option<Vec<String>>,
}

/// Arguments for `web_fetch`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    /// One to ten http or https URLs to read.
    pub urls: Vec<String>,
    /// What you are looking for in these pages. When it matches, only the
    /// relevant passages are returned instead of the whole page.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional character limit per page. Without a query or limit, the whole
    /// clean page is returned; focused reads default to 6000 characters.
    #[serde(default)]
    pub max_characters: Option<usize>,
    /// Character offset to continue reading from: pass a page's `next_offset`
    /// from an earlier call to read what followed.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// The JSON shape a search tool returns: one answer per query, in order.
#[derive(serde::Serialize)]
struct SearchOutput {
    queries: Vec<Answer>,
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
        // Zero asks for the host maximum; the client caps explicit limits.
        let limit = args.limit.unwrap_or(0);
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
        description = "Read one or more public URLs (HTML, text or PDF) as clean text. Without options the whole page is returned. Pass a query for relevant excerpts, or max_characters to read a window; a page that continues reports next_offset, which you pass back as offset to read on. Private and link-local addresses are refused."
    )]
    async fn web_fetch(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<CallToolResult, McpError> {
        validate_urls(&args.urls).map_err(|message| McpError::invalid_params(message, None))?;
        if args.max_characters == Some(0) {
            return Err(McpError::invalid_params(
                "max_characters must be at least 1",
                None,
            ));
        }
        let pages = self.search.fetch(&args.urls).await.map_err(client_error)?;
        let reading = Focus {
            query: args.query,
            max_characters: args.max_characters,
            offset: args.offset.unwrap_or(0),
        };
        let focused: Vec<FocusedPage> = pages
            .into_iter()
            .map(|page| focus(page, &reading))
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

/// Require one to five independent queries, each trimmed by the shared query bound.
fn validate_queries(queries: Vec<String>) -> Result<Vec<String>, String> {
    if !(1..=5).contains(&queries.len()) {
        return Err("provide between one and five queries".into());
    }
    queries.iter().map(|query| validate_query(query)).collect()
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
        config.home = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("search-full-fetch");
        let server = Server::new(Arc::new(Search::open(config)?));
        let answer = server
            .web_fetch(Parameters(FetchArgs {
                urls: vec![url],
                query: None,
                max_characters: None,
                offset: None,
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
        assert!(validate_queries(vec!["rust".into(); 6]).is_err());
        assert!(validate_queries(vec!["rust".into(), " ".into()]).is_err());
        assert!(validate_queries(vec!["x".repeat(513)]).is_err());
    }

    #[test]
    fn tool_results_include_structured_content() {
        let structured = json_result(serde_json::json!({"ok": true}))
            .ok()
            .and_then(|result| result.structured_content);
        assert_eq!(structured, Some(serde_json::json!({"ok": true})));
    }

    /// Tool validation must use the selected engines, not a fixed name list,
    /// and an omitted limit must reach the engine as the host's configured maximum.
    #[tokio::test]
    async fn web_search_accepts_custom_engines_and_defaults_to_the_host_limit(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let config: crate::core::Config = serde_json::from_value(serde_json::json!({
            "engines": {"use": ["custom"], "config": {"custom": {
                "type": "command", "command": if cfg!(windows) { "python.exe" } else { "python3" }, "args": ["-c",
                    "import json,sys; r=json.load(sys.stdin); json.dump({'results':[{'title':r['query'],'url':'https://example.com/','snippet':str(r['limit'])}]},sys.stdout)"]
            }}}, "search": {"max_results": 3}
        }))?;
        let server = Server::new(Arc::new(Search::open(config)?));
        let answer = server
            .web_search(Parameters(SearchArgs {
                queries: vec!["custom query".into()],
                limit: None,
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
        assert_eq!(
            answer.pointer("/queries/0/results/0/snippet"),
            Some(&serde_json::json!("3"))
        );
        Ok(())
    }
}
