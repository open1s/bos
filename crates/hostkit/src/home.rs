//! Deterministic, OS-native home and root resolution (issues #306, #307).
//!
//! The default data directory comes from the platform's home-directory API,
//! never from shell-style tilde expansion of a literal. An override
//! (BSH_HOME, BSH_EXTENSION_ROOT) must be absolute; a relative override is
//! refused with a diagnostic instead of silently depending on the launch
//! working directory. Native OsString values are preserved end to end.
//!
//! Backward compatibility: a leading ~ in an override is expanded against
//! the platform home once, explicitly, and documented; the DEFAULT path no
//! longer contains a tilde at all.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Why a root override was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootError {
    /// The override is relative; roots must not depend on the launch CWD.
    Relative,
    /// The platform home directory could not be determined.
    NoHome,
}

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Relative => write!(
                f,
                "root override must be an absolute path (a relative path would depend on the launch working directory)"
            ),
            Self::NoHome => write!(f, "the platform home directory could not be determined"),
        }
    }
}

impl std::error::Error for RootError {}

/// The platform home directory, native and lossless.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Resolve an application root from an optional override and a default
/// relative location under the home directory.
///
/// Precedence: a non-empty override wins; an empty or absent override uses
/// home/default_relative. A ~ prefix in the override is expanded once
/// against the platform home (documented compatibility); any other relative
/// override is refused.
pub fn resolve_root(
    override_value: Option<&OsStr>,
    default_relative: &str,
) -> Result<PathBuf, RootError> {
    let home = home_dir().ok_or(RootError::NoHome)?;
    let Some(value) = override_value else {
        return Ok(home.join(default_relative));
    };
    if value.is_empty() {
        return Ok(home.join(default_relative));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    // Explicit, documented tilde compatibility for user-written overrides.
    let text = value.to_string_lossy();
    if text == "~" {
        return Ok(home);
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return Ok(home.join(rest));
    }
    Err(RootError::Relative)
}

/// Read an environment override as a native OsString, treating an empty
/// value as absent.
pub fn env_override(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_home_relative_and_absolute() {
        let root = resolve_root(None, ".bos/bsh").unwrap();
        assert!(root.is_absolute());
        assert!(root.ends_with(".bos/bsh"));
    }

    #[test]
    fn empty_override_uses_the_default() {
        let root = resolve_root(Some(OsStr::new("")), ".bos/bsh").unwrap();
        assert!(root.ends_with(".bos/bsh"));
    }

    #[test]
    fn absolute_override_wins() {
        let root = resolve_root(Some(OsStr::new("/custom/root")), ".bos/bsh").unwrap();
        assert_eq!(root, PathBuf::from("/custom/root"));
    }

    #[test]
    fn relative_override_is_refused() {
        assert_eq!(
            resolve_root(Some(OsStr::new("relative/root")), ".bos/bsh"),
            Err(RootError::Relative)
        );
    }

    #[test]
    fn tilde_override_expands_once() {
        let root = resolve_root(Some(OsStr::new("~/data")), ".bos/bsh").unwrap();
        assert!(root.is_absolute());
        assert!(root.ends_with("data"));
        assert!(!root.to_string_lossy().contains('~'));
    }
}
