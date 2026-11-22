//! Working-copy VCS state for the right-pane Files tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VcsKind {
    Jj,
    Git,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum FileStatus {
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflicted,
    Unknown,
}

impl FileStatus {
    pub(crate) fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Renamed => "R",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Conflicted => "C",
            Self::Unknown => "?",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Renamed => "Renamed",
            Self::Modified => "Modified",
            Self::Deleted => "Deleted",
            Self::Conflicted => "Conflicted",
            Self::Unknown => "Changed",
        }
    }
    pub(crate) fn severity(self) -> u8 {
        match self {
            Self::Conflicted => 5,
            Self::Deleted => 4,
            Self::Modified => 3,
            Self::Renamed => 2,
            Self::Added => 1,
            Self::Unknown => 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct VcsStatus {
    pub(crate) kind: Option<VcsKind>,
    pub(crate) files: BTreeMap<PathBuf, FileStatus>,
    pub(crate) error: Option<String>,
}

impl VcsStatus {
    pub(crate) fn get(&self, rel: &Path) -> Option<FileStatus> {
        self.files.get(rel).copied()
    }
    #[allow(dead_code)]
    pub(crate) fn changed_under(&self, rel: &Path) -> usize {
        let prefix = rel.to_string_lossy().replace('\\', "/");
        self.files
            .keys()
            .filter(|path| {
                let path = path.to_string_lossy().replace('\\', "/");
                prefix.is_empty() || path.starts_with(&format!("{prefix}/"))
            })
            .count()
    }
    #[allow(dead_code)]
    pub(crate) fn worst_under(&self, rel: &Path) -> Option<FileStatus> {
        let prefix = rel.to_string_lossy().replace('\\', "/");
        self.files
            .iter()
            .filter(|(path, _)| {
                let path = path.to_string_lossy().replace('\\', "/");
                prefix.is_empty() || path.starts_with(&format!("{prefix}/"))
            })
            .map(|(_, status)| *status)
            .max_by_key(|status| status.severity())
    }
}

pub(crate) fn status(root: &Path) -> VcsStatus {
    let Some((kind, repo_root)) = discover(root) else {
        return VcsStatus::default();
    };
    let workspace_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let prefix = workspace_root
        .strip_prefix(&repo_root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let result = match kind {
        VcsKind::Jj => jj_status(&repo_root),
        VcsKind::Git => git_status(&repo_root),
    };
    match result {
        Ok(files) => VcsStatus {
            kind: Some(kind),
            files: files
                .into_iter()
                .filter_map(|(path, status)| {
                    map_path(&path, &workspace_root, &prefix).map(|path| (path, status))
                })
                .collect(),
            error: None,
        },
        Err(error) => VcsStatus {
            kind: Some(kind),
            files: BTreeMap::new(),
            error: Some(error),
        },
    }
}

pub(crate) fn discover(root: &Path) -> Option<(VcsKind, PathBuf)> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for ancestor in root.ancestors() {
        if ancestor.join(".jj").is_dir() {
            return Some((VcsKind::Jj, ancestor.to_path_buf()));
        }
        if ancestor.join(".git").exists() {
            return Some((VcsKind::Git, ancestor.to_path_buf()));
        }
    }
    None
}

pub(crate) fn diff(root: &Path, rel: &Path) -> Result<String, String> {
    let Some((kind, repo_root)) = discover(root) else {
        return Err("the workspace is not a jj or git checkout".to_string());
    };
    let workspace_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let prefix = workspace_root
        .strip_prefix(&repo_root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let target = prefix.join(rel);
    let target = target.to_string_lossy().replace('\\', "/");
    let output = match kind {
        VcsKind::Jj => run(
            "jj",
            &repo_root,
            &["diff", "--git", "-r", "@", "--", &target],
            false,
        )?,
        VcsKind::Git => run(
            "git",
            &repo_root,
            &["diff", "HEAD", "-M", "--", &target],
            false,
        )?,
    };
    if !output.trim().is_empty() {
        return Ok(output);
    }
    // A clean tracked file has no parent-side diff either. Only an added or
    // renamed path gets the all-addition synthesis; otherwise the diff pane
    // would turn every clean file into a fake new file.
    let current = status(root);
    if !matches!(
        current.get(rel),
        Some(FileStatus::Added | FileStatus::Renamed)
    ) {
        return Ok(String::new());
    }
    let path = workspace_root.join(rel);
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Ok(format!("Binary file {}\n", rel.display()));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().count().max(1);
    let mut patch = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n",
        rel.display(),
        lines
    );
    for line in text.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    Ok(patch)
}

fn jj_status(root: &Path) -> Result<BTreeMap<PathBuf, FileStatus>, String> {
    let template = "status ++ \"\\t\" ++ path ++ \"\\n\"";
    let text = run("jj", root, &["diff", "-r", "@", "-T", template], false)?;
    let mut files = BTreeMap::new();
    for line in text.lines() {
        let Some((status, path)) = line.split_once('\t') else {
            continue;
        };
        let status = jj_status_of(status).unwrap_or(FileStatus::Unknown);
        files.insert(PathBuf::from(path), status);
    }
    if let Ok(conflicts) = run("jj", root, &["resolve", "--list"], true) {
        for line in conflicts.lines() {
            let path = line.split("  ").next().unwrap_or(line).trim();
            if !path.is_empty() {
                files.insert(PathBuf::from(path), FileStatus::Conflicted);
            }
        }
    }
    Ok(files)
}

fn jj_status_of(value: &str) -> Option<FileStatus> {
    match value.trim() {
        "added" | "copied" => Some(FileStatus::Added),
        "modified" => Some(FileStatus::Modified),
        "deleted" => Some(FileStatus::Deleted),
        "renamed" => Some(FileStatus::Renamed),
        "conflicted" => Some(FileStatus::Conflicted),
        "type-changed" => Some(FileStatus::Modified),
        _ => None,
    }
}

fn git_status(root: &Path) -> Result<BTreeMap<PathBuf, FileStatus>, String> {
    let text = run(
        "git",
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        false,
    )?;
    let mut files = BTreeMap::new();
    let entries: Vec<&[u8]> = text.as_bytes().split(|byte| *byte == 0).collect();
    let mut index = 0;
    while index < entries.len() {
        let entry = entries[index];
        index += 1;
        if entry.len() < 4 {
            continue;
        }
        let x = entry[0] as char;
        let y = entry[1] as char;
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let status = if x == 'R' || y == 'R' || x == 'C' || y == 'C' {
            index += 1;
            FileStatus::Renamed
        } else if x == 'D' || y == 'D' {
            FileStatus::Deleted
        } else if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
            FileStatus::Conflicted
        } else if x == 'A' || y == 'A' || (x == '?' && y == '?') {
            FileStatus::Added
        } else {
            FileStatus::Modified
        };
        files.insert(PathBuf::from(path), status);
    }
    Ok(files)
}

fn map_path(path: &Path, workspace_root: &Path, prefix: &Path) -> Option<PathBuf> {
    let path = path.strip_prefix(prefix).ok()?;
    let full = workspace_root.join(path);
    full.strip_prefix(workspace_root)
        .ok()
        .map(Path::to_path_buf)
}

fn run(program: &str, root: &Path, args: &[&str], allow_failure: bool) -> Result<String, String> {
    let mut child = Command::new(program)
        .current_dir(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{program} could not start: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() && !allow_failure {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_vocabulary_is_stable() {
        assert_eq!(FileStatus::Modified.letter(), "M");
        assert_eq!(FileStatus::Added.letter(), "A");
        assert_eq!(FileStatus::Deleted.letter(), "D");
        assert_eq!(FileStatus::Renamed.letter(), "R");
        assert_eq!(FileStatus::Conflicted.letter(), "C");
    }

    #[test]
    fn rollup_prefers_conflict_then_counts() {
        let status = VcsStatus {
            kind: Some(VcsKind::Git),
            files: BTreeMap::from([
                (PathBuf::from("src/a.rs"), FileStatus::Modified),
                (PathBuf::from("src/b.rs"), FileStatus::Conflicted),
                (PathBuf::from("docs/x.md"), FileStatus::Added),
            ]),
            error: None,
        };
        assert_eq!(status.changed_under(Path::new("src")), 2);
        assert_eq!(
            status.worst_under(Path::new("src")),
            Some(FileStatus::Conflicted)
        );
        assert_eq!(status.changed_under(Path::new("")), 3);
    }
    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bsh-vcs-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn git_status_maps_all_shared_states() {
        let root = scratch("git");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "bsh@example.com"]);
        git(&root, &["config", "user.name", "BSH"]);
        std::fs::write(
            root.join("tracked.txt"),
            "one
",
        )
        .expect("write");
        git(&root, &["add", "tracked.txt"]);
        git(&root, &["commit", "-qm", "one"]);
        std::fs::write(
            root.join("tracked.txt"),
            "two
",
        )
        .expect("modify");
        std::fs::write(
            root.join("added.txt"),
            "new
",
        )
        .expect("add");
        std::fs::remove_file(root.join("tracked.txt")).expect("delete?");
        let status = status(&root);
        assert_eq!(status.kind, Some(VcsKind::Git));
        assert_eq!(status.get(Path::new("added.txt")), Some(FileStatus::Added));
        assert_eq!(
            status.get(Path::new("tracked.txt")),
            Some(FileStatus::Deleted)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jj_status_is_preferred_in_a_colocated_checkout() {
        let root = scratch("jj");
        if Command::new("jj").arg("--version").output().is_err() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let init = Command::new("jj")
            .args(["git", "init", "--colocate"])
            .arg(&root)
            .status()
            .expect("jj runs");
        assert!(init.success());
        std::fs::write(
            root.join("a.txt"),
            "one
",
        )
        .expect("write");
        let before = status(&root);
        assert_eq!(before.kind, Some(VcsKind::Jj));
        assert_eq!(before.get(Path::new("a.txt")), Some(FileStatus::Added));
        let commit = Command::new("jj")
            .current_dir(&root)
            .args(["commit", "-m", "one"])
            .status()
            .expect("jj commit");
        assert!(commit.success());
        std::fs::write(root.join("a.txt"), "two\n").expect("modify");
        let after = status(&root);
        assert_eq!(after.get(Path::new("a.txt")), Some(FileStatus::Modified));
        let patch = diff(&root, Path::new("a.txt")).expect("jj diff");
        assert!(patch.contains("+two"), "{patch}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_rename_and_diff_use_the_same_path() {
        let root = scratch("git-rename");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "bsh@example.com"]);
        git(&root, &["config", "user.name", "BSH"]);
        std::fs::write(
            root.join("old.txt"),
            "one
",
        )
        .expect("write");
        git(&root, &["add", "old.txt"]);
        git(&root, &["commit", "-qm", "one"]);
        git(&root, &["mv", "old.txt", "new.txt"]);
        let status = status(&root);
        assert_eq!(status.get(Path::new("new.txt")), Some(FileStatus::Renamed));
        std::fs::write(
            root.join("new.txt"),
            "two
",
        )
        .expect("modify");
        let patch = diff(&root, Path::new("new.txt")).expect("diff");
        assert!(patch.contains("new.txt"), "{patch}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_workspace_maps_repo_paths_to_the_subtree() {
        let repo = scratch("nested");
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "bsh@example.com"]);
        git(&repo, &["config", "user.name", "BSH"]);
        std::fs::create_dir_all(repo.join("crate/src")).expect("dirs");
        std::fs::write(
            repo.join("crate/src/a.rs"),
            "fn a() {}
",
        )
        .expect("write");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "one"]);
        std::fs::write(
            repo.join("crate/src/a.rs"),
            "fn a() { 1; }
",
        )
        .expect("modify");
        let status = status(&repo.join("crate"));
        assert_eq!(
            status.get(Path::new("src/a.rs")),
            Some(FileStatus::Modified)
        );
        assert!(status.get(Path::new("crate/src/a.rs")).is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
