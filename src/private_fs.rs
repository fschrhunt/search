//! Windows private storage: protected user DACLs, handle validation and pinned no-reparse walks.
//! Administrative principals are trusted only for ancestors, never for private file access.
use std::{
    ffi::c_void,
    fs::{File, Metadata},
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
};

/// Storage boundaries distinguish private data, shareable settings and trusted ancestors.
#[derive(Clone, Copy)]
pub(crate) enum Boundary {
    Private,
    #[cfg(feature = "cli")]
    Settings,
    Ancestor,
}

/// Own LocalAlloc memory returned by the Windows security APIs.
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: these pointers are allocated by APIs documenting LocalFree ownership.
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// Convert Win32 failure into an OS error without including paths or content.
fn last_error() -> String {
    io::Error::last_os_error().to_string()
}

/// Encode a path without allowing device namespaces, ADS, or Win32 filename aliases.
fn wide(path: &Path) -> Result<Vec<u16>, String> {
    if !path.is_absolute() {
        return Err("storage path must be absolute".into());
    }
    for part in path.components() {
        match part {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                    let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
                    // SAFETY: root is a live NUL-terminated drive-root string.
                    if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE {
                        return Err("storage paths must not use mapped network drives".into());
                    }
                }
                _ => return Err("storage path must be a contained local disk path".into()),
            },
            Component::RootDir => (),
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                let stem = name
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_uppercase();
                if name.ends_with(['.', ' '])
                    || name.contains([':', '<', '>', '"', '|', '?', '*'])
                    || name.chars().any(|c| c < ' ')
                    || matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || ["COM", "LPT"].iter().any(|p| {
                        stem.strip_prefix(p).is_some_and(|n| {
                            matches!(
                                n,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        })
                    })
                {
                    return Err("storage path contains a Windows filename alias".into());
                }
            }
            _ => return Err("storage path must be a contained local disk path".into()),
        }
    }
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err("storage path contains NUL".into());
    }
    value.push(0);
    Ok(value)
}

/// Include junctions and other reparse tags, not just symbolic links.
pub(crate) fn reparse(meta: &Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Read the effective token's user SID; the aligned buffer owns its embedded SID.
fn user() -> Result<Vec<usize>, String> {
    let mut token = null_mut();
    // SAFETY: valid pseudo-handles and a live HANDLE output slot.
    unsafe {
        if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0
            && (GetLastError() != ERROR_NO_TOKEN
                || OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0)
        {
            return Err(last_error());
        }
    }
    let result = (|| {
        let mut bytes = 0;
        // SAFETY: a null buffer queries required size; token remains open.
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes);
        }
        if bytes < size_of::<TOKEN_USER>() as u32 {
            return Err(last_error());
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: aligned buffer has at least bytes capacity and lives through the call.
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(buffer)
    })();
    // SAFETY: token is a successfully opened, uniquely owned token handle.
    unsafe {
        CloseHandle(token);
    }
    result
}

/// Return the SID pointer backed by a successfully queried token buffer.
fn user_sid(buffer: &[usize]) -> PSID {
    // SAFETY: callers only pass the aligned TokenUser buffer returned by user().
    unsafe { (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid }
}

/// Render a valid SID for an explicit security descriptor or principal comparison.
fn sid_string(sid: PSID) -> Result<String, String> {
    let mut value = null_mut();
    // SAFETY: sid belongs to a live token or security descriptor; output is LocalAlloc-owned.
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(last_error());
    }
    let _allocation = Local(value.cast());
    let mut len = 0;
    // SAFETY: the API returns a NUL-terminated UTF-16 SID string.
    unsafe {
        while *value.add(len) != 0 {
            len += 1;
        }
    }
    // SAFETY: len excludes the terminating NUL in the API-owned buffer.
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(value, len)
    }))
}

/// Build a protected DACL at creation, with no interval of inherited public access.
fn descriptor() -> Result<Local, String> {
    let token = user()?;
    let sid = sid_string(user_sid(&token))?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut value = null_mut();
    // SAFETY: SDDL is NUL-terminated, the output slot is live and owned on success.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut value,
            null_mut(),
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(Local(value))
}

/// System, administrators and TrustedInstaller may administer ancestor directories.
fn administrator(sid: PSID) -> Result<bool, String> {
    Ok(matches!(
        sid_string(sid)?.as_str(),
        "S-1-5-18"
            | "S-1-5-32-544"
            | "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
    ))
}

/// Validate owner and every effective allow ACE; unknown ACE forms fail closed.
pub(crate) fn check(file: &File, directory: bool, boundary: Boundary) -> Result<(), String> {
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if reparse(&meta) || (directory && !meta.is_dir()) || (!directory && !meta.is_file()) {
        return Err("private storage must be a real directory or regular file".into());
    }
    // SAFETY: this Win32 output structure consists only of integer fields.
    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    // SAFETY: the live File owns the handle; info is a correctly sized output structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(last_error());
    }
    if !directory && info.nNumberOfLinks != 1 {
        return Err("private storage must not have hard links".into());
    }
    let (mut owner, mut dacl, mut sd) = (null_mut(), null_mut(), null_mut());
    // SAFETY: valid handle and live output slots; sd owns the returned owner and ACL pointers.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut sd,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32).to_string());
    }
    let _allocation = Local(sd);
    let token = user()?;
    let me = user_sid(&token);
    // SAFETY: security APIs returned the SID and ACL; check validity before inspecting them.
    if owner.is_null()
        || dacl.is_null()
        || unsafe { IsValidSid(owner) == 0 || IsValidAcl(dacl) == 0 }
    {
        return Err("private storage requires an owner and a valid DACL".into());
    }
    let ancestor = matches!(boundary, Boundary::Ancestor);
    // SAFETY: both SID pointers are validated or returned by TokenUser.
    if unsafe { EqualSid(owner, me) } == 0 && !(ancestor && administrator(owner)?) {
        return Err("private storage must belong to the current user".into());
    }
    if matches!(boundary, Boundary::Private) {
        let (mut control, mut revision) = (0, 0);
        // SAFETY: sd is the live descriptor returned above and outputs are correctly sized.
        if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err("private storage requires a protected owner-only DACL".into());
        }
    }
    // SAFETY: IsValidAcl succeeded, so the ACL header and GetAce entries are readable.
    for index in 0..unsafe { (*dacl).AceCount } as u32 {
        let mut ace = null_mut();
        // SAFETY: index is within the validated ACL's AceCount, ace is a live output slot.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(last_error());
        }
        // SAFETY: GetAce returned a valid ACE header within the live ACL.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if header.AceFlags as u32 & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match header.AceType {
            1 => continue, // Standard deny ACEs never grant access.
            0 => (),
            _ => return Err("private storage has an unsupported access ACE".into()),
        }
        if (header.AceSize as usize) < 16 {
            return Err("invalid storage access ACE".into());
        }
        // SAFETY: this is a validated standard allow ACE containing its SID start.
        let allow = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let sid: PSID = (&allow.SidStart as *const u32).cast_mut().cast();
        // SAFETY: the 16-byte minimum includes the ACE prefix and the eight-byte SID header.
        let sid_bytes = 8 + 4 * unsafe { (*sid.cast::<SID>()).SubAuthorityCount } as usize;
        if sid_bytes > header.AceSize as usize - 8 {
            return Err("storage principal exceeds its access ACE".into());
        }
        // SAFETY: the entire SID fits within this live ACE, including all subauthorities.
        if unsafe { IsValidSid(sid) } == 0 {
            return Err("invalid storage principal".into());
        }
        // SAFETY: both SIDs are valid and live.
        if unsafe { EqualSid(sid, me) } != 0 || (ancestor && administrator(sid)?) {
            continue;
        }
        let forbidden = match boundary {
            Boundary::Private => u32::MAX,
            #[cfg(feature = "cli")]
            Boundary::Settings => {
                GENERIC_ALL
                    | GENERIC_WRITE
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | FILE_WRITE_DATA
                    | FILE_APPEND_DATA
                    | FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
            }
            // Creating siblings does not permit replacement of an existing, validated child.
            Boundary::Ancestor => {
                GENERIC_ALL
                    | GENERIC_WRITE
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | FILE_DELETE_CHILD
                    | FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
            }
        };
        if allow.Mask & forbidden != 0 {
            return Err("private storage grants access to another principal".into());
        }
    }
    Ok(())
}

/// Open the object itself, never a final reparse target; pins disallow directory rename/delete.
fn raw(
    path: &Path,
    access: u32,
    creation: u32,
    sd: Option<&Local>,
    pin: bool,
) -> Result<File, String> {
    let name = wide(path)?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.map_or(null_mut(), |sd| sd.0),
        bInheritHandle: 0,
    };
    // SAFETY: all input buffers and optional descriptor remain live through CreateFileW.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            FILE_SHARE_READ
                | if pin {
                    0
                } else {
                    FILE_SHARE_WRITE | FILE_SHARE_DELETE
                },
            if sd.is_some() {
                &mut attributes
            } else {
                null_mut()
            },
            creation,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: successful CreateFileW returned a new uniquely owned handle.
    let file = unsafe { File::from_raw_handle(handle) };
    if reparse(&file.metadata().map_err(|e| e.to_string())?) {
        return Err("storage paths must not contain reparse points".into());
    }
    Ok(file)
}

/// Pin every existing ancestor before using an absolute path; each prefix is opened no-follow.
fn parents(path: &Path, trusted: bool, missing: bool) -> Result<Vec<File>, String> {
    wide(path)?;
    let mut prefix = PathBuf::new();
    let mut pins = Vec::new();
    let parent = path.parent().ok_or("storage path has no parent")?;
    for component in parent.components() {
        prefix.push(component);
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        if missing && !prefix.try_exists().map_err(|e| e.to_string())? {
            break;
        }
        let file = raw(
            &prefix,
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            OPEN_EXISTING,
            None,
            true,
        )?;
        if !file.metadata().map_err(|e| e.to_string())?.is_dir() {
            return Err("storage ancestor must be a directory".into());
        }
        if trusted {
            check(&file, true, Boundary::Ancestor)?;
        }
        pins.push(file);
    }
    Ok(pins)
}

/// Verify ancestors before a mutation, including an existing final directory.
pub(crate) fn trusted_ancestors(path: &Path) -> Result<(), String> {
    let probe = path.join(".ancestor-probe");
    parents(&probe, true, true).map(|_| ())
}

/// Open a source file without following any reparse point, pinning ancestors through open.
pub(crate) fn read(path: &Path) -> Result<File, String> {
    let _pins = parents(path, false, false)?;
    raw(path, GENERIC_READ, OPEN_EXISTING, None, false)
}

/// Open or securely create a private file, checking the resulting handle before returning it.
pub(crate) fn open(
    path: &Path,
    write: bool,
    create_new: bool,
    create: bool,
    boundary: Boundary,
) -> Result<File, String> {
    let _pins = parents(path, true, false)?;
    let sd = if create || create_new {
        Some(descriptor()?)
    } else {
        None
    };
    let file = raw(
        path,
        GENERIC_READ | if write { GENERIC_WRITE } else { 0 },
        if create_new {
            CREATE_NEW
        } else if create {
            OPEN_ALWAYS
        } else {
            OPEN_EXISTING
        },
        sd.as_ref(),
        false,
    )?;
    check(&file, false, boundary)?;
    Ok(file)
}

/// Inspect a private object by its no-follow handle, never by pathname ACL lookup.
pub(crate) fn private(path: &Path, directory: bool, boundary: Boundary) -> Result<(), String> {
    let _pins = parents(path, true, false)?;
    let file = raw(
        path,
        READ_CONTROL | FILE_READ_ATTRIBUTES,
        OPEN_EXISTING,
        None,
        false,
    )?;
    check(&file, directory, boundary)
}

/// Create one directory with a protected owner DACL; existing objects must already be private.
pub(crate) fn mkdir(path: &Path) -> Result<(), String> {
    let _pins = parents(path, true, false)?;
    let sd = descriptor()?;
    let name = wide(path)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    // SAFETY: path and descriptor are live and correctly sized for CreateDirectoryW.
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0
        && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
    {
        return Err(last_error());
    }
    let file = raw(
        path,
        READ_CONTROL | FILE_READ_ATTRIBUTES,
        OPEN_EXISTING,
        None,
        false,
    )?;
    check(&file, true, Boundary::Private)
}

/// Ensure a private directory tree exists; validate existing targets instead of changing their ACLs.
#[cfg(feature = "cli")]
pub(crate) fn directories(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => private(path, true, Boundary::Private),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("storage directory has no parent")?;
            if !parent.try_exists().map_err(|e| e.to_string())? {
                directories(parent)?;
            }
            mkdir(path)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Replace a synced private file on the same volume, retaining ancestor pins through publication.
#[cfg(feature = "cli")]
pub(crate) fn replace(temp: &Path, path: &Path) -> Result<(), String> {
    let _pins = parents(path, true, false)?;
    if temp.parent() != path.parent() {
        return Err("private replacement requires the same directory".into());
    }
    private(temp, false, Boundary::Private)?;
    match std::fs::symlink_metadata(path) {
        Ok(_) => private(path, false, Boundary::Settings)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let (source, target) = (wide(temp)?, wide(path)?);
    // SAFETY: both paths are NUL-terminated and parents remain pinned through publication.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "cli")]
    use std::io::Write;

    /// Each security test owns a protected directory on the runner's local filesystem.
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("search-windows-private-{}", uuid::Uuid::new_v4()));
            mkdir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Deliberately change a test file's DACL without invoking a shell or permission utility.
    fn dacl(path: &Path, suffix: &str, protected: bool) {
        let file = raw(
            path,
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            OPEN_EXISTING,
            None,
            false,
        )
        .unwrap();
        let token = user().unwrap();
        let sid = sid_string(user_sid(&token)).unwrap();
        let sddl: Vec<u16> = format!("D:P(A;;FA;;;{sid}){suffix}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut sd = null_mut();
        // SAFETY: test SDDL is terminated and all output slots live through their calls.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut sd,
                    null_mut()
                ),
                0
            );
            let _allocation = Local(sd);
            let (mut present, mut acl, mut defaulted) = (0, null_mut(), 0);
            assert_ne!(
                GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted),
                0
            );
            assert_eq!(
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION
                        | if protected {
                            PROTECTED_DACL_SECURITY_INFORMATION
                        } else {
                            UNPROTECTED_DACL_SECURITY_INFORMATION
                        },
                    null_mut(),
                    null_mut(),
                    acl,
                    null_mut()
                ),
                ERROR_SUCCESS
            );
        }
    }

    /// Public reads and writes are refused for credentials; shareable settings permit reads alone.
    #[test]
    fn protected_owner_acl_is_created_and_public_grants_fail_closed() {
        let temp = Temp::new();
        let path = temp.0.join("credential.json");
        let file = open(&path, true, true, false, Boundary::Private).unwrap();
        check(&file, false, Boundary::Private).unwrap();
        for grant in ["(A;;FR;;;WD)", "(A;;FW;;;WD)"] {
            dacl(&path, grant, true);
            assert!(check(&file, false, Boundary::Private).is_err());
            assert!(private(&path, false, Boundary::Private).is_err());
            #[cfg(feature = "cli")]
            if grant.contains("FR") {
                check(&file, false, Boundary::Settings).unwrap();
            } else {
                assert!(check(&file, false, Boundary::Settings).is_err());
            }
        }
        dacl(&path, "", false);
        assert!(check(&file, false, Boundary::Private).is_err());
    }

    /// Privileged token groups must never be mistaken for the current token user owner.
    #[test]
    fn foreign_owner_is_not_adopted_by_an_administrative_process() {
        let temp = Temp::new();
        let path = temp.0.join("foreign-owner");
        let token = user().unwrap();
        let sid = sid_string(user_sid(&token)).unwrap();
        let sddl: Vec<u16> = format!("O:BAD:P(A;;FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut sd = null_mut();
        // SAFETY: terminated test SDDL and a live descriptor output slot.
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut sd,
                    null_mut(),
                )
            },
            0
        );
        let sd = Local(sd);
        // Unprivileged users cannot assign Administrators ownership. Elevated runners can,
        // and must reject that object even though its DACL grants their user full access.
        if let Ok(file) = raw(
            &path,
            GENERIC_READ | GENERIC_WRITE,
            CREATE_NEW,
            Some(&sd),
            false,
        ) {
            assert!(check(&file, false, Boundary::Private).is_err());
            assert!(open(&path, true, false, false, Boundary::Private).is_err());
        } else {
            assert!(!path.exists());
        }
    }

    /// A second filesystem name must prevent both credential reads and mutations through a handle.
    #[test]
    fn hardlinked_control_files_are_rejected() {
        let temp = Temp::new();
        let path = temp.0.join("control");
        let file = open(&path, true, true, false, Boundary::Private).unwrap();
        std::fs::hard_link(&path, temp.0.join("alias")).unwrap();
        assert!(check(&file, false, Boundary::Private).is_err());
        assert!(open(&path, true, false, true, Boundary::Private).is_err());
    }

    /// Existing locks can be reopened, and synced replacement keeps the private ACL.
    #[cfg(feature = "cli")]
    #[test]
    fn locks_and_replacement_work_with_live_windows_handles() {
        let temp = Temp::new();
        let path = temp.0.join("registry.json");
        let mut file = open(&path, true, true, false, Boundary::Private).unwrap();
        file.write_all(b"old").unwrap();
        let lock_path = temp.0.join("registry.lock");
        let lock = open(&lock_path, true, false, true, Boundary::Private).unwrap();
        lock.lock().unwrap();
        let competing = open(&lock_path, true, false, true, Boundary::Private).unwrap();
        assert!(competing.try_lock().is_err());
        let stage = temp.0.join("stage");
        let mut next = open(&stage, true, true, false, Boundary::Private).unwrap();
        next.write_all(b"new").unwrap();
        next.sync_all().unwrap();
        drop(next);
        replace(&stage, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        private(&path, false, Boundary::Private).unwrap();
    }

    /// Create a mount-point junction through Win32 without developer-mode or symlink privileges.
    fn make_junction(path: &Path, target: &Path) {
        use windows_sys::Win32::System::{
            Ioctl::FSCTL_SET_REPARSE_POINT, SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
            IO::DeviceIoControl,
        };
        mkdir(path).unwrap();
        let file = raw(path, GENERIC_WRITE, OPEN_EXISTING, None, false).unwrap();
        let target = target.to_string_lossy();
        let target = target.strip_prefix(r"\\?\").unwrap_or(&target);
        let substitute: Vec<u16> = format!(r"\??\{target}").encode_utf16().collect();
        let name_bytes = u16::try_from(substitute.len() * 2).unwrap();
        let mut buffer = Vec::new();
        // MountPointReparseBuffer has four u16 fields followed by two terminated names.
        buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buffer.extend_from_slice(&(8 + name_bytes + 4).to_le_bytes());
        buffer.extend_from_slice(&0u16.to_le_bytes());
        for field in [0, name_bytes, name_bytes + 2, 0] {
            buffer.extend_from_slice(&field.to_le_bytes());
        }
        for character in substitute.into_iter().chain([0, 0]) {
            buffer.extend_from_slice(&character.to_le_bytes());
        }
        let mut returned = 0;
        // SAFETY: the live handle is writable and buffer contains a bounded mount-point layout.
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    FSCTL_SET_REPARSE_POINT,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    null_mut(),
                    0,
                    &mut returned,
                    null_mut(),
                )
            },
            0
        );
    }

    /// Junctions must never redirect a read or creation; pins exclude ancestor rename.
    #[test]
    fn junctions_are_rejected_and_ancestor_pins_prevent_replacement() {
        let temp = Temp::new();
        let target = temp.0.join("target");
        mkdir(&target).unwrap();
        let junction = temp.0.join("junction");
        make_junction(&junction, &target);
        assert!(private(&junction, true, Boundary::Private).is_err());
        assert!(open(
            &junction.join("escape"),
            true,
            true,
            false,
            Boundary::Private
        )
        .is_err());
        assert!(!target.join("escape").exists());
        let pins = parents(&target.join("child"), true, false).unwrap();
        assert!(std::fs::rename(&target, temp.0.join("moved")).is_err());
        drop(pins);
        std::fs::rename(&target, temp.0.join("moved")).unwrap();
        std::fs::remove_dir(&junction).unwrap();
    }

    /// Windows name normalization must never create ADS, devices, or ambiguous file names.
    #[test]
    fn windows_aliases_are_rejected_before_disk_access() {
        let temp = Temp::new();
        for name in [
            "file:stream",
            "NUL",
            "con.txt",
            "COM1.py",
            "LPT¹",
            "trailing.",
            "trailing ",
        ] {
            assert!(
                open(&temp.0.join(name), true, true, false, Boundary::Private).is_err(),
                "{name}"
            );
        }
        assert!(wide(Path::new(r"\\.\pipe\search")).is_err());
        assert!(wide(Path::new(r"\\server\share\search")).is_err());
    }
}
