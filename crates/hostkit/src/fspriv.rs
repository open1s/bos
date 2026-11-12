//! Private secret files: owner-only on every host (issue #299).
//!
//! Unix keeps the historical mode 0600. Windows applies a PROTECTED DACL
//! granting access only to the current user, on creation AND on replacement,
//! because an inherited parent DACL is not a privacy guarantee. Every path
//! fails closed: if the private policy cannot be established, the write
//! errors instead of silently storing the secret world-readable.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Open options for a private file: mode 0600 at creation on Unix.
///
/// On Windows the options are plain; the caller MUST follow creation with
/// [private_permissions] (or use [write_private], which does both).
pub fn private_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Apply the owner-only policy to an existing file.
///
/// Unix: chmod 0600. Windows: a protected DACL with a single access-allowed
/// ACE for the current user. Errors are returned, never swallowed.
pub fn private_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        windows_private_dacl(path)
    }
}

/// Atomically write a secret file with the owner-only policy.
///
/// The bytes land in a sibling temp file that is private FROM CREATION,
/// are fsynced, then renamed over the target; the target is hardened again
/// after the rename so a pre-existing world-readable file cannot survive the
/// replacement. A failure at any stage leaves the previous file intact.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "secret path has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "secret".to_string()),
        std::process::id()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = private_open_options().open(&tmp)?;
        private_permissions(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        private_permissions(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Windows: replace the file's DACL with a protected, owner-only one.
#[cfg(windows)]
fn windows_private_dacl(path: &Path) -> io::Result<()> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_ALL, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, GetLengthSid, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL,
        ACL_REVISION, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // The current user's SID from the process token.
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess is a pseudo-handle; token is an out param.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut needed = 0u32;
    // SAFETY: first call queries the required size.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        // SAFETY: token is owned here.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
        return Err(io::Error::last_os_error());
    }
    let mut buf = vec![0u8; needed as usize];
    // SAFETY: buf is sized from the query.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    // SAFETY: token is owned here.
    unsafe { windows_sys::Win32::Foundation::CloseHandle(token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the buffer holds a TOKEN_USER written by the kernel.
    let user: &TOKEN_USER = unsafe { &*buf.as_ptr().cast() };
    let sid = user.User.Sid;
    // SAFETY: sid is a valid SID from the token.
    let sid_len = unsafe { GetLengthSid(sid) };

    // One ACE: the current user, full control.
    let acl_bytes = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() + sid_len as usize;
    let mut acl_buf = vec![0u8; acl_bytes];
    // SAFETY: acl_buf is sized for the ACL plus one ACE plus the SID.
    let acl: *mut ACL = acl_buf.as_mut_ptr().cast();
    // SAFETY: initializing the ACL header in owned memory.
    let ok =
        unsafe { windows_sys::Win32::Security::InitializeAcl(acl, acl_bytes as u32, ACL_REVISION) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: acl is initialized; sid is valid; the buffer has room.
    let ok = unsafe { AddAccessAllowedAce(acl, ACL_REVISION, GENERIC_ALL, sid) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: wide is NUL-terminated; acl is a valid DACL; the protected flag
    // blocks inheritance from the parent directory.
    let status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl as *const _,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_private(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        write_private(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_world_readable_file_is_hardened_on_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        fs::write(&path, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_private(&path, b"new").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
