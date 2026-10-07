//! Local package management. Inspection never executes code; settings changes are private and atomic.

use std::{
    fs::{self, File},
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use crate::core::config;
use crate::engines::{Installed, DEFAULT_ENGINE};
use serde_json::{json, Map, Value};

/// Dispatch package commands locally, regardless of the selected remote profile.
pub async fn command(
    action: &str,
    args: Vec<String>,
    config: Option<String>,
) -> Result<(), String> {
    let home = config::home().map_err(|e| e.to_string())?;
    let explicit_settings = config.is_some() || std::env::var_os("CONFIG").is_some();
    let path = settings_path(config)?;
    match action {
        "available" if args.is_empty() => print_json(&crate::engines::catalog()?),
        "list" if args.is_empty() => {
            let settings = read_settings(&path)?;
            let mut packages = crate::engines::list(&home)?;
            if !packages.iter().any(|p| p.manifest.id == DEFAULT_ENGINE) {
                packages.push(crate::engines::resolve(&home, DEFAULT_ENGINE)?);
            }
            let selected = selection(&settings)?;
            let globally_enabled = settings
                .pointer("/engines/enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut rows = Vec::new();
            for package in packages {
                let adapter = merged_adapter(&package, &settings)?;
                let (executables, environment) = missing_prerequisites(&package, &adapter)?;
                rows.push(json!({
                    "id": package.manifest.id,
                    "version": package.manifest.version,
                    "source": package.source,
                    "enabled": globally_enabled && selected.contains(&package.manifest.id),
                    "selected": selected.contains(&package.manifest.id),
                    "missing": missing(&package, &adapter),
                    "missing_executables": executables,
                    "missing_env": environment,
                }));
            }
            for id in selected {
                if !rows
                    .iter()
                    .any(|row| row.get("id").and_then(Value::as_str) == Some(&id))
                {
                    rows.push(json!({"id": id, "enabled": false, "selected": true, "missing": ["package"], "source": "missing"}));
                }
            }
            print_json(&rows)
        }
        "install" if args.len() == 1 => {
            let target = first(&args)?;
            let package = publish_package(&home, &path, || {
                if Path::new(target).is_absolute()
                    || target.starts_with('.')
                    || target.contains(std::path::MAIN_SEPARATOR)
                {
                    crate::engines::install_local(&home, Path::new(target))
                } else {
                    crate::engines::install_catalog(&home, target)
                }
            })?;
            println!(
                "Installed {}; use search configure/enable to activate it",
                package.manifest.id
            );
            Ok(())
        }
        "update" if args.len() == 1 => {
            let package = crate::engines::update(&home, first(&args)?)?;
            println!(
                "Updated {} to {}",
                package.manifest.id, package.manifest.version
            );
            Ok(())
        }
        "remove" if args.len() == 1 => {
            let id = first(&args)?;
            validate_id(id)?;
            // Keep the settings lock through removal so a concurrent enable cannot select it mid-removal.
            change_settings(
                &path,
                |settings| disable(settings, id),
                || {
                    crate::engines::remove(&home, id).map_err(|e| format!("package removal failed; engine disabled and configuration retained: {e}"))
                },
            )?;
            println!("Removed {id}; per-engine configuration retained");
            Ok(())
        }
        "disable" if args.len() == 1 => {
            let id = first(&args)?;
            validate_id(id)?;
            change_settings(&path, |settings| disable(settings, id), || Ok(()))?;
            println!("Disabled {id}");
            Ok(())
        }
        "enable" => {
            let trusted = args.iter().any(|s| s == "--trust");
            let names: Vec<_> = args.iter().filter(|s| s.as_str() != "--trust").collect();
            if names.len() != 1 || args.iter().filter(|s| s.as_str() == "--trust").count() > 1 {
                return Err("usage: search enable NAME [--trust]".into());
            }
            let id = names.first().ok_or("missing engine name")?.as_str();
            let package = crate::engines::resolve(&home, id)?;
            let adapter = merged_adapter(&package, &read_settings(&path)?)?;
            validate_adapter(&package, &adapter, true)?;
            require_prerequisites(&package, &adapter)?;
            let command = adapter.get("type").and_then(Value::as_str) == Some("command");
            if command {
                consent(id, trusted)?;
            }
            change_settings(
                &path,
                |settings| {
                    let current = crate::engines::resolve(&home, id)?;
                    if current.digest != package.digest || current.path != package.path {
                        return Err(
                            "package changed during activation; inspect it and retry".into()
                        );
                    }
                    let current_adapter = merged_adapter(&current, settings)?;
                    if command && current_adapter != adapter {
                        return Err("command configuration changed during trust decision; inspect it and retry".into());
                    }
                    validate_adapter(&current, &current_adapter, true)?;
                    require_prerequisites(&current, &current_adapter)?;
                    let mut selected = selection(settings)?;
                    if !selected.iter().any(|name| name == id) {
                        selected.push(id.to_owned());
                    }
                    engines_object(settings)?.insert("use".into(), json!(selected));
                    Ok(())
                },
                || Ok(()),
            )?;
            println!("Selected {id}; engines.enabled remains unchanged");
            Ok(())
        }
        "configure" if args.len() == 1 || args.len() == 3 => {
            let id = first(&args)?;
            let package = crate::engines::resolve(&home, id)?;
            if args.len() == 1 {
                let adapter = merged_adapter(&package, &read_settings(&path)?)?;
                println!(
                    "{}: missing required fields: {}",
                    id,
                    missing(&package, &adapter).join(", ")
                );
                println!(
                    "Use search configure {id} KEY VALUE (direct adapter fields; JSON or string)."
                );
                println!("Use header_env (HTTP) or env (command) for credentials; never provide secrets as values.");
                return Ok(());
            }
            let key = args.get(1).ok_or("missing configuration field")?;
            let text = args.get(2).ok_or("missing configuration value")?;
            let value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
            if key == "cwd"
                || (key == "type" && package.manifest.adapter.get("type") != Some(&value))
            {
                return Err(
                    "adapter type cannot change and package working directory cannot be configured"
                        .into(),
                );
            }
            eprintln!("Configuration values are stored in settings; use header_env/env references for secrets.");
            change_settings(
                &path,
                |settings| {
                    let configs = engines_object(settings)?
                        .entry("config")
                        .or_insert_with(|| json!({}));
                    let configs = configs
                        .as_object_mut()
                        .ok_or("engines.config must be an object")?;
                    let overrides = configs.entry(id.to_owned()).or_insert_with(|| json!({}));
                    overrides
                        .as_object_mut()
                        .ok_or("engine configuration must be an object")?
                        .insert(key.clone(), value);
                    validate_adapter(&package, &merged_adapter(&package, settings)?, false)
                },
                || Ok(()),
            )?;
            println!("Configured {id}.{key}");
            Ok(())
        }
        "test" if args.len() >= 2 => {
            let id = first(&args)?;
            let query = args.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            if query.trim().is_empty() {
                return Err("engine test needs a nonempty query".into());
            }
            let package = crate::engines::resolve(&home, id)?;
            let adapter = merged_adapter(&package, &read_settings(&path)?)?;
            validate_adapter(&package, &adapter, true)?;
            if adapter.get("type").and_then(Value::as_str) == Some("command") {
                eprintln!("Trust boundary: search test {id} explicitly executes package code as your user, with declared environment access; it is not sandboxed.");
            }
            let mut settings = config::load((explicit_settings || path.exists()).then_some(path))
                .map_err(|e| e.to_string())?;
            settings.engines.enabled = true;
            settings.engines.use_engines = vec![id.to_owned()];
            let service = crate::core::Search::open(settings).map_err(|e| e.to_string())?;
            let response = service
                .search(crate::engines::Query {
                    text: query,
                    engines: vec![id.to_owned()],
                    ..Default::default()
                })
                .await;
            print_json(&response)?;
            if response
                .engines
                .iter()
                .any(|p| p.status != crate::engines::EngineStatus::Ok)
            {
                return Err(format!(
                    "engine {id} test failed; check its configuration and declared environment"
                ));
            }
            Ok(())
        }
        _ => Err("invalid engines command or arguments; see search help".into()),
    }
}

/// Refuse publication while any selected nondefault ID is unavailable, holding the settings lock throughout.
/// Existing packages cannot be overwritten; the embedded default is reserved by the package installer.
fn publish_package(
    home: &Path,
    path: &Path,
    publish: impl FnOnce() -> Result<Installed, String>,
) -> Result<Installed, String> {
    let mut package = None;
    change_settings(
        path,
        |settings| {
            for id in selection(settings)? {
                if id != DEFAULT_ENGINE && crate::engines::resolve(home, &id).is_err() {
                    return Err(format!("selected engine {id} is unavailable; run search disable {id} before installing packages (configuration is retained)"));
                }
            }
            Ok(())
        },
        || {
            package = Some(publish()?);
            Ok(())
        },
    )?;
    package.ok_or_else(|| "package publication did not complete".into())
}

/// Inspect executable prerequisites and declared environment presence without reading values or running programs.
fn missing_prerequisites(
    package: &Installed,
    adapter: &Value,
) -> Result<(Vec<String>, Vec<String>), String> {
    let executables = package
        .manifest
        .requires
        .iter()
        .filter(|name| !executable_on_path(name))
        .cloned()
        .collect();
    let mut names = std::collections::BTreeSet::new();
    if let Some(env) = adapter.get("env").and_then(Value::as_array) {
        names.extend(env.iter().filter_map(Value::as_str));
    }
    if let Some(env) = adapter.get("header_env").and_then(Value::as_object) {
        names.extend(env.values().filter_map(Value::as_str));
    }
    for name in &names {
        let mut bytes = name.bytes();
        if !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("env/header_env must declare valid environment variable names".into());
        }
    }
    let environment = names
        .into_iter()
        .filter(|name| std::env::var_os(name).is_none())
        .map(str::to_owned)
        .collect();
    Ok((executables, environment))
}

/// Activation requires operator-provided prerequisites; no dependency installation or version probing.
fn require_prerequisites(package: &Installed, adapter: &Value) -> Result<(), String> {
    let (executables, environment) = missing_prerequisites(package, adapter)?;
    if !executables.is_empty() || !environment.is_empty() {
        return Err(format!("engine {} missing runtime executables [{}] or environment variables [{}]; provide these prerequisites and retry enable", package.manifest.id, executables.join(", "), environment.join(", ")));
    }
    Ok(())
}

/// Check declared runtime names on absolute PATH entries; relative entries must not imply cwd discovery.
fn executable_on_path(name: &str) -> bool {
    #[cfg(windows)]
    let names = windows_executable_names(name);
    #[cfg(not(windows))]
    let names = [name.to_string()];
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .any(|dir| {
                names.iter().any(|name| {
                    fs::metadata(dir.join(name)).is_ok_and(|meta| {
                        if !meta.is_file() {
                            return false;
                        }
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStrExt;
                            let Ok(path) =
                                std::ffi::CString::new(dir.join(name).as_os_str().as_bytes())
                            else {
                                return false;
                            };
                            // SAFETY: path is NUL-terminated and lives through this permission-only syscall.
                            unsafe {
                                libc::faccessat(
                                    libc::AT_FDCWD,
                                    path.as_ptr(),
                                    libc::X_OK,
                                    libc::AT_EACCESS,
                                ) == 0
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            true
                        }
                    })
                })
            })
    })
}

/// PATHEXT supplies executable suffixes; malformed entries cannot introduce path traversal.
#[cfg(windows)]
fn windows_executable_names(name: &str) -> Vec<String> {
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let extensions: Vec<_> = extensions
        .split(';')
        .filter(|extension| {
            extension.starts_with('.')
                && extension.len() > 1
                && extension.bytes().skip(1).all(|b| b.is_ascii_alphanumeric())
        })
        .collect();
    let lower = name.to_ascii_lowercase();
    if [".exe", ".com", ".bat", ".cmd"]
        .iter()
        .any(|extension| lower.ends_with(extension))
        || extensions
            .iter()
            .any(|extension| lower.ends_with(&extension.to_ascii_lowercase()))
    {
        vec![name.into()]
    } else {
        extensions
            .into_iter()
            .map(|extension| format!("{name}{extension}"))
            .collect()
    }
}

/// Extract an argument without assuming callers used the command-line parser.
fn first(args: &[String]) -> Result<&str, String> {
    args.first()
        .map(String::as_str)
        .ok_or_else(|| "missing engine name or path".into())
}

/// Serialize only public metadata or explicit test responses, never settings values.
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|_| "cannot serialize engine response")?
    );
    Ok(())
}

/// Find the settings file independently of the package home.
fn settings_path(explicit: Option<String>) -> Result<PathBuf, String> {
    explicit
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CONFIG").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(|| config::settings_path().map_err(|e| e.to_string()))
}

/// Missing files permit a first configuration write; malformed JSON fails without quoting contents.
fn read_settings(path: &Path) -> Result<Value, String> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "invalid settings JSON; repair the settings file")?;
            if !value.is_object() {
                return Err("settings must be an object".into());
            }
            Ok(value)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("read settings: {e}")),
    }
}

/// Omitted selection means the embedded default; an explicit empty array stays empty.
fn selection(settings: &Value) -> Result<Vec<String>, String> {
    if settings.get("engines").is_some_and(|v| !v.is_object()) {
        return Err("engines must be an object".into());
    }
    if settings
        .pointer("/engines/enabled")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err("engines.enabled must be a boolean".into());
    }
    let selected: Vec<String> = match settings.pointer("/engines/use") {
        None => vec![DEFAULT_ENGINE.into()],
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| "engines.use must be an array of engine names")?,
    };
    let mut seen = std::collections::BTreeSet::new();
    for id in &selected {
        validate_id(id)?;
        if !seen.insert(id) {
            return Err("engines.use contains a repeated engine ID".into());
        }
    }
    Ok(selected)
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

/// Persist deselection, including the default, while retaining all configuration.
fn disable(settings: &mut Value, id: &str) -> Result<(), String> {
    let mut selected = selection(settings)?;
    selected.retain(|name| name != id);
    engines_object(settings)?.insert("use".into(), json!(selected));
    Ok(())
}

/// Keep settings identifiers safe even when disabling a package that is no longer installed.
fn validate_id(id: &str) -> Result<(), String> {
    if !config::valid_engine_id(id) {
        return Err(
            "invalid engine ID; use lowercase letters, digits, hyphens or underscores (max 64)"
                .into(),
        );
    }
    Ok(())
}

/// Shallow-merge operator overrides, forbidding transport changes without resolving credentials.
fn merged_adapter(package: &Installed, settings: &Value) -> Result<Value, String> {
    let mut adapter = package.manifest.adapter.clone();
    let object = adapter
        .as_object_mut()
        .ok_or("package adapter must be an object")?;
    if let Some(overrides) = settings
        .pointer("/engines/config")
        .and_then(|v| v.get(&package.manifest.id))
    {
        let overrides = overrides
            .as_object()
            .ok_or("engine configuration must be an object")?;
        if overrides.contains_key("cwd")
            || overrides
                .get("type")
                .is_some_and(|value| Some(value) != object.get("type"))
        {
            return Err("engine configuration cannot change type or override cwd".into());
        }
        object.extend(overrides.clone());
    }
    Ok(adapter)
}

/// Report field names only, never configured values.
fn missing(package: &Installed, adapter: &Value) -> Vec<String> {
    package
        .manifest
        .required
        .iter()
        .filter(|key| {
            adapter.get(*key).is_none_or(|v| match v {
                Value::Null => true,
                Value::String(s) => s.trim().is_empty(),
                Value::Array(a) => a.is_empty(),
                Value::Object(o) => o.is_empty(),
                _ => false,
            })
        })
        .cloned()
        .collect()
}

/// Reuse runtime schema validation without executing code; allow incomplete setup during configuration.
fn validate_adapter(package: &Installed, adapter: &Value, complete: bool) -> Result<(), String> {
    let mut merged = package.clone();
    merged.manifest.adapter = adapter.clone();
    if !complete {
        let absent = missing(package, adapter);
        merged
            .manifest
            .required
            .retain(|field| !absent.contains(field));
        // A missing required URL can be configured later; validate every other field now.
        if absent.iter().any(|field| field == "url") {
            merged
                .manifest
                .adapter
                .as_object_mut()
                .ok_or("adapter must be an object")?
                .insert("url".into(), json!("https://example.invalid/"));
        }
    }
    config::resolve_adapter(&merged, None)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Require a distinct trust decision before activating executable host code.
fn consent(id: &str, trusted: bool) -> Result<(), String> {
    eprintln!("Trust boundary: {id} runs package code as your user, with declared environment access; it is not sandboxed.");
    if trusted {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err("command engine activation requires explicit trust; inspect the package, then use search enable NAME --trust".into());
    }
    eprint!("Trust this package and enable it? [y/N] ");
    std::io::stderr().flush().map_err(|e| e.to_string())?;
    let mut reply = String::new();
    std::io::stdin()
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    if matches!(reply.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err("engine activation declined; selection unchanged".into())
    }
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

/// Lock the entire read/change/replace transaction; retain the lock through optional package removal.
fn change_settings(
    path: &Path,
    change: impl FnOnce(&mut Value) -> Result<(), String>,
    after: impl FnOnce() -> Result<(), String>,
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
        after()
    })();
    let _ = fs::remove_file(temp);
    result
}
