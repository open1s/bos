//! The folder chooser behind "Add a workspace".
//!
//! This reads directories for one reason: a workspace needs a root before it
//! exists, so the person adding one has to point at a folder the app has never
//! been trusted with. That is the opposite of [`crate::filetree`], which exists
//! to read *inside* an already-chosen root and refuses to leave it. The two
//! cannot share a rule, and pretending otherwise would either let the sidebar
//! escape its workspace or make the chooser unable to reach past the current
//! one — so the difference is named here rather than hidden.
//!
//! What this module will not do: read a file's contents. It answers with
//! directory names and absolute paths, and the resolved path must exist and be
//! a directory before any of them is returned.

use std::path::{Path, PathBuf};

/// Hard cap on sub-folders kept from one directory.
pub(crate) const MAX_ENTRIES: usize = 500;

/// One sub-folder the chooser may step into.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FolderEntry {
    /// Absolute path, `..` already resolved — what the UI sends back.
    pub(crate) path: String,
    /// Last path segment; what the UI prints.
    pub(crate) name: String,
}

/// What one level of the chooser yields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FolderListing {
    /// The resolved directory these entries came from.
    pub(crate) path: String,
    /// Its parent, absent at a filesystem root.
    pub(crate) parent: Option<String>,
    /// Sub-folders only, sorted case-insensitively by name.
    pub(crate) entries: Vec<FolderEntry>,
    /// Folders the cap left out; `0` when everything fit.
    pub(crate) truncated: usize,
    /// Entries that could not be stat'd or read, counted but not named.
    pub(crate) unreadable: usize,
}

/// The folder the chooser opens on when nothing is named.
///
/// `HOME`, then `USERPROFILE` for Windows, then the executable's directory as a
/// last resort — a chooser that opens nowhere is useless, and `/` at least
/// always exists on the platforms that have neither variable.
pub(crate) fn home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_else(|_| String::from("/"))
        })
}

/// Resolve `path` (or [`home`]) to an existing directory.
fn resolve(path: Option<&str>) -> Result<PathBuf, String> {
    let raw = path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(home);
    let expanded = shellexpand::tilde(&raw).into_owned();
    let canon = Path::new(&expanded)
        .canonicalize()
        .map_err(|e| format!("{raw}: {e}"))?;
    if !canon.is_dir() {
        return Err(format!("{raw} is not a folder"));
    }
    Ok(canon)
}

/// List the sub-folders of `path`, or of [`home`] when it is blank.
pub(crate) fn list(path: Option<&str>) -> Result<FolderListing, String> {
    list_capped(path, MAX_ENTRIES)
}

/// [`list`] with the cap as an argument, so a test can reach the truncation
/// path without creating five hundred folders.
pub(crate) fn list_capped(path: Option<&str>, cap: usize) -> Result<FolderListing, String> {
    let dir = resolve(path)?;
    let read = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut entries = Vec::new();
    let mut unreadable = 0usize;
    for entry in read {
        let Ok(entry) = entry else {
            // A name that vanished between the read and the stat, or one this
            // process may not see: counted, never fatal.
            unreadable += 1;
            continue;
        };
        let child = entry.path();
        // Follow the link rather than trusting `file_type`: a symlinked folder
        // is a folder the user can enter, and it is the target that decides.
        if !std::fs::metadata(&child).map(|m| m.is_dir()).unwrap_or(false) {
            continue;
        }
        entries.push(FolderEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: child.to_string_lossy().to_string(),
        });
    }
    entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    let truncated = entries.len().saturating_sub(cap);
    entries.truncate(cap);
    Ok(FolderListing {
        parent: dir.parent().map(|p| p.to_string_lossy().to_string()),
        path: dir.to_string_lossy().to_string(),
        entries,
        truncated,
        unreadable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-folders-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("archive")).expect("mkdir archive");
        std::fs::create_dir_all(dir.join("Bos")).expect("mkdir Bos");
        std::fs::create_dir_all(dir.join("zeta")).expect("mkdir zeta");
        std::fs::write(dir.join("notes.txt"), b"x").expect("write file");
        dir
    }

    #[test]
    fn only_folders_are_listed_sorted_case_insensitively() {
        let dir = tree("list");
        let listing = list(Some(&dir.to_string_lossy())).expect("list");
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["archive", "Bos", "zeta"]);
        assert!(listing.entries.iter().all(|e| Path::new(&e.path).is_dir()));
        assert_eq!(listing.truncated, 0);
        assert_eq!(listing.unreadable, 0);
        // Both paths are the *resolved* ones: on macOS `/var` is a link to
        // `/private/var`, and a chooser that mixed the two would offer a parent
        // the next listing cannot enter.
        let canon = dir.canonicalize().unwrap();
        assert_eq!(listing.path, canon.to_string_lossy());
        assert_eq!(
            listing.parent,
            canon.parent().map(|p| p.to_string_lossy().to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_is_not_a_folder_and_a_missing_path_says_which_one() {
        let dir = tree("refuse");
        let file = dir.join("notes.txt");
        let err = list(Some(&file.to_string_lossy())).expect_err("a file is refused");
        assert!(err.contains("is not a folder"), "{err}");
        let gone = dir.join("nowhere");
        let err = list(Some(&gone.to_string_lossy())).expect_err("a missing path is refused");
        assert!(err.contains("nowhere"), "{err}");
        // A blank path is not an error: it means "open at home".
        std::env::set_var("HOME", &dir);
        let home_listing = list(Some("  ")).expect("blank opens home");
        assert_eq!(home_listing.path, dir.canonicalize().unwrap().to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_is_reported_rather_than_silently_dropped() {
        let dir = tree("cap");
        let listing = list_capped(Some(&dir.to_string_lossy()), 2).expect("list");
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.truncated, 1);
        assert_eq!(listing.entries[0].name, "archive");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
