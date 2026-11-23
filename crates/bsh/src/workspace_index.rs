//! The `@` picker's candidates: one flat, ignore-aware path index per
//! workspace.
//!
//! A picker on the input path cannot walk a tree per keystroke: this workspace
//! has 9,732 files outside its build trees, and the old reference source walked
//! the filesystem from the render function, from every arrow press and again
//! from the Enter handler. The index is built once on a worker thread by
//! [`scan`], swapped in by generation, and rebuilt only when the workspace
//! changes, the card opens after [`STALE_AFTER`], or the workspace watcher
//! reports a change.
//!
//! The walk is NOT a second ignore policy: [`scan`] calls
//! [`crate::glob::build_walker`], so `.gitignore`, `.ignore`, `.rgignore`,
//! the heavy-directory skips, the hidden-entry policy and the no-symlink policy
//! are the file tools' own. The reader's `.bsh/.ignore` is loaded through
//! [`crate::workspace_ignore::Rules`], the same cached rules the tools use.
//!
//! The index holds FOLDERS too, not only files: a directory can be matched by
//! name (the reader types `ui` and sees `src/ui/`) and drilling into one is
//! how a query is scoped.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The most entries one workspace index holds.
///
/// A monorepo past this is already unusable in a 320px card; the cap keeps the
/// index bounded in memory and in build time, and marks itself truncated so the
/// status line can be honest about it.
pub(crate) const MAX_ENTRIES: usize = 200_000;

/// How long a ready index is trusted before a card that opens onto it rescans.
///
/// The watcher is the live path; this is the fallback for a watcher that could
/// not start (a network volume, or an event the platform coalesced away) and
/// for edits made outside the watched window. Short enough that reopening the
/// card after a turn sees what the turn wrote.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(2);

/// Where the index is in its lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum IndexStatus {
    /// No workspace has been indexed yet.
    #[default]
    Idle,
    /// A scan is running on a worker thread.
    Building,
    /// The entries below are the last completed scan.
    Ready,
    /// The last scan failed; [`FileIndex::error`] says why.
    Failed,
}

/// One indexed path, relative to the workspace root with `/` separators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The path as the reader and the mention grammar spell it.
    pub(crate) rel: String,
    /// Whether the entry is a directory.
    pub(crate) dir: bool,
}

impl Entry {
    /// The entry's own name, the label a row draws.
    pub(crate) fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    /// The directory the entry lives in, `""` for a root entry.
    pub(crate) fn parent(&self) -> &str {
        match self.rel.rfind('/') {
            Some(index) => &self.rel[..index],
            None => "",
        }
    }
}

/// What one scan found.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scan {
    /// Every indexed path, sorted by [`Entry::rel`].
    pub(crate) entries: Vec<Entry>,
    /// Whether the scan stopped at the cap rather than at the end of the tree.
    pub(crate) truncated: bool,
}

/// Index one workspace root.
pub(crate) fn scan(root: &Path) -> Result<Scan, String> {
    scan_limited(root, MAX_ENTRIES)
}

/// Index one workspace root, capped for tests and for the entry ceiling.
pub(crate) fn scan_limited(root: &Path, max: usize) -> Result<Scan, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let rules = crate::workspace_ignore::Rules::load(root);
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    for walked in crate::glob::build_walker(root, false, Some(&rules)).build() {
        let Ok(walked) = walked else {
            continue;
        };
        // The root itself is the scope, not a row.
        if walked.depth() == 0 {
            continue;
        }
        let Some(kind) = walked.file_type() else {
            continue;
        };
        // A symlink to a directory is not a folder to drill into: the walk
        // does not follow it, and indexing it as a file would offer the reader
        // a path the file tools refuse to list.
        if kind.is_symlink() && std::fs::metadata(walked.path()).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        let Ok(relative) = walked.path().strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        let rel = relative.replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        entries.push(Entry {
            rel,
            dir: kind.is_dir(),
        });
        if entries.len() >= max {
            truncated = true;
            break;
        }
    }
    entries.sort_by(|left, right| left.rel.cmp(&right.rel));
    Ok(Scan { entries, truncated })
}

/// One workspace's candidate list, with the scan that produced it.
#[derive(Debug, Default)]
pub(crate) struct FileIndex {
    /// The root the entries belong to.
    root: Option<PathBuf>,
    /// The indexed paths, sorted by [`Entry::rel`].
    entries: Vec<Entry>,
    /// Whether the scan stopped at [`MAX_ENTRIES`].
    truncated: bool,
    /// When the entries were installed, for [`STALE_AFTER`].
    built_at: Option<Instant>,
    /// The generation a running or finished scan belongs to.
    generation: u64,
    /// Where the index is in its lifecycle.
    status: IndexStatus,
}

impl FileIndex {
    /// Where the index is in its lifecycle.
    pub(crate) fn status(&self) -> IndexStatus {
        self.status
    }

    /// The generation a scan result must match to be installed.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Every indexed path, sorted.
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether the scan stopped at the entry cap.
    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    /// Whether a scan is running.
    pub(crate) fn is_building(&self) -> bool {
        self.status == IndexStatus::Building
    }

    /// Start a scan for `root` when one is needed, and return its generation.
    ///
    /// A build already running wins; a ready index younger than
    /// [`STALE_AFTER`] wins; a failed scan is retried only after the same
    /// window, so a broken volume does not spawn one worker per keystroke.
    pub(crate) fn begin_if_needed(&mut self, root: &Path) -> Option<u64> {
        let same_root = self.root.as_deref() == Some(root);
        if same_root {
            let fresh = self.built_at.is_some_and(|at| at.elapsed() < STALE_AFTER);
            match self.status {
                IndexStatus::Building => return None,
                IndexStatus::Ready if fresh => return None,
                IndexStatus::Failed if fresh => return None,
                _ => {}
            }
        }
        Some(self.begin(root))
    }

    /// Start a scan unconditionally, keeping the old entries while it runs.
    pub(crate) fn begin(&mut self, root: &Path) -> u64 {
        if self.root.as_deref() != Some(root) {
            self.entries.clear();
            self.truncated = false;
        }
        self.root = Some(root.to_path_buf());
        self.status = IndexStatus::Building;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Install a completed scan, dropping one from an older generation.
    pub(crate) fn install(&mut self, generation: u64, scan: Scan) {
        if generation != self.generation {
            return;
        }
        self.entries = scan.entries;
        self.truncated = scan.truncated;
        self.status = IndexStatus::Ready;
        self.built_at = Some(Instant::now());
    }

    /// Record a scan that could not run.
    pub(crate) fn fail(&mut self, generation: u64, message: String) {
        if generation != self.generation {
            return;
        }
        log::warn!("workspace index scan failed: {message}");
        self.status = IndexStatus::Failed;
        self.built_at = Some(Instant::now());
    }

    /// The direct children of `scope` (`""` is the root), folders first.
    ///
    /// This is the browse list a blank query draws, so it is sorted the way a
    /// tree reads rather than by the index's own path order.
    pub(crate) fn children(&self, scope: &str) -> Vec<&Entry> {
        let mut rows: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.parent() == scope)
            .collect();
        rows.sort_by(|left, right| {
            right
                .dir
                .cmp(&left.dir)
                .then_with(|| left.rel.cmp(&right.rel))
        });
        rows
    }

    /// Every entry at or below `scope`, the universe a scoped search ranks.
    pub(crate) fn under<'a>(&'a self, scope: &str) -> impl Iterator<Item = &'a Entry> + 'a {
        let prefix = (!scope.is_empty()).then(|| format!("{scope}/"));
        self.entries.iter().filter(move |entry| {
            prefix
                .as_deref()
                .is_none_or(|prefix| entry.rel.starts_with(prefix))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory each test owns.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bsh-workspace-index-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, body).expect("fixture file");
    }

    fn rels(scan: &Scan) -> Vec<&str> {
        scan.entries
            .iter()
            .map(|entry| entry.rel.as_str())
            .collect()
    }

    #[test]
    fn scan_indexes_files_and_folders() {
        let root = scratch("tree");
        write(&root, "alpha.txt", "a");
        write(&root, "sub/gamma.rs", "g");
        let scan = scan(&root).expect("scan");
        assert_eq!(rels(&scan), vec!["alpha.txt", "sub", "sub/gamma.rs"]);
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.rel == "sub" && entry.dir)
        );
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.rel == "sub/gamma.rs" && !entry.dir)
        );
    }

    #[test]
    fn scan_honors_gitignore_and_the_heavy_tree_skips() {
        let root = scratch("ignored");
        write(&root, "kept.txt", "k");
        write(&root, "ignored.txt", "i");
        write(&root, "node_modules/dep/index.js", "n");
        write(&root, "target/debug/out", "t");
        write(&root, ".secret", "s");
        write(&root, ".git/HEAD", "ref");
        write(&root, ".gitignore", "ignored.txt\n");
        let scan = scan(&root).expect("scan");
        let rels = rels(&scan);
        assert!(rels.contains(&"kept.txt"), "{rels:?}");
        assert!(!rels.contains(&"ignored.txt"), "{rels:?}");
        assert!(
            !rels.iter().any(|rel| rel.starts_with("node_modules")),
            "{rels:?}"
        );
        assert!(
            !rels.iter().any(|rel| rel.starts_with("target")),
            "{rels:?}"
        );
        assert!(!rels.iter().any(|rel| rel.starts_with('.')), "{rels:?}");
    }

    #[test]
    fn scan_honors_the_workspace_ignore_file() {
        let root = scratch("workspace-ignore");
        write(&root, "kept.txt", "k");
        write(&root, "blocked.txt", "b");
        // Rules::load creates and parses .bsh/.ignore; the workspace's own rule
        // must exclude the path from the picker exactly as it does from glob.
        crate::workspace_ignore::Rules::load(&root);
        write(&root, ".bsh/.ignore", "blocked.txt\n");
        let scan = scan(&root).expect("scan");
        let rels = rels(&scan);
        assert!(rels.contains(&"kept.txt"), "{rels:?}");
        assert!(!rels.contains(&"blocked.txt"), "{rels:?}");
    }

    #[test]
    fn scan_caps_and_says_so() {
        let root = scratch("cap");
        for index in 0..10 {
            write(&root, &format!("file-{index}.txt"), "x");
        }
        let scan = scan_limited(&root, 3).expect("scan");
        assert_eq!(scan.entries.len(), 3);
        assert!(scan.truncated);
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let missing = std::env::temp_dir().join("bsh-workspace-index-missing");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(scan(&missing).is_err());
    }

    #[test]
    fn entries_know_their_name_and_parent() {
        let entry = Entry {
            rel: "crates/bsh/src/ui/slash_palette.rs".to_string(),
            dir: false,
        };
        assert_eq!(entry.name(), "slash_palette.rs");
        assert_eq!(entry.parent(), "crates/bsh/src/ui");
        let root_entry = Entry {
            rel: "Cargo.toml".to_string(),
            dir: false,
        };
        assert_eq!(root_entry.parent(), "");
    }

    #[test]
    fn children_read_folders_first_and_under_reads_the_subtree() {
        let root = scratch("scope");
        write(&root, "zeta.txt", "z");
        write(&root, "alpha.txt", "a");
        write(&root, "sub/gamma.rs", "g");
        write(&root, "sub/deeper/delta.rs", "d");
        let scan = scan(&root).expect("scan");
        let mut index = FileIndex::default();
        let generation = index.begin_if_needed(&root).expect("build starts");
        index.install(generation, scan);
        let root_children: Vec<&str> = index
            .children("")
            .iter()
            .map(|entry| entry.rel.as_str())
            .collect();
        assert_eq!(root_children, vec!["sub", "alpha.txt", "zeta.txt"]);
        let sub_children: Vec<&str> = index
            .children("sub")
            .iter()
            .map(|entry| entry.rel.as_str())
            .collect();
        assert_eq!(sub_children, vec!["sub/deeper", "sub/gamma.rs"]);
        let under: Vec<&str> = index.under("sub").map(|entry| entry.rel.as_str()).collect();
        assert_eq!(
            under,
            vec!["sub/deeper", "sub/deeper/delta.rs", "sub/gamma.rs"]
        );
        let all = index.under("").count();
        assert_eq!(all, index.entries().len());
    }

    #[test]
    fn a_build_runs_once_and_an_older_result_is_dropped() {
        let root = scratch("lifecycle");
        write(&root, "a.txt", "a");
        let mut index = FileIndex::default();
        assert_eq!(index.status(), IndexStatus::Idle);
        let generation = index.begin_if_needed(&root).expect("starts");
        assert_eq!(index.status(), IndexStatus::Building);
        assert!(
            index.begin_if_needed(&root).is_none(),
            "a running build is not started twice"
        );
        let stale_scan = Scan {
            entries: vec![Entry {
                rel: "stale.txt".to_string(),
                dir: false,
            }],
            truncated: false,
        };
        // A result from an older generation cannot overwrite the current one.
        index.begin(&root);
        index.install(generation, stale_scan.clone());
        assert_ne!(
            index.status(),
            IndexStatus::Ready,
            "the stale result is dropped"
        );
        let fresh = index.generation();
        index.install(
            fresh,
            Scan {
                entries: vec![Entry {
                    rel: "a.txt".to_string(),
                    dir: false,
                }],
                truncated: false,
            },
        );
        assert_eq!(index.status(), IndexStatus::Ready);
        assert_eq!(index.entries().len(), 1);
        assert!(
            index.begin_if_needed(&root).is_none(),
            "a fresh index is not rescanned"
        );
        let other = scratch("lifecycle-other");
        assert!(
            index.begin_if_needed(&other).is_some(),
            "a different root is a new build"
        );
        assert!(index.entries().is_empty(), "and the old rows are gone");
    }

    /// Measure the real workspace: the numbers the fzf upgrade is judged on.
    ///
    /// Ignored by default because it reads this checkout rather than a fixture;
    /// run it explicitly with `cargo test -p bsh --lib -- --ignored this_workspaces`.
    #[test]
    #[ignore = "measures this checkout; run explicitly"]
    fn this_workspaces_scan_and_rank_are_measured() {
        let root = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .expect("the workspace root");
        let start = Instant::now();
        let scan = scan(&root).expect("scan");
        eprintln!(
            "scan: {} entries, truncated={}, {:?}",
            scan.entries.len(),
            scan.truncated,
            start.elapsed()
        );
        for needle in ["supal", "ui", "bsh/ui", "command"] {
            let start = Instant::now();
            let hits =
                crate::fuzzy::rank(needle, scan.entries.iter().map(|entry| entry.rel.as_str()));
            eprintln!(
                "rank {needle:8}: {:5} hits, {:?}",
                hits.len(),
                start.elapsed()
            );
        }
    }

    #[test]
    fn a_failed_scan_is_reported() {
        let root = scratch("failure");
        let mut index = FileIndex::default();
        let generation = index.begin(&root);
        index.fail(generation, "no volume".to_string());
        assert_eq!(index.status(), IndexStatus::Failed);
        assert!(
            index.begin_if_needed(&root).is_none(),
            "a fresh failure is not retried per keystroke"
        );
    }
}
