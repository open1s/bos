//! One directory level of the workspace, for the file sidebar.
//!
//! The sidebar expands a folder at a time, so the seam is deliberately
//! shallow: [`list_dir`] answers "what is in this folder — sorted, sized" and
//! never recurses. Containment is not something a caller may forget: a
//! listing refuses to leave the workspace exactly as [`crate::mentions`]
//! refuses to attach a file outside it, and the two share one skip list so
//! "what the sidebar shows" and "what `@` can mention" cannot drift apart.

use std::path::Path;

/// Directory names never listed anywhere in the workspace.
///
/// Also used by the `@`-mention walker: what the sidebar hides, `@` cannot
/// mention, because both are noise the model should not read.
pub(crate) const SKIP_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// Hard cap on rows kept from one directory.
pub(crate) const MAX_ENTRIES: usize = 1000;

/// Read cap: a directory that yields this many raw entries is abandoned, so
/// one pathological folder cannot exhaust memory or stall the webview. What
/// was already read is still sorted and returned.
const READ_CAP: usize = MAX_ENTRIES * 4;

/// One row of the file sidebar.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FileEntry {
    /// Workspace-relative, `/`-separated — exactly what the UI sends back.
    pub(crate) path: String,
    /// Last path segment; what the UI prints.
    pub(crate) name: String,
    /// Directories sort first and are the only expandable rows.
    pub(crate) is_dir: bool,
    /// Size on disk; always `0` for a directory, because summing one would
    /// mean walking it, and a listing never walks.
    pub(crate) bytes: u64,
}

/// What one expansion of the sidebar yields.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FileList {
    /// Rows in display order: directories first, then case-insensitive name.
    pub(crate) entries: Vec<FileEntry>,
    /// Whether the read cap or the row cap dropped anything. The UI states
    /// it rather than hiding it, the way a truncated file read does.
    pub(crate) truncated: bool,
}

/// List one directory of the workspace.
///
/// `rel` is workspace-relative and `/`-separated; `""` lists the workspace
/// root itself. Refusals — an unconfigured root, a missing directory, a
/// symlinked directory that resolves outside the workspace — come back as
/// the message the UI shows, never as a partial listing that hides them.
///
/// Entries that are hidden (dot-prefixed), inside [`SKIP_DIRS`], or symlinks
/// are omitted. A symlink is the one entry that can leave the workspace
/// without a `..`, which is why the mention walker skips them too.
pub(crate) fn list_dir(root: &Path, rel: &str) -> Result<FileList, String> {
    let rel = rel.trim();
    if Path::new(rel).is_absolute() {
        return Err("path escapes the workspace".into());
    }
    let rel = rel.trim_start_matches('/');
    if rel.split(['/', '\\']).any(|part| part == "..") {
        return Err("path escapes the workspace".into());
    }
    if root.as_os_str().is_empty() {
        return Err("no workspace configured".into());
    }
    let canon_root = root
        .canonicalize()
        .map_err(|err| format!("workspace unavailable: {err}"))?;
    let dir = canon_root
        .join(rel)
        .canonicalize()
        .map_err(|_| format!("not found: {rel}"))?;
    if !dir.starts_with(&canon_root) {
        return Err("path escapes the workspace".into());
    }
    if !dir.is_dir() {
        return Err("not a directory".into());
    }
    let read = std::fs::read_dir(&dir).map_err(|err| format!("not readable: {err}"))?;

    let mut entries: Vec<FileEntry> = Vec::new();
    let mut truncated = false;
    // The cap counts raw reads, not kept rows: filtering happens per entry,
    // so the budget is what the directory handed back.
    for (seen, entry) in read.flatten().enumerate() {
        if seen >= READ_CAP {
            truncated = true;
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let full = entry.path();
        let Ok(rel_path) = full.strip_prefix(&canon_root) else {
            continue;
        };
        let is_dir = file_type.is_dir();
        entries.push(FileEntry {
            path: rel_path.to_string_lossy().replace('\\', "/"),
            name,
            is_dir,
            bytes: if is_dir {
                0
            } else {
                entry.metadata().map(|meta| meta.len()).unwrap_or(0)
            },
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    if entries.len() > MAX_ENTRIES {
        entries.truncate(MAX_ENTRIES);
        truncated = true;
    }
    Ok(FileList { entries, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch workspace per test, named so a parallel run cannot
    /// collide and a crashed run's leftovers are recognisable.
    fn workspace(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bos-gui-test-filetree-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch workspace");
        dir
    }

    #[test]
    fn directories_sort_first_then_names_ignore_case() {
        let root = workspace("sort");
        std::fs::create_dir(root.join("Zeta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
        std::fs::write(root.join("b.txt"), b"b").unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();

        let list = list_dir(&root, "").unwrap();
        let shown: Vec<&str> = list.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(shown, vec!["alpha", "Zeta", "a.txt", "b.txt"]);
        assert!(list.entries[0].is_dir && list.entries[1].is_dir);
        assert!(!list.entries[2].is_dir && !list.entries[3].is_dir);
        assert_eq!(list.entries[2].bytes, 1);
        assert_eq!(list.entries[0].bytes, 0, "a directory is never sized");
        assert_eq!(list.entries[1].path, "Zeta");
    }

    #[test]
    fn hidden_and_skipped_directories_are_not_listed() {
        let root = workspace("skip");
        for name in [".git", "target", "node_modules", ".hidden", "src"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        std::fs::write(root.join(".env"), b"x").unwrap();

        let list = list_dir(&root, "").unwrap();
        let shown: Vec<&str> = list.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(shown, vec!["src"], "shown={shown:?}");
        assert!(!list.truncated);
    }

    #[test]
    fn an_empty_path_lists_the_workspace_root() {
        let root = workspace("root");
        std::fs::write(root.join("top.txt"), b"x").unwrap();

        let list = list_dir(&root, "").unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].path, "top.txt");
    }

    #[test]
    fn a_nested_path_lists_that_directory() {
        let root = workspace("nested");
        std::fs::create_dir_all(root.join("crates/gui")).unwrap();
        std::fs::write(root.join("crates/gui/lib.rs"), b"//").unwrap();
        std::fs::write(root.join("crates/agent.rs"), b"//").unwrap();

        let list = list_dir(&root, "crates/gui").unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].path, "crates/gui/lib.rs");
    }

    #[test]
    fn escapes_are_refused() {
        let root = workspace("escape");
        for rel in ["..", "a/../../b", "/etc"] {
            let err = list_dir(&root, rel).unwrap_err();
            assert_eq!(err, "path escapes the workspace", "rel={rel}");
        }
    }

    #[test]
    fn a_backslash_path_is_a_literal_name_here_not_a_separator() {
        let root = workspace("backslash");
        // A backslash is a file-name character on this platform, so `\etc` is
        // the relative name "\etc" — confined like any other name, and never
        // an escape. The `..` check reads both separators, which only ever
        // makes this stricter.
        let err = list_dir(&root, "\\etc").unwrap_err();
        assert_eq!(err, "not found: \\etc");
    }

    #[test]
    fn a_symlink_pointing_out_of_the_workspace_is_refused() {
        let root = workspace("link");
        let outside = root
            .parent()
            .expect("temp dir")
            .join("bos-filetree-outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();

        let err = list_dir(&root, "linked").unwrap_err();
        assert_eq!(err, "path escapes the workspace");
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_missing_directory_says_not_found() {
        let root = workspace("missing");
        assert_eq!(list_dir(&root, "nope").unwrap_err(), "not found: nope");
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let root = workspace("file");
        std::fs::write(root.join("plain.txt"), b"x").unwrap();
        assert_eq!(list_dir(&root, "plain.txt").unwrap_err(), "not a directory");
    }

    #[test]
    fn an_unconfigured_workspace_says_so() {
        let err = list_dir(Path::new(""), "").unwrap_err();
        assert_eq!(err, "no workspace configured");
    }

    #[test]
    fn a_huge_directory_is_capped_and_the_cut_is_reported() {
        let root = workspace("cap");
        for i in 0..(MAX_ENTRIES + 5) {
            std::fs::write(root.join(format!("f{i:04}.txt")), b"x").unwrap();
        }

        let list = list_dir(&root, "").unwrap();
        assert_eq!(list.entries.len(), MAX_ENTRIES);
        assert!(list.truncated);
    }
}
