//! The glob tool's search: which files match a path pattern.
//!
//! The walk and the matcher are ripgrep's own ignore crate - the engine behind
//! rg --files - through WalkBuilder and overrides::OverrideBuilder. That buys
//! the three facts a file search is judged on, all well tested upstream:
//! .gitignore / .ignore / .rgignore handling, hidden-entry policy, and a walk
//! that does not follow symlinks. The glob semantics are ripgrep's -g ones: a
//! pattern with no separator matches the FILE NAME at any depth, one with a
//! separator is anchored to the search base, a single star stops at a
//! separator and a double star crosses directories. This module owns only what
//! is specific to the tool: the one-include-pattern contract, the result bound,
//! the directories the metadata paths never surrender, and the refusal for a
//! pattern that cannot compile.
//!
//! The ripgrep crate on crates.io is a binary-only package (autolib = false,
//! its only target is rg), so there is no library to call inside it; ignore is
//! its reusable half, and the globset matcher underneath is already in the
//! workspace lock.

use std::path::{Path, PathBuf};

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// The most bytes one pattern may carry.
///
/// A path pattern is a search filter, not a program: past this the reader has
/// pasted a file list into the field, and compiling it would cost more than the
/// search answers.
pub(crate) const MAX_PATTERN_BYTES: usize = 4_096;

/// The most paths one call may return.
pub(crate) const MAX_RESULTS: usize = 1_000;

/// The default number of paths one call returns.
pub(crate) const DEFAULT_RESULTS: usize = 100;

/// The matches a call collects before it stops and reports truncation.
///
/// The walk must finish before the result can be sorted, so this is the memory
/// bound on that sort: 10k relative paths is a few hundred kilobytes, where an
/// unbounded walk over a dependency tree is not.
const MAX_CANDIDATES: usize = 10_000;

/// Entries that are never returned, even when ignored files are requested.
///
/// Version-control internals are unreadable to the file tools and .bsh holds
/// the workspace policy itself; listing any of them would hand the model names
/// it cannot then open. BOS develops in jj, so .jj is VCS metadata here too.
const ALWAYS_SKIP: [&str; 3] = [".git", ".jj", ".bsh"];

/// Dependency, build and cache trees the walk does not descend into by
/// default.
///
/// This is the build/dependency half of zvec-grep's default exclusions. The
/// content directories its indexing policy also skips (locales, logs, tmp,
/// generated) are deliberately NOT here: a file search may legitimately want
/// them, and hiding them would look like a broken pattern. An explicit search
/// root is exempt, so globbing inside one of these stays possible.
const DEFAULT_SKIP_DIRS: [&str; 27] = [
    "node_modules",
    "vendor",
    "thirdparty",
    "third_party",
    "external",
    "deps",
    "dist",
    "build",
    "out",
    "target",
    "coverage",
    "__pycache__",
    "venv",
    ".venv",
    ".tox",
    ".eggs",
    "Pods",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".vite",
    ".parcel-cache",
    ".cache",
    ".gradle",
    ".pytest_cache",
    ".mypy_cache",
];

/// One glob question.
#[derive(Debug)]
pub(crate) struct Query<'a> {
    /// The glob pattern, relative to the search base.
    pub(crate) pattern: &'a str,
    /// Whether hidden and ignored entries are searched too.
    pub(crate) include_ignored: bool,
    /// The most paths to return; clamped to MAX_RESULTS.
    pub(crate) limit: usize,
}

/// What one search found.
#[derive(Debug)]
pub(crate) struct Found {
    /// Matching paths, relative to the search base and sorted.
    pub(crate) paths: Vec<PathBuf>,
    /// Whether more matches existed than the result limit returned.
    pub(crate) truncated: bool,
}

/// Find the files under root whose paths match the query.
///
/// The base is what an anchored pattern is relative to: the workspace root for
/// a confined build, or the search root itself when no workspace is installed.
/// The walk never leaves the root and never follows a symlink, so a link inside
/// the workspace cannot turn a listing into a read of the outside.
pub(crate) fn search(root: &Path, base: &Path, query: &Query) -> Result<Found, String> {
    if query.pattern.trim().is_empty() {
        return Err("pattern must not be empty".to_string());
    }
    if query.pattern.len() > MAX_PATTERN_BYTES {
        return Err(format!(
            "pattern is {} bytes, past the {MAX_PATTERN_BYTES} byte ceiling",
            query.pattern.len()
        ));
    }
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    // One include pattern is the tool's contract. Override globs read a leading
    // ! as an exclusion (ripgrep's -g), and a lone exclusion would match
    // nothing, so it is refused by name instead of returning a silent empty
    // list.
    if query.pattern.starts_with('!') {
        return Err("a leading ! is an exclusion; pass the pattern to include instead".to_string());
    }
    let mut overrides = OverrideBuilder::new(base);
    overrides
        .add(query.pattern)
        .map_err(|error| format!("invalid glob {:?}: {error}", query.pattern))?;
    let overrides = overrides
        .build()
        .map_err(|error| format!("invalid glob {:?}: {error}", query.pattern))?;

    let limit = query.limit.clamp(1, MAX_RESULTS);
    let mut found: Vec<PathBuf> = Vec::new();
    let mut capped = false;
    for result in build_walker(root, query.include_ignored).build() {
        let Ok(entry) = result else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            continue;
        }
        // A symlink to a directory is not a file to read, and following it is
        // exactly what this walk refuses to do.
        if file_type.is_symlink() && std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
        {
            continue;
        }
        let relative = entry.path().strip_prefix(base).unwrap_or(entry.path());
        if !overrides.matched(relative, false).is_whitelist() {
            continue;
        }
        if found.len() >= MAX_CANDIDATES {
            capped = true;
            break;
        }
        found.push(relative_to(base, entry.path()));
    }
    found.sort();
    let truncated = capped || found.len() > limit;
    found.truncate(limit);
    Ok(Found {
        paths: found,
        truncated,
    })
}

/// The ripgrep walk the glob and grep tools share.
///
/// One policy for both: ignore files and hidden entries are respected unless
/// asked otherwise, symlinks are never followed, the build/dependency trees are
/// skipped, and the VCS/policy metadata is never handed back.
pub(crate) fn build_walker(root: &Path, include_ignored: bool) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!include_ignored)
        .follow_links(false)
        // A .gitignore is a fact about the directory, not about whether the
        // directory is a git repository: a workspace that has the file and no
        // .git still means its rules.
        .require_git(false);
    if include_ignored {
        builder
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false);
    } else {
        // ripgrep's third ignore file, beside .gitignore and .ignore.
        builder.add_custom_ignore_filename(".rgignore");
    }
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        if ALWAYS_SKIP.iter().any(|skip| *skip == name) {
            return false;
        }
        if include_ignored {
            return true;
        }
        !(entry.file_type().is_some_and(|kind| kind.is_dir())
            && DEFAULT_SKIP_DIRS.iter().any(|skip| *skip == name))
    });
    builder
}

/// A path as the tools report it: relative to the base when it lives there.
pub(crate) fn relative_to(base: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory the tests build their fixtures in.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-glob-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn search_all(dir: &Path, pattern: &str, include_ignored: bool, limit: usize) -> Found {
        search(
            dir,
            dir,
            &Query {
                pattern,
                include_ignored,
                limit,
            },
        )
        .expect("search succeeds")
    }

    fn names(found: &Found) -> Vec<String> {
        found
            .paths
            .iter()
            .map(|path| {
                path.to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            })
            .collect()
    }

    /// A pattern without a separator matches the file NAME at any depth; a
    /// separator anchors a single star to one level.
    #[test]
    fn a_pattern_without_a_separator_matches_any_depth() {
        let dir = scratch("depth");
        std::fs::create_dir_all(dir.join("src").join("deep")).expect("dirs");
        std::fs::write(dir.join("root.rs"), "").expect("root");
        std::fs::write(dir.join("src").join("one.rs"), "").expect("one");
        std::fs::write(dir.join("src").join("deep").join("two.rs"), "").expect("two");
        std::fs::write(dir.join("README.md"), "").expect("readme");

        assert_eq!(
            names(&search_all(&dir, "*.rs", false, 100)),
            vec!["root.rs", "src/deep/two.rs", "src/one.rs"]
        );
        assert_eq!(
            names(&search_all(&dir, "src/*.rs", false, 100)),
            vec!["src/one.rs"],
            "a separator anchors the star to one level"
        );
        assert_eq!(
            names(&search_all(&dir, "src/**/*.rs", false, 100)),
            vec!["src/deep/two.rs", "src/one.rs"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ignored and hidden entries are skipped by default; include_ignored
    /// brings both back, which is the one switch the tool exposes.
    #[test]
    fn ignored_and_hidden_entries_are_opt_in() {
        let dir = scratch("ignored");
        std::fs::create_dir_all(dir.join("generated")).expect("generated");
        std::fs::write(dir.join(".gitignore"), "generated/\n").expect("gitignore");
        std::fs::write(dir.join("visible.rs"), "").expect("visible");
        std::fs::write(dir.join("generated").join("built.rs"), "").expect("built");
        std::fs::write(dir.join(".hidden.rs"), "").expect("hidden");

        assert_eq!(
            names(&search_all(&dir, "*.rs", false, 100)),
            vec!["visible.rs"]
        );
        assert_eq!(
            names(&search_all(&dir, "*.rs", true, 100)),
            vec![".hidden.rs", "generated/built.rs", "visible.rs"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dependency and build trees zvec-grep's scan policy excludes are
    /// skipped by default; include_ignored lifts them, and an explicit root is
    /// always walked.
    #[test]
    fn build_and_dependency_trees_are_skipped_until_asked() {
        let dir = scratch("defaults");
        std::fs::create_dir_all(dir.join("node_modules").join("pkg")).expect("node_modules");
        std::fs::create_dir_all(dir.join("target")).expect("target");
        std::fs::write(dir.join("node_modules").join("pkg").join("index.rs"), "")
            .expect("dependency file");
        std::fs::write(dir.join("target").join("built.rs"), "").expect("build file");
        std::fs::write(dir.join("src.rs"), "").expect("source");

        assert_eq!(names(&search_all(&dir, "*.rs", false, 100)), vec!["src.rs"]);
        assert_eq!(
            names(&search_all(&dir, "*.rs", true, 100)),
            vec!["node_modules/pkg/index.rs", "src.rs", "target/built.rs"]
        );

        let target = dir.join("target");
        let inside = search(
            &target,
            &target,
            &Query {
                pattern: "*.rs",
                include_ignored: false,
                limit: 100,
            },
        )
        .expect("an explicit root is walked");
        assert_eq!(
            names(&inside),
            vec!["built.rs"],
            "an explicit root is exempt from the default exclusions"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The metadata directories never surrender a path, even to a request for
    /// ignored files: the file tools cannot read them and the policy refuses.
    #[test]
    fn version_control_and_policy_metadata_are_never_returned() {
        let dir = scratch("metadata");
        std::fs::create_dir_all(dir.join(".git")).expect(".git");
        std::fs::create_dir_all(dir.join(".jj")).expect(".jj");
        std::fs::create_dir_all(dir.join(".bsh")).expect(".bsh");
        std::fs::write(dir.join(".git").join("config"), "").expect("config");
        std::fs::write(dir.join(".jj").join("repo"), "").expect("repo");
        std::fs::write(dir.join(".bsh").join("policy.rego"), "").expect("policy");
        std::fs::write(dir.join("notes.md"), "").expect("notes");

        let found = search_all(&dir, "**/*", true, 100);
        assert_eq!(names(&found), vec!["notes.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlinked directory is not walked, so a link inside the workspace
    /// cannot turn a listing into a read of the outside.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_directory_is_not_followed() {
        let dir = scratch("symlink");
        let outside = scratch("symlink-outside");
        std::fs::write(outside.join("secret.rs"), "").expect("secret");
        std::os::unix::fs::symlink(&outside, dir.join("bridge")).expect("symlink");
        std::fs::write(dir.join("inside.rs"), "").expect("inside");

        assert_eq!(
            names(&search_all(&dir, "*.rs", false, 100)),
            vec!["inside.rs"],
            "the walk never leaves the searched tree"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// The result is sorted, bounded, and says when it stopped early.
    #[test]
    fn the_result_is_sorted_bounded_and_flagged() {
        let dir = scratch("bound");
        for name in ["d.rs", "a.rs", "c.rs", "b.rs"] {
            std::fs::write(dir.join(name), "").expect("file");
        }
        let found = search_all(&dir, "*.rs", false, 2);
        assert_eq!(names(&found), vec!["a.rs", "b.rs"]);
        assert!(found.truncated, "more matches existed than the limit");

        let all = search_all(&dir, "*.rs", false, 100);
        assert_eq!(names(&all), vec!["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert!(!all.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pattern that cannot compile is refused with the pattern in the
    /// message, not silently treated as matching nothing.
    #[test]
    fn an_invalid_pattern_names_itself() {
        let dir = scratch("invalid");
        let error = search(
            &dir,
            &dir,
            &Query {
                pattern: "[unclosed",
                include_ignored: false,
                limit: 10,
            },
        )
        .expect_err("an unclosed class does not compile");
        assert!(error.contains("invalid glob"), "{error}");
        assert!(error.contains("[unclosed"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Empty and oversized patterns are refused before any walk starts.
    #[test]
    fn an_empty_or_oversized_pattern_is_refused() {
        let dir = scratch("empty");
        let empty = search(
            &dir,
            &dir,
            &Query {
                pattern: "   ",
                include_ignored: false,
                limit: 10,
            },
        );
        assert!(empty.err().is_some_and(|error| error.contains("empty")));

        let long = "a".repeat(MAX_PATTERN_BYTES + 1);
        let oversized = search(
            &dir,
            &dir,
            &Query {
                pattern: &long,
                include_ignored: false,
                limit: 10,
            },
        );
        assert!(oversized
            .err()
            .is_some_and(|error| error.contains("ceiling")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file path is not a walk root; the tool says so instead of returning
    /// the file itself as its only match.
    #[test]
    fn a_file_root_is_refused() {
        let dir = scratch("file-root");
        let file = dir.join("one.rs");
        std::fs::write(&file, "").expect("file");
        let error = search(
            &file,
            &dir,
            &Query {
                pattern: "*.rs",
                include_ignored: false,
                limit: 10,
            },
        )
        .expect_err("a file is not a directory");
        assert!(error.contains("not a directory"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual parity check against the real ripgrep binary:
    ///
    ///     cargo test -p bsh --lib glob::tests::the_walk_agrees_with_rg -- --ignored
    ///
    /// The tool's contract is rg --files -g PATTERN: basename matching when the
    /// pattern has no separator, one-level anchoring when it does, and `**` for
    /// depth. --no-require-git mirrors the walk's require_git(false).
    #[test]
    #[ignore = "manual: needs rg on PATH"]
    fn the_walk_agrees_with_rg() {
        let dir = scratch("rg-parity");
        std::fs::create_dir_all(dir.join("src").join("deep")).expect("dirs");
        std::fs::write(dir.join("root.rs"), "").expect("root");
        std::fs::write(dir.join("src").join("one.rs"), "").expect("one");
        std::fs::write(dir.join("src").join("deep").join("two.rs"), "").expect("two");
        std::fs::write(dir.join("notes.md"), "").expect("notes");

        for pattern in ["*.rs", "src/*.rs", "src/**/*.rs"] {
            let expected = rg_files(&dir, pattern);
            let found = names(&search_all(&dir, pattern, false, 100));
            assert_eq!(found, expected, "pattern {pattern}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The paths rg reports for one pattern, sorted and stripped of "./".
    fn rg_files(dir: &Path, pattern: &str) -> Vec<String> {
        let output = std::process::Command::new("rg")
            .current_dir(dir)
            .args(["--files", "--no-require-git", "-g", pattern])
            .output()
            .expect("rg is on PATH");
        assert!(
            output.status.success(),
            "rg failed for {pattern}: {:?}",
            output
        );
        let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim_start_matches("./").to_string())
            .collect();
        paths.sort();
        paths
    }

    /// A lone exclusion is refused: a single-pattern discovery call cannot mean
    /// "everything except this", and returning nothing would look like a miss.
    #[test]
    fn a_leading_exclusion_is_refused() {
        let dir = scratch("exclusion");
        let error = search(
            &dir,
            &dir,
            &Query {
                pattern: "!*.rs",
                include_ignored: false,
                limit: 10,
            },
        )
        .expect_err("a lone exclusion is not a discovery query");
        assert!(error.contains("exclusion"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
