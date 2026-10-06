//! Offline end-to-end pairing, exclusive routing, stdio MCP and revocation.
use search_cli::{
    remote,
    trust::{self, Devices, Host, Profiles, RemoteCredential},
};
use search_mcp::backend::{self, Backend, Operation, Remote};
use std::{path::PathBuf, sync::Arc};

/// Own an isolated HTTPS host, its corpus, and shutdown cleanup.
struct Fixture {
    root: PathBuf,
    host: Arc<Host>,
    url: String,
    handle: axum_server::Handle<std::net::SocketAddr>,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_window(std::time::Duration::from_secs(900)).await
    }
    async fn with_window(window: std::time::Duration) -> Self {
        let root = std::env::temp_dir().join(format!("search-remote-{}", uuid::Uuid::new_v4()));
        let host = Arc::new(Host::open(&root, "localhost", window).unwrap());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let config = search::Config {
            dir: root.clone(),
            address: address.to_string(),
            engines: search::config::EngineSettings {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        // Seed only the remote corpus; no public network calls are needed.
        let store = search::index::Store::open(&root, config.index.clone()).unwrap();
        store_fixture(&store);
        let engine = Arc::new(search::Search::open(config).unwrap());
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            host.cert.as_bytes().to_vec(),
            host.key.as_bytes().to_vec(),
        )
        .await
        .unwrap();
        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        let router = search_cli::http::router(
            engine,
            host.clone(),
            &format!("localhost:{}", address.port()),
        );
        let task = tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(server_handle)
                .serve(router.into_make_service())
                .await
                .unwrap();
        });
        Self {
            root,
            host,
            url: format!("https://localhost:{}", address.port()),
            handle,
            task,
        }
    }
    fn profile(&self) -> Remote {
        Remote {
            url: self.url.clone(),
            cert: self.host.cert.clone(),
            fingerprint: self.host.fingerprint.clone(),
            secret: String::new(),
            device: String::new(),
        }
    }
    async fn pair(&self) -> Remote {
        let mut profile = self.profile();
        let response = backend::client(&profile)
            .unwrap()
            .post(format!("{}/pair", self.url))
            .json(&serde_json::json!({"code": self.host.code, "name": "laptop"}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let credential: RemoteCredential = response.json().await.unwrap();
        profile.secret = credential.secret;
        profile.device = credential.device;
        profile
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.shutdown();
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Pairing pin checks precede code transmission and wrong TLS identities fail handshakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_trust_reuse_and_revoke_over_https() {
    let fixture = Fixture::new().await;
    let mut wrong = fixture.profile();
    wrong.fingerprint = "00".repeat(32);
    assert!(backend::client(&wrong).is_err());
    let other = Fixture::new().await;
    let mut wrong_host = other.profile();
    wrong_host.url = fixture.url.clone();
    assert!(backend::client(&wrong_host)
        .unwrap()
        .post(format!("{}/pair", fixture.url))
        .json(&serde_json::json!({"code": fixture.host.code, "name": "wrong"}))
        .send()
        .await
        .is_err());
    // A failed pin/handshake has not consumed the host code.
    let profile = fixture.pair().await;
    let client = backend::client(&profile).unwrap();
    assert_eq!(
        client
            .post(format!("{}/pair", fixture.url))
            .json(&serde_json::json!({"code": fixture.host.code, "name": "reuse"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let backend = Backend::remote(profile.clone()).unwrap();
    assert!(backend.execute(Operation::Refresh).await.is_ok());
    let mcp = client.post(format!("{}/mcp", fixture.url)).bearer_auth(&profile.secret)
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"paired-http","version":"1"}}})).send().await.unwrap();
    assert!(mcp.status().is_success());
    let host_settings = fixture.root.join("host-settings.json");
    std::fs::write(
        &host_settings,
        serde_json::json!({"dir": fixture.root}).to_string(),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_search");
    let devices = std::process::Command::new(binary)
        .args(["devices", "-config", host_settings.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(devices.status.success());
    assert!(String::from_utf8_lossy(&devices.stdout).contains(&profile.device));
    assert!(!String::from_utf8_lossy(&devices.stdout).contains(&profile.secret));
    assert!(std::process::Command::new(binary)
        .args([
            "revoke",
            &profile.device,
            "-config",
            host_settings.to_str().unwrap()
        ])
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        client
            .post(format!("{}/mcp", fixture.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let error = backend.execute(Operation::Refresh).await.unwrap_err();
    assert!(error.contains("401") && error.contains("no local fallback"));
}

/// CLI and the actual stdio MCP process read the same selection and remote corpus.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_and_stdio_route_all_operations_and_fail_explicitly() {
    use std::io::{BufRead, BufReader, Write};
    let fixture = Fixture::new().await;
    let client_root = fixture.root.join("client");
    std::fs::create_dir_all(&client_root).unwrap();
    let config_path = client_root.join("settings.json");
    std::fs::write(
        &config_path,
        serde_json::json!({"dir": client_root, "engines":{"enabled":false}}).to_string(),
    )
    .unwrap();
    let config = Some(config_path.to_string_lossy().into_owned());
    let path = remote::storage(config.clone()).unwrap();
    let binary = env!("CARGO_BIN_EXE_search");
    let certificate = fixture.host.path.join("host.pem");
    let mut pairing = std::process::Command::new(binary)
        .args([
            "remote",
            "pair",
            "home",
            &fixture.url,
            certificate.to_str().unwrap(),
            &fixture.host.fingerprint,
            "-config",
            config.as_deref().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(pairing.stdin.take().unwrap(), "{}", fixture.host.code).unwrap();
    let paired = pairing.wait_with_output().unwrap();
    assert!(
        paired.status.success(),
        "{}",
        String::from_utf8_lossy(&paired.stderr)
    );
    let profiles: Profiles = trust::read(&path.join("remotes.json")).unwrap();
    assert!(profiles.selected.is_none());
    assert!(!String::from_utf8_lossy(&paired.stdout).contains(&profiles.remotes["home"].secret));
    assert!(std::process::Command::new(binary)
        .args([
            "remote",
            "use",
            "home",
            "-config",
            config.as_deref().unwrap()
        ])
        .output()
        .unwrap()
        .status
        .success());
    let backend = remote::selected(config.clone()).unwrap().unwrap();
    let response = backend
        .search(search::Query {
            text: "needle".into(),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(response.results.iter().any(|r| r.title == "Remote needle"));
    let hits = backend
        .execute(Operation::Index {
            query: "needle".into(),
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(hits[0]["title"], "Remote needle");
    let pages = backend
        .fetch(&["http://127.0.0.1/private".into()])
        .await
        .unwrap();
    assert!(pages[0].error.is_some());
    assert_eq!(backend.execute(Operation::Refresh).await.unwrap(), 0);
    let output = std::process::Command::new(binary)
        .args([
            "index",
            "needle",
            "-json",
            "-config",
            config.as_deref().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Remote needle"));
    assert!(!client_root.join("search.db").exists());
    // Speak JSON-RPC to the real harness-facing stdio transport.
    let mut child = std::process::Command::new(binary)
        .args(["stdio", "-config", config.as_deref().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"offline-test","version":"1"}}})).unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("protocolVersion"));
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"web_search","arguments":{"queries":["needle"]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("Remote needle"), "{line}");
    trust::update(
        &fixture.host.path.join("devices.json"),
        |devices: &mut Devices| {
            devices.clear();
            Ok(())
        },
    )
    .unwrap();
    writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{"urls":["https://example.invalid"]}}})).unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    assert!(line.contains("no local fallback"), "{line}");
    drop(stdin);
    child.kill().unwrap();
    child.wait().unwrap();
    let output = std::process::Command::new(binary)
        .args(["refresh", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no local fallback"));
    // Explicitly selecting local execution works without any authentication.
    assert!(std::process::Command::new(binary)
        .args(["remote", "off", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    let local = std::process::Command::new(binary)
        .args([
            "index",
            "needle",
            "-json",
            "-config",
            config.as_deref().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(local.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&local.stdout).unwrap(),
        serde_json::json!([])
    );
}

/// Insert a unique host-only article using the engine's normal corpus contract.
fn store_fixture(store: &search::index::Store) {
    store
        .put(&search::index::Doc {
            url: "https://example.invalid/needle".into(),
            title: "Remote needle".into(),
            text: "needle in the remote corpus".into(),
            host: "example.invalid".into(),
            fetched_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        })
        .unwrap();
}

/// Even a redirect to a trusted host must never relay a device credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_clients_refuse_redirects() {
    let fixture = Fixture::new().await;
    let mut profile = fixture.pair().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    profile.url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let target = format!("{}/v1/execute", fixture.url);
    let router = axum::Router::new().route(
        "/v1/execute",
        axum::routing::post(move || async move { axum::response::Redirect::temporary(&target) }),
    );
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        fixture.host.cert.as_bytes().to_vec(),
        fixture.host.key.as_bytes().to_vec(),
    )
    .await
    .unwrap();
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(server_handle)
            .serve(router.into_make_service())
            .await
            .unwrap();
    });
    let error = Backend::remote(profile)
        .unwrap()
        .execute(Operation::Refresh)
        .await
        .unwrap_err();
    assert!(error.contains("307"), "{error}");
    handle.shutdown();
    task.await.unwrap();
}

/// An expired code is refused through the HTTPS endpoint, without enrolling a device.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_pairing_code_is_refused_over_https() {
    let fixture = Fixture::with_window(std::time::Duration::ZERO).await;
    let response = backend::client(&fixture.profile())
        .unwrap()
        .post(format!("{}/pair", fixture.url))
        .json(&serde_json::json!({"code": fixture.host.code, "name": "expired"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert!(
        trust::read::<Devices>(&fixture.host.path.join("devices.json"))
            .unwrap()
            .is_empty()
    );
}

/// A local command renews a live HTTPS host, retaining existing devices and persisted bounds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_code_renews_a_running_host_with_persisted_attempts() {
    let fixture = Fixture::new().await;
    let existing = fixture.pair().await;
    let settings = fixture.root.join("host-settings.json");
    std::fs::write(
        &settings,
        serde_json::json!({"dir": fixture.root}).to_string(),
    )
    .unwrap();
    let renew = || {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_search"))
            .args(["pair-code", "-config", settings.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let code = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert_eq!(code.len(), 64);
        let stored = std::fs::read_to_string(fixture.host.path.join("pairing.json")).unwrap();
        assert!(!stored.contains(&code));
        code
    };
    let client = backend::client(&fixture.profile()).unwrap();
    let pair = |code: String| {
        client
            .post(format!("{}/pair", fixture.url))
            .json(&serde_json::json!({"code": code, "name": "another"}))
            .send()
    };
    let exhausted = renew();
    // The old startup code cannot be used, even though the server still holds its initial display.
    assert_eq!(pair(fixture.host.code.clone()).await.unwrap().status(), 403);
    for _ in 0..19 {
        assert_eq!(pair("wrong".into()).await.unwrap().status(), 403);
    }
    let state: serde_json::Value = trust::read(&fixture.host.path.join("pairing.json")).unwrap();
    assert_eq!(state["attempts"], 20);
    assert_eq!(pair(exhausted).await.unwrap().status(), 403);
    let superseded = renew();
    let code = renew();
    assert_eq!(pair(superseded).await.unwrap().status(), 403);
    let (first, second) = tokio::join!(pair(code.clone()), pair(code.clone()));
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(statuses.iter().filter(|s| s.is_success()).count(), 1);
    assert_eq!(statuses.iter().filter(|s| **s == 403).count(), 1);
    assert_eq!(pair(code).await.unwrap().status(), 403);
    assert!(Backend::remote(existing)
        .unwrap()
        .execute(Operation::Refresh)
        .await
        .is_ok());
}
