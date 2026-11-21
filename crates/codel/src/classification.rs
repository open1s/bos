//! Deterministic, path-based file classification.
//!
//! Classification uses only the workspace-relative file name and extension.
//! It never reads file contents, invokes language tooling, or executes project
//! commands. Results are hints for selecting later providers, not proof that a
//! file is valid source code.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fs::{WorkspaceEntry, WorkspaceEntryKind};

/// Broad purpose category assigned to a file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileCategory {
    /// A programming or query language source file.
    Source,
    /// A project, tool, or environment configuration file.
    Configuration,
    /// Human-readable documentation.
    Documentation,
    /// Structured data, manifests, or schema data.
    Data,
    /// Markup and component-template documents.
    Markup,
    /// Stylesheets and style preprocessors.
    Style,
    /// Binary, media, font, or packaged resources.
    Asset,
    /// No supported category could be inferred from the path.
    Unknown,
}

/// Language or format inferred from a recognized file name or extension.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    /// Rust.
    Rust,
    /// TypeScript and TSX.
    TypeScript,
    /// JavaScript and JSX.
    JavaScript,
    /// Python.
    Python,
    /// Go.
    Go,
    /// Java.
    Java,
    /// C.
    C,
    /// C++.
    Cpp,
    /// C or C++ header; the extension alone is ambiguous.
    CHeader,
    /// C#.
    CSharp,
    /// Swift.
    Swift,
    /// Kotlin.
    Kotlin,
    /// Scala.
    Scala,
    /// Ruby.
    Ruby,
    /// PHP.
    Php,
    /// Shell scripts.
    Shell,
    /// SQL.
    Sql,
    /// JSON.
    Json,
    /// TOML.
    Toml,
    /// YAML.
    Yaml,
    /// XML.
    Xml,
    /// Markdown.
    Markdown,
    /// HTML.
    Html,
    /// CSS.
    Css,
    /// SCSS.
    Scss,
    /// Sass.
    Sass,
    /// Less.
    Less,
    /// SVG.
    Svg,
    /// Protocol Buffers.
    ProtocolBuffers,
    /// GraphQL.
    Graphql,
    /// Dockerfile syntax.
    Dockerfile,
    /// Makefile syntax.
    Makefile,
    /// Just task-runner syntax.
    Justfile,
    /// Git ignore/attributes syntax.
    GitMetadata,
    /// Vue single-file components.
    Vue,
    /// Svelte components.
    Svelte,
}

/// Why a classification was selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassificationReason {
    /// A well-known project file name matched.
    WellKnownFileName,
    /// A recognized extension matched.
    Extension,
    /// The path has an extension, but it is not in the built-in registry.
    UnknownExtension,
    /// The path has no extension and no well-known file name matched.
    NoExtension,
}

/// Result of deterministic file classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileClassification {
    /// Broad file purpose.
    pub category: FileCategory,
    /// Optional language or format identifier.
    pub language: Option<LanguageId>,
    /// Evidence used to make the classification.
    pub reason: ClassificationReason,
}

/// Classify a workspace-relative path without opening or reading the file.
#[must_use]
pub fn classify_path(path: &Path) -> FileClassification {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return FileClassification {
            category: FileCategory::Unknown,
            language: None,
            reason: if path.extension().is_some() {
                ClassificationReason::UnknownExtension
            } else {
                ClassificationReason::NoExtension
            },
        };
    };

    let normalized_name = file_name.to_ascii_lowercase();
    if let Some(classification) = classify_well_known_name(&normalized_name) {
        return classification;
    }

    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return FileClassification {
            category: FileCategory::Unknown,
            language: None,
            reason: ClassificationReason::NoExtension,
        };
    };

    classify_extension(&extension.to_ascii_lowercase())
}

/// Classify a workspace entry. Directories have no file classification.
#[must_use]
pub fn classify_entry(entry: &WorkspaceEntry) -> Option<FileClassification> {
    (entry.kind == WorkspaceEntryKind::File).then(|| classify_path(&entry.relative_path))
}

fn classification(
    category: FileCategory,
    language: Option<LanguageId>,
    reason: ClassificationReason,
) -> FileClassification {
    FileClassification {
        category,
        language,
        reason,
    }
}

fn classify_well_known_name(name: &str) -> Option<FileClassification> {
    use ClassificationReason::WellKnownFileName;
    use FileCategory::{Configuration, Documentation};
    use LanguageId::{Dockerfile, GitMetadata, Justfile, Makefile, Ruby, Toml, Yaml};

    let value = match name {
        "dockerfile" => classification(Configuration, Some(Dockerfile), WellKnownFileName),
        "makefile" | "gnumakefile" => {
            classification(Configuration, Some(Makefile), WellKnownFileName)
        }
        "justfile" => classification(Configuration, Some(Justfile), WellKnownFileName),
        "cargo.toml" | "pyproject.toml" => {
            classification(Configuration, Some(Toml), WellKnownFileName)
        }
        "cargo.lock" | "poetry.lock" | "uv.lock" | "gemfile.lock" => {
            classification(Configuration, None, WellKnownFileName)
        }
        "package.json" | "tsconfig.json" | "jsconfig.json" => {
            classification(Configuration, Some(LanguageId::Json), WellKnownFileName)
        }
        "package-lock.json" => {
            classification(Configuration, Some(LanguageId::Json), WellKnownFileName)
        }
        "pnpm-lock.yaml" => classification(Configuration, Some(Yaml), WellKnownFileName),
        "yarn.lock" => classification(Configuration, None, WellKnownFileName),
        "go.mod" | "go.sum" => {
            classification(Configuration, Some(LanguageId::Go), WellKnownFileName)
        }
        "gemfile" | "rakefile" => classification(Configuration, Some(Ruby), WellKnownFileName),
        ".gitignore" | ".gitattributes" => {
            classification(Configuration, Some(GitMetadata), WellKnownFileName)
        }
        ".editorconfig" | ".env" | ".env.example" | "requirements.txt" => {
            classification(Configuration, None, WellKnownFileName)
        }
        "readme" | "license" | "changelog" | "contributing" | "authors" | "code_of_conduct"
        | "security" => classification(Documentation, None, WellKnownFileName),
        _ if name.starts_with("dockerfile.") => {
            classification(Configuration, Some(Dockerfile), WellKnownFileName)
        }
        _ => return None,
    };
    Some(value)
}

fn classify_extension(extension: &str) -> FileClassification {
    use ClassificationReason::{Extension, UnknownExtension};
    use FileCategory::{Asset, Configuration, Data, Documentation, Markup, Source, Style};
    use LanguageId::*;

    match extension {
        "rs" => classification(Source, Some(Rust), Extension),
        "ts" | "tsx" | "mts" | "cts" => classification(Source, Some(TypeScript), Extension),
        "js" | "jsx" | "mjs" | "cjs" => classification(Source, Some(JavaScript), Extension),
        "py" | "pyi" => classification(Source, Some(Python), Extension),
        "go" => classification(Source, Some(Go), Extension),
        "java" => classification(Source, Some(Java), Extension),
        "c" => classification(Source, Some(C), Extension),
        "h" => classification(Source, Some(CHeader), Extension),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => classification(Source, Some(Cpp), Extension),
        "cs" => classification(Source, Some(CSharp), Extension),
        "swift" => classification(Source, Some(Swift), Extension),
        "kt" | "kts" => classification(Source, Some(Kotlin), Extension),
        "scala" | "sc" => classification(Source, Some(Scala), Extension),
        "rb" => classification(Source, Some(Ruby), Extension),
        "php" => classification(Source, Some(Php), Extension),
        "sh" | "bash" | "zsh" | "fish" => classification(Source, Some(Shell), Extension),
        "sql" => classification(Source, Some(Sql), Extension),
        "graphql" | "gql" => classification(Source, Some(Graphql), Extension),
        "proto" => classification(Data, Some(ProtocolBuffers), Extension),
        "toml" => classification(Configuration, Some(Toml), Extension),
        "yaml" | "yml" => classification(Configuration, Some(Yaml), Extension),
        "jsonc" => classification(Configuration, Some(Json), Extension),
        "ini" | "conf" | "cfg" | "properties" | "env" => {
            classification(Configuration, None, Extension)
        }
        "json" | "jsonl" | "ndjson" | "csv" | "tsv" | "parquet" | "ipynb" => {
            classification(Data, Some(Json), Extension)
        }
        "md" | "mdx" | "rst" | "adoc" => classification(Documentation, Some(Markdown), Extension),
        "html" | "htm" | "xhtml" => classification(Markup, Some(Html), Extension),
        "xml" => classification(Markup, Some(Xml), Extension),
        "vue" => classification(Markup, Some(Vue), Extension),
        "svelte" => classification(Markup, Some(Svelte), Extension),
        "css" => classification(Style, Some(Css), Extension),
        "scss" => classification(Style, Some(Scss), Extension),
        "sass" => classification(Style, Some(Sass), Extension),
        "less" => classification(Style, Some(Less), Extension),
        "svg" => classification(Asset, Some(Svg), Extension),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "tif" | "tiff" | "pdf"
        | "woff" | "woff2" | "ttf" | "otf" | "eot" | "zip" | "gz" | "wasm" | "exe" | "dll"
        | "so" | "dylib" | "a" | "lib" | "o" | "obj" | "class" | "jar" | "war" | "ear" | "bin"
        | "dat" => classification(Asset, None, Extension),
        "txt" => classification(Documentation, None, Extension),
        _ => classification(
            FileCategory::Unknown,
            None,
            if extension.is_empty() {
                ClassificationReason::NoExtension
            } else {
                UnknownExtension
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn classify(path: &str) -> FileClassification {
        classify_path(Path::new(path))
    }

    #[test]
    fn classifies_primary_source_languages() {
        for (path, language) in [
            ("src/lib.rs", LanguageId::Rust),
            ("src/app.tsx", LanguageId::TypeScript),
            ("src/index.mjs", LanguageId::JavaScript),
            ("tools/index.py", LanguageId::Python),
            ("cmd/server.go", LanguageId::Go),
            ("src/Main.java", LanguageId::Java),
            ("src/main.cpp", LanguageId::Cpp),
            ("src/main.swift", LanguageId::Swift),
            ("src/main.kt", LanguageId::Kotlin),
        ] {
            let result = classify(path);
            assert_eq!(result.category, FileCategory::Source, "{path}");
            assert_eq!(result.language, Some(language), "{path}");
            assert_eq!(result.reason, ClassificationReason::Extension, "{path}");
        }
    }

    #[test]
    fn classifies_common_project_manifests_by_name() {
        for (path, language) in [
            ("Cargo.toml", Some(LanguageId::Toml)),
            ("package.json", Some(LanguageId::Json)),
            ("go.mod", Some(LanguageId::Go)),
            ("pyproject.toml", Some(LanguageId::Toml)),
            ("Dockerfile.dev", Some(LanguageId::Dockerfile)),
        ] {
            let result = classify(path);
            assert_eq!(result.category, FileCategory::Configuration, "{path}");
            assert_eq!(result.language, language, "{path}");
            assert_eq!(
                result.reason,
                ClassificationReason::WellKnownFileName,
                "{path}"
            );
        }
    }

    #[test]
    fn classifies_documentation_data_markup_styles_and_assets() {
        for (path, category) in [
            ("README.md", FileCategory::Documentation),
            ("data/events.jsonl", FileCategory::Data),
            ("docs/schema.xml", FileCategory::Markup),
            ("web/site.scss", FileCategory::Style),
            ("assets/logo.svg", FileCategory::Asset),
            ("assets/photo.png", FileCategory::Asset),
        ] {
            assert_eq!(classify(path).category, category, "{path}");
        }
    }

    #[test]
    fn unknown_and_extensionless_files_remain_explicitly_unknown() {
        assert_eq!(
            classify("data/custom.blob").reason,
            ClassificationReason::UnknownExtension
        );
        assert_eq!(classify("LICENSE").category, FileCategory::Documentation);
        assert_eq!(
            classify("LICENSE").reason,
            ClassificationReason::WellKnownFileName
        );
        assert_eq!(
            classify("unknownfile").reason,
            ClassificationReason::NoExtension
        );
    }

    #[test]
    fn extension_matching_is_ascii_case_insensitive() {
        assert_eq!(classify("src/MAIN.RS").language, Some(LanguageId::Rust));
    }

    #[test]
    fn entry_classifier_skips_directories() {
        let entry = WorkspaceEntry {
            relative_path: PathBuf::from("src"),
            kind: WorkspaceEntryKind::Directory,
            size_bytes: None,
        };
        assert_eq!(classify_entry(&entry), None);
    }

    #[test]
    fn non_utf8_file_names_are_not_guessed() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = Path::new(std::ffi::OsStr::from_bytes(b"source.\xff"));
            assert_eq!(classify_path(path).category, FileCategory::Unknown);
        }
    }

    #[test]
    fn classifies_justfiles_and_jsonc_as_configuration() {
        assert_eq!(classify("Justfile").language, Some(LanguageId::Justfile));
        assert_eq!(
            classify("tsconfig.jsonc").category,
            FileCategory::Configuration
        );
    }
}
