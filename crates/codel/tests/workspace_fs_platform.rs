//! Cross-platform regression tests for workspace filesystem boundaries.
//!
//! Windows directory symlinks are reparse points. These tests exercise the
//! same no-follow contract on Windows and Unix; on Windows hosts without
//! permission to create symlinks, the affected test is skipped.

use std::path::Path;

use codel::{WorkspaceDescriptor, WorkspaceEntryKind, WorkspaceFs, WorkspaceId};
use tempfile::tempdir;

fn workspace_fs(root: &Path) -> WorkspaceFs {
    let descriptor =
        WorkspaceDescriptor::new(WorkspaceId::new(), root).expect("create workspace descriptor");
    WorkspaceFs::open(&descriptor).expect("open workspace capability")
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[test]
fn directory_listing_never_returns_file_symlinks() {
    let root = tempdir().expect("create workspace");
    std::fs::write(root.path().join("target.rs"), "source").expect("write target");

    if create_file_symlink(Path::new("target.rs"), &root.path().join("alias.rs")).is_err() {
        // Windows may require Developer Mode or an elevated test runner.
        #[cfg(windows)]
        return;
        #[cfg(unix)]
        panic!("failed to create file symlink");
    }

    let workspace = workspace_fs(root.path());
    let entries = workspace.read_dir("", 16).expect("enumerate workspace");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].relative_path, Path::new("target.rs"));
    assert_eq!(entries[0].kind, WorkspaceEntryKind::File);
}

#[test]
fn directory_symlinks_cannot_be_used_as_traversal_roots() {
    let root = tempdir().expect("create workspace");
    let outside = tempdir().expect("create outside directory");
    std::fs::write(outside.path().join("secret.rs"), "outside").expect("write outside file");

    if create_dir_symlink(outside.path(), &root.path().join("linked")).is_err() {
        // Windows may require Developer Mode or an elevated test runner.
        #[cfg(windows)]
        return;
        #[cfg(unix)]
        panic!("failed to create directory symlink");
    }

    let workspace = workspace_fs(root.path());
    assert!(workspace.read_dir("linked", 16).is_err());
    assert!(workspace.read_file("linked/secret.rs", 1024).is_err());
}

#[test]
fn absolute_workspace_paths_are_rejected_on_the_host_platform() {
    let root = tempdir().expect("create workspace");
    let workspace = workspace_fs(root.path());

    assert!(workspace.read_dir(root.path(), 16).is_err());
    assert!(
        workspace
            .read_file(root.path().join("outside"), 1024)
            .is_err()
    );
}
