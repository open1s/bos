//! Child-environment baselines: one documented allowlist per host.
//!
//! A child process must not inherit the parent's whole environment (secrets,
//! shell-only variables), but clearing it and restoring Unix names breaks
//! Windows, where SystemRoot and the profile variables are part of the
//! process-startup contract (issues #317, #318). This module builds the
//! baseline from the CURRENT process environment through a platform
//! allowlist, merges overrides case-insensitively on Windows, and preserves
//! native OsString values (no lossy UTF-8 round trip).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::process::Command;

/// The Unix baseline: the variables a POSIX child needs to start and find
/// its executables, locale and temp space.
#[cfg(unix)]
pub const UNIX_BASELINE: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "USER", "SHELL", "TERM",
];

/// The Windows baseline: the native variables process startup, DLL
/// resolution, profile and temp lookup depend on. Names are matched
/// case-insensitively.
#[cfg(windows)]
pub const WINDOWS_BASELINE: &[&str] = &[
    "SystemRoot",
    "windir",
    "SystemDrive",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "TEMP",
    "TMP",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "PATH",
    "PATHEXT",
    "ComSpec",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
];

/// A case-aware environment map.
///
/// On Windows variable names are case-insensitive: setting PATH replaces
/// Path rather than creating a duplicate the child would resolve ambiguously.
/// On Unix names are exact.
#[derive(Clone, Debug, Default)]
pub struct EnvMap {
    entries: BTreeMap<String, (OsString, OsString)>,
}

fn key(name: &OsStr) -> String {
    let text = name.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_ascii_uppercase()
    } else {
        text
    }
}

impl EnvMap {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// The platform baseline taken from the current process environment.
    pub fn baseline() -> Self {
        let mut map = Self::new();
        #[cfg(unix)]
        let allow: &[&str] = UNIX_BASELINE;
        #[cfg(windows)]
        let allow: &[&str] = WINDOWS_BASELINE;
        for name in allow {
            if let Some(value) = std::env::var_os(name) {
                map.set(OsString::from(name), value);
            }
        }
        map
    }

    /// Set a variable, replacing any case variant on Windows.
    pub fn set(&mut self, name: impl Into<OsString>, value: impl Into<OsString>) {
        let name = name.into();
        let k = key(&name);
        self.entries.insert(k, (name, value.into()));
    }

    /// Remove a variable (case-insensitive on Windows).
    pub fn remove(&mut self, name: &OsStr) {
        self.entries.remove(&key(name));
    }

    /// Look a variable up (case-insensitive on Windows).
    pub fn get(&self, name: &OsStr) -> Option<&OsStr> {
        self.entries.get(&key(name)).map(|(_, v)| v.as_os_str())
    }

    /// Merge PATH entries from another value, keeping the existing order and
    /// skipping duplicates (case-insensitive entry comparison on Windows,
    /// where PATH entries are paths too).
    pub fn merge_path(&mut self, extra: &OsStr) {
        let mut parts: Vec<std::path::PathBuf> = self
            .get(OsStr::new("PATH"))
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default();
        for part in std::env::split_paths(extra) {
            let dup = parts.iter().any(|existing| {
                if cfg!(windows) {
                    existing.to_string_lossy().to_ascii_lowercase()
                        == part.to_string_lossy().to_ascii_lowercase()
                } else {
                    existing == &part
                }
            });
            if !dup {
                parts.push(part);
            }
        }
        if let Ok(joined) = std::env::join_paths(parts) {
            self.set(OsString::from("PATH"), joined);
        }
    }

    /// Apply the map to a command that has been env_clear()ed.
    pub fn apply(&self, cmd: &mut Command) {
        for (name, value) in self.entries.values() {
            cmd.env(name, value);
        }
    }

    /// The entries, for inspection and tests.
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.entries
            .values()
            .map(|(n, v)| (n.as_os_str(), v.as_os_str()))
    }

    /// How many variables are held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The deterministic user environment for built-in tools (issue #317).
///
/// Unix: the process environment (a login-shell snapshot is layered on top
/// by the agent's shell_env module, which keeps its existing behavior).
/// Windows: the process environment overlaid with the persisted user
/// environment from HKCU\\Environment, so executables installed through the
/// user's profile are visible even when BSH was launched as a desktop app
/// with a minimal environment. Registry failure falls back to the process
/// environment; it never produces a misleading empty map.
pub fn user_environment() -> EnvMap {
    let mut map = EnvMap::new();
    for (name, value) in std::env::vars_os() {
        map.set(name, value);
    }
    #[cfg(windows)]
    {
        if let Some(user) = windows_user_environment() {
            for (name, value) in user {
                if key(&name) == "PATH" {
                    map.merge_path(&value);
                } else {
                    map.set(name, value);
                }
            }
        }
    }
    map
}

/// Read the persisted user environment from the registry, bounded.
#[cfg(windows)]
fn windows_user_environment() -> Option<Vec<(OsString, OsString)>> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ,
    };

    let subkey: Vec<u16> = "Environment"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut handle = std::ptr::null_mut();
    // SAFETY: subkey is a NUL-terminated wide string; handle is an out param.
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut handle) };
    if status != 0 {
        return None;
    }
    let mut out = Vec::new();
    // Bounded enumeration: a hostile profile cannot make this loop forever.
    for index in 0..256u32 {
        let mut name_buf = [0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let mut data = [0u8; 32 * 1024];
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: buffers are sized; handle is live.
        let status = unsafe {
            RegEnumValueW(
                handle,
                index,
                name_buf.as_mut_ptr(),
                &mut name_len,
                std::ptr::null_mut(),
                &mut kind,
                data.as_mut_ptr(),
                &mut data_len,
            )
        };
        if status != 0 {
            break;
        }
        // Only string values (REG_SZ=1, REG_EXPAND_SZ=2).
        if kind != 1 && kind != 2 {
            continue;
        }
        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        let text_len = (data_len as usize / 2).saturating_sub(1);
        // SAFETY: the registry wrote data_len bytes of UTF-16 into data.
        let value = unsafe {
            let wide = std::slice::from_raw_parts(data.as_ptr() as *const u16, text_len);
            String::from_utf16_lossy(wide)
        };
        out.push((OsString::from(name), OsString::from(value)));
    }
    // SAFETY: the key handle is owned here.
    unsafe { RegCloseKey(handle) };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_holds_the_platform_contract() {
        let map = EnvMap::baseline();
        if cfg!(windows) {
            // The test process always has SystemRoot and PATH.
            assert!(map.get(OsStr::new("SystemRoot")).is_some());
            assert!(map.get(OsStr::new("systemroot")).is_some());
            assert!(map.get(OsStr::new("PATH")).is_some());
        } else {
            assert!(map.get(OsStr::new("PATH")).is_some());
            assert!(map.get(OsStr::new("HOME")).is_some());
        }
    }

    #[test]
    fn overrides_replace_case_variants_on_windows() {
        let mut map = EnvMap::new();
        map.set(OsString::from("Path"), OsString::from("a"));
        map.set(OsString::from("PATH"), OsString::from("b"));
        if cfg!(windows) {
            assert_eq!(map.len(), 1, "PATH and Path must be one variable");
        } else {
            assert_eq!(map.len(), 2);
        }
        assert_eq!(map.get(OsStr::new("PATH")).unwrap(), OsStr::new("b"));
    }

    #[test]
    fn path_merge_deduplicates() {
        let mut map = EnvMap::new();
        let sep = if cfg!(windows) { ";" } else { ":" };
        map.set(
            OsString::from("PATH"),
            OsString::from(format!("/usr/bin{sep}/bin")),
        );
        map.merge_path(OsStr::new(&format!("/bin{sep}/opt/bin")));
        let path = map
            .get(OsStr::new("PATH"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let sep = if cfg!(windows) { ';' } else { ':' };
        let entries: Vec<&str> = path.split(sep).collect();
        assert_eq!(
            entries.iter().filter(|e| **e == "/bin").count(),
            1,
            "duplicate /bin must be merged: {path}"
        );
        assert!(entries.contains(&"/opt/bin"));
        assert!(entries.contains(&"/usr/bin"));
    }

    #[test]
    fn user_environment_is_never_empty_when_the_process_has_one() {
        let map = user_environment();
        assert!(map.get(OsStr::new("PATH")).is_some());
    }
}
