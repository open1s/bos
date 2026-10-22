//! The right panel's workspace tree: listing, typing and mutation.
//!
//! This is the framework-free half of Workspace Files. It knows how to walk a
//! directory, how a name maps onto an editor language, and how a row is
//! created, renamed or deleted - and nothing about GPUI, so every rule here is
//! testable against a scratch directory.
//!
//! Two properties are load-bearing and neither is a formatting detail:
//!
//! * Every path the panel touches is [`resolve`]d against the workspace root,
//!   so a name built from user input cannot walk the panel out of its own
//!   workspace. The check is lexical - `..` is collapsed before it is joined -
//!   which is the right scope for a viewer the reader drives by hand; the agent
//!   tools keep their own canonical check because they take paths from a model.
//! * A file is classified before it is read. The editor is a text control, so
//!   a binary or an oversized file is refused up front rather than being pasted
//!   into a rope as mojibake.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The directories the listing never descends into.
///
/// The same set the `@` reference lookup skips: version control, build output
/// and dependency trees are noise in a file list and ruin its first paint.
const SKIP: [&str; 7] = [
    ".git",
    ".jj",
    "target",
    "node_modules",
    "dist",
    "build",
    ".DS_Store",
];

/// The ceiling on one directory listing, so a pathological folder cannot
/// stall the frame that asked for it.
const MAX_ROWS: usize = 2_000;

/// The largest file the editor will open, in bytes.
///
/// Past this the rope would cost more than the reading is worth and the frame
/// would stall on the read, so the panel refuses and says why instead.
pub(crate) const MAX_OPEN_BYTES: u64 = 4 * 1024 * 1024;

/// One row of the panel's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// The file or directory name, without any parent path.
    pub(crate) name: String,
    /// Whether the row is a directory.
    pub(crate) dir: bool,
    /// The size in bytes, zero for a directory.
    pub(crate) size: u64,
}

/// How the panel opens a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Editable text, carrying the tree-sitter language id to highlight it
    /// with. `None` is a text file no grammar covers, which the editor shows
    /// unhighlighted rather than refusing.
    Text(Option<&'static str>),
    /// An image, which the pane renders as a picture rather than a rope.
    Image,
    /// Anything the pane will not show: a binary, or a file too large to load.
    /// The panel reports the file rather than the reason, so the wording is
    /// the caller's to pick.
    Opaque,
}

/// Join a path onto a workspace root without ever leaving it.
///
/// `rel` is collapsed lexically and refused outright when it does not stay
/// under `root`: a rooted or prefixed path is rejected, and a `..` component
/// that would climb out of the root is rejected rather than clamped, so a
/// traversal attempt reads as a refusal instead of quietly landing elsewhere.
/// Every read and write the panel performs goes through this, which is what
/// keeps a name typed into the rename field inside the workspace.
///
/// The check is lexical, so a symlink inside the workspace that points out of
/// it is followed. That is the right scope for a viewer the reader drives by
/// hand; the agent's own file tools take paths from a model and keep their
/// canonical check for that reason.
pub(crate) fn resolve(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
                if !out.starts_with(root) {
                    return None;
                }
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    out.starts_with(root).then_some(out)
}

/// One directory's rows, directories first and each group by name.
///
/// Hidden entries are skipped unless asked for; the skipped directories are
/// skipped by name; and the listing is capped. A directory that cannot be read
/// is an error the caller reports, not an empty listing the reader has to
/// guess at.
pub(crate) fn list(dir: &Path, show_hidden: bool) -> io::Result<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        if SKIP.contains(&name.as_str()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let dir = meta.is_dir();
        rows.push(Row {
            name,
            dir,
            size: if dir { 0 } else { meta.len() },
        });
        if rows.len() >= MAX_ROWS {
            break;
        }
    }
    // The reference's own tree is folders above files, both alphabetical, which
    // is what makes a directory's shape readable at a glance.
    rows.sort_by(|left, right| {
        right
            .dir
            .cmp(&left.dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(rows)
}

/// How a file of this name and size opens in the panel.
///
/// The extension decides the language and the image family; the size decides
/// whether an otherwise editable file is refused, because the editor is a
/// rope and a 40MB log would be pasted into one.
pub(crate) fn kind_for(name: &str, size: u64) -> Kind {
    if size > MAX_OPEN_BYTES {
        return Kind::Opaque;
    }
    if image_extension(name) {
        return Kind::Image;
    }
    // A file with a NUL in its first bytes is binary whatever its name says;
    // that is decided on read, here the name is all there is.
    Kind::Text(language_for(name))
}

/// The tree-sitter language id a filename maps to, or none for plain text.
///
/// The ids are exactly the ones the kit's registry registers - the same list
/// `gpui-component`'s `Language` enum builds in - because a name the registry
/// does not know resolves to a config with no grammar, which is a file that
/// opens unhighlighted while looking like it should not. `tree-sitter-languages`
/// is what makes the whole list real; see the dependency note in Cargo.toml.
///
/// A name is matched in three steps, in this order:
/// 1. the bare file name, for the extensionless files a language owns
///    (`Makefile`, `CMakeLists.txt`),
/// 2. the extension after the last dot, case-blind,
/// 3. nothing - an unknown extension is PLAIN TEXT, not a guess. The editor
///    still opens it, edits it and saves it; it simply does not colour it.
pub(crate) fn language_for(name: &str) -> Option<&'static str> {
    if let Some(language) = language_for_name(name) {
        return Some(language);
    }
    let extension = Path::new(name)
        .file_name()
        .and_then(|file| file.to_str())
        .and_then(|file| file.rsplit_once('.').map(|(_, ext)| ext))?;
    language_for_extension(&extension.to_ascii_lowercase())
}

/// The languages a whole file name owns, for files that carry no extension.
fn language_for_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let language = match lower.as_str() {
        "makefile" | "gnumakefile" | "bsdmakefile" => "make",
        "cmakelists.txt" => "cmake",
        // The kit has no Dockerfile, nginx, terraform or justfile grammar, so
        // those names stay plain text rather than claiming a language that the
        // registry would then fail to parse.
        _ => return None,
    };
    Some(language)
}

/// The language an extension owns, in the registry's own spelling.
fn language_for_extension(extension: &str) -> Option<&'static str> {
    let language = match extension {
        // The plain-text names: a file the reader expects to read as text but no
        // grammar covers, so it opens unhighlighted rather than not at all.
        "txt" | "text" | "log" | "ini" | "cfg" | "conf" | "env" | "gitignore" | "gitattributes"
        | "editorconfig" | "csv" | "tsv" | "rtf" => return None,
        "rs" => "rust",
        "py" | "pyi" | "pyw" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" | "json5" | "jsonl" | "ndjson" | "geojson" | "webmanifest" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "mdx" | "markdown" | "mkd" => "markdown",
        "html" | "htm" | "xhtml" | "vue" => "html",
        "astro" => "astro",
        "svelte" => "svelte",
        "ejs" => "ejs",
        "erb" => "erb",
        "css" | "scss" | "less" => "css",
        "jsdoc" => "jsdoc",
        "go" => "go",
        "sh" | "bash" | "zsh" | "ksh" => "bash",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "tpp" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" | "ktm" => "kotlin",
        "scala" | "sc" | "sbt" => "scala",
        "swift" => "swift",
        "rb" | "rake" | "gemspec" | "ru" => "ruby",
        "php" | "php3" | "php4" | "php5" | "phtml" => "php",
        "lua" => "lua",
        "ex" | "exs" => "elixir",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" | "protobuf" => "proto",
        "zig" => "zig",
        "cmake" => "cmake",
        "mk" => "make",
        "diff" | "patch" => "diff",
        "xml" | "svg" | "xsl" | "xslt" | "plist" | "rss" | "atom" => "html",
        _ => return None,
    };
    Some(language)
}

/// Whether a filename names an image the pane renders as a picture.
pub(crate) fn image_extension(name: &str) -> bool {
    let Some(extension) = Path::new(name)
        .file_name()
        .and_then(|file| file.to_str())
        .and_then(|file| file.rsplit_once('.').map(|(_, ext)| ext))
    else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
    )
}

/// Whether the bytes are text the editor can hold.
///
/// NUL in the first 8KiB is the portable test: a UTF-8 file never contains
/// one, and every binary format the panel will meet either starts with them or
/// settles into them quickly. A file that is not valid UTF-8 is refused too,
/// because the rope stores strings and lossy replacement would corrupt a save.
pub(crate) fn is_text(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8 * 1024)];
    !probe.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// Create an empty file, refusing a name that is blank or escapes the root.
pub(crate) fn create(root: &Path, rel: &Path) -> io::Result<PathBuf> {
    let path = resolve(root, rel).ok_or_else(|| io_err("invalid name"))?;
    if rel.as_os_str().is_empty() {
        return Err(io_err("invalid name"));
    }
    if path.exists() {
        return Err(io_err("already exists"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, b"")?;
    Ok(path)
}

/// Rename a row, refusing a name that is blank, escapes the root or exists.
pub(crate) fn rename(root: &Path, from: &Path, to: &Path) -> io::Result<PathBuf> {
    let source = resolve(root, from).ok_or_else(|| io_err("invalid name"))?;
    let target = resolve(root, to).ok_or_else(|| io_err("invalid name"))?;
    if to.as_os_str().is_empty() {
        return Err(io_err("invalid name"));
    }
    if target.exists() {
        return Err(io_err("already exists"));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&source, &target)?;
    Ok(target)
}

/// Delete a file or an empty directory. A directory with contents is refused,
/// so the panel never becomes a recursive delete with one stray click.
pub(crate) fn delete(root: &Path, rel: &Path) -> io::Result<()> {
    let path = resolve(root, rel).ok_or_else(|| io_err("invalid name"))?;
    if path.is_dir() {
        fs::remove_dir(&path)
    } else {
        fs::remove_file(&path)
    }
}

/// Read a file's bytes, refusing one that is not a file under the root.
pub(crate) fn read(root: &Path, rel: &Path) -> io::Result<Vec<u8>> {
    let path = resolve(root, rel).ok_or_else(|| io_err("invalid name"))?;
    if !path.is_file() {
        return Err(io_err("not a file"));
    }
    fs::read(path)
}

/// Write a file's text, refusing a path that leaves the root.
pub(crate) fn write(root: &Path, rel: &Path, text: &str) -> io::Result<()> {
    let path = resolve(root, rel).ok_or_else(|| io_err("invalid name"))?;
    fs::write(path, text)
}

/// The error every helper here returns for a path or name it refuses.
fn io_err(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory the tests build their fixtures in.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-files-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A resolved path stays inside the root, and a traversal cannot leave it.
    #[test]
    fn resolve_confines_every_path_to_the_root() {
        let root = Path::new("/ws");
        assert_eq!(
            resolve(root, Path::new("src/lib.rs")),
            Some(root.join("src/lib.rs"))
        );
        assert_eq!(
            resolve(root, Path::new("./src/./lib.rs")),
            Some(root.join("src/lib.rs"))
        );
        // A `..` that stays inside collapses normally.
        assert_eq!(
            resolve(root, Path::new("src/../Cargo.toml")),
            Some(root.join("Cargo.toml"))
        );
        // One that would climb out is refused rather than clamped.
        assert_eq!(resolve(root, Path::new("../etc/passwd")), None);
        assert_eq!(
            resolve(root, Path::new("src/../../etc/passwd")),
            None,
            "a traversal that climbs out anywhere is refused"
        );
        assert_eq!(
            resolve(root, Path::new("/etc/passwd")),
            None,
            "a rooted path is refused"
        );
    }

    /// The listing puts directories first, both groups alphabetical, and
    /// skips hidden entries and the build directories by name.
    #[test]
    fn the_listing_is_sorted_and_skips_noise() {
        let root = scratch("listing");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("Crates")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("Cargo.toml"), b"").unwrap();
        fs::write(root.join("alpha.rs"), b"").unwrap();
        fs::write(root.join(".hidden"), b"").unwrap();
        fs::write(root.join("README.md"), b"").unwrap();

        let rows = list(&root, false).expect("lists");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        // Case-insensitive, which is how a file manager sorts and what a
        // reader's eye expects: `alpha.rs` sorts under a, not under A.
        assert_eq!(
            names,
            vec!["Crates", "src", "alpha.rs", "Cargo.toml", "README.md"]
        );
        assert!(rows[0].dir && rows[1].dir, "directories lead the listing");
        assert!(!rows[2].dir, "files follow");
    }

    /// Asking for hidden entries brings them back without lifting the skips
    /// that keep the listing readable.
    #[test]
    fn hidden_entries_are_opt_in() {
        let root = scratch("hidden");
        fs::write(root.join(".env"), b"A=1\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(list(&root, false).unwrap().len(), 0);
        let shown = list(&root, true).unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].name, ".env");
    }

    /// Every extension the panel claims to edit resolves to a language the
    /// kit's registry knows, and plain-text names resolve to none rather than
    /// to a grammar that will not load.
    #[test]
    fn extensions_map_to_languages() {
        assert_eq!(language_for("lib.rs"), Some("rust"));
        assert_eq!(language_for("Cargo.toml"), Some("toml"));
        assert_eq!(language_for("Cargo.lock"), Some("toml"));
        assert_eq!(
            language_for("main.rs.bak"),
            None,
            "an unknown extension is plain"
        );
        assert_eq!(language_for("no-extension"), None);
        assert_eq!(
            language_for(".gitignore"),
            None,
            "a dotfile has no extension"
        );
        for name in [
            "lib.rs", "app.py", "a.ts", "a.tsx", "d.json", "R.md", "i.html", "s.css",
        ] {
            assert!(
                language_for(name).is_some(),
                "{name} should map to a grammar"
            );
        }
        for name in ["notes.txt", "server.conf", "setup.ini"] {
            assert_eq!(language_for(name), None, "{name} is plain text");
        }
    }

    /// Every grammar the kit COMPILED IN is reachable from a file name.
    ///
    /// This is the contract the panel needs and the one that was silently
    /// missing: `tree-sitter-languages` can be off - the kit then registers the
    /// same language NAMES with no grammar behind them - so a table that maps
    /// extensions to ids proves nothing on its own. The list below is
    /// `gpui-component`'s own `Language` enum; a name missing from it is a
    /// language the editor cannot highlight at all, so the two have to be kept
    /// in step by hand.
    #[test]
    fn every_bundled_grammar_has_an_extension() {
        const GRAMMARS: [&str; 34] = [
            "astro", "bash", "c", "cmake", "cpp", "csharp", "css", "diff", "ejs", "elixir",
            "erb", "go", "graphql", "html", "java", "javascript", "jsdoc", "kotlin", "lua",
            "make", "markdown", "php", "proto", "python", "ruby", "rust", "scala", "sql",
            "svelte", "swift", "toml", "tsx", "typescript", "yaml",
        ];
        // json is always compiled in, and zig is registered by the bundle too.
        let mut covered: Vec<&'static str> = vec!["json", "zig"];
        for name in [
            "a.astro", "a.sh", "a.c", "a.cmake", "a.cpp", "a.cs", "a.css", "a.diff", "a.ejs",
            "a.ex", "a.erb", "a.go", "a.graphql", "a.html", "a.java", "a.js", "a.jsdoc",
            "a.kt", "a.lua", "a.mk", "a.md", "a.php", "a.proto", "a.py", "a.rb", "a.rs",
            "a.scala", "a.sql", "a.svelte", "a.swift", "a.toml", "a.tsx", "a.ts", "a.yaml",
            "a.json", "a.zig",
        ] {
            if let Some(language) = language_for(name) {
                covered.push(language);
            }
        }
        for grammar in GRAMMARS {
            assert!(
                covered.contains(&grammar),
                "no file name maps to the {grammar} grammar the editor ships"
            );
        }
        // The three extensionless names that own a language.
        assert_eq!(language_for("Makefile"), Some("make"));
        assert_eq!(language_for("CMakeLists.txt"), Some("cmake"));
    }

    /// Images are recognised apart from the editor's text families.
    #[test]
    fn images_are_their_own_family() {
        assert_eq!(kind_for("shot.png", 100), Kind::Image);
        assert_eq!(
            kind_for("SHOT.JPG", 100),
            Kind::Image,
            "the check is case-blind"
        );
        assert_eq!(
            kind_for("shot.svg", 100),
            Kind::Text(None),
            "svg is text the editor shows"
        );
        assert!(image_extension("a.webp"));
        assert!(!image_extension("a.rs"));
        assert!(!image_extension("nope"));
    }

    /// An oversized file is refused before it is read, not after.
    #[test]
    fn an_oversized_file_is_opaque() {
        assert_eq!(kind_for("huge.log", MAX_OPEN_BYTES), Kind::Text(None));
        assert_eq!(kind_for("huge.log", MAX_OPEN_BYTES + 1), Kind::Opaque);
    }

    /// The text test refuses NUL-bearing and non-UTF-8 bytes and accepts both
    /// an ASCII and a multi-byte file.
    #[test]
    fn text_is_tested_before_it_is_opened() {
        assert!(is_text(b"fn main() {}\n"));
        assert!(is_text("héllo\n".as_bytes()), "valid UTF-8 is text");
        assert!(
            !is_text(&[0x7f, b'E', b'L', b'F', 0, 0, 0]),
            "an ELF header is not text"
        );
        assert!(!is_text(&[0xff, 0xfe, 0x00]), "invalid UTF-8 is not text");
    }

    /// Create, rename and delete round-trip, and each refuses the name that
    /// would leave the workspace or collide with what is there.
    #[test]
    fn mutations_stay_in_the_root_and_refuse_collisions() {
        let root = scratch("mutate");
        let made = create(&root, Path::new("new.rs")).expect("creates");
        assert!(made.starts_with(&root) && made.is_file());
        assert!(
            create(&root, Path::new("new.rs")).is_err(),
            "a repeat is refused"
        );
        assert!(
            create(&root, Path::new("../escape.rs")).is_err(),
            "a traversal is confined"
        );

        let renamed = rename(&root, Path::new("new.rs"), Path::new("renamed.rs")).expect("renames");
        assert!(renamed.is_file() && !made.exists());
        assert!(rename(&root, Path::new("missing.rs"), Path::new("x.rs")).is_err());

        fs::write(root.join("doomed.rs"), b"").unwrap();
        delete(&root, Path::new("doomed.rs")).expect("deletes");
        assert!(!root.join("doomed.rs").exists());

        fs::create_dir_all(root.join("full")).unwrap();
        fs::write(root.join("full/child"), b"").unwrap();
        assert!(
            delete(&root, Path::new("full")).is_err(),
            "a full directory is refused"
        );
        delete(&root, Path::new("full/child")).unwrap();
        delete(&root, Path::new("full")).expect("an empty directory goes");
    }

    /// Reading refuses a directory and a path outside the root, and writing
    /// lands inside it.
    #[test]
    fn reads_and_writes_are_confined() {
        let root = scratch("io");
        fs::create_dir_all(root.join("dir")).unwrap();
        assert!(
            read(&root, Path::new("dir")).is_err(),
            "a directory is not a file"
        );
        assert!(read(&root, Path::new("../outside")).is_err());
        write(&root, Path::new("dir/note.md"), "# hi\n").expect("writes");
        assert_eq!(read(&root, Path::new("dir/note.md")).unwrap(), b"# hi\n");
    }
}
