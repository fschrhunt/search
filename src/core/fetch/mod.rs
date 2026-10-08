//! Fetch: retrieve URLs on behalf of callers and turn HTML into readable text.
//!
//! Because URLs are model-chosen, every request is treated as hostile:
//! destinations are resolved and refused if they point inside the network, each
//! redirect hop is re-checked, bodies are size-capped, and deadlines are
//! enforced. Successful pages are cached briefly in memory.

mod cache;
mod extract;
pub(crate) mod guard;

use std::time::Duration;

use crate::core::config::FetchSettings;

pub use guard::{is_public_ip, GuardError};

/// Decide whether to follow a client-side redirect target. The target came from
/// an untrusted page, so it is accepted only when it is a well-formed http(s)
/// URL that passes the SSRF guard, does not loop, and is within the hop budget.
/// Returns the URL to fetch next, or `None` to stop.
fn follow_target(
    next: &str,
    current: &str,
    hops: usize,
    max_hops: usize,
    allow_private_networks: bool,
) -> Option<String> {
    if hops > max_hops || next == current {
        return None;
    }
    let parsed = url::Url::parse(next).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    guard::check_host(parsed.host_str().unwrap_or(""), allow_private_networks).ok()?;
    Some(next.to_string())
}

/// The outcome of one fetch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Page {
    pub url: String,
    /// Unix timestamp when Search fetched this page; retained for cache hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    pub status: u16,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// A client-side redirect target, followed by the caller. Not serialized:
    /// a model is shown the final page, never told to fetch another URL.
    #[serde(skip)]
    pub redirect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A refused or failed fetch. Each variant is a distinct caller-facing reason.
#[derive(Debug, Clone)]
pub enum FetchError {
    /// The URL was not http or https.
    Scheme,
    /// The destination is private, loopback, or otherwise unreachable by policy.
    Refused(String),
    /// The request never completed.
    Network(String),
    /// The server answered with an error status.
    Status(u16),
    /// The response carried no readable text.
    Empty,
    /// The body is a type Search cannot read as text, such as an image or archive.
    Unsupported(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::Scheme => "only http and https URLs are supported".into(),
            FetchError::Refused(reason) => reason.clone(),
            FetchError::Network(reason) => reason.clone(),
            FetchError::Status(code) => format!("HTTP {code}"),
            FetchError::Empty => "no readable text in response".into(),
            FetchError::Unsupported(reason) => reason.clone(),
        }
    }
}

/// Holds the guarded client, fetch bounds, and a bounded in-memory TTL cache.
pub struct Fetcher {
    settings: FetchSettings,
    client: reqwest::Client,
    cache: cache::Cache,
    permits: tokio::sync::Semaphore,
}

impl Fetcher {
    /// Build the fetcher, failing if its guarded HTTP client cannot be built.
    /// The client reuses no default transport: a [`guard::GuardedResolver`] vets
    /// every address at connect time, and redirects are re-validated per hop.
    pub fn new(settings: FetchSettings, user_agent: &str) -> Result<Self, reqwest::Error> {
        let redirects = settings.max_redirects;
        let allow_private = settings.allow_private_networks;
        let client = reqwest::Client::builder()
            .no_proxy()
            .user_agent(user_agent.to_string())
            .timeout(settings.timeout())
            .connect_timeout(Duration::from_secs(8))
            // The resolver is the trust boundary: it refuses a private address
            // even if a name's answer changed since `check_host` ran.
            .dns_resolver(std::sync::Arc::new(guard::GuardedResolver::new(
                settings.allow_private_networks,
            )))
            .redirect(reqwest::redirect::Policy::custom(
                move |attempt| match check_redirect(
                    attempt.url(),
                    attempt.previous().len(),
                    redirects,
                    allow_private,
                ) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(error),
                },
            ))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        let permits = tokio::sync::Semaphore::new(settings.max_concurrency.max(1));
        let cache = cache::Cache::new(settings.cache_ttl(), settings.cache_bytes);
        Ok(Fetcher {
            settings,
            client,
            cache,
            permits,
        })
    }

    /// Retrieve one URL, reusing recent answers from memory. A failure is returned, not
    /// panicked, so a batch caller can report partial success.
    ///
    /// A page that exists only to redirect the reader elsewhere (a trailing-slash
    /// or canonical-URL move, delivered by a meta refresh or a script) is
    /// followed to its target. Each target came from an untrusted page, so it is
    /// run back through the SSRF guard, and the chain is bounded so a redirect
    /// loop cannot spin.
    pub async fn fetch(&self, raw: &str) -> Result<Page, FetchError> {
        let mut target = raw.to_string();
        let mut hops = 0usize;
        loop {
            let fetched = self.fetch_one(&target).await?;
            let Some(next) = fetched.redirect.clone() else {
                return Ok(fetched);
            };
            hops += 1;
            match follow_target(
                &next,
                &target,
                hops,
                self.settings.max_redirects,
                self.settings.allow_private_networks,
            ) {
                Some(next) => {
                    target = next;
                }
                // A redirect that is unsafe, malformed, or looping stops here and
                // the stub itself is returned — never followed blindly.
                None => return Ok(fetched),
            }
        }
    }

    /// One request and extraction, with no redirect following.
    async fn fetch_one(&self, raw: &str) -> Result<Page, FetchError> {
        let parsed = url::Url::parse(raw).map_err(|_| FetchError::Scheme)?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(FetchError::Scheme);
        }
        guard::check_host(
            parsed.host_str().unwrap_or(""),
            self.settings.allow_private_networks,
        )
        .map_err(|e| FetchError::Refused(e.message()))?;

        if let Some(cached) = self.cache.get(raw) {
            return Ok(cached);
        }

        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| FetchError::Network("fetcher is shutting down".into()))?;

        let response = self
            .client
            .get(parsed.clone())
            .send()
            .await
            .map_err(|e| FetchError::Network(classify(&e)))?;

        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if status.as_u16() >= 400 {
            return Err(FetchError::Status(status.as_u16()));
        }

        let (body, truncated) = read_capped(response, self.settings.max_response_bytes)
            .await
            .map_err(FetchError::Network)?;

        // Extraction is CPU-bound and its types are not `Send`, so it runs on a
        // blocking thread with the body moved in and a plain `Page` returned. The
        // caller waits at most `fetch.timeout` for it, so a pathological document
        // cannot hold a fetch open; the abandoned thread finishes on its own.
        let kind = body_kind(&content_type, &body)?;
        if kind == Kind::Pdf && truncated {
            return Err(FetchError::Unsupported(
                "PDF larger than fetch.max_response_bytes".into(),
            ));
        }
        let base = final_url.clone();
        let declared = content_type.clone();
        let page = tokio::task::spawn_blocking(move || match kind {
            Kind::Html => Ok(extract::read(&decode(&body, &declared, true), &base)),
            Kind::Text => Ok(extract::Page::text(extract::sanitize(&decode(
                &body, &declared, false,
            )))),
            Kind::Pdf => extract::pdf(&body).map(extract::Page::text),
        });
        let page = tokio::time::timeout(self.settings.timeout(), page)
            .await
            .map_err(|_| FetchError::Network("extraction timed out".into()))?
            .map_err(|_| FetchError::Network("extraction failed".into()))?
            .map_err(FetchError::Unsupported)?;
        if page.text.trim().is_empty() {
            return Err(FetchError::Empty);
        }
        let text = page.text;

        let fetched_at = now_unix();
        let fetched = Page {
            url: raw.to_string(),
            fetched_at: Some(fetched_at),
            final_url: Some(final_url),
            status: status.as_u16(),
            content_type,
            title: Some(page.title).filter(|t| !t.is_empty()),
            byline: page.byline,
            published: page.published,
            site: page.site,
            text,
            truncated: Some(truncated).filter(|t| *t),
            redirect: page.redirect,
            error: None,
        };
        self.cache.put(raw, &fetched);
        Ok(fetched)
    }

    /// Fetch URLs concurrently, preserving input order.
    pub async fn fetch_many(&self, urls: &[String]) -> Vec<Page> {
        let mut futures = Vec::with_capacity(urls.len());
        for url in urls {
            futures.push(self.fetch(url));
        }
        let results = futures::future::join_all(futures).await;
        results
            .into_iter()
            .zip(urls)
            .map(|(outcome, url)| match outcome {
                Ok(fetched) => fetched,
                Err(error) => Page {
                    url: url.clone(),
                    fetched_at: None,
                    final_url: None,
                    status: 0,
                    content_type: String::new(),
                    title: None,
                    byline: None,
                    published: None,
                    site: None,
                    text: String::new(),
                    truncated: None,
                    redirect: None,
                    error: Some(error.message()),
                },
            })
            .collect()
    }
}

/// Validate HTTP redirects before reqwest can dial a literal address or hostname.
/// `hops` counts every URL visited so far, including the initial one, so the
/// budget allows exactly `maximum` redirects: hop `maximum + 1` is refused.
fn check_redirect(
    url: &url::Url,
    hops: usize,
    maximum: usize,
    allow_private: bool,
) -> Result<(), String> {
    if hops > maximum {
        return Err("too many redirects".into());
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsupported redirect scheme".into());
    }
    guard::check_host(url.host_str().unwrap_or_default(), allow_private)
        .map_err(|error| error.message())
}

/// Read a response body up to `max` bytes, reporting whether more remained.
/// The bytes are returned undecoded: the charset and content kind decide how
/// they become text.
async fn read_capped(response: reqwest::Response, max: u64) -> Result<(Vec<u8>, bool), String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut collected: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("body failed: {e}"))?;
        let remaining = max.saturating_sub(collected.len() as u64);
        if chunk.len() as u64 > remaining {
            let take = remaining as usize;
            collected.extend_from_slice(chunk.get(..take).unwrap_or(&chunk));
            return Ok((collected, true));
        }
        collected.extend_from_slice(&chunk);
    }
    Ok((collected, false))
}

/// Turn a transport error into a concise message that does not leak the URL.
/// A resolver refusal — the SSRF guard catching a name that resolves inside the
/// network — is preserved, because that is a deliberate refusal, not a failure.
fn classify(error: &reqwest::Error) -> String {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    for _ in 0..6 {
        let Some(current) = source else { break };
        let text = current.to_string();
        if text.contains("refusing") || text.contains("resolves to private") {
            return text;
        }
        source = current.source();
    }
    if error.is_timeout() {
        "request timed out".into()
    } else if error.is_connect() {
        "host could not be reached".into()
    } else {
        "request failed".into()
    }
}

/// How a response body is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Html,
    Text,
    Pdf,
}

/// Decide how to read a body from its declared media type, sniffing the bytes
/// when the server declares nothing useful or mislabels a PDF. Images, archives,
/// media and other binaries are refused by type rather than returned as noise.
fn body_kind(content_type: &str, body: &[u8]) -> Result<Kind, FetchError> {
    if body.starts_with(b"%PDF-") {
        return Ok(Kind::Pdf);
    }
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        "text/html" | "application/xhtml+xml" => Ok(Kind::Html),
        "application/pdf" => Ok(Kind::Pdf),
        "" | "application/octet-stream" | "binary/octet-stream" | "application/unknown" => {
            sniff(body)
        }
        textual
            if textual.starts_with("text/")
                || textual.ends_with("+json")
                || textual.ends_with("+xml")
                || matches!(
                    textual,
                    "application/json"
                        | "application/xml"
                        | "application/javascript"
                        | "application/x-javascript"
                        | "application/ecmascript"
                        | "application/yaml"
                        | "application/x-yaml"
                        | "application/toml"
                        | "application/x-sh"
                        | "application/sql"
                        | "application/x-ndjson"
                ) =>
        {
            Ok(Kind::Text)
        }
        other => Err(FetchError::Unsupported(format!(
            "unsupported content type {other}; Search reads HTML, text and PDF"
        ))),
    }
}

/// Classify an undeclared body: markup that opens like HTML is HTML, bytes
/// without NULs are text, and anything else is an unreadable binary.
fn sniff(body: &[u8]) -> Result<Kind, FetchError> {
    let head = body.get(..body.len().min(4096)).unwrap_or(body);
    let lowered = String::from_utf8_lossy(head).to_ascii_lowercase();
    let opening = lowered.trim_start_matches('\u{feff}').trim_start();
    if opening.starts_with("<!doctype html")
        || opening.starts_with("<html")
        || lowered.contains("<body")
    {
        return Ok(Kind::Html);
    }
    if head.contains(&0) {
        return Err(FetchError::Unsupported(
            "unsupported binary content; Search reads HTML, text and PDF".into(),
        ));
    }
    Ok(Kind::Text)
}

/// Decode a body using its byte-order mark, the declared `charset`, or for HTML
/// a `<meta>` charset near the start of the document, defaulting to UTF-8.
/// Undecodable bytes become replacement characters rather than errors.
fn decode(body: &[u8], content_type: &str, html: bool) -> String {
    let label =
        charset_param(content_type).or_else(|| if html { meta_charset(body) } else { None });
    let encoding = label
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()))
        // A document cannot declare itself UTF-16 from inside its own text;
        // only a byte-order mark selects it, which `decode` honors below.
        .filter(|encoding| *encoding != encoding_rs::UTF_16LE && *encoding != encoding_rs::UTF_16BE)
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(body);
    text.into_owned()
}

/// The `charset` parameter of a Content-Type header, if any.
fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// The charset an HTML document declares in a `<meta charset>` or
/// `<meta http-equiv="Content-Type" content="...; charset=...">` tag within its
/// first 4 KiB, the prescan window browsers use (with some slack).
fn meta_charset(body: &[u8]) -> Option<String> {
    let head = body.get(..body.len().min(4096)).unwrap_or(body);
    let lowered = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut rest = lowered.as_str();
    while let Some(start) = rest.find("<meta") {
        let tag = rest.get(start..)?;
        let end = tag.find('>').unwrap_or(tag.len());
        let attributes = tag.get(..end)?;
        if let Some(at) = attributes.find("charset") {
            let value = attributes
                .get(at + "charset".len()..)?
                .trim_start()
                .strip_prefix('=')?
                .trim_start()
                .trim_start_matches(['"', '\'']);
            let label: String = value
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                .collect();
            if !label.is_empty() {
                return Some(label);
            }
        }
        rest = tag.get(end..)?;
    }
    None
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_redirects_cannot_bypass_the_guard_with_literal_addresses() {
        for target in [
            "http://127.0.0.1/",
            "http://169.254.169.254/",
            "http://[::1]/",
            "http://metadata.google.internal/",
        ] {
            let url = url::Url::parse(target).unwrap();
            assert!(check_redirect(&url, 1, 5, false).is_err());
        }
        let public = url::Url::parse("https://example.com/").unwrap();
        assert!(check_redirect(&public, 1, 5, false).is_ok());
        assert!(check_redirect(&public, 1, 0, false).is_err());
        // The budget allows exactly `maximum` redirects, not maximum + 1.
        assert!(check_redirect(&public, 5, 5, false).is_ok());
        assert!(check_redirect(&public, 6, 5, false).is_err());
    }
    use crate::core::config::FetchSettings;

    fn fetcher() -> Fetcher {
        Fetcher::new(FetchSettings::default(), "search-test").expect("valid test client")
    }

    #[test]
    fn invalid_user_agent_fails_client_construction() {
        let result = Fetcher::new(FetchSettings::default(), "bad\nuser-agent");
        assert!(
            result.is_err(),
            "invalid user agents must not get a default client"
        );
    }

    #[tokio::test]
    async fn private_and_metadata_addresses_are_refused() {
        let fetcher = fetcher();
        for raw in [
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost:8642/healthz",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://2130706433/",
            "file:///etc/passwd",
        ] {
            let result = fetcher.fetch(raw).await;
            assert!(result.is_err(), "{raw} should be refused");
        }
    }

    #[tokio::test]
    async fn a_non_http_scheme_is_a_scheme_error() {
        let fetcher = fetcher();
        assert!(matches!(
            fetcher.fetch("ftp://example.com/x").await,
            Err(FetchError::Scheme)
        ));
    }

    /// A client-side redirect target is untrusted page content, so the follow
    /// decision must refuse a private destination, a non-http scheme, a loop,
    /// and anything past the hop budget.
    #[test]
    fn follow_target_guards_every_redirect() {
        let current = "https://example.com/a";
        // A public http(s) target is followed, bounded by the hop budget.
        assert_eq!(
            follow_target("https://example.com/b", current, 1, 3, false).as_deref(),
            Some("https://example.com/b")
        );
        // Private destinations are refused.
        for private in [
            "http://169.254.169.254/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://localhost/",
            "http://[64:ff9b::a9fe:a9fe]/",
        ] {
            assert_eq!(
                follow_target(private, current, 1, 3, false),
                None,
                "{private}"
            );
        }
        // A non-http scheme is refused.
        assert_eq!(
            follow_target("file:///etc/passwd", current, 1, 3, false),
            None
        );
        // A loop back to where we are is refused.
        assert_eq!(follow_target(current, current, 1, 3, false), None);
        // Past the hop budget, nothing more is followed.
        assert_eq!(
            follow_target("https://example.com/c", current, 4, 3, false),
            None
        );
        // The guard's escape hatch, used only by tests and mirrors.
        assert!(follow_target("http://127.0.0.1/", current, 1, 3, true).is_some());
        assert_eq!(
            follow_target("https://example.com/b", current, 1, 0, false),
            None
        );
    }

    /// Serve a fixed redirect chain: /hop1 -> ... -> /hopN -> /end, then stop.
    async fn serve_redirect_chain(hops: usize) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let path = String::from_utf8_lossy(&request);
                    let path = path.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let response = if path == "/end" {
                        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfinal"
                            .to_string()
                    } else {
                        let hop: usize = path.trim_start_matches("/hop").parse().unwrap_or(0);
                        let location = if hop >= hops {
                            "/end".to_string()
                        } else {
                            format!("/hop{}", hop + 1)
                        };
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        port
    }

    /// The HTTP redirect budget allows exactly `max_redirects` hops: a chain of
    /// that length reaches its final page, and one hop more is refused.
    #[tokio::test]
    async fn http_redirect_budget_allows_exactly_the_configured_hops() {
        let settings = FetchSettings {
            allow_private_networks: true,
            max_redirects: 5,
            ..FetchSettings::default()
        };
        let fetcher = Fetcher::new(settings, "search-test").expect("valid test client");

        let port = serve_redirect_chain(5).await;
        let page = fetcher
            .fetch(&format!("http://127.0.0.1:{port}/hop1"))
            .await
            .expect("a chain of exactly `max_redirects` hops is followed");
        assert_eq!(page.text, "final");

        let port = serve_redirect_chain(6).await;
        assert!(fetcher
            .fetch(&format!("http://127.0.0.1:{port}/hop1"))
            .await
            .is_err());
    }

    /// Serve fixed `(path, content type, body)` responses on a loopback port.
    async fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes = std::sync::Arc::new(routes);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let routes = std::sync::Arc::clone(&routes);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let (_, content_type, body) =
                        routes.iter().find(|(route, _, _)| *route == path).unwrap();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                });
            }
        });
        port
    }

    fn local_fetcher(max_response_bytes: u64) -> Fetcher {
        let settings = FetchSettings {
            allow_private_networks: true,
            max_response_bytes,
            ..FetchSettings::default()
        };
        Fetcher::new(settings, "search-test").unwrap()
    }

    fn article(marker: &str) -> String {
        format!(
            "<article><h1>Ownership</h1>{}</article>",
            (0..6)
                .map(|i| format!(
                    "<p>{marker} paragraph {i}: the borrow checker tracks ownership, \
                     moves and borrows at compile time, so memory safety costs nothing at runtime.</p>"
                ))
                .collect::<String>()
        )
    }

    /// Modern pages carry hundreds of kilobytes of inline script and style
    /// before the article. The whole article must survive, untruncated.
    #[tokio::test]
    async fn an_article_after_a_large_head_is_read_whole() {
        let script = format!("<script>var bundle = '{}';</script>", "x".repeat(400_000));
        let html = format!(
            "<html><head><title>Big</title>{script}</head><body>{}</body></html>",
            article("LATE")
        );
        let port = serve(vec![(
            "/big",
            "text/html; charset=utf-8",
            html.into_bytes(),
        )])
        .await;
        let page = local_fetcher(DEFAULT_TEST_BYTES)
            .fetch(&format!("http://127.0.0.1:{port}/big"))
            .await
            .unwrap();
        assert!(page.text.contains("LATE paragraph 5"), "{}", page.text);
        assert!(!page.text.contains("bundle"));
        assert_eq!(page.truncated, None);
    }

    const DEFAULT_TEST_BYTES: u64 = 16 << 20;

    /// A body beyond the byte bound is reported as truncated, not passed off as whole.
    #[tokio::test]
    async fn the_byte_bound_reports_truncation() {
        let text = "line of plain text\n".repeat(1000);
        let port = serve(vec![("/long.txt", "text/plain", text.into_bytes())]).await;
        let page = local_fetcher(1000)
            .fetch(&format!("http://127.0.0.1:{port}/long.txt"))
            .await
            .unwrap();
        assert_eq!(page.truncated, Some(true));
        assert!(page.text.chars().count() <= 1000);
    }

    /// Legacy encodings are decoded from the header or the document's own meta tag.
    #[tokio::test]
    async fn declared_charsets_are_decoded() {
        let sentence = "私はその人を常に先生と呼んでいた。だからここでもただ先生と書くだけで本名は打ち明けない。";
        let html = format!(
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=Shift_JIS\">\
             <title>こころ</title></head><body><article>{}</article></body></html>",
            format!("<p>{sentence}</p>").repeat(6)
        );
        let (shift_jis, _, _) = encoding_rs::SHIFT_JIS.encode(&html);
        let (latin, _, _) = encoding_rs::WINDOWS_1252.encode("<p>Café crème brûlée</p>");
        let port = serve(vec![
            ("/sjis", "text/html", shift_jis.into_owned()),
            (
                "/latin",
                "text/plain; charset=windows-1252",
                latin.into_owned(),
            ),
        ])
        .await;
        let fetcher = local_fetcher(DEFAULT_TEST_BYTES);
        let page = fetcher
            .fetch(&format!("http://127.0.0.1:{port}/sjis"))
            .await
            .unwrap();
        assert!(page.text.contains(sentence), "{}", page.text);
        let page = fetcher
            .fetch(&format!("http://127.0.0.1:{port}/latin"))
            .await
            .unwrap();
        assert!(page.text.contains("Café crème brûlée"), "{}", page.text);
    }

    /// A PDF's text layer is read, even when the server mislabels it.
    #[tokio::test]
    async fn pdf_text_is_extracted() {
        let pdf = minimal_pdf("Search reads portable documents");
        let port = serve(vec![
            ("/paper.pdf", "application/pdf", pdf.clone()),
            ("/download", "application/octet-stream", pdf),
        ])
        .await;
        let fetcher = local_fetcher(DEFAULT_TEST_BYTES);
        for path in ["/paper.pdf", "/download"] {
            let page = fetcher
                .fetch(&format!("http://127.0.0.1:{port}{path}"))
                .await
                .unwrap();
            assert!(
                page.text.contains("Search reads portable documents"),
                "{path}: {:?}",
                page.text
            );
        }
    }

    /// Images and other binaries are refused by type instead of returned as noise.
    #[tokio::test]
    async fn binary_bodies_are_refused() {
        let port = serve(vec![
            (
                "/logo.png",
                "image/png",
                b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
            ),
            ("/blob", "", b"\0\x01\x02binary".to_vec()),
        ])
        .await;
        let fetcher = local_fetcher(DEFAULT_TEST_BYTES);
        for path in ["/logo.png", "/blob"] {
            let error = fetcher
                .fetch(&format!("http://127.0.0.1:{port}{path}"))
                .await
                .unwrap_err();
            assert!(
                matches!(error, FetchError::Unsupported(_)),
                "{path}: {}",
                error.message()
            );
        }
    }

    /// A one-page PDF whose text layer is `text`, with a correct xref table.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_string(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            pdf.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        pdf
    }
}
