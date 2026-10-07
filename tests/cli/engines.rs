//! Offline package CLI contracts. Every subprocess owns its home and installs explicit fixtures.

#[cfg(windows)]
#[path = "../support/windows.rs"]
mod windows;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

/// Isolated settings, package source, and command execution marker for one test.
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    package: PathBuf,
}

/// Terminal output must not disguise a broken engine as a successful empty search.
#[test]
fn text_search_reports_engine_failures_without_leaking_program_stderr() {
    let fixture =
        Fixture::new("import sys\nprint('PRIVATE_MARKER', file=sys.stderr)\nsys.exit(1)\n");
    fixture.install();
    fixture.success(&["enable", "fixture", "--trust"]);
    fixture.success(&["remote", "off"]);
    let output = fixture.run(&["offline query"]);
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("fixture: failed"), "{diagnostic}");
    assert!(!diagnostic.contains("PRIVATE_MARKER"));
}

impl Fixture {
    /// Write a declarative package whose program runs only through an explicit engine test.
    fn new(script: &str) -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-engines-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = home.join("settings.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "notes": {"owner": "keep this note"},
                "search": {"timeout": 3000},
                "engines": {"use": []}
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        }
        // An unusable selected remote makes accidental remote routing fail in every package test.
        let trust = home.join("trust");
        fs::create_dir(&trust).unwrap();
        fs::write(
            trust.join("remotes.json"),
            r#"{"selected":"missing","remotes":{}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&trust, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(
                trust.join("remotes.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        #[cfg(windows)]
        for path in [&root, &home, &config, &trust, &trust.join("remotes.json")] {
            windows::secure(path);
        }
        let package = root.join("source");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("engine.json"),
            serde_json::to_vec(&json!({
                "schema_version": 1, "id": "fixture", "version": "1", "description": "CLI fixture",
                "adapter": {"type": "command", "command": if cfg!(windows) { "python.exe" } else { "python3" }, "args": ["program.py"]},
                "files": ["program.py"]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(package.join("program.py"), script).unwrap();
        Self {
            root,
            home,
            config,
            package,
        }
    }

    /// Keep environment changes in children, including explicit credential references for protocol tests.
    fn process(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_search"));
        command
            .args(args)
            .env("SEARCH_HOME", &self.home)
            .env_remove("CONFIG")
            .env_remove("ADDRESS")
            .env("HOME", &self.root)
            .env("FIXTURE_DECLARED", "declared-value")
            .env("FIXTURE_UNDECLARED", "hidden-value")
            .env("MY_TOKEN", "credential-secret")
            .current_dir(&self.root);
        command
    }

    /// Run with default home settings; no process-global environment mutation.
    fn run(&self, args: &[&str]) -> Output {
        self.process(args).output().unwrap()
    }

    /// Install the local package rather than removed engines.custom configuration.
    fn install(&self) {
        let output = self
            .process(&["engines", "install"])
            .arg(&self.package)
            .output()
            .unwrap();
        assert!(output.status.success(), "home {:?}: {output:?}", self.home);
    }

    /// Read persisted JSON to assert preservation and explicit selection.
    fn settings(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap()
    }

    /// Assert success with actionable process output when the contract breaks.
    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(output.status.success(), "{output:?}");
        output
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Version-1 packages remain independent of source files and catalog maintenance.
#[test]
fn installed_v1_engine_survives_catalog_updates_with_its_contract_and_settings() {
    let fixture = Fixture::new(
        r#"
import json, os, sys
from pathlib import Path
request = json.load(sys.stdin)
assert request == {'version': 1, 'query': 'literal query', 'limit': 3, 'config': {'label': 'retained'}}
assert Path('engine.json').is_file()
assert os.environ['FIXTURE_DECLARED'] == 'declared-value'
assert 'FIXTURE_UNDECLARED' not in os.environ
json.dump({'results': [{'title': 'Version one', 'url': 'https://example.com/', 'snippet': 'retained'}]}, sys.stdout)
"#,
    );
    fixture.install();
    fixture.success(&["configure", "fixture", "config", r#"{"label":"retained"}"#]);
    fixture.success(&["configure", "fixture", "env", r#"["FIXTURE_DECLARED"]"#]);
    fixture.success(&["enable", "fixture", "--trust"]);
    let settings = fs::read(&fixture.config).unwrap();
    let installed = fixture.home.join("engines/fixture");
    let manifest = fs::read(installed.join("engine.json")).unwrap();
    let program = fs::read(installed.join("program.py")).unwrap();
    fs::remove_dir_all(&fixture.package).unwrap();

    fixture.success(&["install", "searxng"]);
    fixture.success(&["update", "searxng"]);
    assert!(!fixture.run(&["update", "fixture"]).status.success());
    assert_eq!(fs::read(&fixture.config).unwrap(), settings);
    assert_eq!(fs::read(installed.join("engine.json")).unwrap(), manifest);
    assert_eq!(fs::read(installed.join("program.py")).unwrap(), program);

    fs::remove_file(fixture.home.join("trust/remotes.json")).unwrap();
    let output = fixture.success(&["literal query", "-limit", "3", "-json"]);
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["engines"][0]["status"], "ok");
    assert_eq!(answer["results"][0]["title"], "Version one");
    assert_eq!(answer["results"][0]["engines"], json!(["fixture"]));
}

/// Primary engine verbs dispatch locally even when a selected remote is unusable.
#[test]
fn top_level_engine_commands_manage_and_test_local_packages() {
    let fixture = Fixture::new(
        "import json,sys; r=json.load(sys.stdin); json.dump({'results':[{'title':r['query'],'url':'https://example.com/'}]},sys.stdout)",
    );
    let installed = fixture
        .process(&["install"])
        .arg(&fixture.package)
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");
    fixture.success(&["configure", "fixture"]);
    fixture.success(&["enable", "fixture", "--trust"]);
    let tested = fixture.success(&["test", "fixture", "--", "-literal"]);
    let answer: Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(answer["results"][0]["title"], "-literal");
    assert_eq!(answer["engines"][0]["status"], "ok");
    fixture.success(&["disable", "fixture"]);
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    fixture.success(&["remove", "fixture"]);
    assert!(!fixture.home.join("engines/fixture").exists());
}

#[test]
fn install_and_configure_never_execute_or_enable_and_require_trust_for_activation() {
    let fixture = Fixture::new("from pathlib import Path\nPath('ran').touch()\n");
    fixture.install();
    fixture.success(&[
        "engines",
        "configure",
        "fixture",
        "config",
        r#"{"label":"value"}"#,
    ]);
    let list = fixture.success(&["engines", "list"]);
    let rows: Value = serde_json::from_slice(&list.stdout).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "fixture")
        .unwrap();
    assert_eq!(row["enabled"], false);
    assert_eq!(row["source"], "local");
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    assert!(!fixture.home.join("engines/fixture/ran").exists());
    let refused = fixture.run(&["engines", "enable", "fixture"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--trust"));
    fixture.success(&["engines", "enable", "fixture", "--trust"]);
    assert_eq!(fixture.settings()["engines"]["use"], json!(["fixture"]));
    assert!(!fixture.home.join("engines/fixture/ran").exists());
    assert_eq!(fixture.settings()["notes"]["owner"], "keep this note");
    assert_eq!(fixture.settings()["search"]["timeout"], 3000);
}

#[test]
fn test_is_explicit_execution_with_config_declared_env_and_package_cwd() {
    let fixture = Fixture::new(
        r#"
import json, os, sys
from pathlib import Path
assert Path('engine.json').is_file()
request = json.load(sys.stdin)
assert request['version'] == 1
assert request['query'] == '-two words'
assert request['limit'] == 10
assert request['config'] == {'label': 'value'}
assert os.environ['FIXTURE_DECLARED'] == 'declared-value'
assert 'FIXTURE_UNDECLARED' not in os.environ
json.dump({'results':[{'title':'Fixture','url':'https://example.com/page','snippet':'Found'}]}, sys.stdout)
"#,
    );
    fixture.install();
    fixture.success(&[
        "engines",
        "configure",
        "fixture",
        "config",
        r#"{"label":"value"}"#,
    ]);
    fixture.success(&[
        "engines",
        "configure",
        "fixture",
        "env",
        r#"["FIXTURE_DECLARED"]"#,
    ]);
    let tested = fixture.success(&["engines", "test", "fixture", "--", "-two", "words"]);
    assert!(String::from_utf8_lossy(&tested.stderr).contains("Trust boundary"));
    let answer: Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(answer["results"][0]["engines"], json!(["fixture"]));
    assert_eq!(answer["engines"][0]["status"], "ok");
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
}

#[test]
fn configuration_survives_catalog_update_and_remove_without_package_mutation() {
    let fixture = Fixture::new("");
    fixture.success(&["engines", "install", "mwmbl"]);
    let package = fixture.home.join("engines/mwmbl/engine.json");
    let before = fs::read(&package).unwrap();
    fixture.success(&[
        "engines",
        "configure",
        "mwmbl",
        "query_param",
        "custom-query",
    ]);
    assert_eq!(fs::read(&package).unwrap(), before);
    fixture.success(&["engines", "update", "mwmbl"]);
    assert_eq!(
        fixture.settings()["engines"]["config"]["mwmbl"]["query_param"],
        "custom-query"
    );
    fixture.success(&["engines", "enable", "mwmbl"]);
    fixture.success(&["engines", "remove", "mwmbl"]);
    assert!(!package.exists());
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    assert_eq!(
        fixture.settings()["engines"]["config"]["mwmbl"]["query_param"],
        "custom-query"
    );
}

#[test]
fn disabling_default_persists_empty_selection_and_global_off_survives_enable() {
    let fixture = Fixture::new("");
    fs::write(&fixture.config, r#"{"engines":{"enabled":false}}"#).unwrap();
    fixture.success(&["engines", "disable", "mwmbl"]);
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    let listed = fixture.success(&["engines", "list"]);
    let rows: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(rows[0]["enabled"], false);
    fixture.success(&["engines", "enable", "mwmbl"]);
    assert_eq!(fixture.settings()["engines"]["enabled"], false);
    assert_eq!(fixture.settings()["engines"]["use"], json!(["mwmbl"]));
}

#[test]
fn configure_requires_setup_rejects_transport_changes_and_redacts_values() {
    let fixture = Fixture::new("");
    fixture.success(&["engines", "install", "searxng"]);
    let listed = fixture.success(&["engines", "list"]);
    let rows: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "searxng" && p["missing"] == json!(["url"])));
    let refused = fixture.run(&["engines", "enable", "searxng"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("url"));
    for args in [
        vec!["engines", "configure", "searxng", "type", "command"],
        vec![
            "engines",
            "configure",
            "searxng",
            "max_response_bytes",
            "credential-secret",
        ],
        vec![
            "engines",
            "configure",
            "searxng",
            "url",
            "https://secret:credential-secret@example.com/",
        ],
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("credential-secret"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("credential-secret"));
    }
    fixture.success(&[
        "engines",
        "configure",
        "searxng",
        "header_env",
        r#"{"Authorization":"MY_TOKEN"}"#,
    ]);
    fixture.success(&[
        "engines",
        "configure",
        "searxng",
        "url",
        "https://example.com/search",
    ]);
    fixture.success(&["engines", "enable", "searxng"]);
    let inspected = fixture.success(&["engines", "configure", "searxng"]);
    assert!(!String::from_utf8_lossy(&inspected.stdout).contains("MY_TOKEN"));
}

#[test]
fn local_package_removal_preserves_source_and_configuration() {
    let fixture = Fixture::new("");
    fixture.install();
    fixture.success(&[
        "engines",
        "configure",
        "fixture",
        "config",
        r#"{"keep":true}"#,
    ]);
    fixture.success(&["engines", "enable", "fixture", "--trust"]);
    assert!(!fixture
        .run(&["engines", "update", "fixture"])
        .status
        .success());
    fixture.success(&["engines", "remove", "fixture"]);
    assert!(fixture.package.join("program.py").exists());
    assert_eq!(
        fixture.settings()["engines"]["config"]["fixture"],
        json!({"config":{"keep":true}})
    );
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
}

#[test]
fn unknown_packages_flags_and_command_errors_fail_without_leaking_stderr() {
    let fixture =
        Fixture::new("import sys; print('credential-secret',file=sys.stderr); sys.exit(3)");
    fixture.install();
    for args in [
        vec!["engines", "enable", "unknown"],
        vec!["engines", "test", "unknown", "query"],
        vec!["engines", "install", "unknown"],
        vec!["engines", "list", "--trust"],
        vec!["engines", "test", "fixture", "--bad"],
    ] {
        assert!(!fixture.run(&args).status.success());
    }
    let tested = fixture.run(&["engines", "test", "fixture", "query"]);
    assert!(!tested.status.success());
    let answer: Value = serde_json::from_slice(&tested.stdout).unwrap();
    assert_eq!(answer["engines"][0]["status"], "error");
    assert!(!String::from_utf8_lossy(&tested.stdout).contains("credential-secret"));
    assert!(!String::from_utf8_lossy(&tested.stderr).contains("credential-secret"));
}

#[test]
fn settings_updates_serialize_and_explicit_config_does_not_move_package_home() {
    let fixture = Fixture::new("");
    fixture.install();
    let alternate = fixture.root.join("alternate.json");
    fs::copy(&fixture.config, &alternate).unwrap();
    #[cfg(windows)]
    windows::secure(&alternate);
    let mut children = Vec::new();
    for (key, value) in [
        ("env", r#"["FIXTURE_DECLARED"]"#),
        ("config", r#"{"keep":true}"#),
    ] {
        children.push(
            fixture
                .process(&["engines", "configure", "fixture", key, value, "--config"])
                .arg(&alternate)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let settings: Value = serde_json::from_slice(&fs::read(&alternate).unwrap()).unwrap();
    assert_eq!(
        settings["engines"]["config"]["fixture"],
        json!({"env":["FIXTURE_DECLARED"], "config":{"keep":true}})
    );
    assert!(fixture.home.join("engines/fixture/engine.json").exists());
    assert!(fixture.settings()["engines"].get("config").is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(alternate).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&fixture.home).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn settings_and_lock_symlinks_are_rejected_without_writing_the_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("");
    let target = fixture.root.join("target.json");
    fs::rename(&fixture.config, &target).unwrap();
    symlink(&target, &fixture.config).unwrap();
    assert!(!fixture
        .run(&["engines", "disable", "mwmbl"])
        .status
        .success());
    fs::remove_file(&fixture.config).unwrap();
    fs::rename(&target, &fixture.config).unwrap();
    let lock = fixture.home.join("settings.json.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&fixture.config, &lock).unwrap();
    let before = fs::read(&fixture.config).unwrap();
    assert!(!fixture
        .run(&["engines", "disable", "mwmbl"])
        .status
        .success());
    assert_eq!(fs::read(&fixture.config).unwrap(), before);
}

#[test]
fn inspection_does_not_create_home_or_discover_a_cwd_package() {
    let fixture = Fixture::new("");
    fs::remove_dir_all(&fixture.home).unwrap();
    for action in ["available", "list"] {
        let output = fixture
            .process(&["engines", action])
            .current_dir(&fixture.package)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!rows
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "fixture"));
    }
    assert!(!fixture.home.exists());
    let invalid = fixture
        .process(&["engines", "list"])
        .env("SEARCH_HOME", "")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(!fixture.home.exists());
}

#[test]
fn active_package_removal_refuses_deletion_and_retains_configuration() {
    let fixture = Fixture::new("");
    fixture.install();
    fixture.success(&[
        "engines",
        "configure",
        "fixture",
        "config",
        r#"{"keep":true}"#,
    ]);
    fixture.success(&["engines", "enable", "fixture", "--trust"]);
    let mut settings: search::core::Config = serde_json::from_value(json!({
        "engines": {"use": ["fixture"]}
    }))
    .unwrap();
    settings.home = fixture.home.clone();
    let _running_service = search::core::Search::open(settings).unwrap();
    let output = fixture.run(&["engines", "remove", "fixture"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("configuration retained"));
    assert!(fixture.home.join("engines/fixture").exists());
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    assert_eq!(
        fixture.settings()["engines"]["config"]["fixture"],
        json!({"config":{"keep":true}})
    );
}

#[test]
fn publication_refuses_dangling_selection_until_explicitly_disabled() {
    let fixture = Fixture::new("from pathlib import Path\nPath('ran').touch()\n");
    let settings = json!({"notes":{"keep":"note"}, "engines":{"use":["fixture"], "config":{"fixture":{"config":{"keep":true}}}}});
    fs::write(&fixture.config, serde_json::to_vec(&settings).unwrap()).unwrap();
    {
        let output = fixture
            .process(&["engines", "install"])
            .arg(&fixture.package)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("search disable fixture"));
        assert!(!fixture.home.join("engines/fixture").exists());
        assert_eq!(fixture.settings(), settings);
    }
    fixture.success(&["engines", "disable", "fixture"]);
    fixture.install();
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    assert_eq!(
        fixture.settings()["engines"]["config"],
        settings["engines"]["config"]
    );
    assert!(!fixture.package.join("ran").exists());
    assert!(!fixture.home.join("engines/fixture/ran").exists());
}

#[cfg(unix)]
#[test]
fn shareable_config_is_replaced_privately_without_changing_parent_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("");
    fixture.install();
    let parent = fixture.root.join("shared-settings");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    let config = parent.join("settings.json");
    fs::copy(&fixture.config, &config).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
    let output = fixture
        .process(&[
            "engines",
            "configure",
            "fixture",
            "config",
            r#"{"keep":true}"#,
        ])
        .env("CONFIG", &config)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let settings: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(settings["notes"]["owner"], "keep this note");
    assert_eq!(
        settings["engines"]["config"]["fixture"],
        json!({"config":{"keep":true}})
    );
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(parent.join("settings.json.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    for (path, mode) in [(&config, 0o666), (&parent, 0o777)] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(!fixture
            .process(&["engines", "disable", "fixture"])
            .env("CONFIG", &config)
            .output()
            .unwrap()
            .status
            .success());
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if path == &config { 0o600 } else { 0o755 }),
        )
        .unwrap();
    }
    fs::remove_dir_all(&fixture.home).unwrap();
    fixture.success(&["engines", "disable", "mwmbl"]);
    assert_eq!(
        fs::metadata(&fixture.home).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&fixture.config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn prerequisite_diagnostics_inspect_names_and_presence_without_execution() {
    let fixture = Fixture::new("from pathlib import Path\nPath('ran').touch()\n");
    let manifest = fixture.package.join("engine.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["requires"] = json!(["search-cli-requirement-fixture"]);
    value["adapter"]["env"] = json!(["SEARCH_CLI_MISSING_ENV", "FIXTURE_DECLARED"]);
    fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    fixture.install();
    let inspect = || {
        let output = fixture
            .process(&["engines", "list"])
            .env_remove("SEARCH_CLI_MISSING_ENV")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains("declared-value"));
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "fixture")
            .unwrap()
            .clone()
    };
    let row = inspect();
    assert_eq!(
        row["missing_executables"],
        json!(["search-cli-requirement-fixture"])
    );
    assert_eq!(row["missing_env"], json!(["SEARCH_CLI_MISSING_ENV"]));
    let output = fixture
        .process(&["engines", "enable", "fixture", "--trust"])
        .env_remove("SEARCH_CLI_MISSING_ENV")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("search-cli-requirement-fixture"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("SEARCH_CLI_MISSING_ENV"));
    assert!(!fixture.home.join("engines/fixture/ran").exists());
    assert_eq!(fixture.settings()["engines"]["use"], json!([]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = fixture.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join("search-cli-requirement-fixture");
        fs::write(&executable, "#!/bin/sh\n: > \"$0.ran\"\nexit 77\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
        let nonexecutable = fixture
            .process(&["engines", "enable", "fixture", "--trust"])
            .env("PATH", &bin)
            .env("SEARCH_CLI_MISSING_ENV", "credential-secret")
            .output()
            .unwrap();
        assert!(!nonexecutable.status.success());
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        // A PATH containing only our fixture proves presence checks never invoke it, including version probes.
        let output = fixture
            .process(&["engines", "enable", "fixture", "--trust"])
            .env("PATH", &bin)
            .env("SEARCH_CLI_MISSING_ENV", "credential-secret")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("credential-secret"));
        assert!(!bin.join("search-cli-requirement-fixture.ran").exists());
        assert!(!fixture.home.join("engines/fixture/ran").exists());
    }
    #[cfg(windows)]
    {
        let bin = fixture.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join("search-cli-requirement-fixture.EXE");
        // Only inspection occurs: a bogus PE is sufficient to prove no execution or version probe.
        fs::write(&executable, b"not a program").unwrap();
        let output = fixture
            .process(&["engines", "enable", "fixture", "--trust"])
            .env("PATH", &bin)
            .env("PATHEXT", ".EXE;.CMD")
            .env("SEARCH_CLI_MISSING_ENV", "credential-secret")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!fixture.home.join("engines/fixture/ran").exists());
        let no_extension = fixture
            .process(&["engines", "list"])
            .env("PATH", &bin)
            .env("PATHEXT", ".CMD")
            .env("SEARCH_CLI_MISSING_ENV", "credential-secret")
            .output()
            .unwrap();
        assert!(no_extension.status.success());
        let rows: Value = serde_json::from_slice(&no_extension.stdout).unwrap();
        let row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "fixture")
            .unwrap();
        assert_eq!(
            row["missing_executables"],
            json!(["search-cli-requirement-fixture"])
        );
    }
}

#[test]
fn embedded_default_cannot_be_replaced_by_local_code() {
    let fixture = Fixture::new("from pathlib import Path\nPath('ran').touch()\n");
    let manifest = fixture.package.join("engine.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["id"] = json!("mwmbl");
    fs::write(manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::write(&fixture.config, r#"{"engines":{"use":["mwmbl"]}}"#).unwrap();
    {
        let output = fixture
            .process(&["engines", "install"])
            .arg(&fixture.package)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "embedded default replacement: {output:?}"
        );
        assert!(!fixture.home.join("engines/mwmbl").exists());
    }
    let listed = fixture.success(&["engines", "list"]);
    let rows: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(rows[0]["source"], "embedded");
    assert_eq!(rows[0]["enabled"], true);
}

/// A Git Bash HOME must not redirect native Windows's implicit package root.
#[cfg(windows)]
#[test]
fn implicit_windows_home_uses_userprofile_and_explicit_home_still_wins() {
    let fixture = Fixture::new("");
    let profile = fixture.root.join("profile");
    fs::create_dir(&profile).unwrap();
    windows::secure(&profile);
    let output = fixture
        .process(&["engines", "install", "mwmbl"])
        .env_remove("SEARCH_HOME")
        .env("USERPROFILE", &profile)
        .env("HOME", "/git-bash-home")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(profile.join(".search/engines/mwmbl/engine.json").is_file());
    let explicit = fixture.root.join("explicit");
    let output = fixture
        .process(&["engines", "install", "mwmbl"])
        .env("SEARCH_HOME", &explicit)
        .env("USERPROFILE", "relative-invalid-profile")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(explicit.join("engines/mwmbl/engine.json").is_file());
}
