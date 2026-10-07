//! Local remote management: trust is explicit, and only selection controls routing.
use crate::cli::auth::{self, Credential, Devices, Remotes};
use crate::client::{self as client, Client, Remote};
use std::{collections::BTreeMap, path::PathBuf};

/// Resolve private storage from the Search home without opening a local engine.
pub fn storage(config: Option<String>) -> Result<PathBuf, String> {
    let config = crate::core::config::load(config.map(PathBuf::from)).map_err(|e| e.to_string())?;
    auth::dir(&config.home)
}

/// Choose the sole execution target; corrupt or missing selected credentials fail closed.
pub fn selected(config: Option<String>) -> Result<Option<Client>, String> {
    let settings =
        crate::core::config::load(config.map(PathBuf::from)).map_err(|e| e.to_string())?;
    // A clean local install needs neither credentials nor private storage.
    if !settings
        .home
        .join("trust")
        .try_exists()
        .map_err(|e| e.to_string())?
    {
        return Ok(None);
    }
    let path = auth::dir(&settings.home)?;
    let remotes: Remotes = auth::read(&path.join("remotes.json"))?;
    remotes
        .selected
        .map(|name| {
            let remote = remotes
                .remotes
                .get(&name)
                .ok_or("selected remote has no credentials")?;
            Client::remote_with_settings(remote.clone(), &settings.remote)
        })
        .transpose()
}

/// Manage remotes and host devices; print only newly issued one-use pairing codes.
pub async fn command(
    action: &str,
    args: Vec<String>,
    config: Option<String>,
) -> Result<(), String> {
    let path = storage(config)?;
    let remotes_path = path.join("remotes.json");
    match action {
        "pair-code" => {
            if !args.is_empty() {
                return Err("usage: search pair-code [-config PATH]".into());
            }
            let code = auth::pair_code(&path, std::time::Duration::from_secs(900))?;
            eprintln!("search: one-use pairing code (expires in 15 minutes; 20 attempts):");
            println!("{code}");
        }
        "pair" => {
            let [name, url, cert_path, fingerprint] = args.as_slice() else {
                return Err("usage: search remote pair NAME HTTPS_URL CERT_FILE SHA256 (code read from stdin)".into());
            };
            validate_name(name)?;
            let cert = std::fs::read_to_string(cert_path)
                .map_err(|_| "cannot read host public certificate")?;
            let mut remote = Remote {
                url: url.clone(),
                cert,
                fingerprint: fingerprint.clone(),
                secret: String::new(),
                device: String::new(),
            };
            // Pin and validate the origin before reading or transmitting the pairing code.
            let client = client::client(&remote)?;
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
            let credential: Credential = client::read_response(response, 4096).await?;
            if credential.secret.len() != 64 || uuid::Uuid::parse_str(&credential.device).is_err() {
                return Err("invalid device credential".into());
            }
            remote.secret = credential.secret;
            remote.device = credential.device;
            auth::update(&remotes_path, |remotes: &mut Remotes| {
                remotes.remotes.insert(name.clone(), remote);
                Ok(())
            })?;
            println!("Paired {name}; select with search remote use {name}");
        }
        "use" | "remove" => {
            let [name] = args.as_slice() else {
                return Err(format!("usage: search remote {action} NAME"));
            };
            auth::update(&remotes_path, |remotes: &mut Remotes| {
                if !remotes.remotes.contains_key(name) {
                    return Err("unknown remote".into());
                }
                if action == "use" {
                    remotes.selected = Some(name.clone());
                } else {
                    remotes.remotes.remove(name);
                    if remotes.selected.as_ref() == Some(name) {
                        remotes.selected = None;
                    }
                }
                Ok(())
            })?;
        }
        "off" => {
            if !args.is_empty() {
                return Err("usage: search remote off".into());
            }
            auth::update(&remotes_path, |remotes: &mut Remotes| {
                remotes.selected = None;
                Ok(())
            })?;
        }
        "list" => {
            if !args.is_empty() {
                return Err("usage: search remote list".into());
            }
            let remotes: Remotes = auth::read(&remotes_path)?;
            for (name, remote) in remotes.remotes {
                println!(
                    "{} {name} {} {}",
                    if remotes.selected.as_ref() == Some(&name) {
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
            let devices: Devices = auth::read(&path.join("devices.json"))?;
            for (id, device) in devices {
                println!("{id} {} {}", device.name, device.added_at);
            }
        }
        "revoke" => {
            let [id] = args.as_slice() else {
                return Err("usage: search revoke DEVICE_ID".into());
            };
            auth::update(
                &path.join("devices.json"),
                |devices: &mut BTreeMap<String, auth::Device>| {
                    if devices.remove(id).is_none() {
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

/// Keep remote names printable and unambiguous in command output.
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
