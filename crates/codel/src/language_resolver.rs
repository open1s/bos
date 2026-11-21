//! Per-language import resolvers for the generic grammar layer.
//!
//! A resolver maps an import written in source to a workspace-relative file when
//! the language has a deterministic conventional mapping. It is deliberately
//! conservative: an unresolved import produces no edge, and a resolved edge is
//! still labelled inferred because the mapping is conventional, not compiler
//! proof.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::classification::LanguageId;
use crate::index::SourceFileResult;
use crate::syntax::{SourceRange, SyntaxFactKind};

/// One import that resolved to a workspace file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEdge {
    /// File containing the import.
    pub from: PathBuf,
    /// File the import resolves to.
    pub to: PathBuf,
    /// Source spelling used for evidence.
    pub spelling: String,
    /// Import statement range.
    pub range: SourceRange,
}

/// One conservative import resolver.
pub trait LanguageResolver: Send + Sync {
    /// Language this resolver understands.
    fn language(&self) -> LanguageId;

    /// Resolve imports against the workspace inventory.
    fn resolve(&self, files: &[SourceFileResult], inventory: &BTreeSet<PathBuf>)
    -> Vec<ImportEdge>;
}

/// Java package/class imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaResolver;

impl LanguageResolver for JavaResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Java
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Java) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                let Some(path) = java_import_path(text) else {
                    continue;
                };
                if inventory.contains(&path) {
                    edges.push(ImportEdge {
                        from: file.path.clone(),
                        to: path,
                        spelling: text.to_owned(),
                        range: fact.range,
                    });
                }
            }
        }
        edges
    }
}

/// JavaScript/TypeScript relative ESM or CommonJS imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaScriptResolver;

impl LanguageResolver for JavaScriptResolver {
    fn language(&self) -> LanguageId {
        LanguageId::JavaScript
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::JavaScript) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                let Some(specifier) = quoted(text) else {
                    continue;
                };
                let Some(path) = relative_module(&file.path, specifier, inventory) else {
                    continue;
                };
                edges.push(ImportEdge {
                    from: file.path.clone(),
                    to: path,
                    spelling: text.to_owned(),
                    range: fact.range,
                });
            }
        }
        edges
    }
}

/// Go package imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoResolver;

impl LanguageResolver for GoResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Go
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Go) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                for specifier in quoted_all(text) {
                    let Some(package) = specifier.rsplit('/').next() else {
                        continue;
                    };
                    let candidates: Vec<PathBuf> = inventory
                        .iter()
                        .filter(|path| {
                            path.extension().is_some_and(|extension| extension == "go")
                                && path
                                    .parent()
                                    .and_then(|parent| parent.file_name())
                                    .is_some_and(|name| name == package)
                        })
                        .cloned()
                        .collect();
                    for candidate in candidates {
                        edges.push(ImportEdge {
                            from: file.path.clone(),
                            to: candidate,
                            spelling: text.to_owned(),
                            range: fact.range,
                        });
                    }
                }
            }
        }
        edges
    }
}

/// C# namespace imports resolved by unique conventional file name.
#[derive(Clone, Copy, Debug, Default)]
pub struct CSharpResolver;

impl LanguageResolver for CSharpResolver {
    fn language(&self) -> LanguageId {
        LanguageId::CSharp
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::CSharp) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                let Some(namespace) = text
                    .trim()
                    .strip_prefix("using ")
                    .map(|value| value.trim_end_matches(';').trim())
                else {
                    continue;
                };
                if namespace.starts_with("static ") || namespace.is_empty() {
                    continue;
                }
                let Some(last) = namespace.rsplit('.').next() else {
                    continue;
                };
                let candidates: Vec<PathBuf> = inventory
                    .iter()
                    .filter(|path| {
                        path.extension().is_some_and(|extension| extension == "cs")
                            && path
                                .file_stem()
                                .is_some_and(|stem| stem.to_string_lossy() == last)
                    })
                    .cloned()
                    .collect();
                if let [candidate] = candidates.as_slice() {
                    edges.push(ImportEdge {
                        from: file.path.clone(),
                        to: candidate.clone(),
                        spelling: text.to_owned(),
                        range: fact.range,
                    });
                }
            }
        }
        edges
    }
}

fn quoted_all(text: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['"', '\'']) {
        let after = &rest[start + 1..];
        let Some(end) = after.find(['"', '\'']) else {
            break;
        };
        values.push(&after[..end]);
        rest = &after[end + 1..];
    }
    values
}

/// Kotlin package/class imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct KotlinResolver;

impl LanguageResolver for KotlinResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Kotlin
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        resolve_qualified(files, inventory, LanguageId::Kotlin, "kt")
    }
}

/// Scala package/class imports.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScalaResolver;

impl LanguageResolver for ScalaResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Scala
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        resolve_qualified(files, inventory, LanguageId::Scala, "scala")
    }
}

fn resolve_qualified(
    files: &[SourceFileResult],
    inventory: &BTreeSet<PathBuf>,
    language: LanguageId,
    extension: &str,
) -> Vec<ImportEdge> {
    let mut edges = Vec::new();
    for file in files {
        if file.language != Some(language) {
            continue;
        }
        let Some(parse) = &file.parse else {
            continue;
        };
        for fact in &parse.facts {
            if fact.kind != SyntaxFactKind::Import {
                continue;
            }
            let Some(text) = fact.name.as_deref() else {
                continue;
            };
            let Some(target) = qualified_path(text, extension) else {
                continue;
            };
            let target = if inventory.contains(&target) {
                Some(target)
            } else {
                let Some(stem) = target.file_stem() else {
                    continue;
                };
                unique_file(inventory, extension, stem)
            };
            if let Some(target) = target {
                edges.push(ImportEdge {
                    from: file.path.clone(),
                    to: target,
                    spelling: text.to_owned(),
                    range: fact.range,
                });
            }
        }
    }
    edges
}

fn qualified_path(text: &str, extension: &str) -> Option<PathBuf> {
    let text = text.trim();
    let text = text.strip_prefix("import ")?;
    let text = text.split(" as ").next().unwrap_or(text);
    let text = text.trim_end_matches(';').trim();
    if text.ends_with(".*") || text.ends_with("._") || text.ends_with('*') {
        return None;
    }
    if text.contains('{') {
        return None;
    }
    let mut segments = text.split('.').filter(|segment| !segment.is_empty());
    let name = segments.next_back()?;
    let mut path = PathBuf::new();
    for segment in segments {
        path.push(segment);
    }
    path.push(format!("{name}.{extension}"));
    Some(path)
}

fn unique_file(
    inventory: &BTreeSet<PathBuf>,
    extension: &str,
    stem: &std::ffi::OsStr,
) -> Option<PathBuf> {
    let matches: Vec<PathBuf> = inventory
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|candidate| candidate == extension)
                && path.file_stem().is_some_and(|candidate| candidate == stem)
        })
        .cloned()
        .collect();
    match matches.as_slice() {
        [single] => Some(single.clone()),
        _ => None,
    }
}

/// Ruby `require_relative` and conventional `require` paths.
#[derive(Clone, Copy, Debug, Default)]
pub struct RubyResolver;

impl LanguageResolver for RubyResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Ruby
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Ruby) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                let Some(specifier) = quoted(text) else {
                    continue;
                };
                let candidate = if text.contains("require_relative") || specifier.starts_with('.') {
                    let base = file.path.parent().unwrap_or_else(|| Path::new(""));
                    normalize(base.join(specifier))
                } else {
                    normalize(Path::new("lib").join(specifier))
                };
                let candidates = [
                    candidate.clone(),
                    candidate.with_extension("rb"),
                    normalize(Path::new("lib").join(specifier)).with_extension("rb"),
                ];
                for path in candidates {
                    if inventory.contains(&path) {
                        edges.push(ImportEdge {
                            from: file.path.clone(),
                            to: path,
                            spelling: text.to_owned(),
                            range: fact.range,
                        });
                        break;
                    }
                }
            }
        }
        edges
    }
}

/// PHP namespace imports and quoted include/require paths.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhpResolver;

impl LanguageResolver for PhpResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Php
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Php) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                if let Some(specifier) = quoted(text) {
                    let base = file.path.parent().unwrap_or_else(|| Path::new(""));
                    let candidate = normalize(base.join(specifier));
                    if inventory.contains(&candidate) {
                        edges.push(ImportEdge {
                            from: file.path.clone(),
                            to: candidate,
                            spelling: text.to_owned(),
                            range: fact.range,
                        });
                    }
                    continue;
                }
                let Some(namespace) = text
                    .trim()
                    .strip_prefix("use ")
                    .map(|value| value.trim_end_matches(';').trim())
                else {
                    continue;
                };
                let Some(last) = namespace.rsplit('\\').next() else {
                    continue;
                };
                let candidates: Vec<PathBuf> = inventory
                    .iter()
                    .filter(|path| {
                        path.extension().is_some_and(|extension| extension == "php")
                            && path
                                .file_stem()
                                .is_some_and(|stem| stem.to_string_lossy() == last)
                    })
                    .cloned()
                    .collect();
                if let [candidate] = candidates.as_slice() {
                    edges.push(ImportEdge {
                        from: file.path.clone(),
                        to: candidate.clone(),
                        spelling: text.to_owned(),
                        range: fact.range,
                    });
                }
            }
        }
        edges
    }
}

/// C/C++ quoted includes.
#[derive(Clone, Copy, Debug, Default)]
pub struct CIncludeResolver;

impl LanguageResolver for CIncludeResolver {
    fn language(&self) -> LanguageId {
        LanguageId::C
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        inventory: &BTreeSet<PathBuf>,
    ) -> Vec<ImportEdge> {
        let mut edges = Vec::new();
        for file in files {
            if !matches!(
                file.language,
                Some(LanguageId::C | LanguageId::Cpp | LanguageId::CHeader)
            ) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                if fact.kind != SyntaxFactKind::Import {
                    continue;
                }
                let Some(text) = fact.name.as_deref() else {
                    continue;
                };
                let Some(include) = quoted(text) else {
                    continue;
                };
                let candidate = file
                    .path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(include);
                if inventory.contains(&candidate) {
                    edges.push(ImportEdge {
                        from: file.path.clone(),
                        to: candidate,
                        spelling: text.to_owned(),
                        range: fact.range,
                    });
                }
            }
        }
        edges
    }
}

fn java_import_path(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    let text = text.strip_prefix("import ")?;
    let text = text.strip_prefix("static ").unwrap_or(text);
    let path = text.trim_end_matches(';').trim();
    if path.ends_with('*') {
        return None;
    }
    let mut segments = path.split('.').filter(|segment| !segment.is_empty());
    let class = segments.next_back()?;
    let mut path = PathBuf::new();
    for segment in segments {
        path.push(segment);
    }
    path.push(format!("{class}.java"));
    Some(path)
}

fn quoted(text: &str) -> Option<&str> {
    let start = text.find(['"', '\''])?.saturating_add(1);
    let rest = text.get(start..)?;
    let end = rest.find(['"', '\''])?;
    rest.get(..end)
}

fn relative_module(
    source: &Path,
    specifier: &str,
    inventory: &BTreeSet<PathBuf>,
) -> Option<PathBuf> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let base = source.parent().unwrap_or_else(|| Path::new(""));
    let mut candidate = normalize(base.join(specifier));
    let extensions = ["", ".js", ".ts", ".tsx", "/index.js", "/index.ts"];
    for extension in extensions {
        let mut path = candidate.clone();
        if extension.starts_with('/') {
            path.push(extension.trim_start_matches('/'));
        } else if !extension.is_empty() {
            let file = path
                .file_name()
                .map(|name| format!("{}{}", name.to_string_lossy(), extension))
                .unwrap_or_default();
            path.set_file_name(file);
        }
        if inventory.contains(&path) {
            return Some(path);
        }
    }
    candidate.pop();
    None
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                output.pop();
            }
            std::path::Component::CurDir => {}
            other => output.push(other.as_os_str()),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceId;
    use crate::index::{SourceFileResult, SourceFileStatus};
    use crate::syntax::{ParseResult, ParseStatus, SyntaxFact};

    fn file(path: &str, language: LanguageId, import: &str) -> SourceFileResult {
        SourceFileResult {
            path: PathBuf::from(path),
            language: Some(language),
            status: SourceFileStatus::Parsed,
            parse: Some(ParseResult {
                workspace_id: WorkspaceId::new(),
                relative_path: PathBuf::from(path),
                language,
                provider_id: "test".to_owned(),
                provider_version: "1".to_owned(),
                schema_version: 1,
                status: ParseStatus::Complete,
                limit_reason: None,
                facts: vec![SyntaxFact {
                    kind: SyntaxFactKind::Import,
                    name: Some(import.to_owned()),
                    scope_path: Vec::new(),
                    is_module_scope: true,
                    visibility: None,
                    range: SourceRange {
                        start_byte: 0,
                        end_byte: import.len(),
                    },
                }],
                imports: Vec::new(),
                relations: Vec::new(),
                modules: Vec::new(),
                diagnostics: Vec::new(),
            }),
        }
    }

    #[test]
    fn java_imports_resolve_to_conventional_class_files() {
        let files = vec![file(
            "src/Main.java",
            LanguageId::Java,
            "import com.example.Foo;",
        )];
        let inventory = BTreeSet::from([PathBuf::from("com/example/Foo.java")]);
        let edges = JavaResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("com/example/Foo.java"));
    }

    #[test]
    fn javascript_relative_imports_resolve_to_files() {
        let files = vec![file(
            "src/app.js",
            LanguageId::JavaScript,
            r#"import { x } from "./foo";"#,
        )];
        let inventory = BTreeSet::from([PathBuf::from("src/foo.js")]);
        let edges = JavaScriptResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("src/foo.js"));
    }

    #[test]
    fn c_includes_resolve_relative_to_the_source_file() {
        let files = vec![file("src/main.c", LanguageId::C, r#"#include "util.h""#)];
        let inventory = BTreeSet::from([PathBuf::from("src/util.h")]);
        let edges = CIncludeResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("src/util.h"));
    }
    #[test]
    fn go_package_imports_resolve_to_the_package_directory() {
        let files = vec![file(
            "src/main.go",
            LanguageId::Go,
            r#"import "example.com/mod/pkg""#,
        )];
        let inventory = BTreeSet::from([PathBuf::from("pkg/util.go")]);
        let edges = GoResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("pkg/util.go"));
    }

    #[test]
    fn csharp_using_resolves_to_a_unique_file() {
        let files = vec![file(
            "src/Program.cs",
            LanguageId::CSharp,
            "using App.Widget;",
        )];
        let inventory = BTreeSet::from([PathBuf::from("src/Widget.cs")]);
        let edges = CSharpResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("src/Widget.cs"));
    }
    #[test]
    fn ruby_require_relative_resolves_to_a_file() {
        let files = vec![file(
            "src/app.rb",
            LanguageId::Ruby,
            r#"require_relative "./foo""#,
        )];
        let inventory = BTreeSet::from([PathBuf::from("src/foo.rb")]);
        let edges = RubyResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("src/foo.rb"));
    }

    #[test]
    fn php_namespace_use_resolves_to_a_unique_file() {
        let files = vec![file(
            "src/index.php",
            LanguageId::Php,
            r"use App\Models\User;",
        )];
        let inventory = BTreeSet::from([PathBuf::from("src/User.php")]);
        let edges = PhpResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("src/User.php"));
    }
    #[test]
    fn kotlin_imports_resolve_to_conventional_files() {
        let files = vec![file(
            "src/Main.kt",
            LanguageId::Kotlin,
            "import com.example.Foo",
        )];
        let inventory = BTreeSet::from([PathBuf::from("com/example/Foo.kt")]);
        let edges = KotlinResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("com/example/Foo.kt"));
    }

    #[test]
    fn scala_imports_resolve_to_conventional_files() {
        let files = vec![file(
            "src/Main.scala",
            LanguageId::Scala,
            "import com.example.Bar",
        )];
        let inventory = BTreeSet::from([PathBuf::from("com/example/Bar.scala")]);
        let edges = ScalaResolver.resolve(&files, &inventory);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, PathBuf::from("com/example/Bar.scala"));
    }
}
