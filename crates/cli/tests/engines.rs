//! Offline CLI diagnostics exercise host-owned adapter configuration without corpus access.

use std::process::{Command, Output};

/// A private test configuration with an executable adapter and no public network calls.
struct Fixture {
    root: std::path::PathBuf,
    config: std::path::PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let root = std::env::temp_dir().join(format!("search-engines-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("settings.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "dir": root.join("corpus"),
                "engines": {
                    "only": ["fixture"],
                    "custom": {"fixture": {
                        "type": "command", "command": "python3", "args": ["-c", script]
                    }}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        Self { root, config }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_search"))
            .args(args)
            .arg("-config")
            .arg(&self.config)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn diagnostics_test_one_adapter_without_opening_the_index() {
    let fixture = Fixture::new(
        r#"
import json, sys
request = json.load(sys.stdin)
assert request['version'] == 1
assert request['query'] == 'two words'
assert request['limit'] == 10
json.dump({'results':[{'title':'Fixture','url':'https://example.com/page','snippet':'Found'}]}, sys.stdout)
"#,
    );
    let listed = fixture.run(&["engines", "list"]);
    assert!(listed.status.success(), "{:?}", listed);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap(),
        serde_json::json!(["fixture"])
    );
    let tested = fixture.run(&["engines", "test", "fixture", "two words"]);
    assert!(tested.status.success(), "{:?}", tested);
    let answer: serde_json::Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(
        answer["results"][0]["providers"],
        serde_json::json!(["fixture"])
    );
    assert_eq!(answer["providers"][0]["status"], "ok");
    assert!(!fixture.root.join("corpus").exists());
}

#[test]
fn diagnostic_failures_exit_nonzero_without_exposing_adapter_stderr() {
    let fixture =
        Fixture::new("import sys; print('credential-secret',file=sys.stderr); sys.exit(3)");
    let tested = fixture.run(&["engines", "test", "fixture", "query"]);
    assert!(!tested.status.success());
    let answer: serde_json::Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(answer["providers"][0]["status"], "error");
    assert!(!String::from_utf8_lossy(&tested.stdout).contains("credential-secret"));
    assert!(!String::from_utf8_lossy(&tested.stderr).contains("credential-secret"));
    assert!(!fixture.root.join("corpus").exists());
}
