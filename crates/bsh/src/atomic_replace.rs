//! Atomic file replacement on every supported platform.
//!
//! On Unix a rename over an existing path is atomic. Windows has no equivalent
//! in `std::fs::rename`: it fails when the destination exists, so a temp-and-
//! rename write would turn every overwrite into an error. `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING` is the platform's atomic replace; this module is
//! the one place that knows the difference, so the file tools and sed share it.

use std::path::Path;

/// Replace `to` with `from` in one step.
///
/// The caller keeps both paths on the same volume (a temp beside the target),
/// so the filesystem can make the replacement atomic. A failure leaves `to`
/// untouched on Unix; Windows reports its own error.
#[cfg(not(windows))]
pub(crate) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Replace `to` with `from` in one step.
///
/// Windows needs `MoveFileExW`: `std::fs::rename` refuses an existing
/// destination. The same-volume guarantee still holds because callers write
/// the temp beside the target.
#[cfg(windows)]
pub(crate) fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from: Vec<u16> = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let replaced = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_overwrites_an_existing_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        std::fs::write(&source, "new").expect("source");
        std::fs::write(&target, "old").expect("target");

        replace(&source, &target).expect("replace");

        assert_eq!(std::fs::read_to_string(&target).expect("target"), "new");
        assert!(!source.exists(), "the source is consumed by the replace");
    }
}
