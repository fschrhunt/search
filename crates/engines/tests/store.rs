//! Store contract tests use private temporary homes and never execute engine code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use search_engines::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Own and clean an isolated temporary tree; nested engine homes start absent.
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("search-engines-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }
    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
    fn local(&self) -> PathBuf {
        let path = self.0.join("local");
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("scripts")).unwrap();
        fs::write(path.join("scripts/run.py"), "print('not executed')").unwrap();
        fs::write(path.join("engine.json"), serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "id": "fixture", "version": "1", "description": "local fixture",
            "adapter": {"type": "command", "command": "python3", "args": ["scripts/run.py"]},
            "files": ["scripts/run.py"]
        })).unwrap()).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_fallback_is_embedded_and_does_not_create_home() {
    let temp = Temp::new();
    let home = temp.home();
    assert!(list(&home).unwrap().is_empty());
    let p = resolve(&home, DEFAULT_ENGINE).unwrap();
    assert_eq!(p.source, "embedded");
    assert_eq!(p.manifest.adapter["query_param"], "s");
    assert_eq!(p.manifest.adapter["results_pointer"], "");
    assert_eq!(p.manifest.adapter["title_pointer"], "/title");
    assert_eq!(p.manifest.adapter["text_part_pointer"], "/value");
    assert!(!home.exists());
    assert!(resolve(&home, "searxng").is_err());
    assert!(remove(&home, DEFAULT_ENGINE).is_err());
}

#[test]
fn catalog_install_collision_update_remove_and_fallback() {
    let temp = Temp::new();
    let home = temp.home();
    let package = install_catalog(&home, "mwmbl").unwrap();
    assert_eq!(package.source, "catalog");
    assert_eq!(resolve(&home, "mwmbl").unwrap().digest, package.digest);
    assert_eq!(update(&home, "mwmbl").unwrap().digest, package.digest);
    assert!(install_catalog(&home, "mwmbl").is_err());
    assert_eq!(list(&home).unwrap().len(), 1);
    remove(&home, "mwmbl").unwrap();
    assert!(list(&home).unwrap().is_empty());
    assert_eq!(resolve(&home, "mwmbl").unwrap().source, "embedded");
    assert!(remove(&home, "mwmbl").is_err());
}

#[test]
fn catalog_tampering_refuses_resolve_and_update_without_overwriting() {
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "brave").unwrap();
    fs::write(p.path.join("engine.py"), "edited sentinel").unwrap();
    assert!(resolve(&home, "brave").is_err());
    assert!(update(&home, "brave").is_err());
    assert_eq!(
        fs::read_to_string(p.path.join("engine.py")).unwrap(),
        "edited sentinel"
    );
    remove(&home, "brave").unwrap();
}

#[test]
fn malformed_receipts_fail_closed_without_exposing_contents() {
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "mwmbl").unwrap();
    fs::write(p.path.join(".receipt.json"), "malformed receipt secret").unwrap();
    for error in [
        resolve(&home, "mwmbl").unwrap_err(),
        update(&home, "mwmbl").unwrap_err(),
        remove(&home, "mwmbl").unwrap_err(),
    ] {
        assert!(!error.contains("secret"));
    }
}

#[test]
fn local_install_is_an_independent_copy_and_cannot_be_updated() {
    let temp = Temp::new();
    let source = temp.local();
    let home = temp.home();
    let p = install_local(&home, &source).unwrap();
    assert_eq!(p.source, "local");
    fs::write(source.join("scripts/run.py"), "changed source").unwrap();
    assert_eq!(
        fs::read_to_string(p.path.join("scripts/run.py")).unwrap(),
        "print('not executed')"
    );
    assert!(install_local(&home, &source).is_err());
    assert!(update(&home, "fixture").is_err());
    remove(&home, "fixture").unwrap();
    assert!(source.join("engine.json").exists());
}

#[test]
fn undeclared_traversal_duplicate_and_install_hook_manifests_are_rejected() {
    for bad in [
        "extra",
        "traversal",
        "duplicate",
        "hooks",
        "reserved",
        "wrong-type",
        "file-prefix",
        "undeclared-executable",
        "executable-traversal",
        "requirement-path",
    ] {
        let temp = Temp::new();
        let source = temp.local();
        let path = source.join("engine.json");
        let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match bad {
            "extra" => fs::write(source.join("undeclared"), "secret").unwrap(),
            "traversal" => m["files"] = serde_json::json!(["../secret"]),
            "duplicate" => m["files"] = serde_json::json!(["scripts/run.py", "scripts/run.py"]),
            "hooks" => m["install"] = "execute secret".into(),
            "reserved" => m["id"] = "index".into(),
            "wrong-type" => m["adapter"]["type"] = "shell".into(),
            "file-prefix" => m["files"] = serde_json::json!(["scripts", "scripts/run.py"]),
            "undeclared-executable" => m["executables"] = serde_json::json!(["missing"]),
            "executable-traversal" => m["executables"] = serde_json::json!(["../secret"]),
            "requirement-path" => m["requires"] = serde_json::json!(["/usr/bin/python3"]),
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
        let error = install_local(&temp.home(), &source).unwrap_err();
        assert!(!error.contains("secret"));
        assert!(!temp.home().exists());
    }
}

#[test]
fn oversized_files_and_manifests_are_rejected_before_installation() {
    let temp = Temp::new();
    let source = temp.local();
    let file = fs::File::create(source.join("scripts/run.py")).unwrap();
    file.set_len(2 * 1024 * 1024 + 1).unwrap();
    assert!(install_local(&temp.home(), &source).is_err());
    file.set_len(0).unwrap();
    let manifest = fs::File::create(source.join("engine.json")).unwrap();
    manifest.set_len(2 * 1024 * 1024 + 1).unwrap();
    assert!(install_local(&temp.home(), &source).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_assets_roots_and_store_paths_are_rejected() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let source = temp.local();
    fs::write(temp.0.join("outside"), "outside sentinel").unwrap();
    fs::remove_file(source.join("scripts/run.py")).unwrap();
    symlink(temp.0.join("outside"), source.join("scripts/run.py")).unwrap();
    assert!(install_local(&temp.home(), &source).is_err());
    symlink(&source, temp.0.join("alias")).unwrap();
    assert!(install_local(&temp.home(), &temp.0.join("alias")).is_err());
    symlink(&source, temp.home()).unwrap();
    assert!(install_catalog(&temp.home(), "mwmbl").is_err());
    assert!(resolve(&temp.home(), "mwmbl").is_err());
    assert_eq!(
        fs::read_to_string(temp.0.join("outside")).unwrap(),
        "outside sentinel"
    );
}

#[cfg(unix)]
#[test]
fn insecure_permissions_hardlinks_and_replaced_receipts_fail_closed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "mwmbl").unwrap();
    for path in [
        &home,
        &home.join("engines"),
        &p.path,
        &p.path.join("engine.json"),
    ] {
        let mode = fs::metadata(path).unwrap().permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o004)).unwrap();
        assert!(resolve(&home, "mwmbl").is_err());
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    fs::hard_link(p.path.join("engine.json"), temp.0.join("alias")).unwrap();
    assert!(resolve(&home, "mwmbl").is_err());
    fs::remove_file(temp.0.join("alias")).unwrap();
    let receipt = p.path.join(".receipt.json");
    fs::rename(&receipt, temp.0.join("receipt")).unwrap();
    symlink(temp.0.join("receipt"), &receipt).unwrap();
    assert!(remove(&home, "mwmbl").is_err());
    assert!(temp.0.join("receipt").exists());
}

#[test]
fn invalid_ids_and_nonabsolute_home_do_not_touch_disk() {
    let temp = Temp::new();
    for id in ["../other", "UPPER", "index", "", "a/b", ".stage-x", "🦀"] {
        assert!(resolve(&temp.home(), id).is_err());
        assert!(install_catalog(&temp.home(), id).is_err());
        assert!(update(&temp.home(), id).is_err());
        assert!(remove(&temp.home(), id).is_err());
    }
    assert!(resolve(Path::new(""), "mwmbl").is_err());
    assert!(install_catalog(Path::new("relative"), "mwmbl").is_err());
    assert!(!temp.home().exists());
}

#[test]
fn only_mutations_recover_interrupted_replacements() {
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "mwmbl").unwrap();
    let old = home.join("engines/.old-mwmbl");
    fs::rename(&p.path, &old).unwrap();
    assert_eq!(resolve(&home, "mwmbl").unwrap().source, "embedded");
    assert!(list(&home).unwrap().is_empty());
    assert!(old.exists());
    install_catalog(&home, "searxng").unwrap();
    assert_eq!(resolve(&home, "mwmbl").unwrap().digest, p.digest);
    assert!(!old.exists());
}

#[test]
fn partial_deletion_tombstones_survive_inspection_and_recover_on_mutation() {
    for delete_lease in [false, true] {
        let temp = Temp::new();
        let home = temp.home();
        let p = install_catalog(&home, "brave").unwrap();
        let tombstone = home.join(format!("engines/.delete-{}", uuid::Uuid::new_v4()));
        fs::rename(p.path, &tombstone).unwrap();
        fs::remove_file(tombstone.join(".receipt.json")).unwrap();
        if delete_lease {
            fs::remove_file(tombstone.join(".lease")).unwrap();
        }
        assert!(list(&home).unwrap().is_empty());
        assert_eq!(resolve(&home, "mwmbl").unwrap().source, "embedded");
        assert!(resolve(&home, "brave").is_err());
        assert!(tombstone.exists());
        install_catalog(&home, "searxng").unwrap();
        assert!(!tombstone.exists());
    }
}

#[test]
fn absent_default_ignores_unrelated_damage_and_installed_default_fails_closed() {
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "brave").unwrap();
    fs::write(p.path.join(".receipt.json"), "broken").unwrap();
    fs::remove_file(home.join("engines/.lock")).unwrap();
    assert_eq!(resolve(&home, "mwmbl").unwrap().source, "embedded");
    assert!(list(&home).is_err());
    install_catalog(&home, "mwmbl").unwrap();
    fs::write(home.join("engines/mwmbl/engine.json"), "broken").unwrap();
    assert!(resolve(&home, "mwmbl").is_err());
}

#[test]
fn resolved_leases_block_mutation_until_every_clone_is_dropped() {
    let temp = Temp::new();
    let home = temp.home();
    assert!(install_catalog(&home, "mwmbl").unwrap().lease.is_none());
    let running = resolve(&home, "mwmbl").unwrap();
    let retained = running.clone();
    assert!(running.lease.is_some());
    assert!(list(&home).unwrap()[0].lease.is_none());
    for error in [
        update(&home, "mwmbl").unwrap_err(),
        remove(&home, "mwmbl").unwrap_err(),
    ] {
        assert!(error.contains("stop running Search hosts"));
    }
    install_catalog(&home, "searxng").unwrap();
    remove(&home, "searxng").unwrap();
    drop(running);
    assert!(update(&home, "mwmbl").is_err());
    drop(retained);
    update(&home, "mwmbl").unwrap();
    remove(&home, "mwmbl").unwrap();
}

#[cfg(unix)]
#[test]
fn declared_native_executable_is_runnable_and_other_assets_are_not_executable() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let temp = Temp::new();
    let source = temp.local();
    fs::create_dir(source.join("bin")).unwrap();
    fs::copy("/usr/bin/true", source.join("bin/native")).unwrap();
    fs::set_permissions(
        source.join("scripts/run.py"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let mut m: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join("engine.json")).unwrap()).unwrap();
    m["files"] = serde_json::json!(["bin/native", "scripts/run.py"]);
    m["executables"] = serde_json::json!(["bin/native"]);
    m["adapter"] = serde_json::json!({"type":"command", "command":"./bin/native"});
    fs::write(source.join("engine.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    let p = install_local(&temp.home(), &source).unwrap();
    assert_eq!(
        fs::metadata(p.path.join("bin/native")).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(p.path.join("scripts/run.py")).unwrap().mode() & 0o777,
        0o600
    );
    assert!(Command::new(p.path.join("bin/native"))
        .status()
        .unwrap()
        .success());
}

#[cfg(unix)]
#[test]
fn mutation_rejects_writable_ancestors_before_changing_packages() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    fs::set_permissions(&temp.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(install_catalog(&temp.home(), "mwmbl")
        .unwrap_err()
        .contains("trusted ancestors"));
    assert!(!temp.home().exists());
    fs::set_permissions(&temp.0, fs::Permissions::from_mode(0o700)).unwrap();
    install_catalog(&temp.home(), "mwmbl").unwrap();
    fs::set_permissions(&temp.0, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(update(&temp.home(), "mwmbl").is_err());
    assert!(remove(&temp.home(), "mwmbl").is_err());
}

/// Child entry point for the real cross-process collision test.
#[test]
fn install_in_child() {
    if let Some(home) = std::env::var_os("SEARCH_ENGINES_CHILD_HOME") {
        let result = install_catalog(Path::new(&home), "mwmbl");
        std::process::exit(if result.is_ok() { 0 } else { 7 });
    }
}

#[test]
fn concurrent_process_installs_publish_exactly_one_complete_package() {
    let temp = Temp::new();
    let home = temp.home();
    let executable = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    for _ in 0..6 {
        children.push(
            Command::new(&executable)
                .args(["--exact", "install_in_child"])
                .env("SEARCH_ENGINES_CHILD_HOME", &home)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    let successes = children
        .iter_mut()
        .map(|c| c.wait().unwrap())
        .filter(|s| s.success())
        .count();
    assert_eq!(successes, 1);
    assert_eq!(list(&home).unwrap().len(), 1);
    update(&home, "mwmbl").unwrap();
}

#[test]
fn every_catalog_package_has_valid_assets_and_requires_no_execution_to_install() {
    let temp = Temp::new();
    let home = temp.home();
    let manifests = catalog().unwrap();
    assert_eq!(manifests.len(), 9);
    for m in manifests {
        let p = install_catalog(&home, &m.id).unwrap();
        assert_eq!(p.manifest.id, m.id);
        if m.id == "searxng" {
            assert_eq!(m.required, ["url"]);
        }
        if m.adapter["type"] == "command" {
            assert_eq!(m.requires, ["python3"]);
            assert_eq!(
                m.adapter["args"],
                serde_json::json!(["-S", "-B", "engine.py"])
            );
        }
    }
    assert_eq!(list(&home).unwrap().len(), 9);
}

/// A shared lease held by a separate process must prevent nonblocking mutation too.
#[cfg(unix)]
#[test]
fn another_process_holding_a_lease_blocks_update_and_remove() {
    use std::io::{BufRead, BufReader, Write};
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "mwmbl").unwrap();
    let mut child = Command::new("python3")
        .args(["-S", "-B", "-c", "import fcntl,sys,select; f=open(sys.argv[1],'rb'); fcntl.flock(f,fcntl.LOCK_SH); print('ready',flush=True); select.select([sys.stdin],[],[],10)"])
        .arg(p.path.join(".lease")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    assert!(update(&home, "mwmbl")
        .unwrap_err()
        .contains("stop running Search hosts"));
    assert!(remove(&home, "mwmbl")
        .unwrap_err()
        .contains("stop running Search hosts"));
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    assert!(child.wait().unwrap().success());
    update(&home, "mwmbl").unwrap();
    remove(&home, "mwmbl").unwrap();
}

/// Inspecting orphan stages must not delete their files; explicit mutation performs cleanup.
#[test]
fn inspection_leaves_unpublished_stage_contents_untouched() {
    let temp = Temp::new();
    let home = temp.home();
    let p = install_catalog(&home, "brave").unwrap();
    let stage = home.join(format!("engines/.stage-{}", uuid::Uuid::new_v4()));
    fs::rename(&p.path, &stage).unwrap();
    let before = fs::read(stage.join("engine.json")).unwrap();
    assert!(list(&home).unwrap().is_empty());
    assert_eq!(resolve(&home, "mwmbl").unwrap().source, "embedded");
    assert_eq!(fs::read(stage.join("engine.json")).unwrap(), before);
    install_catalog(&home, "searxng").unwrap();
    assert!(!stage.exists());
}
