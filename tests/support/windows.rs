//! Test fixtures deliberately opt into the same protected current-user ACL as production storage.
use std::{ffi::c_void, mem::size_of, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    System::Threading::*,
};

/// Make a newly created test object private without depending on the runner's inherited ACL.
pub fn secure(path: &Path) {
    let mut token = null_mut();
    // SAFETY: valid process pseudo-handle and live output buffers; every allocated handle is closed.
    unsafe {
        assert_ne!(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token),
            0
        );
        let mut len = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
        let mut buffer = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
        assert_ne!(
            GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len),
            0
        );
        CloseHandle(token);
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = null_mut();
        assert_ne!(ConvertSidToStringSidW(user.User.Sid, &mut sid), 0);
        let mut chars = 0;
        while *sid.add(chars) != 0 {
            chars += 1;
        }
        let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, chars));
        LocalFree(sid.cast::<c_void>());
        let descriptor: Vec<u16> = format!("O:{sid_text}D:P(A;OICI;FA;;;{sid_text})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut sd = null_mut();
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor.as_ptr(),
                1,
                &mut sd,
                null_mut()
            ),
            0
        );
        let (mut owner, mut owner_defaulted) = (null_mut(), 0);
        assert_ne!(
            GetSecurityDescriptorOwner(sd, &mut owner, &mut owner_defaulted),
            0
        );
        let (mut present, mut acl, mut defaulted) = (0, null_mut(), 0);
        assert_ne!(
            GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted),
            0
        );
        let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let status = SetNamedSecurityInfoW(
            name.as_mut_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner,
            null_mut(),
            acl,
            null_mut(),
        );
        LocalFree(sd);
        assert_eq!(status, ERROR_SUCCESS);
    }
}
