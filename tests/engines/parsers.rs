//! Verify the custom JSON POST example and its settings entry offline. Preset
//! mappings are checked against the saved answers in `fixtures/` by the
//! adapter unit tests.
#![allow(clippy::unwrap_used)]
use std::{path::Path, process::Command};

#[test]
fn custom_json_post_example_obeys_the_command_contract() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engines/custom.py");
    let output = Command::new(if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    })
    .args(["-S", "-B"])
    .arg(script)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The example's settings entry is a complete command adapter once `cwd` names
/// the copied example directory.
#[test]
fn custom_json_post_example_settings_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/examples/json-post");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap()).unwrap();
    settings["engines"]["config"]["json-post"]["cwd"] = serde_json::json!(root);
    let config: search::core::Config = serde_json::from_value(settings).unwrap();
    let adapter = config.engines.adapter("json-post").unwrap();
    assert!(matches!(adapter, search::core::config::Adapter::Command(_)));
}
