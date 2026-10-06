//! Adapter routing: a selected remote is exclusive and failures never fall back.
use search::{Fetched, Query, Response, Search};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operations shared by CLI and stdio MCP, executed on the hosting machine.
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase")]
pub enum Operation {
    Search { query: Query },
    Fetch { urls: Vec<String> },
    Index { query: String, limit: usize },
    Refresh,
    Providers,
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

/// Local or paired execution, chosen before opening any local corpus.
#[derive(Clone)]
pub enum Backend {
    Local(Arc<Search>),
    Remote {
        profile: Remote,
        client: reqwest::Client,
    },
}
impl Backend {
    /// Validate a saved pin and construct an exclusive remote backend.
    pub fn remote(profile: Remote) -> Result<Self, String> {
        Self::remote_with_timeout(profile, std::time::Duration::from_secs(120))
    }
    /// Construct a remote backend with a configured overall request deadline.
    pub fn remote_with_timeout(
        profile: Remote,
        timeout: std::time::Duration,
    ) -> Result<Self, String> {
        Ok(Self::Remote {
            client: client_with_timeout(&profile, timeout)?,
            profile,
        })
    }
    /// Execute one bounded operation with no retries or local fallback.
    pub async fn execute(&self, operation: Operation) -> Result<serde_json::Value, String> {
        match self {
            Self::Local(search) => match operation {
                Operation::Search { query } => serde_json::to_value(search.search(query).await),
                Operation::Fetch { urls } => serde_json::to_value(search.fetch(&urls).await),
                Operation::Index { query, limit } => serde_json::to_value(
                    search
                        .index_search(&query, limit)
                        .map_err(|e| e.to_string())?,
                ),
                Operation::Refresh => serde_json::to_value(search.refresh_seeded().await),
                Operation::Providers => serde_json::to_value(search.provider_names()),
            }
            .map_err(|e| e.to_string()),
            Self::Remote { profile, client } => {
                let response = client
                    .post(format!("{}/v1/execute", profile.url.trim_end_matches('/')))
                    .bearer_auth(&profile.secret)
                    .json(&operation)
                    .send()
                    .await
                    .map_err(|_| "paired remote connection failed; no local fallback")?;
                if !response.status().is_success() {
                    return Err(format!(
                        "paired remote returned {}; no local fallback",
                        response.status()
                    ));
                }
                read_response(response, 8 << 20).await
            }
        }
    }
    /// Search while preserving the engine's response shape.
    pub async fn search(&self, query: Query) -> Result<Response, String> {
        serde_json::from_value(self.execute(Operation::Search { query }).await?)
            .map_err(|e| e.to_string())
    }
    /// Fetch while preserving metadata and input order.
    pub async fn fetch(&self, urls: &[String]) -> Result<Vec<Fetched>, String> {
        serde_json::from_value(
            self.execute(Operation::Fetch {
                urls: urls.to_vec(),
            })
            .await?,
        )
        .map_err(|e| e.to_string())
    }
    /// Return the host's configured provider names.
    pub async fn providers(&self) -> Result<Vec<String>, String> {
        serde_json::from_value(self.execute(Operation::Providers).await?).map_err(|e| e.to_string())
    }
}
