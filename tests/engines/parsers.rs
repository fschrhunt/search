//! Verify shipped HTTP pointer mappings and the custom JSON POST example offline.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use std::{path::Path, process::Command};

#[test]
fn custom_json_post_example_obeys_the_command_contract() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engines/custom.py");
    let output = Command::new("python3")
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

#[test]
fn http_packages_map_saved_json_with_the_declared_pointers() {
    for id in ["mwmbl", "searxng"] {
        let m = search::engines::catalog()
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/engines/fixtures/{id}.json"));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let rows = value
            .pointer(m.adapter["results_pointer"].as_str().unwrap())
            .unwrap()
            .as_array()
            .unwrap();
        let first = &rows[0];
        for pointer in ["title_pointer", "url_pointer", "snippet_pointer"] {
            let field = first.pointer(m.adapter[pointer].as_str().unwrap()).unwrap();
            if field.is_array() {
                let part = m.adapter["text_part_pointer"].as_str().unwrap();
                assert!(field
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|value| value.pointer(part).unwrap().is_string()));
            } else {
                assert!(field.is_string());
            }
        }
    }
}
