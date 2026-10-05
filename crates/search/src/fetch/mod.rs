//! Fetch: retrieve URLs on behalf of callers and turn HTML into readable text.
//!
//! Because URLs are model-chosen, every request is treated as hostile:
//! destinations are resolved and refused if they point inside the network, each
//! redirect hop is re-checked, bodies are size-capped, and deadlines are
//! enforced. Every successfully fetched page is indexed, so the private corpus
//! grows from real use rather than from crawling.

mod cache;
mod extract;
mod guard;

use std::time::Duration;

use crate::config::FetchSettings;
use crate::index::{self, Store};

pub use guard::{is_public_ip, GuardError};

/// The most client-side redirects followed before giving up, so a loop of
/// redirect stubs cannot spin the fetcher.
const MAX_REDIRECT_HOPS: usize = 3;

/// Decide whether to follow a client-side redirect target. The target came from
/// an untrusted page, so it is accepted only when it is a well-formed http(s)
/// URL that passes the SSRF guard, does not loop, and is within the hop budget.
/// Returns the URL to fetch next, or `None` to stop.
fn follow_target(next: &str, current: &str, hops: usize, allow_private: bool) -> Option<String> {
    if hops > MAX_REDIRECT_HOPS || next == current {
        return None;
    }
    let parsed = url::Url::parse(next).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    guard::check_host(parsed.host_str().unwrap_or(""), allow_private).ok()?;
    Some(next.to_string())
}

/// The outcome of one fetch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Fetched {
    pub url: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed: Option<bool>,
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
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::Scheme => "only http and https URLs are supported".into(),
            FetchError::Refused(reason) => reason.clone(),
            FetchError::Network(reason) => reason.clone(),
            FetchError::Status(code) => format!("HTTP {code}"),
            FetchError::Empty => "no readable text in response".into(),
        }
    }
}

/// Holds the guarded client, the settings, and the index.
pub struct Fetcher {
    settings: FetchSettings,
    client: reqwest::Client,
    store: std::sync::Arc<Store>,
    cache: cache::TtlCache,
    permits: tokio::sync::Semaphore,
}

impl Fetcher {
    /// Build the fetcher, failing if its guarded HTTP client cannot be built.
    /// The client reuses no default transport: a [`guard::GuardedResolver`] vets
    /// every address at connect time, and redirects are re-validated per hop.
    pub fn new(
        settings: FetchSettings,
        store: std::sync::Arc<Store>,
        user_agent: &str,
    ) -> Result<Self, reqwest::Error> {
        let redirects = settings.max_redirects.max(1);
        let client = reqwest::Client::builder()
            .user_agent(user_agent.to_string())
            .timeout(settings.timeout())
            .connect_timeout(Duration::from_secs(8))
            // The resolver is the trust boundary: it refuses a private address
            // even if a name's answer changed since `check_host` ran.
            .dns_resolver(std::sync::Arc::new(guard::GuardedResolver::new(
                settings.allow_private,
            )))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= redirects {
                    attempt.error("too many redirects")
                } else {
                    attempt.follow()
                }
            }))
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        let permits = tokio::sync::Semaphore::new(settings.max_concurrency.max(1));
        let cache = cache::TtlCache::new(settings.cache_ttl());
        Ok(Fetcher {
            settings,
            client,
            store,
            cache,
            permits,
        })
    }

    /// Retrieve one URL, indexing it when configured. A failure is returned, not
    /// panicked, so a batch caller can report partial success.
    ///
    /// A page that exists only to redirect the reader elsewhere (a trailing-slash
    /// or canonical-URL move, delivered by a meta refresh or a script) is
    /// followed to its target. Each target came from an untrusted page, so it is
    /// run back through the SSRF guard, and the chain is bounded so a redirect
    /// loop cannot spin.
    pub async fn fetch(&self, raw: &str) -> Result<Fetched, FetchError> {
        let mut target = raw.to_string();
        let mut hops = 0usize;
        loop {
            let fetched = self.fetch_one(&target).await?;
            let Some(next) = fetched.redirect.clone() else {
                return Ok(fetched);
            };
            hops += 1;
            match follow_target(&next, &target, hops, self.settings.allow_private) {
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
    async fn fetch_one(&self, raw: &str) -> Result<Fetched, FetchError> {
        let parsed = url::Url::parse(raw).map_err(|_| FetchError::Scheme)?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(FetchError::Scheme);
        }
        guard::check_host(parsed.host_str().unwrap_or(""), self.settings.allow_private)
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

        let (body, truncated) = read_capped(response, self.settings.max_bytes)
            .await
            .map_err(FetchError::Network)?;

        // Extraction is CPU-bound and its types are not `Send`, so it runs on a
        // blocking thread with the body moved in and a plain `Page` returned.
        let (page, final_url) = if is_html(&content_type) {
            let url_for_links = final_url.clone();
            let page = tokio::task::spawn_blocking(move || extract::read(&body, &url_for_links))
                .await
                .map_err(|e| FetchError::Network(format!("extraction task failed: {e}")))?;
            (page, final_url)
        } else {
            let text = extract::sanitize(body.as_bytes());
            (
                extract::Page {
                    title: String::new(),
                    byline: None,
                    published: None,
                    site: None,
                    text,
                    redirect: None,
                },
                final_url,
            )
        };
        if page.text.trim().is_empty() {
            return Err(FetchError::Empty);
        }
        let text = page.text;

        let mut indexed = None;
        if self.settings.should_index() {
            let stored = index::cap_chars(&text, self.settings.index_text_chars.max(1));
            let doc = index::Doc {
                url: final_url.clone(),
                title: page.title.clone(),
                text: stored,
                host: index::host_of(&final_url),
                fetched_at: now_unix(),
            };
            indexed = Some(self.store.put(&doc).is_ok());
        }

        let fetched = Fetched {
            url: raw.to_string(),
            final_url: Some(final_url),
            status: status.as_u16(),
            content_type,
            title: Some(page.title).filter(|t| !t.is_empty()),
            byline: page.byline,
            published: page.published,
            site: page.site,
            text,
            truncated: Some(truncated).filter(|t| *t),
            indexed,
            redirect: page.redirect,
            error: None,
        };
        self.cache.put(raw, &fetched);
        Ok(fetched)
    }

    /// Fetch URLs concurrently, preserving input order.
    pub async fn fetch_many(&self, urls: &[String]) -> Vec<Fetched> {
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
                Err(error) => Fetched {
                    url: url.clone(),
                    final_url: None,
                    status: 0,
                    content_type: String::new(),
                    title: None,
                    byline: None,
                    published: None,
                    site: None,
                    text: String::new(),
                    truncated: None,
                    indexed: None,
                    redirect: None,
                    error: Some(error.message()),
                },
            })
            .collect()
    }
}

/// Read a response body up to `max` bytes, reporting whether more remained.
async fn read_capped(response: reqwest::Response, max: u64) -> Result<(String, bool), String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut collected: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("body failed: {e}"))?;
        let remaining = max.saturating_sub(total);
        if chunk.len() as u64 > remaining {
            let take = remaining as usize;
            collected.extend_from_slice(chunk.get(..take).unwrap_or(&chunk));
            return Ok((extract::sanitize(&collected), true));
        }
        collected.extend_from_slice(&chunk);
        total += chunk.len() as u64;
    }
    Ok((extract::sanitize(&collected), false))
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

fn is_html(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.contains("html") || ct.contains("xhtml")
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
    use crate::config::FetchSettings;

    fn fetcher() -> Fetcher {
        let dir = std::env::temp_dir().join(format!("search-fetch-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(
            Store::open(&dir, crate::config::IndexSettings::default()).expect("store"),
        );
        Fetcher::new(FetchSettings::default(), store, "search-test").expect("valid test client")
    }

    #[test]
    fn invalid_user_agent_fails_client_construction() {
        let dir = std::env::temp_dir().join(format!("search-fetch-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(
            Store::open(&dir, crate::config::IndexSettings::default()).expect("store"),
        );
        let result = Fetcher::new(FetchSettings::default(), store, "bad\nuser-agent");
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
            follow_target("https://example.com/b", current, 1, false).as_deref(),
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
            assert_eq!(follow_target(private, current, 1, false), None, "{private}");
        }
        // A non-http scheme is refused.
        assert_eq!(follow_target("file:///etc/passwd", current, 1, false), None);
        // A loop back to where we are is refused.
        assert_eq!(follow_target(current, current, 1, false), None);
        // Past the hop budget, nothing more is followed.
        assert_eq!(
            follow_target(
                "https://example.com/c",
                current,
                MAX_REDIRECT_HOPS + 1,
                false
            ),
            None
        );
        // The guard's escape hatch, used only by tests and mirrors.
        assert!(follow_target("http://127.0.0.1/", current, 1, true).is_some());
    }
}
