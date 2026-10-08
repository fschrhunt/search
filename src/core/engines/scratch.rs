//! Private scratch directories for command engines.
//!
//! `prepare` creates the configured `temp_dir` (exported to the child as
//! `TMPDIR`, `TMP` and `TEMP`) with owner-only permissions, or accepts an
//! existing one only if it is a real directory owned by the current user and
//! closed to others. Ancestors must be owned by root or the current user and not
//! writable by others, except a root-owned sticky system `/tmp`. Symlinks
//! (and Windows reparse points) anywhere on the path are refused.

use std::path::{Component, Path};

/// Create or validate a private scratch directory; errors never quote the path.
pub(super) fn prepare(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("temporary directory must be absolute and contained".into());
    }
    platform(path)
}

/// Unix: no-follow ancestor walk, then a 0700 directory owned by the caller.
#[cfg(unix)]
fn platform(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    trusted_ancestors(path)?;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(io_error(e)),
    }
    let meta = std::fs::symlink_metadata(path).map_err(io_error)?;
    // SAFETY: geteuid has no parameters or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err("temporary directory must be a private directory owned by you".into());
    }
    Ok(())
}

/// Windows: pinned no-reparse ancestors and a protected current-user DACL.
#[cfg(windows)]
fn platform(path: &Path) -> Result<(), String> {
    crate::private_fs::trusted_ancestors(path)?;
    crate::private_fs::mkdir(path)
}

/// Fail closed where ownership and no-follow primitives are unavailable.
#[cfg(not(any(unix, windows)))]
fn platform(_path: &Path) -> Result<(), String> {
    Err("private temporary directories are unsupported on this platform".into())
}

/// Every existing ancestor must be a root- or caller-owned directory that
/// others cannot write, opened without following links. Beneath sticky `/tmp`
/// (or macOS `/private/tmp`), components must be caller-owned and private.
#[cfg(unix)]
fn trusted_ancestors(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no parameters or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    // The filesystem root owner represents root in a UID-mapped namespace too.
    let root_uid = std::fs::metadata("/").map_err(io_error)?.uid();
    let mut prefix = std::path::PathBuf::new();
    let mut beneath_tmp = false;
    for part in path.components() {
        prefix.push(part);
        match std::fs::symlink_metadata(&prefix) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(io_error(e)),
            Ok(_) => (),
        }
        let m = open_no_follow(&prefix)?.metadata().map_err(io_error)?;
        let system_tmp = prefix == Path::new("/tmp")
            || (cfg!(target_os = "macos") && prefix == Path::new("/private/tmp"));
        let sticky_tmp = system_tmp && m.uid() == root_uid && m.mode() & 0o1000 != 0;
        if !m.is_dir()
            || (m.uid() != root_uid && m.uid() != uid)
            || (m.mode() & 0o022 != 0 && !sticky_tmp)
            || (beneath_tmp && (m.uid() != uid || m.mode() & 0o077 != 0))
        {
            return Err(
                "temporary directory requires trusted ancestors; use a private directory owned by you"
                    .into(),
            );
        }
        beneath_tmp |= sticky_tmp;
    }
    Ok(())
}

/// Walk directory descriptors with a no-follow `openat` per component, so a
/// symlink anywhere on the path fails instead of being traversed.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<std::fs::File, String> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    let mut dir = std::fs::File::open("/").map_err(io_error)?;
    for part in path.components() {
        let Component::Normal(name) = part else {
            continue;
        };
        let name = CString::new(name.as_bytes()).map_err(|_| "invalid temporary directory")?;
        // SAFETY: dir is live; name is NUL-terminated; no pointer escapes openat.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY,
            )
        };
        if fd < 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        // SAFETY: successful openat returns a newly owned descriptor.
        dir = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(dir)
}

/// Filesystem errors carry only their kind and OS code, never paths.
#[cfg(unix)]
fn io_error(error: std::io::Error) -> String {
    format!(
        "temporary directory operation failed ({:?}, OS code {:?})",
        error.kind(),
        error.raw_os_error()
    )
}
