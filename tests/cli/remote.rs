//! Offline end-to-end pairing, exclusive routing, stdio MCP and revocation.
#[cfg(windows)]
#[path = "../support/windows.rs"]
mod windows;
use search::cli::auth::{self, Credential, Devices, Host, Remotes};
use search::client::{self as client_api, Client, Remote};
use std::{path::PathBuf, sync::Arc};

/// Own an isolated HTTPS host, its offline settings-defined engine, and shutdown cleanup.
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
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-remote-{}", uuid::Uuid::new_v4()));
        let host = Arc::new(Host::open(&root, "localhost", window).unwrap());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let config = search::core::Config {
            home: root.clone(),
            address: address.to_string(),
            engines: search::core::config::EngineSettings {
                use_engines: vec!["fixture".into()],
                config: [("fixture".into(), fixture_engine(&root))].into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let engine = Arc::new(search::core::Search::open(config).unwrap());
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            host.cert.as_bytes().to_vec(),
            host.key.as_bytes().to_vec(),
        )
        .await
        .unwrap();
        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        let router = search::cli::http::router(
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
        let response = client_api::client(&profile)
            .unwrap()
            .post(format!("{}/pair", self.url))
            .json(&serde_json::json!({"code": self.host.code, "name": "laptop"}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let credential: Credential = response.json().await.unwrap();
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

/// Paired responses honor local bounds without retrying or switching targets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_response_limit_is_configurable_and_fails_without_fallback() {
    let fixture = Fixture::new().await;
    let profile = fixture.pair().await;
    let tiny = search::core::config::RemoteSettings {
        max_response_bytes: 1,
        ..Default::default()
    };
    let client = Client::remote_with_settings(profile.clone(), &tiny).unwrap();
    assert!(client.engines().await.unwrap_err().contains("too large"));
    let client = Client::remote(profile).unwrap();
    assert_eq!(client.engines().await.unwrap(), vec!["fixture"]);
}

/// Pairing pin checks precede code transmission and wrong TLS identities fail handshakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_trust_reuse_and_revoke_over_https() {
    let fixture = Fixture::new().await;
    let mut wrong = fixture.profile();
    wrong.fingerprint = "00".repeat(32);
    assert!(client_api::client(&wrong).is_err());
    let other = Fixture::new().await;
    let mut wrong_host = other.profile();
    wrong_host.url = fixture.url.clone();
    assert!(client_api::client(&wrong_host)
        .unwrap()
        .post(format!("{}/pair", fixture.url))
        .json(&serde_json::json!({"code": fixture.host.code, "name": "wrong"}))
        .send()
        .await
        .is_err());
    // A failed pin/handshake has not consumed the host code.
    let profile = fixture.pair().await;
    let client = client_api::client(&profile).unwrap();
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
    let execution = Client::remote(profile.clone()).unwrap();
    assert!(execution.engines().await.is_ok());
    let mcp = client.post(format!("{}/mcp", fixture.url)).bearer_auth(&profile.secret)
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"paired-http","version":"1"}}})).send().await.unwrap();
    assert!(mcp.status().is_success());
    let host_settings = fixture.root.join("host-settings.json");
    std::fs::write(&host_settings, serde_json::json!({}).to_string()).unwrap();
    let binary = env!("CARGO_BIN_EXE_search");
    let devices = local_command(binary, &fixture.root)
        .args(["devices", "-config", host_settings.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(devices.status.success());
    assert!(String::from_utf8_lossy(&devices.stdout).contains(&profile.device));
    assert!(!String::from_utf8_lossy(&devices.stdout).contains(&profile.secret));
    assert!(local_command(binary, &fixture.root)
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
    let error = execution.engines().await.unwrap_err();
    assert!(error.contains("401") && error.contains("no local fallback"));
}

/// CLI and the actual stdio MCP process read the same selection and remote engine results.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_and_stdio_route_all_operations_and_fail_explicitly() {
    use std::io::{BufRead, BufReader, Write};
    let fixture = Fixture::new().await;
    let client_root = fixture.root.join("client");
    std::fs::create_dir_all(&client_root).unwrap();
    #[cfg(windows)]
    windows::secure(&client_root);
    let config_path = client_root.join("settings.json");
    std::fs::write(
        &config_path,
        serde_json::json!({"engines":{"enabled":false}}).to_string(),
    )
    .unwrap();
    let config = Some(config_path.to_string_lossy().into_owned());
    let path = auth::dir(&client_root).unwrap();
    let binary = env!("CARGO_BIN_EXE_search");
    let certificate = fixture.host.path.join("host.pem");
    let mut pairing = local_command(binary, &client_root)
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
    let remotes: Remotes = auth::read(&path.join("remotes.json")).unwrap();
    assert!(remotes.selected.is_none());
    assert!(!String::from_utf8_lossy(&paired.stdout).contains(&remotes.remotes["home"].secret));
    assert!(local_command(binary, &client_root)
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
    let remotes: Remotes = auth::read(&path.join("remotes.json")).unwrap();
    let client =
        Client::remote(remotes.remotes[remotes.selected.as_ref().unwrap()].clone()).unwrap();
    let response = client
        .search(search::core::Query {
            text: "needle".into(),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(response.results.iter().any(|r| r.title == "Remote needle"));
    let urls = ["http://127.0.0.1/first", "http://127.0.0.1/second"].map(String::from);
    let pages = client.fetch(&urls).await.unwrap();
    assert_eq!(
        pages.iter().map(|p| &p.url).collect::<Vec<_>>(),
        [&urls[0], &urls[1]]
    );
    assert!(pages.iter().all(|page| page.error.is_some()));
    let output = local_command(binary, &client_root)
        .args(["needle", "-json", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Remote needle"));
    // Speak JSON-RPC to the real harness-facing stdio transport.
    let mut child = local_command(binary, &client_root)
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
    auth::update(
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
    let output = local_command(binary, &client_root)
        .args(["needle", "-json", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no local fallback"));
    // Explicitly selecting local execution works without any authentication.
    assert!(local_command(binary, &client_root)
        .args(["remote", "off", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    let local = local_command(binary, &client_root)
        .args(["needle", "-json", "-config", config.as_deref().unwrap()])
        .output()
        .unwrap();
    assert!(local.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&local.stdout).unwrap()["results"],
        serde_json::json!([])
    );
}

/// Define trusted host code that returns a unique result without network access.
fn fixture_engine(root: &std::path::Path) -> serde_json::Value {
    std::fs::write(
        root.join("program.py"),
        r#"import json, sys
request = json.load(sys.stdin)
assert request['version'] == 1
assert request['query'] == 'needle'
json.dump({'results':[{'url':'https://example.invalid/needle','title':'Remote needle','snippet':'needle from the remote engine'}]}, sys.stdout)
"#,
    )
    .unwrap();
    serde_json::json!({
        "type": "command",
        "command": if cfg!(windows) { "python.exe" } else { "python3" },
        "args": ["program.py"],
        "cwd": root
    })
}

/// Keep each CLI subprocess's home isolated without mutating the test runner environment.
fn local_command(binary: &str, home: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command
        .env("SEARCH_HOME", home)
        .env_remove("CONFIG")
        .env_remove("ADDRESS");
    command
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
    let target = format!("{}/v1/status", fixture.url);
    let router = axum::Router::new().route(
        "/v1/status",
        axum::routing::get(move || async move { axum::response::Redirect::temporary(&target) }),
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
    let error = Client::remote(profile)
        .unwrap()
        .engines()
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
    let response = client_api::client(&fixture.profile())
        .unwrap()
        .post(format!("{}/pair", fixture.url))
        .json(&serde_json::json!({"code": fixture.host.code, "name": "expired"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert!(
        auth::read::<Devices>(&fixture.host.path.join("devices.json"))
            .unwrap()
            .is_empty()
    );
}

/// A local command renews a live HTTPS host, retaining existing devices; each code admits one device.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_code_renews_a_running_host() {
    let fixture = Fixture::new().await;
    let existing = fixture.pair().await;
    let settings = fixture.root.join("host-settings.json");
    std::fs::write(&settings, serde_json::json!({}).to_string()).unwrap();
    let renew = || {
        let output = local_command(env!("CARGO_BIN_EXE_search"), &fixture.root)
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
    let client = client_api::client(&fixture.profile()).unwrap();
    let pair = |code: String| {
        client
            .post(format!("{}/pair", fixture.url))
            .json(&serde_json::json!({"code": code, "name": "another"}))
            .send()
    };
    let superseded = renew();
    let code = renew();
    // The old startup code cannot be used, even though the server still holds its initial display.
    assert_eq!(pair(fixture.host.code.clone()).await.unwrap().status(), 403);
    assert_eq!(pair(superseded).await.unwrap().status(), 403);
    let (first, second) = tokio::join!(pair(code.clone()), pair(code.clone()));
    let statuses = [first.unwrap().status(), second.unwrap().status()];
    assert_eq!(statuses.iter().filter(|s| s.is_success()).count(), 1);
    assert_eq!(statuses.iter().filter(|s| **s == 403).count(), 1);
    assert_eq!(pair(code).await.unwrap().status(), 403);
    assert!(Client::remote(existing).unwrap().engines().await.is_ok());
}

/// The host enforces the shared input bounds itself rather than trusting paired clients.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_refuses_out_of_bounds_input() {
    let fixture = Fixture::new().await;
    let profile = fixture.pair().await;
    let client = client_api::client(&profile).unwrap();
    let search = client
        .get(format!("{}/v1/search", fixture.url))
        .query(&[("q", "x".repeat(513))])
        .bearer_auth(&profile.secret)
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), 400);
    let fetch = client
        .post(format!("{}/v1/fetch", fixture.url))
        .json(&serde_json::json!({"urls": vec!["https://example.com"; 11]}))
        .bearer_auth(&profile.secret)
        .send()
        .await
        .unwrap();
    assert_eq!(fetch.status(), 400);
}
