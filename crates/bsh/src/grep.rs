//! The grep tool's search: which lines match a regular expression.
//!
//! Every stage is ripgrep's. The pattern is compiled by grep-regex, the
//! line-oriented scan and its binary detection are grep-searcher, the walk is
//! the ignore crate (the engine behind `rg --files`), and the results are
//! produced by grep-printer's JSON printer - the per-file extraction `rg
//! --json` writes, with the searcher's line numbers and its base64 branch for
//! a line that is not UTF-8 - into one buffer per file that this module decodes
//! into the rows an agent reads. The sink in front of the printer only enforces
//! the row budget; it does not decide what a row is.
//!
//! A binary file is answered the way rg answers one: the lines matched before
//! the first NUL byte are real rows, and a binary row carries the offset of
//! that byte, which is what `binary file matches (found "\0" byte around
//! offset N)` says. What is deliberately not mirrored is rg's multi-file
//! `WARNING:` wording, because a row already names its own path.
//!
//! This module owns only what a tool adds to rg: the row shape, a bound on
//! every axis a caller controls (pattern and include-glob size, context lines,
//! hits, the file size the searcher opens, the width of one reported line), and
//! the sentences for a refusal or an empty answer.

use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use grep::matcher::Matcher;
use grep::printer::{JSONSink, JSON};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkFinish, SinkMatch,
};
use ignore::overrides::{Override, OverrideBuilder};

use crate::glob::{build_walker, relative_to, MAX_PATTERN_BYTES};

/// The most rows one call returns by default.
pub(crate) const DEFAULT_RESULTS: usize = 50;

/// The most rows one call may ask for.
pub(crate) const MAX_RESULTS: usize = 500;

/// The most context lines one call may ask for on each side of a match.
pub(crate) const MAX_CONTEXT: usize = 20;

/// The largest file the searcher opens, in bytes.
///
/// Grep is line oriented and usually pointed at source; a multi-megabyte file
/// is more likely to be a log or generated data than the line wanted, and the
/// bound keeps one file from dominating a call.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// The widest line one row reports, in characters.
const MAX_LINE_CHARS: usize = 300;

/// One grep question.
#[derive(Debug)]
pub(crate) struct Query<'a> {
    /// The regular expression, in ripgrep syntax.
    pub(crate) pattern: &'a str,
    /// Whether hidden and ignored entries are searched too.
    pub(crate) include_ignored: bool,
    /// Whether the expression ignores ASCII case.
    pub(crate) case_insensitive: bool,
    /// The most rows to return; clamped to MAX_RESULTS.
    pub(crate) limit: usize,
    /// Whether to report the lines that do NOT match, like `rg -v`.
    pub(crate) invert: bool,
    /// Lines of context on each side of a match, like `rg -C`.
    pub(crate) context: usize,
    /// Whether to list the files with no match instead of matching lines.
    pub(crate) files_without_match: bool,
    /// One glob filter for which files the walk searches, like `rg -g`.
    pub(crate) include: Option<&'a str>,
    /// Whether to search binary files as text, like `rg -a`.
    pub(crate) text: bool,
}

impl Default for Query<'_> {
    fn default() -> Self {
        Query {
            pattern: "",
            include_ignored: false,
            case_insensitive: false,
            limit: DEFAULT_RESULTS,
            invert: false,
            context: 0,
            files_without_match: false,
            include: None,
            text: false,
        }
    }
}

/// What a row reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A line the pattern matched (or, with `invert`, one it did not).
    Match,
    /// A context line beside a match.
    Context,
}

/// One reported line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    /// The file, relative to the search base.
    pub(crate) path: PathBuf,
    /// The 1-based line number.
    pub(crate) line: u64,
    /// The line itself, trailing terminator removed and width bounded.
    pub(crate) text: String,
    /// Whether this is a match or a context line.
    pub(crate) kind: Kind,
}

/// One file the searcher found binary after it had matched a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binary {
    /// The file, relative to the search base.
    pub(crate) path: PathBuf,
    /// The offset of the first NUL byte.
    pub(crate) offset: u64,
}

impl Binary {
    /// The sentence rg prints for such a file.
    ///
    /// The path is the row's own field, so the sentence does not repeat it.
    pub(crate) fn message(&self) -> String {
        format!(
            "binary file matches (found \"\\0\" byte around offset {})",
            self.offset
        )
    }
}

/// What one search found.
#[derive(Debug)]
pub(crate) struct Found {
    /// Matching and context lines, sorted by path and then line.
    pub(crate) hits: Vec<Hit>,
    /// Files that matched before their first NUL byte, sorted by path.
    pub(crate) binary: Vec<Binary>,
    /// Files with no match, when the query asked for them.
    pub(crate) unmatched: Vec<PathBuf>,
    /// Whether more rows existed than the limit returned.
    pub(crate) truncated: bool,
}

/// Find the lines under root that match query.
///
/// Root is a directory to walk or one file to read: an agent that greps a path
/// it just listed should not have to name the parent and filter the rest out
/// itself. A named file is read whatever the ignore, hidden and include rules
/// say, because naming it is the whole point; a file past the size ceiling or
/// one the searcher finds binary with no match before the NUL is refused by
/// name rather than reported as no match, since there is no other file in the
/// call that could answer.
///
/// The base is what a hit path is relative to: the workspace root for a
/// confined build, or the search root itself when no workspace is installed. The
/// walk never leaves the root and never follows a symlink, so a link inside the
/// workspace cannot turn a content search into a read of the outside.
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
    let single = root.is_file();
    if !single && !root.is_dir() {
        return Err(if root.exists() {
            format!("{} is not a file or directory", root.display())
        } else {
            format!("{} does not exist", root.display())
        });
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.case_insensitive)
        .build(query.pattern)
        .map_err(|error| format!("invalid regex {:?}: {error}", query.pattern))?;
    let context = query.context.min(MAX_CONTEXT);
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .invert_match(query.invert)
        .before_context(context)
        .after_context(context)
        .binary_detection(if query.text {
            BinaryDetection::none()
        } else {
            BinaryDetection::quit(0)
        })
        .build();

    // A named file is searched as itself, so an include glob never narrows a
    // path the caller named - rg's -g ignores explicit file arguments too.
    let overrides = match (single, query.include) {
        (false, Some(pattern)) => Some(include_filter(base, pattern)?),
        _ => None,
    };

    // A hit path strips the base, so a file that IS the base would report an
    // empty path: strip the parent instead and report the file by name.
    let hit_base = if single && base == root {
        root.parent().unwrap_or(base)
    } else {
        base
    };
    let limit = query.limit.clamp(1, MAX_RESULTS);
    let mut found = Found {
        hits: Vec::new(),
        binary: Vec::new(),
        unmatched: Vec::new(),
        truncated: false,
    };
    let mut buffer: Vec<u8> = Vec::new();
    if single {
        match search_file(
            hit_base,
            root,
            &matcher,
            &mut searcher,
            query,
            limit,
            &mut buffer,
            &mut found,
        ) {
            FileResult::Skip(skip) => return Err(skip.sentence(root)),
            FileResult::Binary {
                offset,
                matched: false,
            } => {
                return Err(format!(
                    "{} is binary (a NUL byte at offset {offset}), so its text is not searched",
                    root.display()
                ));
            }
            _ => {}
        }
    } else {
        for result in build_walker(root, query.include_ignored).build() {
            let Ok(entry) = result else { continue };
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            if let Some(overrides) = &overrides {
                let relative = entry.path().strip_prefix(base).unwrap_or(entry.path());
                if !overrides.matched(relative, false).is_whitelist() {
                    continue;
                }
            }
            // A file that disappears or cannot be read mid-walk is skipped, not
            // turned into a failed search: the other files still answer.
            let _ = search_file(
                hit_base,
                entry.path(),
                &matcher,
                &mut searcher,
                query,
                limit,
                &mut buffer,
                &mut found,
            );
            if found.truncated {
                break;
            }
        }
    }
    found.hits.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line.cmp(&right.line))
    });
    found
        .binary
        .sort_by(|left, right| left.path.cmp(&right.path));
    found.unmatched.sort();
    Ok(found)
}

/// The override matcher behind the include glob.
///
/// The semantics are `rg -g`'s: a pattern with no separator matches the file
/// name at any depth, one with a separator is anchored to the search base, and
/// `**` crosses directories. A leading `!` is ripgrep's exclusion syntax, and a
/// lone exclusion would search nothing, so it is refused by name.
fn include_filter(base: &Path, pattern: &str) -> Result<Override, String> {
    if pattern.trim().is_empty() {
        return Err("include must not be empty".to_string());
    }
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(format!(
            "include is {} bytes, past the {MAX_PATTERN_BYTES} byte ceiling",
            pattern.len()
        ));
    }
    if pattern.starts_with('!') {
        return Err("a leading ! is an exclusion; include takes the globs to search".to_string());
    }
    let mut builder = OverrideBuilder::new(base);
    builder
        .add(pattern)
        .map_err(|error| format!("invalid include glob {pattern:?}: {error}"))?;
    builder
        .build()
        .map_err(|error| format!("invalid include glob {pattern:?}: {error}"))
}

/// Why one file was not searched.
#[derive(Debug, Clone, Copy)]
enum Skip {
    /// Not a regular file, or gone between the listing and the read.
    NotRegular,
    /// Past MAX_FILE_BYTES.
    TooLarge,
    /// The file exists but could not be opened or read.
    Unreadable,
}

impl Skip {
    /// The refusal for a file the caller named and the search could not read.
    fn sentence(self, path: &Path) -> String {
        match self {
            Skip::NotRegular => format!("{} is not a regular file", path.display()),
            Skip::TooLarge => format!(
                "{} is past the {MAX_FILE_BYTES} byte ceiling for a search",
                path.display()
            ),
            Skip::Unreadable => format!("{} could not be read", path.display()),
        }
    }
}

/// What one file's search produced beyond its rows.
enum FileResult {
    /// Its rows (which may be none) are in `found`.
    Done,
    /// The searcher met a NUL byte; `matched` says whether a line matched
    /// before it.
    Binary { offset: u64, matched: bool },
    /// The file was not searched.
    Skip(Skip),
}

/// Search one file, appending its rows to `found`.
#[allow(clippy::too_many_arguments)]
fn search_file(
    base: &Path,
    path: &Path,
    matcher: &RegexMatcher,
    searcher: &mut Searcher,
    query: &Query,
    limit: usize,
    buffer: &mut Vec<u8>,
    found: &mut Found,
) -> FileResult {
    let Ok(metadata) = std::fs::metadata(path) else {
        return FileResult::Skip(Skip::NotRegular);
    };
    if !metadata.is_file() {
        return FileResult::Skip(Skip::NotRegular);
    }
    if metadata.len() > MAX_FILE_BYTES {
        return FileResult::Skip(Skip::TooLarge);
    }
    let relative = relative_to(base, path);
    // The sink stops this file's search once its budget is spent, so one file
    // with thousands of matches cannot build a JSON buffer nobody reads. When
    // the answer is a file list, one row already answers it, and the budget is
    // the list's.
    let budget = if query.files_without_match {
        1
    } else {
        limit.saturating_sub(found.hits.len())
    };
    buffer.clear();
    let (binary, exceeded) = {
        let mut printer = JSON::new(&mut *buffer);
        let mut sink = Emit {
            json: printer.sink_with_path(matcher, &relative),
            rows: 0,
            budget,
            exceeded: false,
        };
        if searcher.search_path(matcher, path, &mut sink).is_err() {
            return FileResult::Skip(Skip::Unreadable);
        }
        (sink.json.binary_byte_offset(), sink.exceeded)
    };
    let rows = rows_from_json(buffer, &relative);
    let matched = !rows.is_empty();
    if let Some(offset) = binary {
        // rg reports the lines it read before the NUL and says the file is
        // binary; a file list says nothing about a binary file at all.
        if !query.files_without_match {
            found.hits.extend(rows);
            if matched {
                found.binary.push(Binary {
                    path: relative,
                    offset,
                });
            }
        }
        return FileResult::Binary { offset, matched };
    }
    if query.files_without_match {
        if matched {
            return FileResult::Done;
        }
        if found.unmatched.len() >= limit {
            found.truncated = true;
        } else {
            found.unmatched.push(relative);
        }
        return FileResult::Done;
    }
    found.truncated |= exceeded;
    found.hits.extend(rows);
    FileResult::Done
}

/// The JSON printer with the tool's row budget in front of it.
///
/// The printer decides what a row is; this wrapper only refuses to hand it more
/// rows than the call may return, which is also what tells the searcher to stop
/// reading the file. `finish` is forwarded so the printer still writes its end
/// message, and the searcher's binary verdict is read back from it.
struct Emit<'p, 's, M: Matcher, W> {
    /// ripgrep's own per-file writer.
    json: JSONSink<'p, 's, M, W>,
    /// Rows already written for this file.
    rows: usize,
    /// The most rows this file may contribute.
    budget: usize,
    /// Whether the budget stopped this file's search.
    exceeded: bool,
}

impl<M: Matcher, W: io::Write> Sink for Emit<'_, '_, M, W> {
    type Error = io::Error;

    fn matched(&mut self, searcher: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        if self.rows >= self.budget {
            self.exceeded = true;
            return Ok(false);
        }
        let keep = self.json.matched(searcher, mat)?;
        self.rows += 1;
        Ok(keep)
    }

    fn context(&mut self, searcher: &Searcher, ctx: &SinkContext<'_>) -> io::Result<bool> {
        if self.rows >= self.budget {
            self.exceeded = true;
            return Ok(false);
        }
        let keep = self.json.context(searcher, ctx)?;
        self.rows += 1;
        Ok(keep)
    }

    fn binary_data(&mut self, searcher: &Searcher, offset: u64) -> io::Result<bool> {
        self.json.binary_data(searcher, offset)
    }

    fn begin(&mut self, searcher: &Searcher) -> io::Result<bool> {
        self.json.begin(searcher)
    }

    fn finish(&mut self, searcher: &Searcher, finish: &SinkFinish) -> io::Result<()> {
        self.json.finish(searcher, finish)
    }
}

/// One message of grep-printer's JSON wire format.
#[derive(serde::Deserialize)]
struct Message {
    #[serde(rename = "type")]
    kind: String,
    data: Data,
}

/// The part of a message a row needs.
#[derive(serde::Deserialize)]
struct Data {
    #[serde(default)]
    line_number: Option<u64>,
    #[serde(default)]
    lines: Option<Text>,
}

/// A `text` field, or the base64 `bytes` branch the printer takes for data that
/// is not UTF-8.
#[derive(serde::Deserialize)]
struct Text {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    bytes: Option<String>,
}

impl Text {
    /// The line as a string, decoding the printer's base64 branch.
    fn into_string(self) -> String {
        if let Some(text) = self.text {
            return text;
        }
        let Some(encoded) = self.bytes else {
            return String::new();
        };
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(|raw| String::from_utf8_lossy(&raw).into_owned())
            .unwrap_or_default()
    }
}

/// Decode one file's JSON messages into rows.
///
/// `begin`, `end` and anything a future printer version adds are skipped: a row
/// is a `match` or a `context`, which is the whole vocabulary rg prints for
/// lines.
fn rows_from_json(buffer: &[u8], path: &Path) -> Vec<Hit> {
    let mut rows = Vec::new();
    for line in buffer.split(|byte| *byte == b'\n') {
        let Ok(message) = serde_json::from_slice::<Message>(line) else {
            continue;
        };
        let kind = match message.kind.as_str() {
            "match" => Kind::Match,
            "context" => Kind::Context,
            _ => continue,
        };
        let Some(line_number) = message.data.line_number else {
            continue;
        };
        let text = message
            .data
            .lines
            .map(Text::into_string)
            .unwrap_or_default();
        rows.push(Hit {
            path: path.to_path_buf(),
            line: line_number,
            text: bounded_line(&text),
            kind,
        });
    }
    rows
}

/// One line as a row reports it: terminator gone, width bounded.
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
///
/// It names the switches that most often explain an empty answer: a pattern
/// that is case-sensitive by default, a walk that skipped files, an include
/// glob that narrowed it, `invert` and `files_without_match`, whose empty
/// results mean the opposite of "not found".
pub(crate) fn hint(query: &Query<'_>, root: &Path) -> String {
    let pattern = query.pattern;
    let mut hint = format!(
        "no line matched {pattern:?} under {}; matching is case-sensitive unless case_insensitive is set.",
        root.display()
    );
    // A named file is searched whatever its name or ignore status says, so the
    // sentences about skipped files would be false for one.
    if !root.is_file() {
        if let Some(include) = query.include {
            hint.push_str(&format!(
                " Only files matching the include glob {include:?} were searched."
            ));
        }
        if !query.include_ignored {
            hint.push_str(
                " Ignored and hidden files were skipped; include_ignored=true searches them too.",
            );
        }
    }
    if query.invert {
        hint.push_str(
            " invert=true reports the lines that do NOT match, so this means every line searched matched.",
        );
    }
    if query.files_without_match {
        hint.push_str(
            " files_without_match=true listed no path, so every searched file had a match.",
        );
    }
    hint
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
                ..Query::default()
            },
        )
        .expect("search succeeds")
    }

    fn lines(found: &Found) -> Vec<String> {
        found
            .hits
            .iter()
            .map(|hit| {
                let mark = match hit.kind {
                    Kind::Match => "",
                    Kind::Context => "~ ",
                };
                format!(
                    "{mark}{}:{}:{}",
                    hit.path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    hit.line,
                    hit.text
                )
            })
            .collect()
    }

    /// A plain query for the file-root cases, where the switches are not the
    /// subject.
    fn plain(pattern: &str) -> Query<'_> {
        Query {
            pattern,
            ..Query::default()
        }
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

    /// A file whose first NUL byte comes before any match is skipped, and a
    /// file past the size ceiling is not opened. Neither claims to be binary:
    /// nothing matched, so there is nothing a binary row could be about.
    #[test]
    fn a_binary_file_without_a_match_and_an_oversized_file_are_skipped() {
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

        let found = search_all(&dir, "needle", false, false, 50);
        assert_eq!(
            lines(&found),
            vec!["clean.txt:1:needle"],
            "the NUL file and the oversized file are both out"
        );
        assert!(found.binary.is_empty(), "{:?}", found.binary);
    }

    /// A match before the file turns binary keeps its row, and a binary row
    /// names the offset: that is rg's answer, which reports the lines it read
    /// and then says the file is binary.
    ///
    /// The NUL sits beyond the searcher's 64 KiB line buffer on purpose. Inside
    /// it the buffer is scanned before any line is matched, so the file would be
    /// skipped without a row and this test would pass for the wrong reason.
    #[test]
    fn a_match_before_a_nul_keeps_its_row_beside_a_binary_row() {
        let dir = scratch("binary-late");
        let blob = dir.join("late.bin");
        let mut bytes = b"needle\n".to_vec();
        bytes.resize(70 * 1024, b'a');
        bytes.push(0);
        std::fs::write(&blob, bytes).expect("blob");
        std::fs::write(dir.join("clean.txt"), "needle\n").expect("clean");

        let found = search_all(&dir, "needle", false, false, 50);
        assert_eq!(
            lines(&found),
            vec!["clean.txt:1:needle", "late.bin:1:needle"],
            "the row read before the NUL byte is a real row"
        );
        assert_eq!(found.binary.len(), 1);
        assert_eq!(found.binary[0].path, PathBuf::from("late.bin"));
        assert_eq!(found.binary[0].offset, 70 * 1024);
        assert_eq!(
            found.binary[0].message(),
            "binary file matches (found \"\\0\" byte around offset 71680)"
        );

        // A named file answers the same way instead of being refused.
        let named = search(&blob, &dir, &plain("needle")).expect("a named binary file is searched");
        assert_eq!(lines(&named), vec!["late.bin:1:needle"]);
        assert_eq!(named.binary.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
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
                limit: 10,
                ..Query::default()
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

    /// A root that is a file is searched instead of refused, and its hit path
    /// stays meaningful: relative to the base, or the file's own name when the
    /// file is the base.
    #[test]
    fn a_file_root_is_searched_by_name() {
        let dir = scratch("file-root");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        let file = dir.join("src").join("main.rs");
        std::fs::write(&file, "fn main() {\n    let bubble = 1;\n}\n").expect("file");

        let from_base = search(&file, &dir, &plain("bubble")).expect("a file root is searched");
        assert_eq!(lines(&from_base), vec!["src/main.rs:2:    let bubble = 1;"]);

        let from_file = search(&file, &file, &plain("bubble")).expect("a file root is searched");
        assert_eq!(
            lines(&from_file),
            vec!["main.rs:2:    let bubble = 1;"],
            "a file that is its own base is reported by name, not by an empty path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A named file is read even when its name or a .gitignore rule would hide
    /// it from a walk: naming it is the point.
    #[test]
    fn a_file_root_ignores_the_ignore_and_hidden_rules() {
        let dir = scratch("file-ignored");
        std::fs::create_dir_all(dir.join("generated")).expect("generated");
        std::fs::write(dir.join(".gitignore"), "generated/\n").expect("gitignore");
        std::fs::write(dir.join(".hidden.txt"), "needle\n").expect("hidden");
        let built = dir.join("generated").join("built.txt");
        std::fs::write(&built, "needle\n").expect("built");

        for file in [dir.join(".hidden.txt"), built] {
            let found = search(&file, &dir, &plain("needle")).expect("a named file is searched");
            assert_eq!(found.hits.len(), 1, "{}", file.display());
            assert_eq!(found.hits[0].text, "needle");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A named file that cannot be searched says why, because no other file in
    /// the call could answer instead.
    #[test]
    fn a_named_binary_or_oversized_file_says_why_it_is_not_searched() {
        let dir = scratch("file-skip");
        let blob = dir.join("blob.bin");
        std::fs::write(&blob, [0_u8, b'n', b'e', b'e', b'd', b'l', b'e']).expect("blob");
        let big = dir.join("big.txt");
        let mut bytes = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        bytes[0..6].copy_from_slice(b"needle");
        std::fs::write(&big, bytes).expect("big");

        let error = search(&blob, &dir, &plain("needle")).expect_err("a binary file is refused");
        assert!(error.contains("binary"), "{error}");
        assert!(error.contains("blob.bin"), "{error}");
        let error = search(&big, &dir, &plain("needle")).expect_err("an oversized file is refused");
        assert!(error.contains("ceiling"), "{error}");
        assert!(error.contains("big.txt"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A root that names nothing is refused by name instead of reading as
    /// "no match".
    #[test]
    fn a_missing_root_is_named_in_the_refusal() {
        let dir = scratch("file-missing");
        let error = search(&dir.join("nope.rs"), &dir, &plain("needle"))
            .expect_err("a missing path is refused");
        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("nope.rs"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Something that exists but is neither a file nor a directory is named as
    /// such rather than called missing.
    #[test]
    #[cfg(unix)]
    fn a_root_that_is_neither_file_nor_directory_says_so() {
        let dir = scratch("file-socket");
        let socket = dir.join("sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("socket");
        let error =
            search(&socket, &dir, &plain("needle")).expect_err("a socket is not searchable");
        assert!(error.contains("not a file or directory"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Invert reports the lines the pattern does NOT match, like `rg -v`.
    #[test]
    fn invert_reports_the_lines_that_do_not_match() {
        let dir = scratch("invert");
        std::fs::write(dir.join("one.txt"), "needle\nother\nneedle\nlast\n").expect("file");
        let found = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                invert: true,
                ..Query::default()
            },
        )
        .expect("search succeeds");
        assert_eq!(lines(&found), vec!["one.txt:2:other", "one.txt:4:last"]);
    }

    /// Context lines are reported as their own kind, beside the match they
    /// surround, which is what `rg -C` prints.
    #[test]
    fn context_lines_are_reported_beside_the_match() {
        let dir = scratch("context");
        std::fs::write(dir.join("one.txt"), "a\nb\nneedle\nd\ne\n").expect("file");
        let found = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                context: 1,
                ..Query::default()
            },
        )
        .expect("search succeeds");
        assert_eq!(
            lines(&found),
            vec!["~ one.txt:2:b", "one.txt:3:needle", "~ one.txt:4:d"]
        );
    }

    /// The include glob narrows the walk, and a path the caller names is
    /// searched whatever the glob says - rg's `-g` ignores explicit files too.
    #[test]
    fn an_include_glob_narrows_the_walk() {
        let dir = scratch("include");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        std::fs::write(dir.join("src").join("main.rs"), "needle\n").expect("rs");
        std::fs::write(dir.join("src").join("notes.md"), "needle\n").expect("md");

        let narrowed = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                include: Some("*.rs"),
                ..Query::default()
            },
        )
        .expect("search succeeds");
        assert_eq!(lines(&narrowed), vec!["src/main.rs:1:needle"]);

        let named = search(
            &dir.join("src").join("notes.md"),
            &dir,
            &Query {
                pattern: "needle",
                include: Some("*.rs"),
                ..Query::default()
            },
        )
        .expect("a named file is searched");
        assert_eq!(lines(&named), vec!["src/notes.md:1:needle"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An exclusion is refused by name: `include` says which files to search,
    /// and a lone `!` pattern would search none of them.
    #[test]
    fn a_negated_include_is_refused() {
        let dir = scratch("include-negated");
        let error = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                include: Some("!*.md"),
                ..Query::default()
            },
        )
        .expect_err("negation is not supported");
        assert!(error.contains("include"), "{error}");
        assert!(error.contains("exclusion"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Text mode searches a binary file as text: the line before the NUL is
    /// reported and no binary row is claimed, which is `rg -a`.
    #[test]
    fn text_mode_searches_a_binary_file_as_text() {
        let dir = scratch("text-mode");
        std::fs::write(dir.join("blob.bin"), b"needle\n\x00rest\n".as_slice()).expect("blob");

        let quiet = search_all(&dir, "needle", false, false, 50);
        assert!(
            lines(&quiet).is_empty(),
            "the NUL inside the first buffer stops the text search: {:?}",
            lines(&quiet)
        );
        assert!(quiet.binary.is_empty());

        let text = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                text: true,
                ..Query::default()
            },
        )
        .expect("text mode searches the file");
        assert_eq!(lines(&text), vec!["blob.bin:1:needle"]);
        assert!(text.binary.is_empty(), "{:?}", text.binary);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Files without a match are listed instead of lines, and a binary file is
    /// never one of them: rg does not count a binary file as a text file
    /// without a match.
    #[test]
    fn files_without_match_lists_only_the_paths_with_no_match() {
        let dir = scratch("without-match");
        std::fs::write(dir.join("hit.txt"), "needle\n").expect("hit");
        std::fs::write(dir.join("miss.txt"), "nothing\n").expect("miss");
        std::fs::write(dir.join("blob.bin"), b"\x00nothing\n".as_slice()).expect("blob");

        let found = search(
            &dir,
            &dir,
            &Query {
                pattern: "needle",
                files_without_match: true,
                ..Query::default()
            },
        )
        .expect("search succeeds");
        assert!(found.hits.is_empty(), "no match lines are reported");
        assert_eq!(found.unmatched, vec![PathBuf::from("miss.txt")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A line that is not UTF-8 arrives through the printer's base64 branch and
    /// is still reported, lossily decoded, as one row.
    #[test]
    fn a_line_that_is_not_utf8_is_reported_lossily() {
        let dir = scratch("lossy");
        std::fs::write(
            dir.join("blob.txt"),
            b"needle \xff\xfe text\nplain needle\n".as_slice(),
        )
        .expect("file");
        let found = search_all(&dir, "needle", false, false, 50);
        assert_eq!(found.hits.len(), 2);
        assert!(found.hits[0].text.contains("needle"), "{:?}", found.hits[0]);
        assert_eq!(found.hits[1].text, "plain needle");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One row of the rg comparison: path, line, text and kind.
    type Row = (String, u64, String, char);

    /// This tool's rows in the comparison's shape.
    fn parity_rows(found: &Found) -> Vec<Row> {
        let mut rows: Vec<Row> = found
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    hit.line,
                    hit.text.clone(),
                    match hit.kind {
                        Kind::Match => 'm',
                        Kind::Context => 'c',
                    },
                )
            })
            .collect();
        rows.sort();
        rows
    }

    /// The rows `rg --json` reports for the same pattern and switches.
    ///
    /// A path rg prints is relative to the directory it ran in (`./one.txt`),
    /// so the `./` is stripped to match a hit path.
    fn rg_rows(dir: &Path, flags: &[&str], pattern: &str, path: &str) -> Vec<Row> {
        let output = std::process::Command::new("rg")
            .arg("--json")
            .args(flags)
            .arg(pattern)
            .arg(path)
            .current_dir(dir)
            .output()
            .expect("rg is installed");
        assert!(
            output.status.code() != Some(2),
            "rg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut rows = Vec::new();
        for line in stdout.lines() {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let kind = match message["type"].as_str() {
                Some("match") => 'm',
                Some("context") => 'c',
                _ => continue,
            };
            let data = &message["data"];
            let raw = data["path"]["text"].as_str().unwrap_or_default();
            let path = raw.strip_prefix("./").unwrap_or(raw).to_string();
            let line_number = data["line_number"].as_u64().unwrap_or(0);
            rows.push((path, line_number, rg_text(&data["lines"]), kind));
        }
        rows.sort();
        rows
    }

    /// One line from rg's JSON: its `text`, or the base64 `bytes` branch it
    /// takes for a line that is not UTF-8, bounded the way a row is.
    fn rg_text(value: &serde_json::Value) -> String {
        let bytes = match (value["text"].as_str(), value["bytes"].as_str()) {
            (Some(text), _) => text.as_bytes().to_vec(),
            (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("rg writes valid base64"),
            _ => Vec::new(),
        };
        bounded_line(&String::from_utf8_lossy(&bytes))
    }

    /// The rows this tool reports are the rows the installed rg reports.
    ///
    /// Ignored by default because it shells out to `rg`, which the machine
    /// running the suite may not have:
    /// `cargo test -p bsh --lib grep -- --ignored`.
    ///
    /// The rows are compared against `rg --json`, the printer this module
    /// decodes, so a difference names this module's decode or its row budget
    /// rather than a second grep implementation. A binary file is compared
    /// through rg's own sentence and its standard output instead, because
    /// `--json` and the standard printer answer a NUL byte differently: the
    /// JSON printer CONVERTS it (reporting the lines past it), the standard
    /// printer QUITS, and the tool takes the standard printer's answer.
    #[test]
    #[ignore = "shells out to rg"]
    fn the_rows_match_the_installed_ripgrep() {
        let dir = scratch("parity");
        std::fs::write(
            dir.join("one.txt"),
            "alpha\nneedle\nbeta\ngamma\nneedle\ndelta\n",
        )
        .expect("one");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        std::fs::write(
            dir.join("src").join("main.rs"),
            "fn main() {\n    let needle = 1;\n}\n",
        )
        .expect("main");
        std::fs::write(dir.join("notes.md"), "needle in md\n").expect("notes");
        std::fs::write(dir.join("empty.txt"), "nothing here\n").expect("empty");
        // A NUL before any match: the buffer scan stops the file before a line
        // is matched, so neither side reports a row for it.
        std::fs::write(dir.join("blob.bin"), b"needle\n\x00rest\n".as_slice()).expect("blob");
        // A line that is not UTF-8: rg writes it as base64 `bytes`.
        std::fs::write(dir.join("latin.txt"), b"needle \xff bytes\n".as_slice()).expect("latin");

        let cases: [(&str, &[&str], Query<'_>); 5] = [
            (
                "default",
                &[],
                Query {
                    pattern: "needle",
                    limit: MAX_RESULTS,
                    ..Query::default()
                },
            ),
            (
                "invert",
                &["-v"],
                Query {
                    pattern: "needle",
                    invert: true,
                    limit: MAX_RESULTS,
                    ..Query::default()
                },
            ),
            (
                "context",
                &["-C", "1"],
                Query {
                    pattern: "needle",
                    context: 1,
                    limit: MAX_RESULTS,
                    ..Query::default()
                },
            ),
            (
                "include",
                &["-g", "*.rs"],
                Query {
                    pattern: "needle",
                    include: Some("*.rs"),
                    limit: MAX_RESULTS,
                    ..Query::default()
                },
            ),
            (
                "text",
                &["-a"],
                Query {
                    pattern: "needle",
                    text: true,
                    limit: MAX_RESULTS,
                    ..Query::default()
                },
            ),
        ];
        for (label, flags, query) in cases {
            let ours = parity_rows(&search(&dir, &dir, &query).expect("search succeeds"));
            let theirs = rg_rows(&dir, flags, "needle", ".");
            assert!(
                !ours.is_empty() && !theirs.is_empty(),
                "the {label} case compared nothing"
            );
            assert_eq!(ours, theirs, "rows differ from rg for {label}");
        }

        // A binary file whose match comes before the NUL byte: rg prints the
        // line it read and then the sentence naming the offset.
        let blob = dir.join("late.bin");
        let mut bytes = b"needle\n".to_vec();
        bytes.resize(70 * 1024, b'a');
        bytes.push(0);
        bytes.extend_from_slice(b"needle two\n");
        std::fs::write(&blob, bytes).expect("late");

        let ours = search(&blob, &dir, &plain("needle")).expect("a named binary file is searched");
        assert_eq!(ours.binary.len(), 1, "{:?}", ours.binary);
        let output = std::process::Command::new("rg")
            .args([
                "-n",
                "--with-filename",
                "--no-heading",
                "needle",
                "late.bin",
            ])
            .current_dir(&dir)
            .output()
            .expect("rg is installed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 2, "{stdout}");
        assert_eq!(
            lines[0],
            format!("late.bin:1:{}", ours.hits[0].text),
            "the row read before the NUL is the row rg prints"
        );
        assert!(lines[1].starts_with("late.bin:"), "{stdout}");
        assert!(
            lines[1].ends_with(&ours.binary[0].message()),
            "rg says {:?}, the tool says {:?}",
            lines[1],
            ours.binary[0].message()
        );

        // Text mode is the other half of rg's binary answer: the same file
        // searched as text reports every matching line, including the one past
        // the NUL, because -a turns binary detection off rather than converting.
        let text = search(
            &blob,
            &dir,
            &Query {
                pattern: "needle",
                text: true,
                limit: MAX_RESULTS,
                ..Query::default()
            },
        )
        .expect("text mode searches the file");
        assert_eq!(
            parity_rows(&text),
            rg_rows(&dir, &["-a"], "needle", "late.bin"),
            "text mode differs from rg -a"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
