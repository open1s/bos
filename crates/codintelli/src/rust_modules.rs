//! Conservative Rust module-file candidate mapping.
//!
//! This module uses only parsed module facts and the caller's workspace file
//! inventory. It never reads files, invokes Cargo/rustc, or assumes a crate
//! configuration. Results are path candidates, not semantic module edges.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{
    classification::LanguageId, syntax::valid_relative_path, ParseResult, SourceRange, WorkspaceId,
};

/// State of a syntactic Rust module's file-path lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustModuleResolutionStatus {
    /// Exactly one conventional path exists in the supplied inventory.
    UniqueCandidate,
    /// Both conventional paths exist; neither is selected.
    AmbiguousCandidates,
    /// No conventional path exists in the supplied inventory.
    NoCandidate,
    /// Outer attributes are present; cfg/path semantics are not interpreted.
    SkippedAttributedDeclaration,
}

/// Candidate paths for one external Rust module declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleResolution {
    /// Workspace-relative source file containing the declaration.
    pub source_path: PathBuf,
    /// Lexical inline-module scope, outermost first.
    pub scope_path: Vec<String>,
    /// Declared module name.
    pub module_name: String,
    /// Source evidence for the declaration.
    pub range: SourceRange,
    /// Conventional paths considered, whether or not they exist.
    pub expected_paths: Vec<PathBuf>,
    /// Expected paths present in the supplied workspace inventory.
    pub existing_candidates: Vec<PathBuf>,
    /// Candidate outcome; never a compiler-level resolution claim.
    pub status: RustModuleResolutionStatus,
}

/// Deterministic results for one workspace's Rust module declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleResolutionReport {
    /// Workspace identity shared by all parsed files.
    pub workspace_id: WorkspaceId,
    /// Results ordered by source path and source range.
    pub modules: Vec<RustModuleResolution>,
}

/// Invalid input to Rust module candidate mapping.
#[derive(Debug, thiserror::Error)]
pub enum RustModuleResolutionError {
    /// At least one parsed file is required to establish workspace identity.
    #[error("at least one parse result is required")]
    EmptyInput,
    /// Parse results belong to more than one workspace.
    #[error("parse results contain mixed workspace identities")]
    MixedWorkspace,
    /// Candidate mapping is scoped to exactly one crate root at a time.
    #[error("module candidate mapping accepts at most one crate root")]
    MultipleCrateRoots,
    /// A supplied path is not a normal workspace-relative path.
    #[error("module resolver paths must be normal workspace-relative paths")]
    InvalidPath,
}

/// Build conservative file-path candidates for external Rust module declarations.
///
/// `crate_roots` must contain zero or one path supplied by the host from trusted workspace context. Call once per root; a multi-root graph builder performs this scoping automatically.
/// The engine does not infer Cargo targets or execute Cargo metadata commands.
/// For a crate root, child module lookup starts in the root file's parent
/// directory. For `mod.rs`, lookup starts in its parent directory. For other
/// source files, lookup starts in a sibling directory named after the file stem.
///
/// Inline module names in `scope_path` are appended to that base directory.
/// Both `name.rs` and `name/mod.rs` are checked. Attributed declarations are
/// reported as uncertain and are not resolved because `cfg`, `cfg_attr`,
/// `path`, and other attributes require configuration-aware interpretation.
pub fn resolve_rust_module_candidates(
    parse_results: &[ParseResult],
    inventory_paths: &[PathBuf],
    crate_roots: &[PathBuf],
) -> Result<RustModuleResolutionReport, RustModuleResolutionError> {
    let first = parse_results
        .first()
        .ok_or(RustModuleResolutionError::EmptyInput)?;
    let workspace_id = first.workspace_id;
    if crate_roots.len() > 1 {
        return Err(RustModuleResolutionError::MultipleCrateRoots);
    }
    if parse_results
        .iter()
        .any(|result| result.workspace_id != workspace_id)
    {
        return Err(RustModuleResolutionError::MixedWorkspace);
    }

    for path in inventory_paths.iter().chain(crate_roots.iter()) {
        if !valid_relative_path(path) {
            return Err(RustModuleResolutionError::InvalidPath);
        }
    }
    for result in parse_results {
        if !valid_relative_path(&result.relative_path) {
            return Err(RustModuleResolutionError::InvalidPath);
        }
    }

    let inventory: HashSet<PathBuf> = inventory_paths.iter().cloned().collect();
    let crate_roots: HashSet<PathBuf> = crate_roots.iter().cloned().collect();
    let mut modules = Vec::new();

    for result in parse_results
        .iter()
        .filter(|result| result.language == LanguageId::Rust)
    {
        for fact in &result.modules {
            if !valid_module_segment(&fact.name)
                || fact
                    .scope_path
                    .iter()
                    .any(|segment| !valid_module_segment(segment))
            {
                return Err(RustModuleResolutionError::InvalidPath);
            }
            if fact.is_inline {
                continue;
            }
            let base = module_directory(&result.relative_path, &crate_roots)?;
            let mut scoped_base = base;
            for segment in &fact.scope_path {
                if segment.is_empty()
                    || segment == "."
                    || segment == ".."
                    || segment.contains('/')
                    || segment.contains('\\')
                {
                    return Err(RustModuleResolutionError::InvalidPath);
                }
                scoped_base.push(segment);
            }

            let mut file_candidate = scoped_base.clone();
            file_candidate.push(format!("{}.rs", fact.name));
            let mut mod_candidate = scoped_base.clone();
            mod_candidate.push(&fact.name);
            mod_candidate.push("mod.rs");
            let expected_paths = vec![file_candidate, mod_candidate];
            // An attributed declaration is not eligible for conventional lookup:
            // cfg/path semantics are unknown, so an existing file is not evidence
            // that this declaration refers to it.
            let mut existing_candidates: Vec<PathBuf> = if fact.has_resolution_attributes {
                Vec::new()
            } else {
                expected_paths
                    .iter()
                    .filter(|path| inventory.contains(*path))
                    .cloned()
                    .collect()
            };
            existing_candidates.sort();

            let status = if fact.has_resolution_attributes {
                RustModuleResolutionStatus::SkippedAttributedDeclaration
            } else {
                match existing_candidates.len() {
                    0 => RustModuleResolutionStatus::NoCandidate,
                    1 => RustModuleResolutionStatus::UniqueCandidate,
                    _ => RustModuleResolutionStatus::AmbiguousCandidates,
                }
            };
            modules.push(RustModuleResolution {
                source_path: result.relative_path.clone(),
                scope_path: fact.scope_path.clone(),
                module_name: fact.name.clone(),
                range: fact.range,
                expected_paths,
                existing_candidates,
                status,
            });
        }
    }

    modules.sort_unstable_by(|left, right| {
        (&left.source_path, left.range, &left.module_name).cmp(&(
            &right.source_path,
            right.range,
            &right.module_name,
        ))
    });

    Ok(RustModuleResolutionReport {
        workspace_id,
        modules,
    })
}

fn valid_module_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\0'
            )
        })
}

fn module_directory(
    source_path: &Path,
    crate_roots: &HashSet<PathBuf>,
) -> Result<PathBuf, RustModuleResolutionError> {
    let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RustModuleResolutionError::InvalidPath)?;
    if crate_roots.contains(source_path) || file_name == "mod.rs" {
        return Ok(parent.to_path_buf());
    }
    let stem = source_path
        .file_stem()
        .ok_or(RustModuleResolutionError::InvalidPath)?;
    let mut directory = parent.to_path_buf();
    directory.push(stem);
    if !valid_relative_path(&directory) {
        return Err(RustModuleResolutionError::InvalidPath);
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParseStatus, RustModuleFact, SyntaxFact, SyntaxFactKind};

    fn parsed_with_id(
        path: &str,
        modules: Vec<RustModuleFact>,
        workspace_id: WorkspaceId,
    ) -> ParseResult {
        ParseResult {
            workspace_id,
            relative_path: PathBuf::from(path),
            language: LanguageId::Rust,
            provider_id: "test".to_owned(),
            provider_version: "test".to_owned(),
            schema_version: 1,
            status: ParseStatus::Complete,
            limit_reason: None,
            facts: vec![SyntaxFact {
                kind: SyntaxFactKind::Module,
                name: None,
                scope_path: Vec::new(),
                is_module_scope: true,
                visibility: None,
                range: SourceRange {
                    start_byte: 0,
                    end_byte: 1,
                },
            }],
            imports: Vec::new(),
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn parsed(path: &str, modules: Vec<RustModuleFact>) -> ParseResult {
        parsed_with_id(path, modules, WorkspaceId::new())
    }

    fn module(
        name: &str,
        scope: &[&str],
        inline: bool,
        has_resolution_attributes: bool,
    ) -> RustModuleFact {
        RustModuleFact {
            scope_path: scope.iter().map(|part| (*part).to_owned()).collect(),
            name: name.to_owned(),
            is_inline: inline,
            has_resolution_attributes,
            range: SourceRange {
                start_byte: 0,
                end_byte: 1,
            },
        }
    }

    #[test]
    fn resolves_crate_root_and_inline_scope_candidates() {
        let root = PathBuf::from("src/lib.rs");
        let report = resolve_rust_module_candidates(
            &[parsed(
                "src/lib.rs",
                vec![module("model", &["nested"], false, false)],
            )],
            &[PathBuf::from("src/nested/model.rs")],
            &[root],
        )
        .expect("module candidates");
        assert_eq!(
            report.modules[0].status,
            RustModuleResolutionStatus::UniqueCandidate
        );
        assert_eq!(
            report.modules[0].existing_candidates,
            vec![PathBuf::from("src/nested/model.rs")]
        );
    }

    #[test]
    fn supports_mod_rs_and_non_root_module_file_conventions() {
        let id = WorkspaceId::new();
        let report = resolve_rust_module_candidates(
            &[
                parsed_with_id("src/mod.rs", vec![module("child", &[], false, false)], id),
                parsed_with_id(
                    "src/parent.rs",
                    vec![module("child", &[], false, false)],
                    id,
                ),
            ],
            &[
                PathBuf::from("src/child/mod.rs"),
                PathBuf::from("src/parent/child.rs"),
            ],
            &[],
        )
        .expect("module candidates");
        assert_eq!(
            report.modules[0].expected_paths[0],
            PathBuf::from("src/child.rs")
        );
        assert_eq!(
            report.modules[0].status,
            RustModuleResolutionStatus::UniqueCandidate
        );
        assert_eq!(
            report.modules[1].expected_paths[0],
            PathBuf::from("src/parent/child.rs")
        );
        assert_eq!(
            report.modules[1].status,
            RustModuleResolutionStatus::UniqueCandidate
        );
    }

    #[test]
    fn preserves_ambiguity_when_both_module_forms_exist() {
        let report = resolve_rust_module_candidates(
            &[parsed(
                "src/lib.rs",
                vec![module("model", &[], false, false)],
            )],
            &[
                PathBuf::from("src/model.rs"),
                PathBuf::from("src/model/mod.rs"),
            ],
            &[PathBuf::from("src/lib.rs")],
        )
        .expect("module candidates");
        assert_eq!(
            report.modules[0].status,
            RustModuleResolutionStatus::AmbiguousCandidates
        );
        assert_eq!(report.modules[0].existing_candidates.len(), 2);
    }

    #[test]
    fn attributed_declarations_are_not_resolved() {
        let report = resolve_rust_module_candidates(
            &[parsed(
                "src/lib.rs",
                vec![module("optional", &[], false, true)],
            )],
            &[PathBuf::from("src/optional.rs")],
            &[PathBuf::from("src/lib.rs")],
        )
        .expect("module candidates");
        assert_eq!(
            report.modules[0].status,
            RustModuleResolutionStatus::SkippedAttributedDeclaration
        );
        assert!(report.modules[0].existing_candidates.is_empty());
    }

    #[test]
    fn rejects_module_names_that_could_escape_the_workspace() {
        let error = resolve_rust_module_candidates(
            &[parsed(
                "src/lib.rs",
                vec![module("outside:drive", &[], false, false)],
            )],
            &[],
            &[PathBuf::from("src/lib.rs")],
        )
        .expect_err("unsafe module path");
        assert!(matches!(error, RustModuleResolutionError::InvalidPath));
    }

    #[test]
    fn requires_one_crate_root_per_candidate_mapping() {
        let error = resolve_rust_module_candidates(
            &[parsed("src/lib.rs", Vec::new())],
            &[],
            &[PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")],
        )
        .expect_err("multiple roots");
        assert!(matches!(
            error,
            RustModuleResolutionError::MultipleCrateRoots
        ));
    }

    #[test]
    fn rejects_mixed_workspace_identities() {
        let error = resolve_rust_module_candidates(
            &[
                parsed("src/lib.rs", Vec::new()),
                parsed("src/main.rs", Vec::new()),
            ],
            &[],
            &[],
        )
        .expect_err("mixed workspaces");
        assert!(matches!(error, RustModuleResolutionError::MixedWorkspace));
    }
}
