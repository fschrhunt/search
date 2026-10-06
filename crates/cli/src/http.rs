//! The HTTP surface: a small JSON API and the streamable MCP endpoint on one
//! listener.
//!
//! Paired HTTPS protects every API and MCP request; pairing alone is public.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use search::discovery::Query;
use search::Search;
use search_mcp::mount as mount_mcp;

use crate::{run::build_service, trust::Host};
use search_mcp::backend::{Backend, Operation};

/// Host the local engine behind paired HTTPS until interrupted.
pub async fn serve(
    config_path: Option<String>,
    address: Option<String>,
    hostname: Option<String>,
    dir: Option<String>,
) -> i32 {
    let service = match build_service(config_path, address, dir) {
        Ok(service) => service,
        Err(message) => {
            eprintln!("search: {message}");
            return 1;
        }
    };
    let bind_hostname = service
        .config()
        .address
        .rsplit_once(':')
        .map(|(host, _)| host.trim_matches(['[', ']']))
        .unwrap_or("localhost");
    let hostname = hostname.as_deref().unwrap_or(bind_hostname);
    if matches!(hostname, "" | "0.0.0.0" | "::") {
        eprintln!("search: wildcard binds require -hostname NAME for the TLS identity");
        return 1;
    }
    let host = match Host::open(
        &service.config().home,
        hostname,
        std::time::Duration::from_secs(900),
    ) {
        Ok(host) => Arc::new(host),
        Err(error) => {
            eprintln!("search: {error}");
            return 1;
        }
    };
    let tls = match axum_server::tls_rustls::RustlsConfig::from_pem(
        host.cert.as_bytes().to_vec(),
        host.key.as_bytes().to_vec(),
    )
    .await
    {
        Ok(tls) => tls,
        Err(error) => {
            eprintln!("search: TLS identity: {error}");
            return 1;
        }
    };
    let listener = match std::net::TcpListener::bind(&service.config().address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("search: bind: {error}");
            return 1;
        }
    };
    eprintln!(
        "search: certificate {}",
        host.path.join("host.pem").display()
    );
    eprintln!("search: SHA-256 {}", host.fingerprint);
    eprintln!(
        "search: one-use pairing code {} (expires in 15 minutes; 20 attempts)",
        host.code
    );
    eprintln!(
        "search: listening on {} (data {}, version {})",
        service.config().address,
        service.config().dir.display(),
        search::VERSION
    );
    eprintln!("search: run search pair-code with the same local settings to pair another device");
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
    });
    // MCP validates the advertised authority rather than a wildcard bind address.
    let authority = match service.config().address.rsplit_once(':') {
        Some((_, port)) if hostname.contains(':') => format!("[{hostname}]:{port}"),
        Some((_, port)) => format!("{hostname}:{port}"),
        None => hostname.to_string(),
    };
    let server = match axum_server::from_tcp_rustls(listener, tls) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("search: listener: {error}");
            return 1;
        }
    };
    if let Err(error) = server
        .handle(handle)
        .serve(router(service, host, &authority).into_make_service())
        .await
    {
        eprintln!("search: server stopped: {error}");
        return 1;
    }
    0
}

/// Protect every execution route; `allowed_host` is the advertised MCP authority.
pub fn router(service: Arc<Search>, host: Arc<Host>, allowed_host: &str) -> Router {
    let mcp = mount_mcp(Arc::clone(&service), allowed_host);
    let protected = Router::new()
        .route("/healthz", get(health))
        .route("/v1/status", get(status))
        .route("/v1/search", get(search))
        .route("/v1/index", get(index_search))
        .route("/v1/fetch", post(fetch))
        .route("/v1/execute", post(execute))
        .nest_service("/mcp", mcp)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&host),
            auth,
        ))
        .with_state(service);
    protected
        .merge(Router::new().route("/pair", post(pair)).with_state(host))
        .layer(axum::extract::DefaultBodyLimit::max(1 << 20))
}

/// Refuse browser-origin requests and accept only currently registered device secrets.
async fn auth(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if headers.contains_key(axum::http::header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !host.authorized(presented) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// A pairing request carries the one-time code, never an existing device credential.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PairBody {
    code: String,
    name: String,
}

/// Enroll one device; deliberately return generic failures.
async fn pair(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<PairBody>,
) -> Response {
    if headers.contains_key(axum::http::header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match host.pair(&body.code, &body.name) {
        Ok(credential) => Json(credential).into_response(),
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}

/// Execute the shared CLI/stdio contract with bounded inputs on the host.
async fn execute(State(service): State<Arc<Search>>, Json(operation): Json<Operation>) -> Response {
    match &operation {
        Operation::Search { query }
            if query.text.trim().is_empty()
                || query.text.len() > 512
                || query.limit > 50
                || query.per_provider > 50 =>
        {
            return bad_request("invalid search bounds")
        }
        Operation::Index { query, limit }
            if query.trim().is_empty() || query.len() > 512 || *limit > 50 =>
        {
            return bad_request("invalid index bounds")
        }
        Operation::Fetch { urls }
            if urls.is_empty() || urls.len() > 10 || urls.iter().any(|u| u.len() > 8192) =>
        {
            return bad_request("invalid fetch bounds")
        }
        _ => {}
    }
    match Backend::Local(service).execute(operation).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => status_error(StatusCode::INTERNAL_SERVER_ERROR, "operation failed"),
    }
}

/// Query parameters for the search endpoint.
#[derive(serde::Deserialize)]
struct SearchParams {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    providers: Option<String>,
}

/// `GET /v1/search`.
async fn search(
    State(service): State<Arc<Search>>,
    axum::extract::Query(params): axum::extract::Query<SearchParams>,
) -> Response {
    let query = params.q.trim();
    if query.is_empty() {
        return bad_request("missing query parameter q");
    }
    if query.len() > 512 {
        return bad_request("query too long");
    }
    let limit = params.limit.unwrap_or(10).clamp(1, 50);
    let providers = params
        .providers
        .map(|p| {
            p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let response = service
        .search(Query {
            text: query.to_string(),
            limit,
            per_provider: limit,
            providers,
        })
        .await;
    Json(response).into_response()
}

/// Query parameters for the index endpoint.
#[derive(serde::Deserialize)]
struct IndexParams {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /v1/index`.
async fn index_search(
    State(service): State<Arc<Search>>,
    axum::extract::Query(params): axum::extract::Query<IndexParams>,
) -> Response {
    let query = params.q.trim();
    if query.is_empty() {
        return bad_request("missing query parameter q");
    }
    if query.len() > 512 {
        return bad_request("query too long");
    }
    let limit = params.limit.unwrap_or(10).clamp(1, 50);
    match service.index_search(query, limit) {
        Ok(hits) => Json(serde_json::json!({
            "query": query,
            "results": hits,
            "count": hits.len(),
        }))
        .into_response(),
        Err(error) => {
            eprintln!("search: index search: {error}");
            status_error(StatusCode::INTERNAL_SERVER_ERROR, "index search failed")
        }
    }
}

/// Body for the fetch endpoint.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    urls: Vec<String>,
}

/// `POST /v1/fetch`.
async fn fetch(State(service): State<Arc<Search>>, Json(body): Json<FetchBody>) -> Response {
    if body.urls.is_empty() {
        return bad_request("no urls given");
    }
    if body.urls.len() > 10 {
        return bad_request("at most 10 urls per request");
    }
    let pages = service.fetch(&body.urls).await;
    Json(serde_json::json!({"results": pages, "count": pages.len()})).into_response()
}

/// `GET /v1/status`.
async fn status(State(service): State<Arc<Search>>) -> Response {
    let stats = service.index_stats().ok();
    Json(serde_json::json!({
        "version": search::VERSION,
        "providers": service.provider_names(),
        "index": stats,
    }))
    .into_response()
}

/// `GET /healthz`.
async fn health() -> Response {
    Json(serde_json::json!({"status": "ok", "version": search::VERSION})).into_response()
}

/// A JSON 400.
fn bad_request(message: &str) -> Response {
    status_error(StatusCode::BAD_REQUEST, message)
}

/// A JSON error with a status.
fn status_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}
