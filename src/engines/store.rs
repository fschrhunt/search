//! Private package store with content receipts, lifetime leases and mutation-only recovery.
use super::{
    embedded, manifest, valid_file, valid_id, Assets, Installed, Manifest, DEFAULT_ENGINE,
    MAX_FILE, MAX_FILES, MAX_PACKAGE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const MAX_ENGINES: usize = 256;

/// A plain receipt records identity, installation source and content digest.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    id: String,
    source: String,
    digest: String,
}

/// The lock remains held until this guard is dropped, including during inspection.
struct Store {
    root: PathBuf,
    _lock: File,
}

/// All filesystem errors are content- and credential-free.
fn io_error(error: std::io::Error) -> String {
    format!(
        "engine package filesystem operation failed ({:?}, OS code {:?})",
        error.kind(),
        error.raw_os_error()
    )
}

/// Reject symlinks at every existing ancestor, including paths supplied explicitly.
fn safe_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("engine paths must be absolute and contained".into());
    }
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        // A Windows drive prefix alone is drive-relative; inspect its root on the next step.
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&prefix) {
            Ok(meta) if meta.file_type().is_symlink() || is_reparse(&meta) => {
                return Err("engine paths must not contain symlinks".into())
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}

/// Windows junctions and non-symlink reparse points are unsafe package paths too.
fn is_reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        crate::private_fs::reparse(meta)
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

/// Detect even dangling symlinks as existing objects, so collisions fail closed.
fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(e)),
    }
}

/// Mutation ancestors must be root/current-user owned and protected from foreign writers.
#[cfg(unix)]
fn trusted_ancestors(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no parameters or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    // The filesystem root owner represents root in a UID-mapped execution namespace too.
    let root_uid = File::open("/")
        .map_err(io_error)?
        .metadata()
        .map_err(io_error)?
        .uid();
    let mut prefix = PathBuf::new();
    let mut beneath_tmp = false;
    for part in path.components() {
        prefix.push(part);
        if !exists(&prefix)? {
            break;
        }
        let dir = read_file(&prefix)?;
        let m = dir.metadata().map_err(io_error)?;
        let system_tmp = prefix == Path::new("/tmp")
            || (cfg!(target_os = "macos") && prefix == Path::new("/private/tmp"));
        let sticky_tmp = system_tmp && m.uid() == root_uid && m.mode() & 0o1000 != 0;
        if !m.is_dir()
            || (m.uid() != root_uid && m.uid() != uid)
            || (m.mode() & 0o022 != 0 && !sticky_tmp)
            || (beneath_tmp && (m.uid() != uid || m.mode() & 0o077 != 0))
        {
            return Err(
                "engine mutation requires trusted ancestors; use a private directory owned by you"
                    .into(),
            );
        }
        beneath_tmp |= sticky_tmp;
    }
    Ok(())
}

/// Windows ancestors must resist replacement by untrusted principals.
#[cfg(windows)]
fn trusted_ancestors(path: &Path) -> Result<(), String> {
    crate::private_fs::trusted_ancestors(path)
}

/// Require current-user ownership and owner-only access for store objects.
#[cfg(unix)]
fn private(path: &Path, directory: bool) -> Result<(), String> {
    private_metadata(&fs::symlink_metadata(path).map_err(io_error)?, directory)
}

/// Validate opened handle metadata as well as pathname metadata.
#[cfg(unix)]
fn private_metadata(m: &fs::Metadata, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no parameters or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    if m.uid() != uid
        || m.mode() & 0o077 != 0
        || m.file_type().is_symlink()
        || (directory && !m.is_dir())
        || (!directory && (!m.is_file() || m.nlink() != 1))
    {
        return Err("engine store requires private owned directories and files".into());
    }
    Ok(())
}

/// Windows validates ownership and protected DACLs through a no-follow handle.
#[cfg(windows)]
fn private(path: &Path, directory: bool) -> Result<(), String> {
    crate::private_fs::private(path, directory, crate::private_fs::Boundary::Private)
}

/// Prepare command scratch space using the store's ownership and symlink checks.
pub(super) fn command_temp_dir(path: &Path) -> Result<(), String> {
    safe_path(path)?;
    trusted_ancestors(path)?;
    mkdir(path)
}

/// Create a private directory without changing permissions of existing objects.
fn mkdir(path: &Path) -> Result<(), String> {
    safe_path(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(io_error(e)),
        }
    }
    #[cfg(windows)]
    crate::private_fs::mkdir(path)?;
    private(path, true)
}

/// Open files without following symlinks and with restrictive creation mode.
#[cfg(unix)]
fn options(write: bool) -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
}

/// Walk directory descriptors with no-follow opens, closing ancestor-check races.
#[cfg(unix)]
fn read_file(path: &Path) -> Result<File, String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let mut dir = File::open("/").map_err(io_error)?;
    let mut parts = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part.as_bytes()).map_err(|_| "invalid package path")?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                libc::O_NONBLOCK
            };
        // SAFETY: dir is live; name is NUL-terminated; no pointer escapes openat.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        // SAFETY: successful openat returns a newly owned descriptor.
        dir = unsafe { File::from_raw_fd(fd) };
    }
    Ok(dir)
}

/// Windows pins intermediate directories and rejects all reparse tags on the opened handle.
#[cfg(windows)]
fn read_file(path: &Path) -> Result<File, String> {
    crate::private_fs::read(path)
}

/// Open a no-follow file, validate ownership on its handle and confirm path/handle identity.
fn owned_file(path: &Path) -> Result<File, String> {
    safe_path(path)?;
    private(path, false)?;
    #[cfg(windows)]
    let file = crate::private_fs::open(
        path,
        false,
        false,
        false,
        crate::private_fs::Boundary::Private,
    )?;
    #[cfg(unix)]
    let file = read_file(path)?;
    #[cfg(unix)]
    let opened = file.metadata().map_err(io_error)?;
    #[cfg(unix)]
    private_metadata(&opened, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let named = fs::symlink_metadata(path).map_err(io_error)?;
        if opened.dev() != named.dev() || opened.ino() != named.ino() {
            return Err("engine control file changed during open".into());
        }
    }
    Ok(file)
}

/// Shared leases survive Installed clones; exclusive mutation fails immediately while busy.
fn lease_file(root: &Path, shared: bool) -> Result<File, String> {
    private(root, true)?;
    let file = owned_file(&root.join(".lease"))?;
    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };
    result.map_err(|_| "engine package is busy; stop running Search hosts and release engine handles, then retry".to_string())?;
    Ok(file)
}

/// Bound reads before allocation, and check the opened file is regular.
fn read(path: &Path, cap: u64, owned: bool) -> Result<Vec<u8>, String> {
    safe_path(path)?;
    if owned {
        private(path, false)?;
    }
    let file = if owned {
        owned_file(path)?
    } else {
        read_file(path)?
    };
    let meta = file.metadata().map_err(io_error)?;
    if !meta.is_file() || meta.len() > cap {
        return Err("package file is not regular or exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > cap {
        return Err("package file exceeds size limit".into());
    }
    Ok(bytes)
}

/// Create and sync new owner-only assets and control files.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    safe_path(path)?;
    #[cfg(windows)]
    let mut file = crate::private_fs::open(
        path,
        true,
        true,
        false,
        crate::private_fs::Boundary::Private,
    )?;
    #[cfg(unix)]
    let mut file = options(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    #[cfg(unix)]
    private_metadata(&file.metadata().map_err(io_error)?, false)?;
    #[cfg(windows)]
    crate::private_fs::check(&file, false, crate::private_fs::Boundary::Private)?;
    private(path, false)?;
    sync_dir(path.parent().ok_or("package file parent missing")?)
}

/// Length-prefixed sorted assets prevent filename/content concatenation ambiguity.
fn digest(assets: &Assets) -> String {
    let mut sorted: Vec<_> = assets.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash = Sha256::new();
    for (name, bytes) in sorted {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    hex(&hash.finalize())
}

/// Render digests without a dependency on a hash-array formatting implementation.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Open an existing store without writes, or initialize it for a mutation.
fn open(home: &Path, create: bool, mutate: bool) -> Result<Option<Store>, String> {
    safe_path(home)?;
    if mutate {
        trusted_ancestors(home)?;
    }
    let root = home.join("engines");
    safe_path(&root)?;
    if !create && !root.try_exists().map_err(io_error)? {
        return Ok(None);
    }
    if create {
        // Home's parent must already exist; callers choose the private home explicitly.
        mkdir(home)?;
        mkdir(&root)?;
    } else {
        private(home, true)?;
        private(&root, true)?;
    }
    let lock_path = root.join(".lock");
    if create && !lock_path.try_exists().map_err(io_error)? {
        match write(&lock_path, b"") {
            Ok(()) => (),
            Err(e) if lock_path.try_exists().map_err(io_error)? => {
                private(&lock_path, false).map_err(|_| e)?;
            }
            Err(e) => return Err(e),
        }
    }
    private(&lock_path, false)?;
    let lock = owned_file(&lock_path)?;
    lock.lock().map_err(io_error)?;
    let store = Store { root, _lock: lock };
    if mutate {
        store.recover()?;
    }
    Ok(Some(store))
}

/// Limit directory enumeration, including undeclared/control entries.
fn entries(path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(path).map_err(io_error)? {
        if out.len() >= MAX_ENGINES + 16 {
            return Err("engine directory exceeds entry limit".into());
        }
        out.push(entry.map_err(io_error)?.path());
    }
    Ok(out)
}

/// Read declared assets only; all other files and empty undeclared dirs are rejected.
fn package(path: &Path, owned: bool, controls: bool) -> Result<(Manifest, Assets), String> {
    safe_path(path)?;
    let m = manifest(&read(&path.join("engine.json"), MAX_FILE, owned)?)?;
    let mut declared: BTreeSet<String> = m.files.iter().cloned().collect();
    declared.insert("engine.json".into());
    let mut found = BTreeSet::new();
    let mut pending = vec![(path.to_path_buf(), String::new())];
    let mut count = 0;
    while let Some((dir, prefix)) = pending.pop() {
        if owned {
            private(&dir, true)?;
        }
        for entry in entries(&dir)? {
            count += 1;
            if count > MAX_FILES * 8 {
                return Err("package exceeds entry limit".into());
            }
            let name = entry
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("invalid package filename")?;
            let relative = if prefix.is_empty() {
                name.into()
            } else {
                format!("{prefix}/{name}")
            };
            let meta = fs::symlink_metadata(&entry).map_err(io_error)?;
            if meta.file_type().is_symlink() || is_reparse(&meta) {
                return Err("package contains a symlink".into());
            }
            if controls && prefix.is_empty() && (name == ".receipt.json" || name == ".lease") {
                private(&entry, false)?;
                continue;
            }
            if meta.is_dir()
                && declared
                    .iter()
                    .any(|s| s.starts_with(&format!("{relative}/")))
            {
                pending.push((entry, relative));
            } else if meta.is_file() && declared.contains(&relative) {
                found.insert(relative);
            } else {
                return Err("package contains undeclared paths".into());
            }
        }
    }
    if found != declared {
        return Err("package declared file missing".into());
    }
    let mut assets = Vec::new();
    let mut total = 0usize;
    for name in declared {
        #[cfg(unix)]
        if owned {
            use std::os::unix::fs::MetadataExt;
            let file = owned_file(&path.join(&name))?;
            let mode = file.metadata().map_err(io_error)?.mode() & 0o700;
            let expected = if m.executables.contains(&name) {
                0o700
            } else {
                0o600
            };
            if mode != expected {
                return Err("package execute permissions differ from its manifest".into());
            }
        }
        let bytes = read(&path.join(&name), MAX_FILE, owned)?;
        total = total.saturating_add(bytes.len());
        if total > MAX_PACKAGE {
            return Err("package exceeds size limit".into());
        }
        assets.push((name, bytes));
    }
    Ok((m, assets))
}

impl Store {
    /// Decode bounded receipts, validating identity/source and rejecting malformed digests.
    fn receipt(&self, id: &str) -> Result<Receipt, String> {
        let path = self.root.join(id);
        private(&path, true)?;
        let bytes = read(&path.join(".receipt.json"), 16384, true)?;
        let receipt: Receipt =
            serde_json::from_slice(&bytes).map_err(|_| "invalid engine receipt")?;
        if receipt.id != id
            || !matches!(receipt.source.as_str(), "catalog" | "local")
            || receipt.digest.len() != 64
            || !receipt
                .digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("invalid engine receipt".into());
        }
        Ok(receipt)
    }

    /// Acquire a package lease under the store lock; exclusive mutation never waits for hosts.
    fn lease(&self, id: &str, shared: bool) -> Result<File, String> {
        lease_file(&self.root.join(id), shared)
    }

    /// Resolve package snapshots with digest verification and an optional execution lease.
    fn installed(&self, id: &str, shared: bool) -> Result<Installed, String> {
        let receipt = self.receipt(id)?;
        let path = self.root.join(id);
        let lease = if shared {
            Some(Arc::new(self.lease(id, true)?))
        } else {
            None
        };
        let (manifest, assets) = package(&path, true, true)?;
        let actual = digest(&assets);
        if manifest.id != id {
            return Err("engine package identity changed".into());
        }
        if actual != receipt.digest {
            return Err("engine package files were modified".into());
        }
        Ok(Installed {
            manifest,
            path,
            source: receipt.source,
            digest: actual,
            lease,
        })
    }

    /// Stage a complete private package, with cleanup on any failure.
    fn stage(&self, m: &Manifest, assets: &Assets, source: &str) -> Result<PathBuf, String> {
        let stage = self.root.join(format!(".stage-{}", uuid::Uuid::new_v4()));
        mkdir(&stage)?;
        let result = (|| {
            for (name, bytes) in assets {
                valid_file(name)?;
                let dest = stage.join(name);
                let mut parent = stage.clone();
                if let Some(parts) = Path::new(name).parent() {
                    for part in parts.components() {
                        parent.push(part);
                        mkdir(&parent)?;
                    }
                }
                write(&dest, bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if m.executables.contains(name) {
                        let file = owned_file(&dest)?;
                        file.set_permissions(fs::Permissions::from_mode(0o700))
                            .map_err(io_error)?;
                        file.sync_all().map_err(io_error)?;
                    }
                }
            }
            let receipt = Receipt {
                id: m.id.clone(),
                source: source.into(),
                digest: digest(assets),
            };
            let bytes = serde_json::to_vec(&receipt).map_err(|_| "receipt serialization failed")?;
            write(&stage.join(".receipt.json"), &bytes)?;
            write(&stage.join(".lease"), b"")?;
            let (staged, copied) = package(&stage, true, true)?;
            if staged.id != m.id || digest(&copied) != receipt.digest {
                return Err("staged package validation failed".into());
            }
            sync_dir(&stage)?;
            Ok(stage.clone())
        })();
        if result.is_err() {
            let _ = delete(&stage);
        }
        result
    }

    /// Publish only to an absent ID; the held lock serializes every writer.
    fn install(&self, m: Manifest, assets: Assets, source: &str) -> Result<Installed, String> {
        if exists(&self.root.join(&m.id))? {
            return Err("engine is already installed".into());
        }
        let count = entries(&self.root)?
            .iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| !n.to_string_lossy().starts_with('.'))
            })
            .count();
        if count >= MAX_ENGINES {
            return Err("installed engine limit reached".into());
        }
        let stage = self.stage(&m, &assets, source)?;
        if let Err(e) = fs::rename(&stage, self.root.join(&m.id)) {
            let _ = delete(&stage);
            return Err(io_error(e));
        }
        sync_dir(&self.root)?;
        self.installed(&m.id, false)
    }

    /// Only mutations recover old replacements, unpublished stages and deletion tombstones.
    fn recover(&self) -> Result<(), String> {
        for path in entries(&self.root)? {
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("invalid store filename")?;
            if let Some(id) = name.strip_prefix(".old-") {
                valid_id(id)?;
                let _lease = lease_file(&path, false)?;
                let live = self.root.join(id);
                if exists(&live)? {
                    self.installed(id, false)?;
                    // Windows cannot remove a directory with a delete-pending lease file.
                    // The store lock still excludes readers after the exclusive lease is closed.
                    #[cfg(windows)]
                    drop(_lease);
                    delete(&path)?;
                } else {
                    // Old copies still have complete receipts/assets; validate before restoring.
                    let (m, assets) = package(&path, true, true)?;
                    let receipt: Receipt =
                        serde_json::from_slice(&read(&path.join(".receipt.json"), 16384, true)?)
                            .map_err(|_| "invalid recovery receipt")?;
                    if m.id != id || receipt.id != id || receipt.digest != digest(&assets) {
                        return Err("invalid recovery package".into());
                    }
                    // Windows directory renames require closing descendant handles.
                    // The store lock excludes new leases throughout recovery.
                    #[cfg(windows)]
                    drop(_lease);
                    fs::rename(&path, live).map_err(io_error)?;
                }
                sync_dir(&self.root)?;
            } else if let Some(token) = name.strip_prefix(".delete-") {
                uuid::Uuid::parse_str(token).map_err(|_| "invalid deletion tombstone")?;
                // A previous remove validated and exclusively leased this tree before renaming.
                // A crash can leave its receipt or lease file already deleted.
                let _lease = if exists(&path.join(".lease"))? {
                    Some(lease_file(&path, false)?)
                } else {
                    None
                };
                #[cfg(windows)]
                drop(_lease);
                delete(&path)?;
                sync_dir(&self.root)?;
            } else if let Some(token) = name.strip_prefix(".stage-") {
                uuid::Uuid::parse_str(token).map_err(|_| "invalid package stage")?;
                delete(&path)?;
                sync_dir(&self.root)?;
            }
        }
        Ok(())
    }
}

/// Unix flushes directory entries; Windows syncs file content before its recoverable renames.
/// Win32 directory handles do not support FlushFileBuffers; mutation recovery covers rename gaps.
#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)
}

/// Windows checks the directory boundary; directory flushing is unavailable in Win32.
#[cfg(windows)]
fn sync_dir(path: &Path) -> Result<(), String> {
    private(path, true)
}

/// Remove owned store trees only, rejecting symlinks instead of following them.
fn delete(path: &Path) -> Result<(), String> {
    private(path, true)?;
    let mut pending = vec![path.to_path_buf()];
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut count = 0usize;
    // Validate the entire tree before deleting any file, including its receipt.
    while let Some(dir) = pending.pop() {
        private(&dir, true)?;
        directories.push(dir.clone());
        for entry in entries(&dir)? {
            count += 1;
            if count > MAX_FILES * 8 {
                return Err("managed directory exceeds entry limit".into());
            }
            let meta = fs::symlink_metadata(&entry).map_err(io_error)?;
            if meta.is_dir() {
                private(&entry, true)?;
                pending.push(entry);
            } else {
                private(&entry, false)?;
                files.push(entry);
            }
        }
    }
    for file in files {
        fs::remove_file(file).map_err(io_error)?;
    }
    for dir in directories.into_iter().rev() {
        fs::remove_dir(dir).map_err(io_error)?;
    }
    Ok(())
}

/// List disk-installed packages only; inspection never executes an adapter.
pub fn list(home: &Path) -> Result<Vec<Installed>, String> {
    let Some(store) = open(home, false, false)? else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for path in entries(&store.root)? {
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid engine directory")?;
        if id.starts_with('.') {
            continue;
        }
        valid_id(id)?;
        ids.push(id.to_owned());
    }
    ids.sort();
    ids.iter().map(|id| store.installed(id, false)).collect()
}

/// Resolve installed packages first; only the default has an embedded fallback.
pub fn resolve(home: &Path, id: &str) -> Result<Installed, String> {
    valid_id(id)?;
    safe_path(home)?;
    let path = home.join("engines").join(id);
    safe_path(&path)?;
    // An absent disk default is independent of unrelated receipts, locks and recovery debris.
    if id == DEFAULT_ENGINE && !exists(&path)? {
        let (manifest, assets) = embedded(id)?;
        return Ok(Installed {
            manifest,
            path,
            source: "embedded".into(),
            digest: digest(&assets),
            lease: None,
        });
    }
    let store = open(home, false, false)?.ok_or("engine is not installed")?;
    if !exists(&path)? {
        return Err("engine is not installed".into());
    }
    store.installed(id, true)
}

/// Install the shipped catalog version offline, refusing existing packages.
pub fn install_catalog(home: &Path, id: &str) -> Result<Installed, String> {
    let (m, assets) = embedded(id)?;
    let store = open(home, true, true)?.ok_or("engine store unavailable")?;
    store.install(m, assets, "catalog")
}

/// Copy only declared files from an explicit local package; never execute hooks.
pub fn install_local(home: &Path, path: &Path) -> Result<Installed, String> {
    let path = explicit(path)?;
    let (m, assets) = package(&path, false, false)?;
    if m.id == DEFAULT_ENGINE {
        return Err("the default engine ID is reserved for its maintained package; choose another ID for a custom engine".into());
    }
    let store = open(home, true, true)?.ok_or("engine store unavailable")?;
    store.install(m, assets, "local")
}

/// Normalize explicit relative paths, with no automatic cwd discovery.
fn explicit(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    // An explicit './package' is allowed; parent traversal and symlinks are not.
    let path: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    safe_path(&path)?;
    let canonical = fs::canonicalize(&path).map_err(io_error)?;
    safe_path(&canonical)?;
    Ok(canonical)
}

/// Atomically exchange two private directories on Linux, preserving the live path.
#[cfg(target_os = "linux")]
fn replace(stage: &Path, live: &Path, old: &Path) -> Result<(), String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    fs::rename(stage, old).map_err(io_error)?;
    sync_dir(old.parent().ok_or("package parent missing")?)?;
    let live_name =
        CString::new(live.as_os_str().as_bytes()).map_err(|_| "invalid package path")?;
    let old_name = CString::new(old.as_os_str().as_bytes()).map_err(|_| "invalid package path")?;
    // SAFETY: both paths are NUL-terminated, live for the call, and private/locked.
    let status = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            live_name.as_ptr(),
            libc::AT_FDCWD,
            old_name.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if status != 0 {
        let error = io_error(std::io::Error::last_os_error());
        let _ = delete(old);
        return Err(error);
    }
    Ok(())
}

/// Portable replacement excludes API readers through the held lock; recovery rolls back gaps.
#[cfg(not(target_os = "linux"))]
fn replace(stage: &Path, live: &Path, old: &Path) -> Result<(), String> {
    fs::rename(live, old).map_err(io_error)?;
    sync_dir(old.parent().ok_or("package parent missing")?)?;
    if let Err(e) = fs::rename(stage, live) {
        let _ = fs::rename(old, live);
        return Err(io_error(e));
    }
    Ok(())
}

/// Replace unedited catalog installs only after acquiring an exclusive lifetime lease.
pub fn update(home: &Path, id: &str) -> Result<Installed, String> {
    valid_id(id)?;
    let store = open(home, false, true)?.ok_or("engine is not installed")?;
    let _lease = store.lease(id, false)?;
    let installed = store.installed(id, false)?;
    if installed.source != "catalog" {
        return Err("only catalog installations can be updated".into());
    }
    let (m, assets) = embedded(id)?;
    let stage = store.stage(&m, &assets, "catalog")?;
    let live = store.root.join(id);
    let old = store.root.join(format!(".old-{id}"));
    // An exclusive lease proved there are no hosts. Windows needs its descendant
    // handle closed before directory rename; the store lock excludes new leases.
    #[cfg(windows)]
    drop(_lease);
    if let Err(error) = replace(&stage, &live, &old) {
        if stage.try_exists().map_err(io_error)? {
            let _ = delete(&stage);
        }
        return Err(error);
    }
    sync_dir(&store.root)?;
    delete(&old)?;
    sync_dir(&store.root)?;
    store.installed(id, false)
}

/// Validate and exclusively lease the live package, then rename to a recoverable deletion tombstone.
pub fn remove(home: &Path, id: &str) -> Result<(), String> {
    valid_id(id)?;
    let store = open(home, false, true)?
        .ok_or("engine is not installed; disable the embedded default instead")?;
    let _lease = store.lease(id, false)?;
    store.receipt(id)?;
    // Explicit removal may discard edited content, but never undeclared or unsafe paths.
    let (manifest, _) = package(&store.root.join(id), true, true)?;
    if manifest.id != id {
        return Err("engine package identity changed".into());
    }
    let tombstone = store.root.join(format!(".delete-{}", uuid::Uuid::new_v4()));
    // Close the exclusive descendant handle before Windows directory rename.
    // The store lock continues to exclude readers until deletion finishes.
    #[cfg(windows)]
    drop(_lease);
    fs::rename(store.root.join(id), &tombstone).map_err(io_error)?;
    sync_dir(&store.root)?;
    delete(&tombstone)?;
    sync_dir(&store.root)
}
