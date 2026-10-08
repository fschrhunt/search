//! Engine commands: list, select, configure and explicitly test the engines
//! defined in settings. They always run locally. Writes edit settings.json under
//! an exclusive lock and replace it atomically with an owner-only file.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::core::config::{self, Config, Preset, PRESETS};
use serde_json::{json, Map, Value};

/// Dispatch an engine command locally, regardless of the selected remote profile.
pub async fn command(
    action: &str,
    args: Vec<String>,
    json: bool,
    config: Option<String>,
) -> Result<(), String> {
    let path = settings_path(config)?;
    match (action, args.as_slice()) {
        ("list", []) => list(&load(&path)?, json),
        ("enable", [id]) => {
            select(&path, true, |selected| {
                if !selected.contains(id) {
                    selected.push(id.clone());
                }
            })?;
            let settings = load(&path)?;
            if !settings.engines.enabled {
                println!("Selected {id}; engines.enabled is false, so no engine runs");
            } else {
                println!("Selected {id}");
            }
            Ok(())
        }
        ("disable", [id]) => {
            validate_id(id)?;
            select(&path, false, |selected| selected.retain(|name| name != id))?;
            println!("Deselected {id}");
            Ok(())
        }
        ("configure", [id]) => show(&load(&path)?, id),
        ("configure", [id, field, text]) => {
            validate_id(id)?;
            if field == "type" && Preset::find(id).is_some() {
                return Err("type cannot change for a built-in engine".into());
            }
            let value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
            let mut written = None;
            change_settings(&path, |raw| {
                let configs = engines_object(raw)?
                    .entry("config")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or("engines.config must be an object")?;
                configs
                    .entry(id.clone())
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| format!("engines.config.{id} must be an object"))?
                    .insert(field.clone(), value);
                written = Some(checked(raw, true)?);
                Ok(())
            })?;
            let settings = written.ok_or("settings were not written")?;
            println!("Configured {id}.{field}");
            println!("{id}: {}", status(&settings, id));
            Ok(())
        }
        ("test", [id, words @ ..]) => {
            let query = words.join(" ");
            if query.trim().is_empty() {
                return Err("engine test needs a nonempty query".into());
            }
            let mut settings = load(&path)?;
            settings.engines.enabled = true;
            settings.engines.use_engines = vec![id.clone()];
            let service = crate::core::Search::open(settings).map_err(|e| e.to_string())?;
            let answer = service
                .search(crate::core::Query {
                    text: query,
                    engines: vec![id.clone()],
                    ..Default::default()
                })
                .await;
            if json {
                crate::cli::render::json(&answer);
            } else {
                crate::cli::render::search(&answer);
            }
            if answer
                .engines
                .iter()
                .any(|state| state.status != crate::core::engines::EngineStatus::Ok)
            {
                return Err(format!(
                    "engine {id} test failed; check search configure {id}"
                ));
            }
            Ok(())
        }
        _ => Err("invalid engines command or arguments; see search help".into()),
    }
}

/// One row of `search engines`: built-in presets first, then configured custom
/// engines, then selected IDs with no definition.
fn list(settings: &Config, json: bool) -> Result<(), String> {
    let engines = &settings.engines;
    let mut ids: Vec<String> = PRESETS.iter().map(|preset| preset.id.to_owned()).collect();
    for id in engines.config.keys().chain(&engines.use_engines) {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| {
            let definition = engines.definition(id).ok();
            json!({
                "id": id,
                "builtin": Preset::find(id).is_some(),
                "type": definition.as_ref().and_then(|d| d.get("type")).cloned(),
                "selected": engines.use_engines.contains(id),
                "status": status(settings, id),
            })
        })
        .collect();
    if json {
        crate::cli::render::json(&rows);
        return Ok(());
    }
    for row in &rows {
        let text = |key: &str| {
            row.get(key)
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_owned()
        };
        let selected = row.get("selected").and_then(Value::as_bool) == Some(true);
        let origin = if row.get("builtin").and_then(Value::as_bool) == Some(true) {
            "built-in"
        } else {
            "custom"
        };
        println!(
            "{} {:<12} {:<8} {:<7} {}",
            if selected { "*" } else { " " },
            text("id"),
            origin,
            text("type"),
            text("status")
        );
    }
    if !engines.enabled {
        println!("engines.enabled is false: no engine runs");
    }
    println!("* selected. Edit with search enable|disable|configure ID.");
    Ok(())
}

/// Print an engine's effective adapter (preset merged with its entry) and status.
fn show(settings: &Config, id: &str) -> Result<(), String> {
    let definition = settings.engines.definition(id).map_err(|e| e.to_string())?;
    if let Some(preset) = Preset::find(id) {
        println!("{id}: built-in. {}", preset.description);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&definition).map_err(|e| e.to_string())?
    );
    println!("status: {}", status(settings, id));
    println!("Set a field with search configure {id} FIELD VALUE (JSON, or a string).");
    println!("Reference credentials by variable name with header_env (http) or env (command).");
    Ok(())
}

/// `ready`, or why the engine cannot run; never quotes configured values.
fn status(settings: &Config, id: &str) -> String {
    match settings.engines.adapter(id) {
        Ok(_) => "ready".into(),
        Err(error) => error
            .message()
            .strip_prefix(&format!("engine {id}: "))
            .unwrap_or(error.message())
            .to_owned(),
    }
}

/// Load settings from the resolved path; a missing file means defaults, as
/// the first write will create it.
fn load(path: &Path) -> Result<Config, String> {
    if path.exists() {
        return config::load(Some(path.to_path_buf())).map_err(|e| e.message().to_owned());
    }
    checked(&json!({}), false)
}

/// Rewrite `engines.use` under the lock. Selecting requires every selected
/// engine to resolve; deselecting is always allowed so a broken engine can be
/// switched off.
fn select(path: &Path, resolve: bool, edit: impl FnOnce(&mut Vec<String>)) -> Result<(), String> {
    change_settings(path, |raw| {
        let mut selected = checked(raw, false)?.engines.use_engines;
        edit(&mut selected);
        engines_object(raw)?.insert("use".into(), json!(selected));
        checked(raw, resolve).map(|_| ())
    })
}

/// Parse and validate raw settings with the running Search home. With
/// `resolve`, every selected engine must also resolve (when engines are
/// enabled); unselected engines may stay incomplete while being configured.
fn checked(raw: &Value, resolve: bool) -> Result<Config, String> {
    let mut settings: Config = serde_path_to_error::deserialize(raw.clone()).map_err(|error| {
        format!(
            "invalid settings (field {}); repair the settings file",
            config::engines::safe_field_path(error.path())
        )
    })?;
    settings.home = config::home().map_err(|e| e.message().to_owned())?;
    settings.validate().map_err(|e| e.message().to_owned())?;
    if resolve && settings.engines.enabled {
        for id in &settings.engines.use_engines {
            settings
                .engines
                .adapter(id)
                .map_err(|e| e.message().to_owned())?;
        }
    }
    Ok(settings)
}

/// Find the settings file independently of Search home.
fn settings_path(explicit: Option<String>) -> Result<PathBuf, String> {
    explicit
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CONFIG").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(|| config::settings_path().map_err(|e| e.to_string()))
}

/// Obtain the existing engine settings object without discarding sibling fields.
fn engines_object(settings: &mut Value) -> Result<&mut Map<String, Value>, String> {
    settings
        .as_object_mut()
        .ok_or("settings must be an object")?
        .entry("engines")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "engines must be an object".into())
}

/// Keep engine IDs well formed even when deselecting an undefined engine.
fn validate_id(id: &str) -> Result<(), String> {
    if !config::valid_engine_id(id) {
        return Err(
            "invalid engine ID; use lowercase letters, digits, hyphens or underscores (max 64)"
                .into(),
        );
    }
    Ok(())
}

/// Create new directories privately; existing owner-owned directories may be readable by others.
#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => check_storage(&meta, true, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("settings directory has no parent")?;
            if !parent.exists() {
                private_directory(parent)?;
            }
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(format!("create settings directory: {e}")),
            }
            check_storage(
                &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
                true,
                false,
            )
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Reject links, foreign owners and shared write access; locks and new files also deny shared reads.
#[cfg(unix)]
fn check_storage(meta: &fs::Metadata, directory: bool, owner_only: bool) -> Result<(), String> {
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err("settings storage must be a real directory/file, never a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no arguments and dereferences no memory.
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err("settings storage must belong to the current user".into());
        }
        let forbidden = if owner_only { 0o077 } else { 0o022 };
        if meta.permissions().mode() & forbidden != 0 {
            return Err(if owner_only {
                "settings locks and replacement files must be owner-only"
            } else {
                "settings storage must not be group/world writable"
            }
            .into());
        }
    }
    Ok(())
}

/// Open files without following symlinks, checking descriptor ownership and the requested permission boundary.
#[cfg(unix)]
fn private_open(path: &Path, create_new: bool, owner_only: bool) -> Result<File, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).truncate(false);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("open private settings storage: {e}"))?;
    check_storage(
        &file.metadata().map_err(|e| e.to_string())?,
        false,
        owner_only,
    )?;
    Ok(file)
}

/// Windows settings directories are created with protected current-user ACLs.
#[cfg(windows)]
fn private_directory(path: &Path) -> Result<(), String> {
    crate::private_fs::directories(path)
}

/// Fail closed where ownership and no-follow primitives are unavailable.
#[cfg(not(any(unix, windows)))]
fn private_directory(_path: &Path) -> Result<(), String> {
    Err("private settings writes currently require Unix permissions".into())
}

/// Windows validates the opened settings or lock handle before any content is read or written.
#[cfg(windows)]
fn private_open(path: &Path, create_new: bool, owner_only: bool) -> Result<File, String> {
    let boundary = if owner_only {
        crate::private_fs::Boundary::Private
    } else {
        crate::private_fs::Boundary::Settings
    };
    crate::private_fs::open(path, true, create_new, !create_new, boundary)
}

/// Fail closed where ownership and no-follow primitives are unavailable.
#[cfg(not(any(unix, windows)))]
fn private_open(_path: &Path, _create_new: bool, _owner_only: bool) -> Result<File, String> {
    Err("private settings writes currently require Unix permissions".into())
}

/// Lock the entire read/change/replace transaction. New directories, the lock
/// and the replacement file are owner-only; an existing parent keeps its mode.
fn change_settings(
    path: &Path,
    change: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let parent = absolute.parent().ok_or("settings path has no parent")?;
    private_directory(parent)?;
    let parent = fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let path = parent.join(
        absolute
            .file_name()
            .ok_or("settings path needs a filename")?,
    );
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock = private_open(Path::new(&lock_name), false, true)?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    let mut settings = match fs::symlink_metadata(&path) {
        Ok(_meta) => {
            #[cfg(unix)]
            check_storage(&_meta, false, false)?;
            let mut file = private_open(&path, false, false)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            serde_json::from_slice(&bytes)
                .map_err(|_| "invalid settings JSON; repair the settings file")?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.to_string()),
    };
    change(&mut settings)?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|_| "cannot serialize settings")?;
    let temp = parent.join(format!(".settings-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = private_open(&temp, true, true)?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        #[cfg(windows)]
        crate::private_fs::replace(&temp, &path)?;
        #[cfg(unix)]
        fs::rename(&temp, &path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        File::open(&parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
