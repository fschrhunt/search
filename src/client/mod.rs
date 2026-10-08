//! Shared local and remote execution for CLI and MCP, and the input bounds every
//! surface enforces. Remote calls use the host's REST API and never fall back.
use crate::core::{Answer, Page, Query, Search};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Most results one query may request, before the host's `search.max_results` cap.
pub const MAX_LIMIT: usize = 50;
/// Most URLs one fetch may read.
pub const MAX_URLS: usize = 10;

/// Trim one search query, refusing empty text and text over 512 bytes.
pub fn validate_query(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        Err("query must not be empty".into())
    } else if text.len() > 512 {
        Err("query must be 512 bytes or fewer".into())
    } else {
        Ok(text.to_string())
    }
}

/// Bound a whole search request: a valid query and a limit capped at [`MAX_LIMIT`].
/// A zero limit is kept, meaning the host's configured maximum.
pub fn validate_search(query: Query) -> Result<Query, String> {
    Ok(Query {
        text: validate_query(&query.text)?,
        limit: query.limit.min(MAX_LIMIT),
        engines: query.engines,
    })
}

/// Require one to [`MAX_URLS`] URLs, each nonempty and at most 8192 bytes.
pub fn validate_urls(urls: &[String]) -> Result<(), String> {
    if urls.is_empty() || urls.len() > MAX_URLS {
        return Err("provide between one and ten URLs".into());
    }
    if urls
        .iter()
        .any(|url| url.trim().is_empty() || url.len() > 8192)
    {
        return Err("URLs must be nonempty and at most 8192 bytes".into());
    }
    Ok(())
}

/// A saved credential. Keep it in an owner-only sidecar, never in settings.
#[derive(Clone, Serialize, Deserialize)]
pub struct Remote {
    pub url: String,
    pub cert: String,
    pub fingerprint: String,
    pub secret: String,
    pub device: String,
}

/// Hash the single PEM certificate, rejecting empty or multi-certificate input.
pub fn fingerprint(pem: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let certs = rustls_pemfile::certs(&mut pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid certificate")?;
    if certs.len() != 1 {
        return Err("expected one certificate".into());
    }
    let cert = certs.first().ok_or("missing certificate")?;
    Ok(Sha256::digest(cert.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Build a client trusting only the pinned identity, using normal TLS validation.
pub fn client(remote: &Remote) -> Result<reqwest::Client, String> {
    client_with_timeout(remote, std::time::Duration::from_secs(120))
}

/// Build the pinned client with the caller's configured overall deadline.
pub fn client_with_timeout(
    remote: &Remote,
    timeout: std::time::Duration,
) -> Result<reqwest::Client, String> {
    let url = reqwest::Url::parse(&remote.url).map_err(|_| "invalid remote URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("remote must be an HTTPS origin without a path or user information".into());
    }
    if fingerprint(&remote.cert)? != remote.fingerprint {
        return Err("certificate fingerprint mismatch".into());
    }
    let cert = rustls_pemfile::certs(&mut remote.cert.as_bytes())
        .next()
        .ok_or("missing certificate")?
        .map_err(|_| "invalid certificate")?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(cert.clone())
        .map_err(|_| "invalid root certificate")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|_| "cannot build TLS verifier")?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| "invalid TLS versions")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier { cert, verifier }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .map_err(|_| "cannot build remote client".into())
}

/// Cap even chunked responses before JSON decoding, including pairing credentials.
pub async fn read_response<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<T, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "remote response failed")?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err("remote response too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "invalid remote response".into())
}

/// Enforce the exact leaf pin before standard WebPKI chain, name, expiry and signature checks.
#[derive(Debug)]
struct PinnedVerifier {
    cert: rustls::pki_types::CertificateDer<'static>,
    verifier: Arc<rustls::client::WebPkiServerVerifier>,
}
impl rustls::client::danger::ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        cert: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if cert != &self.cert {
            return Err(rustls::Error::General("certificate pin mismatch".into()));
        }
        self.verifier
            .verify_server_cert(cert, intermediates, name, ocsp, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls12_signature(message, cert, signature)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls13_signature(message, cert, signature)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}

/// Local or paired execution, selected before opening the local engine.
#[derive(Clone)]
pub enum Client {
    Local(Arc<Search>),
    Remote {
        profile: Remote,
        client: reqwest::Client,
        max_response_bytes: usize,
    },
}
impl Client {
    /// Validate a saved pin and construct an exclusive remote client.
    pub fn remote(profile: Remote) -> Result<Self, String> {
        Self::remote_with_settings(profile, &crate::core::config::RemoteSettings::default())
    }
    /// Construct a remote client with a configured overall request deadline.
    pub fn remote_with_timeout(
        profile: Remote,
        timeout: std::time::Duration,
    ) -> Result<Self, String> {
        Self::remote_with_settings(
            profile,
            &crate::core::config::RemoteSettings {
                timeout: u64::try_from(timeout.as_millis())
                    .map_err(|_| "remote timeout too large")?,
                ..Default::default()
            },
        )
    }
    /// Construct a pinned client with configured deadline and response bounds.
    pub fn remote_with_settings(
        profile: Remote,
        settings: &crate::core::config::RemoteSettings,
    ) -> Result<Self, String> {
        if settings.timeout == 0 || settings.max_response_bytes == 0 {
            return Err("remote limits must be greater than zero".into());
        }
        Ok(Self::Remote {
            client: client_with_timeout(&profile, settings.timeout())?,
            max_response_bytes: settings.max_response_bytes,
            profile,
        })
    }
    /// Search within shared bounds, preserving the engine's response shape.
    pub async fn search(&self, query: Query) -> Result<Answer, String> {
        let query = validate_search(query)?;
        match self {
            Self::Local(search) => Ok(search.search(query).await),
            Self::Remote {
                profile,
                client,
                max_response_bytes,
            } => {
                let mut params = vec![("q", query.text), ("limit", query.limit.to_string())];
                if !query.engines.is_empty() {
                    params.push(("engines", query.engines.join(",")));
                }
                let request = client.get(endpoint(profile, "v1/search")).query(&params);
                call(request, profile, *max_response_bytes).await
            }
        }
    }
    /// Fetch within shared bounds, preserving metadata and input order.
    pub async fn fetch(&self, urls: &[String]) -> Result<Vec<Page>, String> {
        validate_urls(urls)?;
        match self {
            Self::Local(search) => Ok(search.fetch(urls).await),
            Self::Remote {
                profile,
                client,
                max_response_bytes,
            } => {
                #[derive(Deserialize)]
                struct Pages {
                    pages: Vec<Page>,
                }
                let request = client
                    .post(endpoint(profile, "v1/fetch"))
                    .json(&serde_json::json!({ "urls": urls }));
                let pages: Pages = call(request, profile, *max_response_bytes).await?;
                Ok(pages.pages)
            }
        }
    }
    /// Return the host's selected engine names.
    pub async fn engines(&self) -> Result<Vec<String>, String> {
        match self {
            Self::Local(search) => Ok(search.engine_names()),
            Self::Remote {
                profile,
                client,
                max_response_bytes,
            } => {
                #[derive(Deserialize)]
                struct Status {
                    engines: Vec<String>,
                }
                let request = client.get(endpoint(profile, "v1/status"));
                let status: Status = call(request, profile, *max_response_bytes).await?;
                Ok(status.engines)
            }
        }
    }
}

/// Send one device-authenticated request with no retries or local fallback,
/// decoding a JSON answer of at most `max_bytes`.
async fn call<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
    profile: &Remote,
    max_bytes: usize,
) -> Result<T, String> {
    let response = request
        .bearer_auth(&profile.secret)
        .send()
        .await
        .map_err(|_| "paired remote connection failed; no local fallback")?;
    if !response.status().is_success() {
        return Err(format!(
            "paired remote returned {}; no local fallback",
            response.status()
        ));
    }
    read_response(response, max_bytes).await
}

/// The absolute URL of one API path on the paired origin.
fn endpoint(profile: &Remote, path: &str) -> String {
    format!("{}/{path}", profile.url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_bounds_trim_text_and_cap_but_keep_a_zero_limit() {
        let bounded = |text: &str, limit| {
            validate_search(Query {
                text: text.into(),
                limit,
                engines: Vec::new(),
            })
        };
        let query = bounded(" rust ", 500).unwrap();
        assert_eq!((query.text.as_str(), query.limit), ("rust", MAX_LIMIT));
        assert_eq!(bounded("rust", 0).unwrap().limit, 0);
        assert!(bounded(" ", 10).is_err());
        assert!(bounded(&"x".repeat(513), 10).is_err());
    }

    #[test]
    fn fetch_bounds_refuse_empty_oversized_and_too_many_urls() {
        let url = |u: &str| vec![u.to_string()];
        assert!(validate_urls(&url("https://example.com")).is_ok());
        assert!(validate_urls(&[]).is_err());
        assert!(validate_urls(&url(" ")).is_err());
        assert!(validate_urls(&url(&"x".repeat(8193))).is_err());
        assert!(validate_urls(&vec!["https://example.com".into(); MAX_URLS + 1]).is_err());
    }
}
