//! SCIP index consumption.
//!
//! SCIP is an external index protocol: language indexers do semantic analysis,
//! and codel consumes their definitions/references without linking another
//! tree-sitter build. This module converts SCIP occurrences into source-backed
//! ranges.

use std::path::{Path, PathBuf};

use protobuf::Message;
use scip::types::{Document, Index, Occurrence, SymbolRole};

use crate::syntax::SourceRange;

/// One SCIP reference/definition pair.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScipReference {
    /// File containing the reference.
    pub from: PathBuf,
    /// Reference range.
    pub from_range: SourceRange,
    /// File containing the definition.
    pub to: PathBuf,
    /// Definition range.
    pub to_range: SourceRange,
    /// SCIP symbol string.
    pub symbol: String,
    /// Symbol role bits from SCIP.
    pub roles: i32,
}

/// Map SCIP references onto existing symbol ranges.
#[must_use]
pub fn to_symbol_edges(
    references: &[ScipReference],
    symbols: &crate::resolution::SymbolCandidateReport,
) -> Vec<crate::symbol_resolver::ScopedSymbolEdge> {
    references
        .iter()
        .filter_map(|reference| {
            let from = containing_symbol(symbols, &reference.from, reference.from_range)?;
            let to = containing_symbol(symbols, &reference.to, reference.to_range)?;
            Some(crate::symbol_resolver::ScopedSymbolEdge {
                from: from.id.clone(),
                to: to.id.clone(),
                kind: if reference.roles & (SymbolRole::WriteAccess as i32) != 0 {
                    crate::resolution::ReferenceKind::ScipWrite
                } else if reference.roles & (SymbolRole::ReadAccess as i32) != 0 {
                    crate::resolution::ReferenceKind::ScipRead
                } else {
                    crate::resolution::ReferenceKind::Scip
                },
                spelling: reference.symbol.clone(),
                range: reference.from_range,
            })
        })
        .collect()
}

fn containing_symbol<'a>(
    symbols: &'a crate::resolution::SymbolCandidateReport,
    path: &Path,
    range: SourceRange,
) -> Option<&'a crate::resolution::SymbolRecord> {
    symbols
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.id.path == path
                && symbol.id.start_byte <= range.start_byte
                && range.end_byte <= symbol.id.end_byte
        })
        .min_by_key(|symbol| symbol.id.end_byte.saturating_sub(symbol.id.start_byte))
}

/// External symbol metadata from a SCIP index.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScipExternalSymbol {
    /// SCIP symbol string.
    pub symbol: String,
    /// Human-readable name.
    pub display_name: String,
    /// Markdown documentation blocks.
    pub documentation: Vec<String>,
    /// Signature text for hover/API display.
    pub signature: String,
}

/// Display names of symbols defined outside the indexed documents.
#[must_use]
pub fn external_symbols(index: &Index) -> Vec<ScipExternalSymbol> {
    index
        .external_symbols
        .iter()
        .map(|symbol| ScipExternalSymbol {
            symbol: symbol.symbol.clone(),
            display_name: if symbol.display_name.is_empty() {
                symbol.symbol.clone()
            } else {
                symbol.display_name.clone()
            },
            documentation: symbol.documentation.clone(),
            signature: symbol
                .signature_documentation
                .as_ref()
                .map_or_else(String::new, |signature| signature.text.clone()),
        })
        .collect()
}

fn index_paths(index_path: &Path) -> Result<Vec<PathBuf>, String> {
    if !index_path.is_dir() {
        return Ok(vec![index_path.to_path_buf()]);
    }
    let mut paths = std::fs::read_dir(index_path)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "scip")
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no .scip files in {}", index_path.display()));
    }
    Ok(paths)
}

fn parse_index(path: &Path) -> Result<Index, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Index::parse_from_bytes(&bytes).map_err(|error| error.to_string())
}

/// A reference whose definition lives outside the indexed documents.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScipExternalReference {
    /// File containing the reference.
    pub from: PathBuf,
    /// Reference range.
    pub from_range: SourceRange,
    /// SCIP symbol string.
    pub symbol: String,
    /// Human-readable external symbol name.
    pub display_name: String,
    /// Markdown documentation blocks.
    pub documentation: Vec<String>,
    /// Signature text for hover/API display.
    pub signature: String,
}

/// Collect references that resolve to `Index.external_symbols`.
///
/// # Errors
///
/// Returns a message when a referenced source file cannot be read.
pub fn external_references(
    index: &Index,
    root: &Path,
) -> Result<Vec<ScipExternalReference>, String> {
    let external: std::collections::BTreeMap<&str, &scip::types::SymbolInformation> = index
        .external_symbols
        .iter()
        .map(|symbol| (symbol.symbol.as_str(), symbol))
        .collect();
    let local: std::collections::BTreeSet<&str> = index
        .documents
        .iter()
        .flat_map(|document| document.occurrences.iter())
        .filter(|occurrence| {
            has_role(occurrence.symbol_roles, SymbolRole::Definition)
                || has_role(occurrence.symbol_roles, SymbolRole::ForwardDefinition)
        })
        .map(|occurrence| occurrence.symbol.as_str())
        .collect();

    let mut references = Vec::new();
    for document in &index.documents {
        let source = document_source(document, root)?;
        for occurrence in &document.occurrences {
            if local.contains(occurrence.symbol.as_str()) {
                continue;
            }
            let Some(symbol) = external.get(occurrence.symbol.as_str()) else {
                continue;
            };
            let Some(from_range) = occurrence_range(occurrence, &source) else {
                continue;
            };
            references.push(ScipExternalReference {
                from: PathBuf::from(&document.relative_path),
                from_range,
                symbol: occurrence.symbol.clone(),
                display_name: if symbol.display_name.is_empty() {
                    symbol.symbol.clone()
                } else {
                    symbol.display_name.clone()
                },
                documentation: symbol.documentation.clone(),
                signature: symbol
                    .signature_documentation
                    .as_ref()
                    .map_or_else(String::new, |signature| signature.text.clone()),
            });
        }
    }
    Ok(references)
}

/// Load external references directly from an index file.
///
/// # Errors
///
/// Returns a message when the index cannot be read/parsed or a source cannot be
/// read.
pub fn load_external_references(
    index_path: &Path,
    root: &Path,
) -> Result<Vec<ScipExternalReference>, String> {
    let mut references = Vec::new();
    for path in index_paths(index_path)? {
        references.extend(external_references(&parse_index(&path)?, root)?);
    }
    references.sort_by(|left, right| {
        (&left.from, left.from_range, &left.symbol).cmp(&(
            &right.from,
            right.from_range,
            &right.symbol,
        ))
    });
    references.dedup();
    Ok(references)
}

/// Load external symbol display names from a SCIP index.
///
/// # Errors
///
/// Returns a message when the index cannot be read or parsed.
pub fn load_external_symbols(index_path: &Path) -> Result<Vec<ScipExternalSymbol>, String> {
    let mut symbols = Vec::new();
    for path in index_paths(index_path)? {
        symbols.extend(external_symbols(&parse_index(&path)?));
    }
    symbols.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    symbols.dedup_by(|left, right| left.symbol == right.symbol);
    Ok(symbols)
}

/// Read a SCIP index and resolve occurrences to source byte ranges.
///
/// # Errors
///
/// Returns a message when the index cannot be read/parsed or a referenced source
/// file cannot be read.
pub fn load_index(index_path: &Path, root: &Path) -> Result<Vec<ScipReference>, String> {
    let mut references = Vec::new();
    for path in index_paths(index_path)? {
        references.extend(resolve_index(&parse_index(&path)?, root)?);
    }
    references.sort_by(|left, right| {
        (
            &left.from,
            left.from_range,
            &left.to,
            left.to_range,
            &left.symbol,
        )
            .cmp(&(
                &right.from,
                right.from_range,
                &right.to,
                right.to_range,
                &right.symbol,
            ))
    });
    references.dedup();
    Ok(references)
}

/// Resolve an already-parsed SCIP index.
///
/// # Errors
///
/// Returns a message when a referenced source file cannot be read.
pub fn resolve_index(index: &Index, root: &Path) -> Result<Vec<ScipReference>, String> {
    let mut definitions: std::collections::BTreeMap<String, (PathBuf, SourceRange)> =
        std::collections::BTreeMap::new();
    for document in &index.documents {
        let source = document_source(document, root)?;
        for occurrence in &document.occurrences {
            if !has_role(occurrence.symbol_roles, SymbolRole::Definition)
                && !has_role(occurrence.symbol_roles, SymbolRole::ForwardDefinition)
            {
                continue;
            }
            if let Some(range) = occurrence_range(occurrence, &source) {
                definitions
                    .entry(definition_key(&document.relative_path, &occurrence.symbol))
                    .or_insert_with(|| (PathBuf::from(&document.relative_path), range));
            }
        }
    }

    let mut references = Vec::new();
    for document in &index.documents {
        let source = document_source(document, root)?;
        for occurrence in &document.occurrences {
            if has_role(occurrence.symbol_roles, SymbolRole::Definition)
                || has_role(occurrence.symbol_roles, SymbolRole::ForwardDefinition)
            {
                continue;
            }
            let key = definition_key(&document.relative_path, &occurrence.symbol);
            let Some((to, to_range)) = definitions.get(&key) else {
                continue;
            };
            let Some(from_range) = occurrence_range(occurrence, &source) else {
                continue;
            };
            references.push(ScipReference {
                from: PathBuf::from(&document.relative_path),
                from_range,
                to: to.clone(),
                to_range: *to_range,
                symbol: occurrence.symbol.clone(),
                roles: occurrence.symbol_roles,
            });
        }
    }
    Ok(references)
}

fn definition_key(path: &str, symbol: &str) -> String {
    if scip::symbol::is_local_symbol(symbol) {
        format!("{path}\u{0}{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn has_role(roles: i32, role: SymbolRole) -> bool {
    roles & (role as i32) != 0
}

fn document_source(document: &Document, root: &Path) -> Result<String, String> {
    if !document.text.is_empty() {
        return Ok(document.text.clone());
    }
    std::fs::read_to_string(root.join(&document.relative_path))
        .map_err(|error| format!("read {}: {error}", document.relative_path))
}

fn occurrence_range(occurrence: &Occurrence, source: &str) -> Option<SourceRange> {
    let (start_line, start_character, end_line, end_character) =
        if occurrence.has_single_line_range() {
            let range = occurrence.single_line_range();
            (
                range.line as usize,
                range.start_character as usize,
                range.line as usize,
                range.end_character as usize,
            )
        } else if occurrence.has_multi_line_range() {
            let range = occurrence.multi_line_range();
            (
                range.start_line as usize,
                range.start_character as usize,
                range.end_line as usize,
                range.end_character as usize,
            )
        } else {
            match occurrence.range.as_slice() {
                [line, start, end] => (
                    *line as usize,
                    *start as usize,
                    *line as usize,
                    *end as usize,
                ),
                [start_line, start, end_line, end] => (
                    *start_line as usize,
                    *start as usize,
                    *end_line as usize,
                    *end as usize,
                ),
                _ => return None,
            }
        };
    let start_byte = line_offset(source, start_line, start_character)?;
    let end_byte = line_offset(source, end_line, end_character)?;
    Some(SourceRange {
        start_byte,
        end_byte,
    })
}

fn line_offset(source: &str, line: usize, character: usize) -> Option<usize> {
    let line_start = source
        .split_inclusive('\n')
        .take(line)
        .map(str::len)
        .sum::<usize>();
    let line_text = source.get(line_start..)?.split('\n').next()?;
    if character > line_text.len() || !line_text.is_char_boundary(character) {
        return None;
    }
    Some(line_start + character)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::{Document, Index, Occurrence, SingleLineRange, SymbolRole};

    #[test]
    fn resolves_scip_occurrences_to_byte_ranges() {
        let mut definition = Occurrence::new();
        definition.symbol = "scip java . . Bar#".to_owned();
        definition.symbol_roles = SymbolRole::Definition as i32;
        let mut range = SingleLineRange::new();
        range.line = 0;
        range.start_character = 6;
        range.end_character = 9;
        definition.set_single_line_range(range);

        let mut reference = Occurrence::new();
        reference.symbol = definition.symbol.clone();
        reference.symbol_roles = SymbolRole::ReadAccess as i32;
        let mut range = SingleLineRange::new();
        range.line = 1;
        range.start_character = 12;
        range.end_character = 15;
        reference.set_single_line_range(range);

        let mut document = Document::new();
        document.relative_path = "Foo.java".to_owned();
        document.text = "class Bar {}
class Foo { Bar b; }"
            .to_owned();
        document.occurrences = vec![definition, reference];

        let mut index = Index::new();
        index.documents.push(document);
        let references = resolve_index(&index, Path::new("")).expect("index");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].from, PathBuf::from("Foo.java"));
        assert_eq!(references[0].from_range.start_byte, 25);
        assert_eq!(references[0].to_range.start_byte, 6);
    }
    #[test]
    fn maps_scip_references_to_symbol_edges() {
        use crate::WorkspaceId;
        use crate::resolution::{SymbolCandidateReport, SymbolId, SymbolKind, SymbolRecord};
        use std::path::PathBuf;

        let workspace = WorkspaceId::new();
        let from = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("A.java"),
                start_byte: 0,
                end_byte: 10,
                kind: SymbolKind::Function,
            },
            name: "run".to_owned(),
        };
        let to = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("A.java"),
                start_byte: 20,
                end_byte: 30,
                kind: SymbolKind::Field,
            },
            name: "value".to_owned(),
        };
        let report = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![from, to],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let references = vec![ScipReference {
            from: PathBuf::from("A.java"),
            from_range: SourceRange {
                start_byte: 0,
                end_byte: 10,
            },
            to: PathBuf::from("A.java"),
            to_range: SourceRange {
                start_byte: 20,
                end_byte: 30,
            },
            symbol: "scip java . . value".to_owned(),
            roles: SymbolRole::ReadAccess as i32,
        }];
        let edges = to_symbol_edges(&references, &report);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, crate::resolution::ReferenceKind::ScipRead);
        assert_eq!(edges[0].from, report.symbols[0].id);
        assert_eq!(edges[0].to, report.symbols[1].id);
    }

    #[test]
    fn lists_external_scip_symbols() {
        use scip::types::SymbolInformation;
        let mut index = Index::new();
        let mut symbol = SymbolInformation::new();
        symbol.symbol = "scip java . . ext".to_owned();
        symbol.display_name = "ext".to_owned();
        index.external_symbols.push(symbol);
        let symbols = external_symbols(&index);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].display_name, "ext");
    }
    #[test]
    fn merges_external_symbol_references() {
        use scip::types::{Document, Occurrence, SingleLineRange, SymbolInformation};
        let mut occurrence = Occurrence::new();
        occurrence.symbol = "scip java . . ext".to_owned();
        occurrence.symbol_roles = SymbolRole::ReadAccess as i32;
        let mut range = SingleLineRange::new();
        range.line = 0;
        range.start_character = 0;
        range.end_character = 3;
        occurrence.set_single_line_range(range);
        let mut document = Document::new();
        document.relative_path = "A.java".to_owned();
        document.text = "ext field;".to_owned();
        document.occurrences = vec![occurrence];
        let mut external = SymbolInformation::new();
        external.symbol = "scip java . . ext".to_owned();
        external.display_name = "ext".to_owned();
        let mut index = Index::new();
        index.documents.push(document);
        index.external_symbols.push(external);
        let refs = external_references(&index, Path::new("")).expect("refs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].display_name, "ext");
    }
    #[test]
    fn local_symbols_do_not_cross_document_boundaries() {
        use scip::types::{Document, Occurrence, SingleLineRange};
        let mut index = Index::new();
        for (path, text) in [("A.java", "int a; a;"), ("B.java", "int b; b;")] {
            let mut definition = Occurrence::new();
            definition.symbol = "local 1".to_owned();
            definition.symbol_roles = SymbolRole::Definition as i32;
            let mut range = SingleLineRange::new();
            range.line = 0;
            range.start_character = 0;
            range.end_character = 5;
            definition.set_single_line_range(range);
            let mut reference = Occurrence::new();
            reference.symbol = "local 1".to_owned();
            reference.symbol_roles = SymbolRole::ReadAccess as i32;
            let mut range = SingleLineRange::new();
            range.line = 0;
            range.start_character = 6;
            range.end_character = 7;
            reference.set_single_line_range(range);
            let mut document = Document::new();
            document.relative_path = path.to_owned();
            document.text = text.to_owned();
            document.occurrences = vec![definition, reference];
            index.documents.push(document);
        }
        let refs = resolve_index(&index, Path::new("")).expect("refs");
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|reference| reference.from == reference.to));
    }
    #[test]
    fn merges_multiple_index_files_in_a_directory() {
        use scip::types::{Document, Index, Occurrence, SingleLineRange, SymbolRole};
        let directory = tempfile::tempdir().expect("tempdir");
        for (name, symbol) in [("A.java", "scip java . . a"), ("B.java", "scip java . . b")] {
            let mut definition = Occurrence::new();
            definition.symbol = symbol.to_owned();
            definition.symbol_roles = SymbolRole::Definition as i32;
            let mut range = SingleLineRange::new();
            range.line = 0;
            range.start_character = 0;
            range.end_character = 1;
            definition.set_single_line_range(range);
            let mut reference = Occurrence::new();
            reference.symbol = symbol.to_owned();
            reference.symbol_roles = SymbolRole::ReadAccess as i32;
            let mut range = SingleLineRange::new();
            range.line = 0;
            range.start_character = 2;
            range.end_character = 3;
            reference.set_single_line_range(range);
            let mut document = Document::new();
            document.relative_path = name.to_owned();
            document.text = "a b".to_owned();
            document.occurrences = vec![definition, reference];
            let mut index = Index::new();
            index.documents.push(document);
            scip::write_message_to_file(directory.path().join(format!("{name}.scip")), index)
                .expect("index");
        }
        let refs = load_index(directory.path(), directory.path()).expect("refs");
        assert_eq!(refs.len(), 2);
    }
}
