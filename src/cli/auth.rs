//! Owner-only trust storage and one-use pairing; server records contain hashes only.
use crate::client::{fingerprint, Remote};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;

/// Credentials and pairing state live privately under the Search home.
#[cfg(unix)]
pub fn dir(root: &Path) -> Result<PathBuf, String> {
    let mut root_builder = fs::DirBuilder::new();
    root_builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        root_builder.mode(0o700);
    }
    root_builder
        .create(root)
        .map_err(|_| "cannot create Search home".to_string())?;
    let root_meta = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if !root_meta.is_dir() {
        return Err("trust root must be a real directory".into());
    }
    #[cfg(unix)]
    check_owner(&root_meta)?;
    let path = root.join("trust");
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|e| e.to_string())?;
    }
    check(&path, true)?;
    Ok(path)
}

/// Windows creates trust storage with a protected owner DACL at every new directory.
#[cfg(windows)]
pub fn dir(root: &Path) -> Result<PathBuf, String> {
    crate::private_fs::directories(root)?;
    let path = root.join("trust");
    crate::private_fs::directories(&path)?;
    Ok(path)
}

/// Refuse symlinks, foreign owners, unexpected types, and public credentials.
#[cfg(unix)]
fn check(path: &Path, directory: bool) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err("trust storage must not be a symlink".into());
    }
    #[cfg(unix)]
    {
        check_owner(&meta)?;
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("trust storage must be owner-only".into());
        }
    }
    Ok(())
}

/// Windows checks trust ACLs and reparse attributes on the opened object.
#[cfg(windows)]
fn check(path: &Path, directory: bool) -> Result<(), String> {
    crate::private_fs::private(path, directory, crate::private_fs::Boundary::Private)
}

/// Privileged processes must never adopt another user's identity or device registry.
#[cfg(unix)]
fn check_owner(meta: &fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid takes no arguments and does not dereference memory.
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        return Err("trust storage must belong to the current user".into());
    }
    Ok(())
}

/// Open private files no-follow and verify the opened Unix owner and permission boundary.
#[cfg(unix)]
fn private_open(path: &Path, create_new: bool, create: bool) -> Result<File, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .write(create_new || create)
        .create_new(create_new)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    check_owner(&meta)?;
    if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
        return Err("trust storage must be an owner-only regular file without hard links".into());
    }
    check(path, false)?;
    Ok(file)
}

/// Windows checks the current-user owner, protected DACL and link count on the no-follow handle.
#[cfg(windows)]
fn private_open(path: &Path, create_new: bool, create: bool) -> Result<File, String> {
    crate::private_fs::open(
        path,
        create_new || create,
        create_new,
        create,
        crate::private_fs::Boundary::Private,
    )
}

/// Read a protected JSON file, treating only absence as an empty registry.
pub fn read<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    #[cfg(windows)]
    crate::private_fs::trusted_ancestors(path.parent().ok_or("trust file has no parent")?)?;
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.to_string()),
        Ok(_) => {
            let file = private_open(path, false, false)?;
            serde_json::from_reader(file).map_err(|_| "invalid private trust file".to_string())
        }
    }
}

/// Serialize writers across CLI processes and replace private JSON atomically.
pub fn update<T: serde::de::DeserializeOwned + Serialize + Default, R>(
    path: &Path,
    change: impl FnOnce(&mut T) -> Result<R, String>,
) -> Result<R, String> {
    let lock_path = path.with_extension("lock");
    let lock = private_open(&lock_path, false, true)?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    let mut value: T = read(path)?;
    let result = change(&mut value)?;
    write(
        path,
        &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    )?;
    Ok(result)
}

/// Replace a private file with a unique, synced owner-only temporary file.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = private_open(&temp, true, false)?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        #[cfg(windows)]
        crate::private_fs::replace(&temp, path)?;
        #[cfg(unix)]
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}

/// Saved paired hosts and the sole selected execution target.
#[derive(Default, Serialize, Deserialize)]
pub struct Remotes {
    pub selected: Option<String>,
    pub remotes: BTreeMap<String, Remote>,
}

/// A safe device listing; the hash is never printed.
#[derive(Clone, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub added_at: u64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub hash: String,
}
pub type Devices = BTreeMap<String, Device>;

/// Persistent host identity; pairing admissions always read the shared private file.
pub struct Host {
    pub path: PathBuf,
    pub cert: String,
    pub key: String,
    pub fingerprint: String,
    /// Initial one-use code for startup display; renewed codes are never cached here.
    pub code: String,
}
/// Shared admission state contains only a code hash, bounded expiry and attempt count.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairing {
    hash: String,
    created_at: u64,
    expires: u64,
    attempts: usize,
    used: bool,
}

/// Read the wall clock for cross-process expiry; clock rollback fails closed.
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "cannot read pairing clock".into())
}

/// Replace the current code under the admission lock; return its plaintext only once.
pub fn pair_code(path: &Path, window: Duration) -> Result<String, String> {
    check(path, true)?;
    let code = secret();
    update(&path.join("pairing.json"), |pairing: &mut Pairing| {
        let created_at = now()?;
        *pairing = Pairing {
            hash: hash(&code),
            created_at,
            expires: created_at.saturating_add(window.as_secs().min(900)),
            attempts: 0,
            used: false,
        };
        Ok(())
    })?;
    Ok(code)
}

/// Generate cryptographically random credentials using the OS-backed UUID source.
fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
/// Hash a high-entropy code or device secret for private server storage.
fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Host {
    /// Load a persistent TLS identity and open a one-use pairing window, capped at 15 minutes.
    pub fn open(root: &Path, hostname: &str, pairing_window: Duration) -> Result<Self, String> {
        let path = dir(root)?;
        let identity = path.join("identity.json");
        #[derive(Default, Serialize, Deserialize)]
        struct Identity {
            cert: String,
            key: String,
        }
        let identity: Identity = update(&identity, |identity: &mut Identity| {
            if identity.cert.is_empty() && identity.key.is_empty() {
                let generated = rcgen::generate_simple_self_signed(vec![
                    hostname.into(),
                    "localhost".into(),
                    "127.0.0.1".into(),
                    "::1".into(),
                ])
                .map_err(|e| e.to_string())?;
                identity.cert = generated.cert.pem();
                identity.key = generated.signing_key.serialize_pem();
            }
            if identity.cert.is_empty() || identity.key.is_empty() {
                return Err("incomplete TLS identity".into());
            }
            Ok(Identity {
                cert: identity.cert.clone(),
                key: identity.key.clone(),
            })
        })?;
        // The export contains only the certificate, never the private key.
        write(&path.join("host.pem"), identity.cert.as_bytes())?;
        let fingerprint = fingerprint(&identity.cert)?;
        let code = pair_code(&path, pairing_window)?;
        Ok(Self {
            path,
            cert: identity.cert,
            key: identity.key,
            fingerprint,
            code,
        })
    }
    /// Persist every attempt and consume before issuing a device, preventing replay after a crash.
    pub fn pair(&self, code: &str, name: &str) -> Result<Credential, String> {
        let admission = update(&self.path.join("pairing.json"), |pairing: &mut Pairing| {
            let now = now()?;
            let outcome = if pairing.hash.is_empty()
                || pairing.used
                || now < pairing.created_at
                || now >= pairing.expires
                || pairing.attempts >= 20
            {
                Err("pairing unavailable".to_string())
            } else {
                pairing.attempts += 1;
                if !bool::from(pairing.hash.as_bytes().ct_eq(hash(code).as_bytes())) {
                    Err("pairing failed".to_string())
                } else if name.trim().is_empty()
                    || name.len() > 80
                    || name.chars().any(char::is_control)
                {
                    Err("invalid device name".to_string())
                } else {
                    pairing.used = true;
                    Ok(())
                }
            };
            // A rejected attempt still commits its updated count under the process lock.
            Ok(outcome)
        })?;
        admission?;
        let device = uuid::Uuid::new_v4().to_string();
        let secret = secret();
        update(&self.path.join("devices.json"), |devices: &mut Devices| {
            if devices.len() >= 64 {
                return Err("device limit reached".into());
            }
            devices.insert(
                device.clone(),
                Device {
                    name: name.into(),
                    hash: hash(&secret),
                    added_at: now()?,
                },
            );
            Ok(())
        })?;
        Ok(Credential { device, secret })
    }
    /// Reload hashes on every admission, so local revocation takes effect immediately.
    pub fn authorized(&self, secret: &str) -> bool {
        if secret.len() != 64 {
            return false;
        }
        let Ok(devices) = read::<Devices>(&self.path.join("devices.json")) else {
            return false;
        };
        let presented = hash(secret);
        devices
            .values()
            .any(|d| bool::from(d.hash.as_bytes().ct_eq(presented.as_bytes())))
    }
}
/// The issued device secret is returned once over trusted HTTPS.
#[derive(Serialize, Deserialize)]
pub struct Credential {
    pub device: String,
    pub secret: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expired, exhausted and consumed codes must never enroll another device.
    #[test]
    fn pairing_bounds_and_hash_storage() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-trust-{}", uuid::Uuid::new_v4()));
        let host = Host::open(&root, "localhost", Duration::from_secs(900)).unwrap();
        let credential = host.pair(&host.code, "laptop").unwrap();
        assert!(host.authorized(&credential.secret));
        assert!(host.pair(&host.code, "reuse").is_err());
        let contents = fs::read_to_string(host.path.join("devices.json")).unwrap();
        assert!(!contents.contains(&credential.secret));
        let fingerprint = host.fingerprint.clone();
        let host = Host::open(&root, "localhost", Duration::from_secs(900)).unwrap();
        assert_eq!(host.fingerprint, fingerprint);
        assert!(host.authorized(&credential.secret));
        let expired = pair_code(&host.path, Duration::ZERO).unwrap();
        assert!(host.pair(&expired, "expired").is_err());
        let host = Host::open(&root, "localhost", Duration::from_secs(900)).unwrap();
        for _ in 0..20 {
            assert!(host.pair("wrong", "attacker").is_err());
        }
        let pairing: Pairing = read(&host.path.join("pairing.json")).unwrap();
        assert_eq!(pairing.attempts, 20);
        assert!(host.pair(&host.code, "locked").is_err());
        let current = pair_code(&host.path, Duration::from_secs(900)).unwrap();
        let stored = fs::read_to_string(host.path.join("pairing.json")).unwrap();
        assert!(!stored.contains(&current));
        assert!(host.pair(&host.code, "superseded").is_err());
        assert!(host.pair(&current, "renewed").is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    /// Unsafe and malformed persisted credentials are errors rather than empty state.
    #[cfg(unix)]
    #[test]
    fn private_storage_fails_closed() {
        use std::os::unix::{fs::symlink, fs::PermissionsExt};
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("search-private-{}", uuid::Uuid::new_v4()));
        let private = dir(&root).unwrap();
        let file = private.join("remotes.json");
        write(&file, b"{}").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read::<Remotes>(&file).is_err());
        fs::remove_file(&file).unwrap();
        symlink(private.join("missing"), &file).unwrap();
        assert!(read::<Remotes>(&file).is_err());
        fs::remove_file(&file).unwrap();
        write(&file, b"broken").unwrap();
        assert!(read::<Remotes>(&file).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
