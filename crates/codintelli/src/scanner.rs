//! Iterative, resource-bounded workspace inventory scanning.

use std::path::PathBuf;

use crate::{
    fs::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceFs},
    IgnorePolicy, ScanBudget, ScanBudgetError, ScanUsage,
};

/// Final state of a workspace inventory scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStatus {
    /// Every directory not pruned by the ignore policy was enumerated within limits.
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
    /// Number of entries excluded by the ignore policy.
    pub ignored_entries: u64,
    /// Number of ignored directories pruned without traversal.
    pub directories_pruned: u64,
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
    scan_workspace_with_policy(workspace, budget, &IgnorePolicy::default())
}

/// Scan a workspace using an explicit ignore policy and shared resource budget.
///
/// The policy is applied before accepted-file accounting. Ignored directory
/// contents are not enumerated unless a later include rule may match below them.
///
/// Later matching policy rules override earlier rules. Ignored entries still
/// consume the directory-entry budget because they must be enumerated to apply
/// the policy. Excluded files do not consume the accepted-file budget.
pub fn scan_workspace_with_policy(
    workspace: &WorkspaceFs,
    budget: &ScanBudget,
    ignore: &IgnorePolicy,
) -> WorkspaceScanReport {
    let mut pending = vec![(PathBuf::new(), 0usize)];
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let mut directories_scanned = 0u64;
    let mut ignored_entries = 0u64;
    let mut directories_pruned = 0u64;
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

            if ignore.is_ignored(&entry.relative_path, entry.kind) {
                ignored_entries = ignored_entries.saturating_add(1);
                if entry.kind == WorkspaceEntryKind::Directory {
                    if ignore.should_descend(&entry.relative_path) {
                        let child_depth = depth.saturating_add(1);
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
                    } else {
                        directories_pruned = directories_pruned.saturating_add(1);
                    }
                }
                continue;
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
        ignored_entries,
        directories_pruned,
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
    use crate::{IgnoreRule, IgnorePolicy, ScanLimits, WorkspaceDescriptor, WorkspaceId};
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
        scan_with_policy(root, limits, &IgnorePolicy::default())
    }

    fn scan_with_policy(
        root: &std::path::Path,
        limits: ScanLimits,
        ignore: &IgnorePolicy,
    ) -> WorkspaceScanReport {
        let descriptor = WorkspaceDescriptor::new(WorkspaceId::new(), root)
            .expect("create workspace descriptor");
        let fs = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits);
        scan_workspace_with_policy(&fs, &budget, ignore)
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

    #[test]
    fn deadline_is_reported_even_when_directory_is_empty() {
        let root = tempdir().expect("create workspace");
        let report = scan(
            root.path(),
            ScanLimits {
                max_duration: Duration::ZERO,
                ..limits()
            },
        );

        assert_eq!(report.status, ScanStatus::Partial);
        assert_eq!(report.directories_scanned, 0);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == ScanDiagnosticKind::Budget(ScanBudgetError::DeadlineExceeded)
        }));
    }

    #[test]
    fn file_limit_produces_one_diagnostic_for_the_directory() {
        let root = tempdir().expect("create workspace");
        for name in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            fs::write(root.path().join(name), "x").expect("write source");
        }

        let report = scan(
            root.path(),
            ScanLimits {
                max_files: 1,
                ..limits()
            },
        );

        assert_eq!(report.status, ScanStatus::Partial);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.kind
                        == ScanDiagnosticKind::Budget(ScanBudgetError::FileLimitExceeded)
                })
                .count(),
            1
        );
    }


    #[test]
    fn default_policy_prunes_generated_directories_but_counts_their_entries() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::create_dir(root.path().join("target")).expect("create target");
        fs::create_dir(root.path().join("node_modules")).expect("create node_modules");
        fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("write source");
        fs::write(root.path().join("target/generated.rs"), "generated").expect("write generated");
        fs::write(root.path().join("node_modules/pkg.js"), "package").expect("write package");

        let report = scan(root.path(), limits());

        assert_eq!(report.status, ScanStatus::Complete);
        assert_eq!(report.ignored_entries, 2);
        assert_eq!(report.directories_pruned, 2);
        assert_eq!(report.usage.files, 1);
        assert_eq!(report.usage.entries, 4);
        assert_eq!(report.entries.len(), 2);
        assert!(report.entries.iter().any(|entry| {
            entry.relative_path == std::path::Path::new("src/main.rs")
        }));
        assert!(!report.entries.iter().any(|entry| {
            entry.relative_path.starts_with("target")
                || entry.relative_path.starts_with("node_modules")
        }));
    }

    #[test]
    fn include_rule_traverses_ignored_parent_only_to_reinclude_target() {
        let root = tempdir().expect("create workspace");
        fs::create_dir_all(root.path().join("target/keep")).expect("create keep");
        fs::create_dir_all(root.path().join("target/drop")).expect("create drop");
        fs::write(root.path().join("target/keep/main.rs"), "source").expect("write kept source");
        fs::write(root.path().join("target/drop/generated.rs"), "generated")
            .expect("write ignored source");

        let mut ignore = IgnorePolicy::default();
        ignore.push(IgnoreRule::include_path("target/keep/main.rs").unwrap());

        let report = scan_with_policy(root.path(), limits(), &ignore);

        assert_eq!(report.status, ScanStatus::Complete);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].relative_path,
            std::path::Path::new("target/keep/main.rs")
        );
        assert_eq!(report.ignored_entries, 3);
        assert_eq!(report.directories_pruned, 1);
        assert_eq!(report.usage.files, 1);
    }

    #[test]
    fn extension_exclusions_do_not_consume_accepted_file_budget() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("main.rs"), "source").expect("write source");
        fs::write(root.path().join("debug.log"), "log").expect("write log");

        let mut ignore = IgnorePolicy::default();
        ignore.push(IgnoreRule::exclude_extension("log").unwrap());

        let report = scan_with_policy(root.path(), limits(), &ignore);

        assert_eq!(report.status, ScanStatus::Complete);
        assert_eq!(report.ignored_entries, 1);
        assert_eq!(report.usage.files, 1);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(
            report.entries[0].relative_path,
            std::path::Path::new("main.rs")
        );
    }

}
