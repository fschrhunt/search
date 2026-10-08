//! Configured GET and executable engines, with bounded output and opaque errors.
//!
//! Both transports map an upstream JSON answer into `Found`: invalid rows are
//! skipped and counted, but an answer whose every row is invalid is malformed.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{Engine, EngineError, EngineFuture, Found};
use crate::core::config::engines::http_url;
use crate::core::config::{Adapter as Settings, Command, Http};
use crate::core::fetch::guard::{check_host, GuardedResolver};
use crate::core::Link;

/// An owned configured identifier and its query transport.
pub(super) struct Adapter {
    name: String,
    transport: Transport,
}

/// HTTP owns a reusable guarded client; commands retain only their launch settings.
enum Transport {
    Http(Http, reqwest::Client),
    Command(Command),
}

impl Adapter {
    /// Build HTTP clients at startup; credentials remain resolved for each query.
    pub(super) fn new(
        name: String,
        settings: Settings,
        user_agent: &str,
    ) -> Result<Self, reqwest::Error> {
        let transport = match settings {
            Settings::Http(settings) => {
                let client = reqwest::Client::builder()
                    .user_agent(user_agent)
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .dns_resolver(Arc::new(GuardedResolver::new(
                        settings.allow_private_networks,
                    )))
                    .build()?;
                Transport::Http(settings, client)
            }
            Settings::Command(settings) => Transport::Command(settings),
        };
        Ok(Self { name, transport })
    }
}

impl Engine for Adapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, query: String, limit: usize) -> EngineFuture {
        let name = self.name.clone();
        match &self.transport {
            Transport::Http(settings, client) => {
                let settings = settings.clone();
                let client = client.clone();
                Box::pin(async move { http(&name, &settings, &client, &query, limit).await })
            }
            Transport::Command(settings) => {
                let settings = settings.clone();
                Box::pin(async move { command(&name, &settings, &query, limit).await })
            }
        }
    }
}

/// Dial only the configured endpoint; environment values are whole sensitive headers.
async fn http(
    name: &str,
    settings: &Http,
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Found, EngineError> {
    let mut url =
        http_url(&settings.url).ok_or_else(|| EngineError::malformed("invalid engine endpoint"))?;
    check_host(
        url.host_str().unwrap_or_default(),
        settings.allow_private_networks,
    )
    .map_err(|_| EngineError::network("engine endpoint refused"))?;
    {
        let dynamic =
            |key: &str| key == settings.query_param || settings.limit_param.as_deref() == Some(key);
        let mut values: Vec<(String, String)> = url
            .query_pairs()
            .into_owned()
            .filter(|(key, _)| !settings.params.contains_key(key) && !dynamic(key))
            .collect();
        values.extend(
            settings
                .params
                .iter()
                .filter(|(key, _)| !dynamic(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        values.push((settings.query_param.clone(), query.into()));
        if let Some(param) = &settings.limit_param {
            values.push((param.clone(), limit.to_string()));
        }
        url.set_query(None);
        let mut params = url.query_pairs_mut();
        params.extend_pairs(&values);
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in &settings.headers {
        insert_header(&mut headers, key, value, false)?;
    }
    for (key, variable) in &settings.header_env {
        let value = std::env::var(variable)
            .map_err(|_| EngineError::rejected("engine credential unavailable"))?;
        insert_header(&mut headers, key, &value, true)?;
    }
    let mut response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|_| EngineError::network("engine HTTP request failed"))?;
    if !response.status().is_success() {
        return Err(EngineError::rejected(format!(
            "engine HTTP status {} rejected",
            response.status().as_u16()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| EngineError::network("engine HTTP read failed"))?
    {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > settings.max_response_bytes {
            return Err(EngineError::malformed("engine output exceeded cap"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    rows(
        name,
        &value,
        Mapping {
            results: &settings.results_pointer,
            title: &settings.title_pointer,
            url: &settings.url_pointer,
            snippet: &settings.snippet_pointer,
            text_part: settings.text_part_pointer.as_deref(),
        },
        limit,
    )
}

/// Validate headers without including either their names or values in errors.
fn insert_header(
    headers: &mut reqwest::header::HeaderMap,
    key: &str,
    value: &str,
    sensitive: bool,
) -> Result<(), EngineError> {
    let key = reqwest::header::HeaderName::from_bytes(key.as_bytes())
        .map_err(|_| EngineError::rejected("engine header invalid"))?;
    let mut value = reqwest::header::HeaderValue::from_str(value)
        .map_err(|_| EngineError::rejected("engine header invalid"))?;
    value.set_sensitive(sensitive);
    headers.insert(key, value);
    Ok(())
}

/// Exchange one strict JSON document; dropping this future kills the direct child.
/// `command` is absolute or a bare name found on `PATH`; settings resolution
/// supplies an absolute working directory.
async fn command(
    name: &str,
    settings: &Command,
    query: &str,
    limit: usize,
) -> Result<Found, EngineError> {
    let cwd = settings
        .cwd
        .as_deref()
        .filter(|cwd| cwd.is_absolute())
        .ok_or_else(|| EngineError::rejected("engine working directory unavailable"))?;
    let mut process = tokio::process::Command::new(&settings.command);
    process.args(&settings.args).current_dir(cwd).env_clear();
    for name in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "PATHEXT",
        "LANG",
        "SystemRoot",
        "SYSTEMROOT",
    ] {
        if let Some(value) = std::env::var_os(name) {
            process.env(name, value);
        }
    }
    for name in &settings.env {
        if matches!(name.as_str(), "TMPDIR" | "TMP" | "TEMP") {
            continue;
        }
        let value = std::env::var_os(name)
            .ok_or_else(|| EngineError::rejected("engine credential unavailable"))?;
        process.env(name, value);
    }
    super::scratch::prepare(&settings.temp_dir)
        .map_err(|_| EngineError::rejected("engine temporary directory unavailable or unsafe"))?;
    for name in ["TMPDIR", "TMP", "TEMP"] {
        process.env(name, &settings.temp_dir);
    }
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| EngineError::network("engine command failed to start"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| EngineError::network("engine command input unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| EngineError::network("engine command output unavailable"))?;
    let mut request = serde_json::json!({"version": 1, "query": query, "limit": limit});
    if !settings.config.is_empty() {
        if let Some(object) = request.as_object_mut() {
            object.insert(
                "config".into(),
                serde_json::to_value(&settings.config).map_err(|_| malformed())?,
            );
        }
    }
    let request = serde_json::to_vec(&request).map_err(|_| malformed())?;
    let write = async move {
        stdin
            .write_all(&request)
            .await
            .map_err(|_| EngineError::network("engine command write failed"))?;
        stdin
            .shutdown()
            .await
            .map_err(|_| EngineError::network("engine command write failed"))?;
        drop(stdin);
        Ok::<(), EngineError>(())
    };
    let read = async {
        let mut bytes = Vec::new();
        stdout
            .take(settings.max_response_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| EngineError::network("engine command read failed"))?;
        if bytes.len() as u64 > settings.max_response_bytes {
            return Err(EngineError::malformed("engine output exceeded cap"));
        }
        Ok(bytes)
    };
    let (_, bytes) = tokio::try_join!(write, read)?;
    let status = child
        .wait()
        .await
        .map_err(|_| EngineError::network("engine command wait failed"))?;
    if !status.success() {
        return Err(EngineError::rejected(
            "engine command exited unsuccessfully",
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    rows(name, &value, Mapping::COMMAND, limit)
}

/// JSON field locations; executable answers use the fixed protocol mapping.
struct Mapping<'a> {
    results: &'a str,
    title: &'a str,
    url: &'a str,
    snippet: &'a str,
    text_part: Option<&'a str>,
}

impl Mapping<'_> {
    const COMMAND: Self = Self {
        results: "/results",
        title: "/title",
        url: "/url",
        snippet: "/snippet",
        text_part: None,
    };
}

/// Decode text or explicitly mapped ordered text fragments; `None` when any part is unusable.
fn mapped_text(value: &serde_json::Value, part: Option<&str>) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.into());
    }
    let pointer = part?;
    let mut text = String::new();
    for part in value.as_array()? {
        text.push_str(part.pointer(pointer)?.as_str()?);
    }
    Some(text)
}

/// Map one row, or `None` when its title is empty, its URL is not an absolute
/// credential-free HTTP(S) URL, or its snippet is present but not text.
fn row(name: &str, row: &serde_json::Value, mapping: &Mapping<'_>) -> Option<Link> {
    let title = mapped_text(row.pointer(mapping.title)?, mapping.text_part)?;
    if title.trim().is_empty() {
        return None;
    }
    let url = row.pointer(mapping.url)?.as_str()?;
    // Result fragments are useful page anchors; endpoint fragments are forbidden.
    let mut parsed = url::Url::parse(url).ok()?;
    parsed.set_fragment(None);
    http_url(parsed.as_str())?;
    let snippet = match row.pointer(mapping.snippet) {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(mapped_text(value, mapping.text_part)?),
    };
    Some(Link {
        title,
        url: url.into(),
        snippet,
        engines: vec![name.into()],
        score: 0.0,
    })
}

/// Collect valid rows up to `limit`, counting skipped invalid rows on the way,
/// and stamp only locally configured provenance. A missing results array, or a
/// non-empty one with no valid row, is malformed.
fn rows(
    name: &str,
    value: &serde_json::Value,
    mapping: Mapping<'_>,
    limit: usize,
) -> Result<Found, EngineError> {
    let rows = value
        .pointer(mapping.results)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(malformed)?;
    let mut found = Found::default();
    for value in rows {
        if found.links.len() >= limit {
            break;
        }
        match row(name, value, &mapping) {
            Some(link) => found.links.push(link),
            None => found.skipped += 1,
        }
    }
    if found.links.is_empty() && found.skipped > 0 {
        return Err(malformed());
    }
    Ok(found)
}

/// Parsing errors never carry any adapter output.
fn malformed() -> EngineError {
    EngineError::malformed("engine response malformed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::engines::{EngineStatus, Pool};
    use crate::core::Query;
    use std::time::Duration;

    /// Python is only an offline executable fixture, never a production dependency.
    fn executable(script: &str, cap: u64) -> Command {
        Command {
            command: if cfg!(windows) {
                "python.exe"
            } else {
                "python3"
            }
            .into(),
            config: Default::default(),
            env: Vec::new(),
            cwd: Some(std::env::temp_dir().canonicalize().unwrap()),
            temp_dir: std::env::temp_dir().canonicalize().unwrap().join("search"),
            args: vec!["-c".into(), script.trim_start().into()],
            max_response_bytes: cap,
        }
    }

    /// Exercise the production HTTP transport with fixture settings.
    async fn query_http(
        name: &str,
        settings: &Http,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Link>, EngineError> {
        Adapter::new(name.into(), Settings::Http(settings.clone()), "search-test")
            .unwrap()
            .search(query.into(), limit)
            .await
            .map(|found| found.links)
    }

    #[tokio::test]
    async fn command_protocol_is_literal_eof_delimited_and_owns_provenance() {
        let settings = executable(
            r#"
import sys,json
request=json.load(sys.stdin)
assert request == {'version':1,'query':'$(echo secret) $HOME ~','limit':1}
assert sys.argv[1] == '$HOME'
print(json.dumps({'results':[{'title':'First','url':'https://example.com/a','engines':['forged'],'score':99},{'title':'Second','url':'https://example.com/b'}]}))
"#,
            4096,
        );
        let mut settings = settings;
        settings.args.push("$HOME".into());
        let results = command("owned", &settings, "$(echo secret) $HOME ~", 1)
            .await
            .unwrap()
            .links;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].engines, ["owned"]);
        assert_eq!(results[0].score, 0.0);
    }

    #[tokio::test]
    async fn commands_receive_only_declared_credentials_cwd_and_json_config() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-command-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("asset.txt"), "engine data").unwrap();
        let declared = format!("SEARCH_DECLARED_{}", uuid::Uuid::new_v4().simple());
        let hidden = format!("SEARCH_HIDDEN_{}", uuid::Uuid::new_v4().simple());
        std::env::set_var(&declared, "fixture-secret");
        std::env::set_var(&hidden, "must-not-leak");
        let mut settings = executable(
            r#"
import sys,json,os
request=json.load(sys.stdin)
assert os.environ[sys.argv[1]] == 'fixture-secret'
assert sys.argv[2] not in os.environ
assert open('asset.txt').read() == 'engine data'
assert request['config'] == {'literal':'$HOME; $(echo secret)'}
print(json.dumps({'results':[]}))
"#,
            4096,
        );
        settings.cwd = Some(root.clone());
        settings.env.push(declared.clone());
        settings.args.extend([declared.clone(), hidden.clone()]);
        settings
            .config
            .insert("literal".into(), serde_json::json!("$HOME; $(echo secret)"));
        let result = command("fixture", &settings, "q", 1).await;
        std::env::remove_var(&declared);
        std::env::remove_var(&hidden);
        std::fs::remove_dir_all(root).unwrap();
        assert!(result.unwrap().links.is_empty());
        let error = command("fixture", &settings, "q", 1).await.unwrap_err();
        assert_eq!(error.message, "engine credential unavailable");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn commands_use_private_uniform_temp_variables_and_reject_unsafe_paths() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-scratch-test-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .recursive(false)
            .create(&root)
            .unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let scratch = root.join("scratch");
        let mut settings = executable(
            r#"
import json,os,sys,tempfile
expected=sys.argv[1]
assert all(os.environ[key] == expected for key in ('TMPDIR','TMP','TEMP'))
with tempfile.NamedTemporaryFile() as f:
    assert os.path.dirname(f.name) == expected
assert os.stat(expected).st_mode & 0o777 == 0o700
print(json.dumps({'results':[]}))
"#,
            4096,
        );
        settings.temp_dir = scratch.clone();
        settings.env = vec!["TMPDIR".into(), "TMP".into(), "TEMP".into()];
        settings.args.push(scratch.to_string_lossy().into_owned());
        assert!(command("fixture", &settings, "q", 1)
            .await
            .unwrap()
            .links
            .is_empty());
        let link = root.join("link");
        symlink(&scratch, &link).unwrap();
        settings.temp_dir = link;
        assert!(command("fixture", &settings, "q", 1).await.is_err());
        std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o777)).unwrap();
        settings.temp_dir = scratch;
        assert!(command("fixture", &settings, "q", 1).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn command_rejects_malformed_oversize_and_nonzero_without_leaks() {
        for (script, cap) in [
            ("print('not-json-secret')", 4096),
            ("print('{\"results\":[]} {\"results\":[]}')", 4096),
            ("print('x'*1000)", 10),
            ("import sys; print('{\"results\":[]}'); sys.stderr.write('stderr-secret'); sys.exit(2)", 4096),
        ] {
            let error = command("fixture", &executable(script, cap), "test", 2).await.unwrap_err();
            assert!(!error.message.contains("secret"));
        }
    }

    #[tokio::test]
    async fn command_reads_while_writing_large_requests() {
        let settings = executable("import sys; sys.stdout.write(' '*131072); sys.stdout.flush(); sys.stdin.read(); print('{\"results\":[]}')", 200000);
        let results = tokio::time::timeout(
            Duration::from_secs(5),
            command("fixture", &settings, &"q".repeat(200000), 2),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(results.links.is_empty());
    }

    /// A one-shot local socket captures the request and emits a chosen wire answer.
    async fn socket_response(
        status: &str,
        body: &str,
        extra: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/search", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 1024];
            while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                let size = socket.read(&mut buffer).await.unwrap();
                if size == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..size]);
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(bytes).unwrap()
        });
        (url, task)
    }

    fn http_settings(url: &str) -> Http {
        serde_json::from_value(serde_json::json!({"url":url,"allow_private_networks":true}))
            .unwrap()
    }

    #[tokio::test]
    async fn http_maps_json_and_sends_whole_env_header_and_encoded_parameters() {
        let (url, task) = socket_response("200 OK", r#"{"data":{"items":[{"meta":{"heading":"Mapped"},"link":"https://example.com/","summary":"text"}]}}"#, "").await;
        let variable = format!("SEARCH_TEST_AUTH_{}", uuid::Uuid::new_v4().simple());
        std::env::set_var(&variable, "Bearer fixture-secret");
        let mut settings = http_settings(&url);
        settings
            .header_env
            .insert("Authorization".into(), variable.clone());
        settings
            .headers
            .insert("X-Fixture".into(), "constant".into());
        settings.params.insert("format".into(), "json".into());
        settings.limit_param = Some("count".into());
        settings.results_pointer = "/data/items".into();
        settings.title_pointer = "/meta/heading".into();
        settings.url_pointer = "/link".into();
        settings.snippet_pointer = "/summary".into();
        let results = query_http("mapped", &settings, "a & b", 3).await.unwrap();
        std::env::remove_var(variable);
        let request = task.await.unwrap().to_lowercase();
        assert!(request.contains("q=a+%26+b"));
        assert!(request.contains("count=3"));
        assert!(request.contains("format=json"));
        assert!(request.contains("authorization: bearer fixture-secret"));
        assert!(request.contains("x-fixture: constant"));
        assert_eq!(results[0].title, "Mapped");
        assert_eq!(results[0].snippet.as_deref(), Some("text"));
        assert_eq!(results[0].engines, ["mapped"]);
    }

    /// Repeated queries reuse a connection and read credentials after construction.
    #[tokio::test]
    async fn http_reuses_client_and_refreshes_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut settings = http_settings(&format!("http://{}/", listener.local_addr().unwrap()));
        let variable = format!("SEARCH_REFRESH_{}", uuid::Uuid::new_v4().simple());
        settings
            .header_env
            .insert("Authorization".into(), variable.clone());
        let adapter =
            Adapter::new("fixture".into(), Settings::Http(settings), "search-test").unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            for value in ["first", "second"] {
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                assert!(String::from_utf8(request)
                    .unwrap()
                    .to_lowercase()
                    .contains(&format!("authorization: {value}\r\n")));
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\n{\"results\":[]}")
                    .await
                    .unwrap();
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            for value in ["first", "second"] {
                std::env::set_var(&variable, value);
                assert!(adapter
                    .search("q".into(), 1)
                    .await
                    .unwrap()
                    .links
                    .is_empty());
            }
            server.await.unwrap();
        })
        .await;
        std::env::remove_var(variable);
        result.unwrap();
    }

    #[tokio::test]
    async fn http_refuses_redirects_and_enforces_body_cap() {
        for (status, body, extra, cap) in [
            (
                "302 Found",
                "",
                "Location: http://127.0.0.1:1/secret\r\n",
                4096,
            ),
            ("200 OK", "body-secret-too-long", "", 4),
        ] {
            let (url, task) = socket_response(status, body, extra).await;
            let mut settings = http_settings(&url);
            settings.max_response_bytes = cap;
            let error = query_http("fixture", &settings, "q", 1).await.unwrap_err();
            task.await.unwrap();
            assert!(!error.message.contains("secret"));
            assert!(!error.message.contains("127.0.0.1"));
        }
    }

    #[tokio::test]
    async fn mapped_text_fragments_keep_the_complete_title_and_snippet() {
        let (url, task) = socket_response("200 OK", r#"{"results":[{"title":[{"value":"Rust"},{"value":" async guide"}],"url":"https://example.com/","snippet":[{"value":"First "},{"value":"second"}]}]}"#, "").await;
        let mut settings = http_settings(&url);
        settings.text_part_pointer = Some("/value".into());
        let results = query_http("fixture", &settings, "query", 1).await.unwrap();
        task.await.unwrap();
        assert_eq!(results[0].title, "Rust async guide");
        assert_eq!(results[0].snippet.as_deref(), Some("First second"));
        assert!(mapped_text(&serde_json::json!([{"value":4}]), Some("/value")).is_none());
    }

    #[tokio::test]
    async fn current_query_and_limit_replace_configured_parameter_values() {
        let (url, task) = socket_response("200 OK", r#"{"results":[]}"#, "").await;
        let mut settings = http_settings(&format!(
            "{url}?q=old&format=html&count=99&filter=a&filter=b"
        ));
        settings.params.insert("q".into(), "also-old".into());
        settings.params.insert("format".into(), "json".into());
        settings.params.insert("count".into(), "100".into());
        settings.limit_param = Some("count".into());
        query_http("fixture", &settings, "new query", 3)
            .await
            .unwrap();
        let wire = task.await.unwrap();
        let target = wire.split_whitespace().nth(1).unwrap();
        let request = url::Url::parse(&format!("http://localhost{target}")).unwrap();
        let params: Vec<_> = request.query_pairs().into_owned().collect();
        assert_eq!(
            params,
            vec![
                ("filter".into(), "a".into()),
                ("filter".into(), "b".into()),
                ("format".into(), "json".into()),
                ("q".into(), "new query".into()),
                ("count".into(), "3".into())
            ]
        );
    }

    #[tokio::test]
    async fn private_permission_is_independent_and_missing_env_remains_a_failed_engine() {
        let mut config = Config::default();
        config.fetch.allow_private_networks = true;
        let mut settings = http_settings("http://127.0.0.1:1/");
        settings.allow_private_networks = false;
        let response = Pool::from_adapters(
            vec![("private", Settings::Http(settings))],
            config.search.clone(),
        )
        .search(Query::default())
        .await;
        assert_eq!(response.engines[0].status, EngineStatus::Error);
        assert_eq!(
            response.engines[0].error.as_deref(),
            Some("engine endpoint refused")
        );
        let mut settings = http_settings("https://example.com/");
        settings.header_env.insert(
            "Authorization".into(),
            format!("SEARCH_MISSING_{}", uuid::Uuid::new_v4().simple()),
        );
        let response =
            Pool::from_adapters(vec![("missing", Settings::Http(settings))], config.search)
                .search(Query::default())
                .await;
        assert_eq!(response.engines[0].name, "missing");
        assert_eq!(response.engines[0].status, EngineStatus::Error);
        assert_eq!(
            response.engines[0].error.as_deref(),
            Some("engine credential unavailable")
        );
    }

    #[tokio::test]
    async fn pool_selects_ranks_and_times_out_configured_commands() {
        let mut config = Config::default();
        config.search.max_results = 2;
        config.search.engine_timeout = 2000;
        let pool = Pool::from_adapters(
            vec![
                (
                    "selected",
                    Settings::Command(executable(
                        r#"
import json,sys
request=json.load(sys.stdin)
assert request['limit'] == 2
print(json.dumps({'results':[{'title':str(i),'url':'https://example.com/'+str(i)} for i in range(3)]}))
"#,
                        4096,
                    )),
                ),
                (
                    "omitted",
                    Settings::Command(executable("raise Exception('must not run')", 4096)),
                ),
            ],
            config.search.clone(),
        );
        let response = pool
            .search(Query {
                limit: 10,
                engines: vec!["selected".into()],
                ..Query::default()
            })
            .await;
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.engines[0].count, 2);
        assert_eq!(response.engines[0].status, EngineStatus::Ok);
        assert!(response.results[0].score > response.results[1].score);
        config.search.engine_timeout = 100;
        let response = Pool::from_adapters(
            vec![(
                "selected",
                Settings::Command(executable("import time; time.sleep(30)", 4096)),
            )],
            config.search,
        )
        .search(Query::default())
        .await;
        assert_eq!(response.engines[0].name, "selected");
        assert_eq!(response.engines[0].status, EngineStatus::Timeout);
    }

    /// One bad row costs that row, not the engine; an answer with no valid row
    /// at all is malformed rather than a silent empty success.
    #[test]
    fn invalid_rows_are_skipped_and_counted() {
        let good = serde_json::json!({"title":"Title","url":"https://example.com/"});
        for bad in [
            serde_json::json!({"title":" ","url":"https://example.com/"}),
            serde_json::json!({"url":"https://example.com/"}),
            serde_json::json!({"title":"Title","url":"/relative"}),
            serde_json::json!({"title":"Title","url":"file:///tmp/page"}),
            serde_json::json!({"title":"Title","url":"https://user:password@example.com/"}),
            serde_json::json!({"title":"Title","url":"https://example.com/","snippet":8}),
        ] {
            let value = serde_json::json!({"results":[bad.clone(), good.clone()]});
            let found = rows("fixture", &value, Mapping::COMMAND, 10).unwrap();
            assert_eq!((found.links.len(), found.skipped), (1, 1), "{bad}");
            let value = serde_json::json!({"results":[bad]});
            assert!(rows("fixture", &value, Mapping::COMMAND, 10).is_err());
        }
        let empty = serde_json::json!({"results":[]});
        assert!(rows("fixture", &empty, Mapping::COMMAND, 10)
            .unwrap()
            .links
            .is_empty());
    }

    /// Regression: a live Mwmbl answer with one empty-titled row among many
    /// returned zero results because that row failed the whole engine.
    #[test]
    fn mwmbl_row_with_empty_title_does_not_fail_the_engine() {
        let Settings::Http(mwmbl) = crate::core::config::EngineSettings::default()
            .adapter("mwmbl")
            .unwrap()
        else {
            panic!("mwmbl is an HTTP preset")
        };
        let value = serde_json::json!([
            {"title":[{"value":"How to Tie a Tie"}],"url":"https://example.com/tie","extract":[{"value":"Steps"}]},
            {"title":[],"url":"https://example.com/untitled","extract":[{"value":"No title"}]},
            {"title":[{"value":"Knots"}],"url":"https://example.com/knots","extract":[]}
        ]);
        let mapping = Mapping {
            results: &mwmbl.results_pointer,
            title: &mwmbl.title_pointer,
            url: &mwmbl.url_pointer,
            snippet: &mwmbl.snippet_pointer,
            text_part: mwmbl.text_part_pointer.as_deref(),
        };
        let found = rows("mwmbl", &value, mapping, 10).unwrap();
        assert_eq!(found.links.len(), 2);
        assert_eq!(found.skipped, 1);
        assert_eq!(found.links[0].title, "How to Tie a Tie");
    }

    /// The presets' mappings parse saved upstream answers.
    #[test]
    fn presets_map_saved_answers() {
        let mut settings = crate::core::config::EngineSettings::default();
        settings.config.insert(
            "searxng".into(),
            serde_json::json!({"url":"https://searx.example/search"}),
        );
        for (id, saved) in [
            (
                "mwmbl",
                include_str!("../../../tests/engines/fixtures/mwmbl.json"),
            ),
            (
                "searxng",
                include_str!("../../../tests/engines/fixtures/searxng.json"),
            ),
        ] {
            let Settings::Http(http) = settings.adapter(id).unwrap() else {
                panic!("{id} is an HTTP preset")
            };
            let mapping = Mapping {
                results: &http.results_pointer,
                title: &http.title_pointer,
                url: &http.url_pointer,
                snippet: &http.snippet_pointer,
                text_part: http.text_part_pointer.as_deref(),
            };
            let value = serde_json::from_str(saved).unwrap();
            let found = rows(id, &value, mapping, 10).unwrap();
            assert_eq!(found.skipped, 0, "{id}");
            assert!(found.links[0].snippet.is_some(), "{id}");
        }
    }

    /// Linux exposes child liveness without adding a process-control dependency.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_and_cancellation_kill_the_direct_child() {
        for timeout in [true, false] {
            let path = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("search-child-{}", uuid::Uuid::new_v4()));
            let mut settings = executable(
                "import os,sys,time; open(sys.argv[1],'w').write(str(os.getpid())); time.sleep(30)",
                4096,
            );
            settings.args.push(path.to_string_lossy().into());
            let future = Adapter::new("slow".into(), Settings::Command(settings), "search-test")
                .unwrap()
                .search("q".into(), 1);
            let task = tokio::spawn(async move {
                if timeout {
                    assert!(tokio::time::timeout(Duration::from_millis(500), future)
                        .await
                        .is_err());
                } else {
                    let _ = future.await;
                }
            });
            let pid = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(pid) = std::fs::read_to_string(&path) {
                        if !pid.is_empty() {
                            break pid;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            if !timeout {
                task.abort();
            }
            let _ = task.await;
            tokio::time::timeout(Duration::from_secs(3), async {
                while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            std::fs::remove_file(path).unwrap();
        }
    }
}
