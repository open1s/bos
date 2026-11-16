//! Iterative, resource-bounded workspace inventory scanning.

use std::path::PathBuf;

use crate::{
    fs::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceFs},
    ScanBudget, ScanBudgetError, ScanUsage,
};

/// Final state of a workspace inventory scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStatus {
    /// Every reachable directory was enumerated within the configured limits.
    Complete,
    /// Some entries or directories were omitted because of a limit or error.
    Partial,
    /// The shared scan budget was explicitly cancelled.
    Cancelled,
    /// The root directory could not be enumerated.
    Failed,
}

/// A diagnostic explaining why part of a workspace was not inventoried.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanDiagnostic {
    /// Workspace-relative path associated with the diagnostic.
    pub path: PathBuf,
    /// Cause of the incomplete scan.
    pub kind: ScanDiagnosticKind,
}

/// A bounded scanner diagnostic category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanDiagnosticKind {
    /// A shared resource limit or cancellation check stopped progress.
    Budget(ScanBudgetError),
    /// A filesystem operation failed.
    Filesystem(String),
}

/// Result of an inventory-only workspace scan.
///
/// The scanner records paths and no-follow file metadata. It does not read file
/// contents, parse source code, execute project commands, or follow symlinks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceScanReport {
    /// Accepted files and directories, sorted by workspace-relative path.
    pub entries: Vec<WorkspaceEntry>,
    /// Number of directories successfully enumerated.
    pub directories_scanned: u64,
    /// Completion state.
    pub status: ScanStatus,
    /// Reasons for partial or failed coverage.
    pub diagnostics: Vec<ScanDiagnostic>,
    /// Shared budget usage at the end of the scan.
    pub usage: ScanUsage,
}

/// Scan a workspace using an explicit stack and one shared resource budget.
///
/// The function is synchronous and performs inventory only. Hosts should run
/// it on a bounded worker rather than from latency-sensitive synchronous tool
/// callbacks. The supplied budget should be dedicated to this scan; clones may
/// be used for cancellation, but concurrent scanners sharing one budget are
/// not supported by this sequential implementation.
pub fn scan_workspace(
    workspace: &WorkspaceFs,
    budget: &ScanBudget,
) -> WorkspaceScanReport {
    let mut pending = vec![(PathBuf::new(), 0usize)];
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let mut directories_scanned = 0u64;
    let mut status = ScanStatus::Complete;

    while let Some((directory, depth)) = pending.pop() {
        if let Err(error) = budget.check() {
            record_budget_stop(&mut status, &mut diagnostics, directory, error);
            break;
        }

        let remaining = budget.remaining_entries();
        let max_entries = usize::try_from(remaining).unwrap_or(usize::MAX);
        let listing = match workspace.read_dir_report(&directory, max_entries) {
            Ok(listing) => listing,
            Err(error) => {
                diagnostics.push(ScanDiagnostic {
                    path: directory.clone(),
                    kind: ScanDiagnosticKind::Filesystem(error.to_string()),
                });
                status = if directories_scanned == 0 {
                    ScanStatus::Failed
                } else {
                    ScanStatus::Partial
                };
                break;
            }
        };
        if let Err(error) = budget.check() {
            record_budget_stop(&mut status, &mut diagnostics, directory, error);
            break;
        }
        directories_scanned = directories_scanned.saturating_add(1);

        // Count all inspected entries, including omitted symlinks and special
        // files. This scanner is sequential, so the earlier remaining_entries
        // snapshot cannot be consumed by another scanner between these calls.
        let mut accounting_failed = false;
        for _ in 0..listing.inspected_entries {
            if let Err(error) = budget.record_entry(depth) {
                record_budget_stop(
                    &mut status,
                    &mut diagnostics,
                    directory.clone(),
                    error,
                );
                accounting_failed = true;
                break;
            }
        }
        if accounting_failed {
            break;
        }

        let mut file_limit_reported = false;
        for entry in listing.entries {
            if let Err(error) = budget.check() {
                record_budget_stop(
                    &mut status,
                    &mut diagnostics,
                    directory.clone(),
                    error,
                );
                accounting_failed = true;
                break;
            }

            match entry.kind {
                WorkspaceEntryKind::File => {
                    if file_limit_reported {
                        continue;
                    }
                    if let Err(error) = budget.record_file() {
                        record_budget_stop(
                            &mut status,
                            &mut diagnostics,
                            entry.relative_path,
                            error,
                        );
                        if error == ScanBudgetError::Cancelled
                            || error == ScanBudgetError::DeadlineExceeded
                        {
                            accounting_failed = true;
                            break;
                        }
                        file_limit_reported = error == ScanBudgetError::FileLimitExceeded;
                        continue;
                    }
                    entries.push(entry);
                }
                WorkspaceEntryKind::Directory => {
                    let child_depth = depth.saturating_add(1);
                    entries.push(entry.clone());
                    if child_depth > budget.limits().max_depth {
                        record_budget_stop(
                            &mut status,
                            &mut diagnostics,
                            entry.relative_path,
                            ScanBudgetError::DepthLimitExceeded,
                        );
                    } else {
                        pending.push((entry.relative_path, child_depth));
                    }
                }
            }
        }
        if accounting_failed {
            break;
        }

        if listing.truncated {
            record_budget_stop(
                &mut status,
                &mut diagnostics,
                directory,
                ScanBudgetError::EntryLimitExceeded,
            );
            break;
        }
    }

    entries.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));

    WorkspaceScanReport {
        entries,
        directories_scanned,
        status,
        diagnostics,
        usage: budget.usage(),
    }
}

fn record_budget_stop(
    status: &mut ScanStatus,
    diagnostics: &mut Vec<ScanDiagnostic>,
    path: PathBuf,
    error: ScanBudgetError,
) {
    if error == ScanBudgetError::Cancelled {
        *status = ScanStatus::Cancelled;
    } else if *status == ScanStatus::Complete {
        *status = ScanStatus::Partial;
    }
    diagnostics.push(ScanDiagnostic {
        path,
        kind: ScanDiagnosticKind::Budget(error),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScanLimits, WorkspaceDescriptor, WorkspaceId};
    use std::{fs, time::Duration};
    use tempfile::tempdir;

    fn limits() -> ScanLimits {
        ScanLimits {
            max_entries: 100,
            max_files: 100,
            max_total_bytes: 1024,
            max_file_bytes: 1024,
            max_depth: 8,
            max_duration: Duration::from_secs(10),
        }
    }

    fn scan(root: &std::path::Path, limits: ScanLimits) -> WorkspaceScanReport {
        let descriptor = WorkspaceDescriptor::new(WorkspaceId::new(), root)
            .expect("create workspace descriptor");
        let fs = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits);
        scan_workspace(&fs, &budget)
    }

    #[test]
    fn inventories_nested_files_in_deterministic_order() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::create_dir(root.path().join("src/nested")).expect("create nested");
        fs::write(root.path().join("z.rs"), "z").expect("write z");
        fs::write(root.path().join("src/lib.rs"), "lib").expect("write lib");
        fs::write(root.path().join("src/nested/mod.rs"), "mod").expect("write mod");

        let report = scan(root.path(), limits());

        assert_eq!(report.status, ScanStatus::Complete);
        assert_eq!(report.directories_scanned, 3);
        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.relative_path.as_path())
                .collect::<Vec<_>>(),
            vec![
                std::path::Path::new("src"),
                std::path::Path::new("src/lib.rs"),
                std::path::Path::new("src/nested"),
                std::path::Path::new("src/nested/mod.rs"),
                std::path::Path::new("z.rs"),
            ]
        );
        assert_eq!(report.usage.files, 3);
        assert_eq!(report.usage.total_bytes, 0);
    }

    #[test]
    fn stops_at_depth_limit_and_reports_partial_coverage() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::create_dir(root.path().join("src/nested")).expect("create nested");
        fs::write(root.path().join("src/nested/file.rs"), "x").expect("write file");

        let report = scan(
            root.path(),
            ScanLimits {
                max_depth: 1,
                ..limits()
            },
        );

        assert_eq!(report.status, ScanStatus::Partial);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == ScanDiagnosticKind::Budget(ScanBudgetError::DepthLimitExceeded)
        }));
        assert!(!report.entries.iter().any(|entry| {
            entry.relative_path == std::path::Path::new("src/nested/file.rs")
        }));
    }

    #[test]
    fn enforces_global_file_count_limit() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("a.rs"), "a").expect("write a");
        fs::write(root.path().join("b.rs"), "b").expect("write b");

        let report = scan(
            root.path(),
            ScanLimits {
                max_files: 1,
                ..limits()
            },
        );

        assert_eq!(report.status, ScanStatus::Partial);
        assert_eq!(report.usage.files, 1);
        assert_eq!(
            report
                .entries
                .iter()
                .filter(|entry| entry.kind == WorkspaceEntryKind::File)
                .count(),
            1
        );
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == ScanDiagnosticKind::Budget(ScanBudgetError::FileLimitExceeded)
        }));
    }

    #[test]
    fn enforces_global_entry_limit_without_recursing_unboundedly() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("a"), "a").expect("write a");
        fs::write(root.path().join("b"), "b").expect("write b");

        let report = scan(
            root.path(),
            ScanLimits {
                max_entries: 1,
                ..limits()
            },
        );

        assert_eq!(report.status, ScanStatus::Partial);
        assert_eq!(report.usage.entries, 1);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == ScanDiagnosticKind::Budget(ScanBudgetError::EntryLimitExceeded)
        }));
    }

    #[test]
    fn cancellation_is_reported_without_panicking() {
        let root = tempdir().expect("create workspace");
        let descriptor = WorkspaceDescriptor::new(WorkspaceId::new(), root.path())
            .expect("create descriptor");
        let fs = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());
        budget.cancel();

        let report = scan_workspace(&fs, &budget);

        assert_eq!(report.status, ScanStatus::Cancelled);
        assert_eq!(report.usage, ScanUsage::default());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_consume_entry_budget_but_are_not_returned() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("target"), "x").expect("write target");
        symlink(root.path().join("target"), root.path().join("link"))
            .expect("create symlink");

        let report = scan(root.path(), limits());

        assert_eq!(report.status, ScanStatus::Complete);
        assert_eq!(report.usage.entries, 2);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].relative_path,
            std::path::Path::new("target")
        );
    }
}
