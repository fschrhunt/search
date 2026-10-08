//! The HTTP surface: one small JSON API and the streamable MCP endpoint on one
//! listener. Paired `Client`s use the same API as scripts.
//!
//! Paired HTTPS protects every API and MCP request; pairing alone is public.
//! Inputs are checked with the shared `client` validators before execution.

use std::sync::Arc;

use crate::core::{Query, Search};
use crate::mcp::mount as mount_mcp;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::cli::{auth::Host, run::build_service};
use crate::client::{validate_search, validate_urls};

/// Host the local engine behind paired HTTPS until interrupted.
pub async fn serve(
    config_path: Option<String>,
    address: Option<String>,
    hostname: Option<String>,
) -> i32 {
    let service = match build_service(config_path, address) {
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
    let listener = match std::net::TcpListener::bind(&service.config().address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("search: bind: {error}");
            return 1;
        }
    };
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
        "search: listening on {} (version {})",
        service.config().address,
        crate::VERSION
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
        .route("/v1/fetch", post(fetch))
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

/// Query parameters for the search endpoint.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    engines: Option<String>,
}

/// `GET /v1/search`: one `Answer`. An omitted limit means the host's configured
/// maximum; `engines` is a comma-separated list of package IDs.
async fn search(
    State(service): State<Arc<Search>>,
    axum::extract::Query(params): axum::extract::Query<SearchParams>,
) -> Response {
    let engines = params
        .engines
        .map(|p| {
            p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let query = match validate_search(Query {
        text: params.q,
        limit: params.limit.unwrap_or(0),
        engines,
    }) {
        Ok(query) => query,
        Err(message) => return bad_request(&message),
    };
    Json(service.search(query).await).into_response()
}

/// Body for the fetch endpoint.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchBody {
    urls: Vec<String>,
}

/// `POST /v1/fetch`: `{"pages": [Page...]}` in input order; per-URL failures
/// are reported on each page rather than failing the request.
async fn fetch(State(service): State<Arc<Search>>, Json(body): Json<FetchBody>) -> Response {
    if let Err(message) = validate_urls(&body.urls) {
        return bad_request(&message);
    }
    let pages = service.fetch(&body.urls).await;
    Json(serde_json::json!({ "pages": pages })).into_response()
}

/// `GET /v1/status`: the host version and its selected engine names.
async fn status(State(service): State<Arc<Search>>) -> Response {
    Json(serde_json::json!({
        "version": crate::VERSION,
        "engines": service.engine_names(),
    }))
    .into_response()
}

/// `GET /healthz`.
async fn health() -> Response {
    Json(serde_json::json!({"status": "ok", "version": crate::VERSION})).into_response()
}

/// A JSON 400 naming the violated bound.
fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}
