//! The grep tool's search: which lines match a regular expression.
//!
//! The matcher and the line searcher are ripgrep's own crates through the
//! grep facade - the library the rg binary is built from - and the walk is the
//! same ignore-based one the glob tool uses. That gives the tool ripgrep's
//! regex syntax, its line-oriented matching, its binary detection and the
//! shared ignore policy instead of a second implementation to keep in step.
//!
//! The search is bounded on every axis a caller controls: the pattern size,
//! the number of hits, the file size the searcher opens, and the width of one
//! reported line. A hit is a path, a 1-based line number and the matched line
//! itself, which is what an agent needs to go straight to read_file.

use std::io::Read;
use std::path::{Path, PathBuf};

use grep::regex::RegexMatcherBuilder;
use grep::searcher::sinks::Lossy;
use grep::searcher::{BinaryDetection, SearcherBuilder};

use crate::glob::{build_walker, relative_to, MAX_PATTERN_BYTES};

/// The most hits one call returns by default.
pub(crate) const DEFAULT_RESULTS: usize = 50;

/// The most hits one call may ask for.
pub(crate) const MAX_RESULTS: usize = 500;

/// The largest file the searcher opens, in bytes.
///
/// Grep is line oriented and usually pointed at source; a multi-megabyte file
/// is more likely to be a log or generated data than the line wanted, and the
/// bound keeps one file from dominating a call.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The widest line one hit reports, in characters.
const MAX_LINE_CHARS: usize = 300;

/// The bytes inspected to classify a file as binary.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// One grep question.
#[derive(Debug)]
pub(crate) struct Query<'a> {
    /// The regular expression, in ripgrep syntax.
    pub(crate) pattern: &'a str,
    /// Whether hidden and ignored entries are searched too.
    pub(crate) include_ignored: bool,
    /// Whether the expression ignores ASCII case.
    pub(crate) case_insensitive: bool,
    /// The most hits to return; clamped to MAX_RESULTS.
    pub(crate) limit: usize,
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    /// The file, relative to the search base.
    pub(crate) path: PathBuf,
    /// The 1-based line number.
    pub(crate) line: u64,
    /// The matched line, trailing terminator removed and width bounded.
    pub(crate) text: String,
}

/// What one search found.
#[derive(Debug)]
pub(crate) struct Found {
    /// Matching lines, sorted by path and then line.
    pub(crate) hits: Vec<Hit>,
    /// Whether more hits existed than the limit returned.
    pub(crate) truncated: bool,
}

/// Find the lines under root that match query.
///
/// The base is what a hit path is relative to: the workspace root for a
/// confined build, or the search root itself when no workspace is installed.
/// The walk never leaves the root and never follows a symlink, so a link inside
/// the workspace cannot turn a content search into a read of the outside.
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
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.case_insensitive)
        .build(query.pattern)
        .map_err(|error| format!("invalid regex {:?}: {error}", query.pattern))?;
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(0))
        .build();

    let limit = query.limit.clamp(1, MAX_RESULTS);
    let mut hits: Vec<Hit> = Vec::new();
    let mut truncated = false;
    for result in build_walker(root, query.include_ignored).build() {
        let Ok(entry) = result else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let path = entry.path();
        if looks_binary(path) {
            continue;
        }
        let sink = Lossy(|line_number, line| {
            if hits.len() >= limit {
                truncated = true;
                return Ok(false);
            }
            hits.push(Hit {
                path: relative_to(base, path),
                line: line_number,
                text: bounded_line(line),
            });
            Ok(true)
        });
        // A file that disappears or cannot be read mid-walk is skipped, not
        // turned into a failed search: the other files still answer.
        let _ = searcher.search_path(&matcher, path, sink);
        if truncated {
            break;
        }
    }
    hits.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
    });
    Ok(Found { hits, truncated })
}

/// Whether the file's first bytes look binary.
///
/// A NUL in the head is the same test read_file uses, and it keeps the
/// searcher from reporting a line out of an image or an object file.
fn looks_binary(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut head = [0_u8; BINARY_PROBE_BYTES];
    let Ok(read) = file.read(&mut head) else {
        return true;
    };
    head[..read].contains(&0)
}

/// One matched line as a hit reports it: terminator gone, width bounded.
fn bounded_line(line: &str) -> String {
    let line = line.trim_end_matches(['\n', '\r']);
    if line.chars().count() > MAX_LINE_CHARS {
        let head: String = line.chars().take(MAX_LINE_CHARS - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        line.to_string()
    }
}

/// The sentence an empty grep result carries.
fn empty_hint(pattern: &str, root: &Path, include_ignored: bool) -> String {
    let mut hint = format!(
        "no line matched {pattern:?} under {}; matching is case-sensitive unless case_insensitive is set.",
        root.display()
    );
    if !include_ignored {
        hint.push_str(
            " Ignored and hidden files were skipped; include_ignored=true searches them too.",
        );
    }
    hint
}

/// The hint an empty call reports.
pub(crate) fn hint(pattern: &str, root: &Path, include_ignored: bool) -> String {
    empty_hint(pattern, root, include_ignored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-grep-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn search_all(
        dir: &Path,
        pattern: &str,
        include_ignored: bool,
        case_insensitive: bool,
        limit: usize,
    ) -> Found {
        search(
            dir,
            dir,
            &Query {
                pattern,
                include_ignored,
                case_insensitive,
                limit,
            },
        )
        .expect("search succeeds")
    }

    fn lines(found: &Found) -> Vec<String> {
        found
            .hits
            .iter()
            .map(|hit| {
                format!(
                    "{}:{}:{}",
                    hit.path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    hit.line,
                    hit.text
                )
            })
            .collect()
    }

    /// A match reports the file, the 1-based line and the line's text.
    #[test]
    fn a_match_reports_path_line_and_text() {
        let dir = scratch("line");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        std::fs::write(
            dir.join("src").join("main.rs"),
            "fn main() {\n    let bubble = 1;\n}\n",
        )
        .expect("file");
        assert_eq!(
            lines(&search_all(&dir, "bubble", false, false, 50)),
            vec!["src/main.rs:2:    let bubble = 1;"]
        );
    }

    /// The pattern is a ripgrep regular expression, not a literal.
    #[test]
    fn the_pattern_is_a_regular_expression() {
        let dir = scratch("regex");
        std::fs::write(dir.join("one.rs"), "fn alpha() {}\nfn beta() {}\n").expect("file");
        assert_eq!(
            lines(&search_all(&dir, r"fn \w+\(", false, false, 50)),
            vec!["one.rs:1:fn alpha() {}", "one.rs:2:fn beta() {}"]
        );
    }

    /// Case folding is opt-in, like ripgrep's own default.
    #[test]
    fn case_insensitive_is_opt_in() {
        let dir = scratch("case");
        std::fs::write(dir.join("one.txt"), "Bubble\n").expect("file");
        assert!(lines(&search_all(&dir, "bubble", false, false, 50)).is_empty());
        assert_eq!(
            lines(&search_all(&dir, "bubble", false, true, 50)),
            vec!["one.txt:1:Bubble"]
        );
    }

    /// Ignored and hidden files are skipped until asked for.
    #[test]
    fn ignored_and_hidden_files_are_opt_in() {
        let dir = scratch("ignored");
        std::fs::create_dir_all(dir.join("generated")).expect("generated");
        std::fs::write(dir.join(".gitignore"), "generated/\n").expect("gitignore");
        std::fs::write(dir.join("visible.txt"), "needle\n").expect("visible");
        std::fs::write(dir.join("generated").join("built.txt"), "needle\n").expect("built");
        std::fs::write(dir.join(".hidden.txt"), "needle\n").expect("hidden");

        assert_eq!(
            lines(&search_all(&dir, "needle", false, false, 50)),
            vec!["visible.txt:1:needle"]
        );
        assert_eq!(
            lines(&search_all(&dir, "needle", true, false, 50)),
            vec![
                ".hidden.txt:1:needle",
                "generated/built.txt:1:needle",
                "visible.txt:1:needle"
            ]
        );
    }

    /// A binary file is not searched, and a file past the size ceiling is not
    /// opened.
    #[test]
    fn binary_and_oversized_files_are_skipped() {
        let dir = scratch("binary");
        std::fs::write(
            dir.join("blob.bin"),
            [0_u8, b'n', b'e', b'e', b'd', b'l', b'e'],
        )
        .expect("blob");
        let big = dir.join("big.txt");
        let mut bytes = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        bytes[0..6].copy_from_slice(b"needle");
        std::fs::write(&big, bytes).expect("big");
        std::fs::write(dir.join("clean.txt"), "needle\n").expect("clean");

        assert_eq!(
            lines(&search_all(&dir, "needle", false, false, 50)),
            vec!["clean.txt:1:needle"],
            "the NUL file and the oversized file are both out"
        );
    }

    /// The result is bounded and says when it stopped early.
    #[test]
    fn the_result_is_limited_and_flagged() {
        let dir = scratch("limit");
        std::fs::write(
            dir.join("many.txt"),
            "needle\nneedle\nneedle\nneedle\nneedle\n",
        )
        .expect("file");
        let found = search_all(&dir, "needle", false, false, 2);
        assert_eq!(found.hits.len(), 2);
        assert!(found.truncated);
        let all = search_all(&dir, "needle", false, false, 50);
        assert_eq!(all.hits.len(), 5);
        assert!(!all.truncated);
    }

    /// A bad expression names itself; an empty or oversized one is refused
    /// before the walk starts.
    #[test]
    fn an_invalid_expression_is_refused() {
        let dir = scratch("invalid");
        let error = search(
            &dir,
            &dir,
            &Query {
                pattern: "(",
                include_ignored: false,
                case_insensitive: false,
                limit: 10,
            },
        )
        .expect_err("an unbalanced group does not compile");
        assert!(error.contains("invalid regex"), "{error}");
        assert!(error.contains("\"("), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlinked file is not searched: the walk does not follow links, so a
    /// link inside the workspace cannot read the outside.
    #[test]
    #[cfg(unix)]
    fn a_symlinked_file_is_not_searched() {
        let dir = scratch("symlink");
        let outside = scratch("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "needle\n").expect("secret");
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("link.txt"))
            .expect("symlink");
        std::fs::write(dir.join("inside.txt"), "clean\n").expect("inside");
        assert!(lines(&search_all(&dir, "needle", false, false, 50)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
