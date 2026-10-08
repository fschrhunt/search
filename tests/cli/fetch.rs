//! Offline contract for `search fetch`: a failed URL makes the command exit nonzero in
//! both text and JSON modes, and JSON still reports the failure per URL.

use std::process::Command;

/// Loopback is refused by the SSRF guard, so the fetch fails without touching the network.
fn fetch(args: &[&str]) -> std::process::Output {
    let home = std::env::temp_dir().join(format!("search-cli-fetch-{}", std::process::id()));
    Command::new(env!("CARGO_BIN_EXE_search"))
        .arg("fetch")
        .args(args)
        .arg("http://127.0.0.1:9/")
        .env("SEARCH_HOME", &home)
        .env_remove("CONFIG")
        .output()
        .unwrap()
}

/// A failed fetch exits 1 in text mode, where the failure is printed in place.
#[test]
fn text_fetch_exits_nonzero_when_a_url_fails() {
    let output = fetch(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Fetch failed"));
}

/// JSON mode reports the failure in the result and exits nonzero, like text mode.
#[test]
fn json_fetch_exits_nonzero_when_a_url_fails() {
    let output = fetch(&["-json"]);
    assert_eq!(output.status.code(), Some(1));
    let pages: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(pages[0]["error"].is_string());
}
