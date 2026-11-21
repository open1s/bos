//! Bounded source loading and syntax-fact extraction pipeline.
//!
//! Connects inventory, deterministic classification, capability-based source
//! reads, shared byte reservations, and language providers. This operation is
//! synchronous; embedding hosts must invoke it on a bounded worker.

use std::path::PathBuf;

use crate::{
    IgnorePolicy, ScanBudget, ScanBudgetError,
    classification::{FileCategory, LanguageId, classify_entry},
    fs::{WorkspaceEntryKind, WorkspaceFs},
    scanner::{ScanStatus, WorkspaceScanReport, scan_workspace_with_policy},
    syntax::{LanguageProvider, ParseControl, ParseInput, ParseLimits, ParseResult, ProviderError},
};

/// Outcome of processing one source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFileStatus {
    /// Parsed without syntax errors.
    Parsed,
    /// Parser recovered from malformed syntax and returned partial facts.
    Recovered,
    /// No installed provider supports the classified language.
    Unsupported,
    /// Parsing stopped at a configured per-file output limit.
    Limited,
    /// The file could not be safely read or parsed.
    Failed,
}

/// Why source processing did not produce a complete parse result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceIndexDiagnosticKind {
    /// The path was not classified as a source file.
    NotSource,
    /// No provider is registered for this language.
    UnsupportedLanguage(LanguageId),
    /// Inventory metadata did not provide a safe size estimate.
    MissingSize,
    /// A shared resource budget denied the read.
    Budget(ScanBudgetError),
    /// A confined filesystem read failed.
    Filesystem(String),
    /// The provider rejected the input.
    Provider(String),
}

/// A source-processing diagnostic. Messages must not include source contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIndexDiagnostic {
    /// Workspace-relative file path.
    pub path: PathBuf,
    /// Failure or omission category.
    pub kind: SourceIndexDiagnosticKind,
}

/// Overall source-index coverage state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceIndexStatus {
    /// Every supported source file was parsed without recovery.
    Complete,
    /// Some source files were unsupported, failed, or required parser recovery.
    Partial,
    /// The shared budget cancelled the operation.
    Cancelled,
    /// The workspace inventory itself failed.
    Failed,
}

/// Per-file processing result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFileResult {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// Classified language, if known.
    pub language: Option<LanguageId>,
    /// Processing outcome.
    pub status: SourceFileStatus,
    /// Syntax facts and diagnostics when parsing produced a result.
    pub parse: Option<ParseResult>,
}

/// Result of one inventory and source parsing pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIndexReport {
    /// Overall source-index coverage state.
    pub status: SourceIndexStatus,
    /// Original inventory report, including completeness and diagnostics.
    pub scan: WorkspaceScanReport,
    /// Per-source-file results, ordered by workspace-relative path.
    pub files: Vec<SourceFileResult>,
    /// Source-stage diagnostics.
    pub diagnostics: Vec<SourceIndexDiagnostic>,
    /// Successfully read and parsed files, including recovered parses.
    pub parsed_files: u64,
    /// Source files without a matching provider.
    pub unsupported_files: u64,
    /// Files parsed with error recovery.
    pub recovered_files: u64,
    /// Files whose extraction reached a configured parser output limit.
    pub limited_files: u64,
    /// Files that failed during source loading or parsing.
    pub failed_files: u64,
}

/// Scan a workspace and parse source files using the supplied providers.
///
/// Every file is read through WorkspaceFs. Its no-follow metadata size is
/// reserved against the shared aggregate byte budget before reading. The read
/// limit is the reserved size, so a file that grows between enumeration and
/// reading is rejected rather than silently exceeding the reservation.
///
/// This operation is synchronous and sequential. Hosts should run it on a
/// bounded background worker. The Rust provider checks cancellation and the
/// shared deadline through Tree-sitter's parse-progress callback and during
/// syntax-tree projection. Providers that rely on the default trait method
/// are only checked before and after their parse call. Per-file node, fact,
/// and diagnostic output limits are enforced by providers that implement
/// controlled parsing. No project commands, build scripts, compilers, or
/// package managers are executed.
pub fn index_workspace(
    workspace: &WorkspaceFs,
    budget: &ScanBudget,
    ignore: &IgnorePolicy,
    providers: &[&dyn LanguageProvider],
) -> SourceIndexReport {
    index_workspace_with_limits(
        workspace,
        budget,
        ignore,
        providers,
        &ParseLimits::default(),
    )
}

/// Index a workspace with explicit per-file syntax extraction limits.
pub fn index_workspace_with_limits(
    workspace: &WorkspaceFs,
    budget: &ScanBudget,
    ignore: &IgnorePolicy,
    providers: &[&dyn LanguageProvider],
    parse_limits: &ParseLimits,
) -> SourceIndexReport {
    let scan = scan_workspace_with_policy(workspace, budget, ignore);
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    let mut parsed_files = 0u64;
    let mut unsupported_files = 0u64;
    let mut recovered_files = 0u64;
    let mut limited_files = 0u64;
    let mut failed_files = 0u64;

    for entry in scan
        .entries
        .iter()
        .filter(|entry| entry.kind == WorkspaceEntryKind::File)
    {
        if let Err(error) = budget.check() {
            diagnostics.push(SourceIndexDiagnostic {
                path: entry.relative_path.clone(),
                kind: SourceIndexDiagnosticKind::Budget(error),
            });
            failed_files = failed_files.saturating_add(1);
            break;
        }

        let Some(classification) = classify_entry(entry) else {
            continue;
        };
        if classification.category != FileCategory::Source {
            continue;
        }
        let Some(language) = classification.language else {
            diagnostics.push(SourceIndexDiagnostic {
                path: entry.relative_path.clone(),
                kind: SourceIndexDiagnosticKind::NotSource,
            });
            failed_files = failed_files.saturating_add(1);
            files.push(SourceFileResult {
                path: entry.relative_path.clone(),
                language: None,
                status: SourceFileStatus::Failed,
                parse: None,
            });
            continue;
        };

        let Some(provider) = providers
            .iter()
            .copied()
            .find(|provider| provider.language() == language)
        else {
            diagnostics.push(SourceIndexDiagnostic {
                path: entry.relative_path.clone(),
                kind: SourceIndexDiagnosticKind::UnsupportedLanguage(language),
            });
            unsupported_files = unsupported_files.saturating_add(1);
            files.push(SourceFileResult {
                path: entry.relative_path.clone(),
                language: Some(language),
                status: SourceFileStatus::Unsupported,
                parse: None,
            });
            continue;
        };

        let Some(size_bytes) = entry.size_bytes else {
            diagnostics.push(SourceIndexDiagnostic {
                path: entry.relative_path.clone(),
                kind: SourceIndexDiagnosticKind::MissingSize,
            });
            failed_files = failed_files.saturating_add(1);
            files.push(SourceFileResult {
                path: entry.relative_path.clone(),
                language: Some(language),
                status: SourceFileStatus::Failed,
                parse: None,
            });
            continue;
        };

        let max_bytes = match usize::try_from(size_bytes) {
            Ok(value) => value,
            Err(_) => {
                diagnostics.push(SourceIndexDiagnostic {
                    path: entry.relative_path.clone(),
                    kind: SourceIndexDiagnosticKind::Budget(ScanBudgetError::FileTooLarge),
                });
                failed_files = failed_files.saturating_add(1);
                files.push(SourceFileResult {
                    path: entry.relative_path.clone(),
                    language: Some(language),
                    status: SourceFileStatus::Failed,
                    parse: None,
                });
                continue;
            }
        };

        if let Err(error) = budget.reserve_file_bytes(size_bytes) {
            diagnostics.push(SourceIndexDiagnostic {
                path: entry.relative_path.clone(),
                kind: SourceIndexDiagnosticKind::Budget(error),
            });
            failed_files = failed_files.saturating_add(1);
            files.push(SourceFileResult {
                path: entry.relative_path.clone(),
                language: Some(language),
                status: SourceFileStatus::Failed,
                parse: None,
            });
            if matches!(
                error,
                ScanBudgetError::Cancelled | ScanBudgetError::DeadlineExceeded
            ) {
                break;
            }
            continue;
        }

        let source = match workspace.read_file(&entry.relative_path, max_bytes) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(SourceIndexDiagnostic {
                    path: entry.relative_path.clone(),
                    kind: SourceIndexDiagnosticKind::Filesystem(error.to_string()),
                });
                failed_files = failed_files.saturating_add(1);
                files.push(SourceFileResult {
                    path: entry.relative_path.clone(),
                    language: Some(language),
                    status: SourceFileStatus::Failed,
                    parse: None,
                });
                continue;
            }
        };

        match provider.parse_with_control(
            ParseInput {
                workspace_id: workspace.workspace_id(),
                relative_path: &entry.relative_path,
                source: &source,
            },
            &ParseControl::new(budget, parse_limits),
        ) {
            Ok(parse) => {
                let status = match parse.status {
                    crate::ParseStatus::Complete => SourceFileStatus::Parsed,
                    crate::ParseStatus::Recovered => SourceFileStatus::Recovered,
                    crate::ParseStatus::Limited => SourceFileStatus::Limited,
                    crate::ParseStatus::Failed => SourceFileStatus::Failed,
                };
                if status == SourceFileStatus::Failed {
                    failed_files = failed_files.saturating_add(1);
                } else {
                    parsed_files = parsed_files.saturating_add(1);
                    if status == SourceFileStatus::Recovered {
                        recovered_files = recovered_files.saturating_add(1);
                    }
                    if status == SourceFileStatus::Limited {
                        limited_files = limited_files.saturating_add(1);
                    }
                }
                files.push(SourceFileResult {
                    path: entry.relative_path.clone(),
                    language: Some(language),
                    status,
                    parse: Some(parse),
                });
            }
            Err(error) => {
                let budget_error = match &error {
                    ProviderError::Budget(error) => Some(*error),
                    _ => None,
                };
                diagnostics.push(SourceIndexDiagnostic {
                    path: entry.relative_path.clone(),
                    kind: if let Some(error) = budget_error {
                        SourceIndexDiagnosticKind::Budget(error)
                    } else {
                        SourceIndexDiagnosticKind::Provider(error.to_string())
                    },
                });
                failed_files = failed_files.saturating_add(1);
                files.push(SourceFileResult {
                    path: entry.relative_path.clone(),
                    language: Some(language),
                    status: SourceFileStatus::Failed,
                    parse: None,
                });
                if matches!(
                    budget_error,
                    Some(ScanBudgetError::Cancelled | ScanBudgetError::DeadlineExceeded)
                ) {
                    break;
                }
            }
        }
    }

    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    diagnostics.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    let was_cancelled = scan.status == ScanStatus::Cancelled
        || diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == SourceIndexDiagnosticKind::Budget(ScanBudgetError::Cancelled)
        });
    let status = if was_cancelled {
        SourceIndexStatus::Cancelled
    } else {
        match scan.status {
            ScanStatus::Cancelled => SourceIndexStatus::Cancelled,
            ScanStatus::Failed => SourceIndexStatus::Failed,
            ScanStatus::Partial => SourceIndexStatus::Partial,
            ScanStatus::Complete
                if failed_files > 0
                    || unsupported_files > 0
                    || recovered_files > 0
                    || limited_files > 0 =>
            {
                SourceIndexStatus::Partial
            }
            ScanStatus::Complete => SourceIndexStatus::Complete,
        }
    };

    SourceIndexReport {
        status,
        scan,
        files,
        diagnostics,
        parsed_files,
        unsupported_files,
        recovered_files,
        limited_files,
        failed_files,
    }
}

/// Convenience entry point for the initial Rust-only provider set.
pub fn index_rust_workspace(
    workspace: &WorkspaceFs,
    budget: &ScanBudget,
    ignore: &IgnorePolicy,
) -> SourceIndexReport {
    let rust = crate::RustProvider;
    index_workspace(workspace, budget, ignore, &[&rust])
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

    #[test]
    fn scans_reads_and_parses_rust_sources_with_workspace_identity() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        let source = "pub fn answer() -> u8 { 42 }";
        fs::write(root.path().join("src/lib.rs"), source).expect("write Rust source");
        fs::write(root.path().join("README.md"), "# Example").expect("write documentation");

        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());
        let report = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(report.scan.status, ScanStatus::Complete);
        assert_eq!(report.status, SourceIndexStatus::Complete);
        assert_eq!(report.parsed_files, 1);
        assert_eq!(report.unsupported_files, 0);
        assert_eq!(report.failed_files, 0);
        assert_eq!(report.files.len(), 1);
        let parse = report.files[0].parse.as_ref().expect("parse result");
        assert_eq!(parse.workspace_id, workspace.workspace_id());
        assert_eq!(parse.relative_path, PathBuf::from("src/lib.rs"));
        assert!(parse.facts.iter().any(|fact| {
            fact.kind == crate::SyntaxFactKind::Function && fact.name.as_deref() == Some("answer")
        }));
        // The scanner accounts every accepted inventory file, including the
        // non-source README; only the Rust source is read and byte-reserved.
        assert_eq!(budget.usage().files, 2);
        assert_eq!(budget.usage().total_bytes, source.len() as u64);
    }

    #[test]
    fn preserves_explicit_cancellation_in_overall_index_status() {
        let root = tempdir().expect("create workspace");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());
        budget.cancel();

        let report = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(report.status, SourceIndexStatus::Cancelled);
        assert_eq!(report.scan.status, ScanStatus::Cancelled);
    }

    #[test]
    fn reports_unsupported_languages_without_dropping_inventory() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("main.py"), "print('hello')").expect("write Python source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());

        let report = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(report.scan.entries.len(), 1);
        assert_eq!(report.unsupported_files, 1);
        assert_eq!(report.status, SourceIndexStatus::Partial);
        assert_eq!(report.files[0].status, SourceFileStatus::Unsupported);
        assert_eq!(budget.usage().total_bytes, 0);
    }

    #[test]
    fn refuses_to_read_files_over_per_file_or_aggregate_byte_limits() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("main.rs"), "fn main() {}").expect("write Rust source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(ScanLimits {
            max_total_bytes: 4,
            max_file_bytes: 4,
            ..limits()
        });

        let report = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(report.failed_files, 1);
        assert!(matches!(
            report.diagnostics[0].kind,
            SourceIndexDiagnosticKind::Budget(ScanBudgetError::FileTooLarge)
        ));
        assert_eq!(budget.usage().total_bytes, 0);
    }

    #[test]
    fn applies_per_file_fact_limits_and_marks_coverage_partial() {
        let root = tempdir().expect("create workspace");
        fs::write(
            root.path().join("main.rs"),
            "fn first() {} fn second() {} fn third() {}",
        )
        .expect("write Rust source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());
        let parse_limits = ParseLimits {
            max_nodes: 1000,
            max_facts: 1,
            max_diagnostics: 10,
        };

        let report = index_workspace_with_limits(
            &workspace,
            &budget,
            &IgnorePolicy::default(),
            &[&crate::RustProvider],
            &parse_limits,
        );

        assert_eq!(report.status, SourceIndexStatus::Partial);
        assert_eq!(report.limited_files, 1);
        assert_eq!(report.files[0].status, SourceFileStatus::Limited);
        let parse = report.files[0].parse.as_ref().expect("limited parse");
        assert_eq!(parse.facts.len(), 1);
        assert_eq!(parse.limit_reason, Some(crate::ParseLimitKind::Facts));
    }

    #[test]
    fn malformed_source_keeps_recovered_facts_and_is_counted_as_parsed() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("main.rs"), "fn good() {} fn broken( {")
            .expect("write malformed Rust source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(limits());

        let report = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(report.parsed_files, 1);
        assert_eq!(report.recovered_files, 1);
        assert_eq!(report.status, SourceIndexStatus::Partial);
        assert_eq!(report.files[0].status, SourceFileStatus::Recovered);
        assert!(
            report.files[0]
                .parse
                .as_ref()
                .is_some_and(|parse| !parse.diagnostics.is_empty())
        );
    }
}
