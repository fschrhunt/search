//! Local remote management: trust is explicit, and only selection controls routing.
use crate::trust::{self, Devices, Profiles, RemoteCredential};
use search_mcp::backend::{self, Backend, Remote};
use std::{collections::BTreeMap, path::PathBuf};

/// Resolve private storage from local settings without opening a local engine.
pub fn storage(config: Option<String>) -> Result<PathBuf, String> {
    let config = search::config::load(config.map(PathBuf::from)).map_err(|e| e.to_string())?;
    trust::dir(&config.dir)
}

/// Choose the sole execution target; corrupt or missing selected credentials fail closed.
pub fn selected(config: Option<String>) -> Result<Option<Backend>, String> {
    let settings = search::config::load(config.map(PathBuf::from)).map_err(|e| e.to_string())?;
    // A clean local install needs neither credentials nor private storage.
    if !settings
        .dir
        .join("trust")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return Ok(None);
    }
    let path = trust::dir(&settings.dir)?;
    let profiles: Profiles = trust::read(&path.join("remotes.json"))?;
    profiles
        .selected
        .map(|name| {
            let remote = profiles
                .remotes
                .get(&name)
                .ok_or("selected remote has no credentials")?;
            Backend::remote_with_timeout(remote.clone(), settings.remote.timeout())
        })
        .transpose()
}

/// Manage profiles and host devices; print only newly issued one-use pairing codes.
pub async fn command(
    action: &str,
    args: Vec<String>,
    config: Option<String>,
) -> Result<(), String> {
    let path = storage(config)?;
    let profiles_path = path.join("remotes.json");
    match action {
        "pair-code" => {
            if !args.is_empty() {
                return Err("usage: search pair-code [-config PATH]".into());
            }
            let code = trust::pair_code(&path, std::time::Duration::from_secs(900))?;
            eprintln!("search: one-use pairing code (expires in 15 minutes; 20 attempts):");
            println!("{code}");
        }
        "pair" => {
            if args.len() != 4 {
                return Err("usage: search remote pair NAME HTTPS_URL CERT_FILE SHA256 (code read from stdin)".into());
            }
            let name = &args[0];
            validate_name(name)?;
            let cert = std::fs::read_to_string(&args[2])
                .map_err(|_| "cannot read host public certificate")?;
            let mut remote = Remote {
                url: args[1].clone(),
                cert,
                fingerprint: args[3].clone(),
                secret: String::new(),
                device: String::new(),
            };
            // Pin and validate the origin before reading or transmitting the pairing code.
            let client = backend::client(&remote)?;
            eprintln!(
                "search: verified certificate SHA-256 {}",
                remote.fingerprint
            );
            eprintln!("search: enter the host's one-use pairing code:");
            let mut code = String::new();
            std::io::stdin()
                .read_line(&mut code)
                .map_err(|_| "cannot read pairing code")?;
            let response = client
                .post(format!("{}/pair", remote.url.trim_end_matches('/')))
                .json(&serde_json::json!({"code": code.trim(), "name": name}))
                .send()
                .await
                .map_err(|_| "pairing connection failed")?;
            if !response.status().is_success() {
                return Err(format!("pairing failed: {}", response.status()));
            }
            let credential: RemoteCredential = backend::read_response(response, 4096).await?;
            if credential.secret.len() != 64 || uuid::Uuid::parse_str(&credential.device).is_err() {
                return Err("invalid device credential".into());
            }
            remote.secret = credential.secret;
            remote.device = credential.device;
            trust::update(&profiles_path, |profiles: &mut Profiles| {
                profiles.remotes.insert(name.clone(), remote);
                Ok(())
            })?;
            println!("Paired {name}; select with search remote use {name}");
        }
        "use" | "remove" => {
            if args.len() != 1 {
                return Err(format!("usage: search remote {action} NAME"));
            }
            trust::update(&profiles_path, |profiles: &mut Profiles| {
                if !profiles.remotes.contains_key(&args[0]) {
                    return Err("unknown remote".into());
                }
                if action == "use" {
                    profiles.selected = Some(args[0].clone());
                } else {
                    profiles.remotes.remove(&args[0]);
                    if profiles.selected.as_ref() == Some(&args[0]) {
                        profiles.selected = None;
                    }
                }
                Ok(())
            })?;
        }
        "off" => {
            if !args.is_empty() {
                return Err("usage: search remote off".into());
            }
            trust::update(&profiles_path, |profiles: &mut Profiles| {
                profiles.selected = None;
                Ok(())
            })?;
        }
        "list" => {
            if !args.is_empty() {
                return Err("usage: search remote list".into());
            }
            let profiles: Profiles = trust::read(&profiles_path)?;
            for (name, remote) in profiles.remotes {
                println!(
                    "{} {name} {} {}",
                    if profiles.selected.as_ref() == Some(&name) {
                        "*"
                    } else {
                        " "
                    },
                    remote.url,
                    remote.device
                );
            }
        }
        "devices" => {
            if !args.is_empty() {
                return Err("usage: search devices".into());
            }
            let devices: Devices = trust::read(&path.join("devices.json"))?;
            for (id, device) in devices {
                println!("{id} {} {}", device.name, device.added_at);
            }
        }
        "revoke" => {
            if args.len() != 1 {
                return Err("usage: search revoke DEVICE_ID".into());
            }
            trust::update(
                &path.join("devices.json"),
                |devices: &mut BTreeMap<String, trust::Device>| {
                    if devices.remove(&args[0]).is_none() {
                        return Err("unknown device".into());
                    }
                    Ok(())
                },
            )?;
        }
        _ => return Err("usage: search remote pair/use/list/off/remove".into()),
    }
    Ok(())
}

/// Keep profile names printable and unambiguous in command output.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("name must contain 1–80 letters, digits, hyphens or underscores".into());
    }
    Ok(())
}
