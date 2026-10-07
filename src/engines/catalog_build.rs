//! Bounded build-time asset reader, shared with offline regression tests.
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};

/// Read a bounded regular file, rejecting package and intermediate symlinks.
pub(crate) fn asset(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut prefix = std::path::PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        // Inspect the drive root rather than resolving a bare Windows drive against cwd.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let meta = fs::symlink_metadata(&prefix)?;
        if meta.file_type().is_symlink()
            || reparse(&meta)
            || (!matches!(component, Component::Normal(_)) && !meta.is_dir())
        {
            return Err("catalog paths must not contain symlinks".into());
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || reparse(&meta) || meta.len() > super::manifest::MAX_FILE {
        return Err("catalog file exceeds size limit or is not regular".into());
    }
    let mut bytes = Vec::new();
    file.take(super::manifest::MAX_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > super::manifest::MAX_FILE {
        return Err("catalog file exceeds size limit".into());
    }
    Ok(bytes)
}

/// Include junctions and every other Windows reparse tag in build-time link rejection.
fn reparse(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Windows build-time reads enforce the same regular-file byte bound.
    #[cfg(windows)]
    #[test]
    fn catalog_reader_bounds_regular_files() {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "search-catalog-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("asset");
        fs::write(&path, b"asset").unwrap();
        assert_eq!(asset(&path).unwrap(), b"asset");
        fs::File::create(&path)
            .unwrap()
            .set_len(super::super::manifest::MAX_FILE + 1)
            .unwrap();
        assert!(asset(&path).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// Check bounded reads and all three symlink placements without invoking Cargo or networking.
    #[cfg(unix)]
    #[test]
    fn catalog_reader_rejects_symlinks_and_oversized_files() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "search-catalog-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let package = root.join("package");
        fs::create_dir(&package).unwrap();
        fs::create_dir(package.join("nested")).unwrap();
        let file = package.join("nested/engine.py");
        fs::write(&file, "sentinel").unwrap();
        assert_eq!(asset(&file).unwrap(), b"sentinel");
        symlink(&package, root.join("linked-package")).unwrap();
        assert!(asset(&root.join("linked-package/nested/engine.py")).is_err());
        symlink(package.join("nested"), package.join("linked-dir")).unwrap();
        assert!(asset(&package.join("linked-dir/engine.py")).is_err());
        symlink(&file, package.join("linked-file")).unwrap();
        assert!(asset(&package.join("linked-file")).is_err());
        fs::File::create(&file)
            .unwrap()
            .set_len(super::super::manifest::MAX_FILE + 1)
            .unwrap();
        assert!(asset(&file).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
