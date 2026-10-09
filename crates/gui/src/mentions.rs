//! `@path` file mentions: fuzzy discovery for the composer's picker and
//! send-side expansion that attaches mentioned file contents to the prompt.
//!
//! Two seams, one module:
//!
//! - [`search_files`] ranks workspace paths for the `@` picker (called by
//!   the `search_files` IPC command).
//! - [`expand`] rewrites `@path` tokens in an outgoing prompt into
//!   `<file path="…">…</file>` blocks. It runs in `begin_stream`, so the
//!   stored transcript keeps the raw text — history re-sends stay lean and
//!   the caps re-apply fresh on every turn.
//!
//! Expansion is containment-safe: paths are canonicalized and must stay
//! under the workspace root, code spans are never rewritten, and every
//! attachment is byte-capped.

use std::collections::HashSet;
use std::path::Path;

use super::filetree::SKIP_DIRS;

/// Max bytes attached from a single mentioned file (UTF-8 safe cut).
pub(crate) const MENTION_MAX_BYTES: usize = 32 * 1024;
/// Max distinct files expanded in one prompt.
pub(crate) const MENTION_MAX_FILES: usize = 4;
/// Hard cap on walker depth relative to the workspace root.
const MAX_DEPTH: usize = 10;

/// Score `query` against `candidate` as a case-insensitive subsequence.
///
/// Higher is better; `None` means some query character never matched.
/// The first matched character gets a large bonus when it opens a path
/// segment (`/ - _ . ` or string start) plus a distance penalty for how far
/// into the string it sits; later characters gain for continuing an
/// unbroken run, pay for gaps, gain smaller boundary bonuses, and gain for
/// exact case — so `read` ranks `docs/readme.md` (segment opener) far above
/// `bread/seed.md` (mid-segment run). An empty query scores `0` (everything
/// ties; callers sort alphabetically).
pub(crate) fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let cand: Vec<char> = candidate.chars().collect();
    let mut score: i64 = 0;
    let mut next = 0usize;
    let mut first = true;
    for qc in query.chars() {
        let rel = cand
            .get(next..)?
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&qc))?;
        let idx = next + rel;
        let boundary = idx == 0 || matches!(cand[idx - 1], '/' | '\\' | '-' | '_' | '.' | ' ');
        if first {
            if boundary {
                score += 12; // opens a path segment
            }
            score -= rel as i64; // distance from the string start
            first = false;
        } else if rel == 0 {
            score += 10; // continues the previous match (unbroken run)
        } else {
            score -= rel as i64; // pay for the gap
            if boundary {
                score += 6; // still lands on a segment opener
            }
        }
        if cand[idx] == qc {
            score += 2; // exact case
        }
        next = idx + 1;
    }
    Some(score)
}

/// Fuzzy-list workspace files for the `@` mention picker.
///
/// Walks `root` skipping VCS/build/dependency directories, dot-directories,
/// symlinks, and anything deeper than [`MAX_DEPTH`], scores every relative
/// path against `query`, and returns up to `limit` best matches as
/// `/`-separated workspace-relative paths. An empty or unreadable `root`
/// yields no results.
pub(crate) fn search_files(root: &Path, query: &str, limit: usize) -> Vec<String> {
    if limit == 0 || root.as_os_str().is_empty() {
        return Vec::new();
    }
    let Ok(canonical) = root.canonicalize() else {
        return Vec::new();
    };
    let mut hits: Vec<(i64, String)> = Vec::new();
    walk(&canonical, &canonical, 0, query, &mut hits);
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    hits.truncate(limit);
    hits.into_iter().map(|(_, p)| p).collect()
}

/// One directory level of the workspace walk; symlinked entries are skipped
/// so a self-referencing link can never loop the traversal.
fn walk(root: &Path, dir: &Path, depth: usize, query: &str, hits: &mut Vec<(i64, String)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if file_type.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, depth + 1, query, hits);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if let Some(score) = fuzzy_score(query, &rel_str) {
                hits.push((score, rel_str));
            }
        }
    }
}

/// Expand `@path` tokens in `text` into attached-file blocks.
///
/// Rules:
/// - only paths inside `root` (canonicalized, `..` rejected) are attached;
/// - tokens inside fenced or inline code spans are left untouched;
/// - at most [`MENTION_MAX_FILES`] distinct files per prompt, each at most
///   [`MENTION_MAX_BYTES`] bytes (UTF-8-safe truncation, flagged with a
///   `truncated` attribute);
/// - non-UTF-8 files and paths that do not resolve are left as raw text;
/// - an empty `root` or a prompt without `@` returns `text` unchanged.
pub(crate) fn expand(text: &str, root: &Path) -> String {
    if !text.contains('@') || root.as_os_str().is_empty() {
        return text.to_string();
    }
    let Ok(root) = root.canonicalize() else {
        return text.to_string();
    };
    let mask = code_mask(text);
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut attached = 0usize;
    let mut i = 0usize;
    while i < text.len() {
        if bytes[i] == b'@' && !mask[i] {
            let mut j = i + 1;
            while j < text.len() && !mask[j] && is_path_byte(bytes[j]) {
                j += 1;
            }
            if j > i + 1 {
                // All path bytes are ASCII, so `j` lands on a char boundary.
                let token = &text[i + 1..j];
                let first_time = !seen.contains(token);
                if attached < MENTION_MAX_FILES && first_time && !token.contains("..") {
                    if let Some(block) = file_block(&root, token) {
                        out.push_str(&block);
                        seen.insert(token.to_string());
                        attached += 1;
                        i = j;
                        continue;
                    }
                }
                out.push_str(&text[i..j]);
                i = j;
                continue;
            }
        }
        let ch_len = text[i..].chars().next().map_or(1, |c| c.len_utf8());
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Bytes that may appear inside an `@path` token.
fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'/' | b'\\' | b'.' | b'_' | b'-' | b'~')
}

/// Mark every byte that sits inside a fenced (``` … ```) or inline
/// (` … `) code span; mention expansion skips those regions.
fn code_mask(text: &str) -> Vec<bool> {
    let b = text.as_bytes();
    let mut mask = vec![false; b.len()];
    let mut i = 0usize;
    let mut fence = false;
    let mut inline = false;
    while i < b.len() {
        if b[i] == b'`' {
            let start = i;
            while i < b.len() && b[i] == b'`' {
                i += 1;
            }
            mask[start..i].fill(true);
            let run = i - start;
            if run >= 3 {
                fence = !fence;
            } else if !fence {
                // A closing run ends an open inline span; an opening one
                // starts it. Either way the backticks themselves are code.
                inline = !inline;
            }
            continue;
        }
        if fence || inline {
            mask[i] = true;
        }
        i += 1;
    }
    mask
}

/// Read `rel` under `root` and render it as a `<file>` block, or `None`
/// when the path escapes the root, is not a regular file, or is not UTF-8.
fn file_block(root: &Path, rel: &str) -> Option<String> {
    let canon = root.join(rel).canonicalize().ok()?;
    if !canon.starts_with(root) || !canon.is_file() {
        return None;
    }
    let bytes = std::fs::read(&canon).ok()?;
    let mut content = String::from_utf8(bytes).ok()?;
    let mut truncated = false;
    if content.len() > MENTION_MAX_BYTES {
        let mut cut = MENTION_MAX_BYTES;
        while !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        truncated = true;
    }
    let shown = canon
        .strip_prefix(root)
        .unwrap_or(&canon)
        .to_string_lossy()
        .replace('\\', "/");
    let flag = if truncated { " truncated" } else { "" };
    Some(format!("<file path=\"{shown}\"{flag}>\n{content}\n</file>"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bos-gui-mentions-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn fuzzy_prefers_boundaries_and_early_runs() {
        let strong = fuzzy_score("read", "docs/readme.md").expect("match");
        let weak = fuzzy_score("read", "bread/seed.md").expect("match");
        assert!(strong > weak, "{strong} should beat {weak}");
        let exact = fuzzy_score("readme", "docs/readme.md").expect("match");
        let partial = fuzzy_score("read", "docs/readme.md").expect("match");
        assert!(exact > partial, "longer exact prefix scores higher");
        assert!(fuzzy_score("zzz", "docs/readme.md").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn search_files_ranks_and_skips_build_dirs() {
        let root = temp_root("search");
        write(&root, "docs/readme.md", "r");
        write(&root, "src/main.rs", "m");
        write(&root, "target/debug/deadbeef", "b");
        write(&root, ".hidden/secret.txt", "s");
        write(&root, "node_modules/pkg/index.js", "n");

        let hits = search_files(&root, "read", 8);
        assert_eq!(hits, vec!["docs/readme.md".to_string()]);

        // Empty query lists files alphabetically, never build/hidden dirs.
        let all = search_files(&root, "", 50);
        assert_eq!(all, vec!["docs/readme.md", "src/main.rs"]);

        // Limit is respected.
        assert_eq!(search_files(&root, "", 1).len(), 1);
        // Missing root yields nothing instead of erroring.
        assert!(search_files(&root.join("nope"), "x", 5).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_attaches_workspace_file() {
        let root = temp_root("expand");
        write(&root, "notes/hello.txt", "Hi from the file");
        let out = expand("check @notes/hello.txt please", &root);
        assert_eq!(
            out,
            "check <file path=\"notes/hello.txt\">\nHi from the file\n</file> please"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_skips_code_missing_paths_and_duplicates() {
        let root = temp_root("skip");
        write(&root, "notes/hello.txt", "body");
        let raw = "inline `@notes/hello.txt` kept";
        assert_eq!(expand(raw, &root), raw);
        let fenced = "```\n@notes/hello.txt\n```";
        assert_eq!(expand(fenced, &root), fenced);
        assert_eq!(expand("see @missing.txt", &root), "see @missing.txt");
        // Only the first occurrence attaches; the rest stay as plain text.
        let twice = "@notes/hello.txt then @notes/hello.txt";
        let out = expand(twice, &root);
        assert_eq!(out.matches("<file path=").count(), 1);
        assert!(out.ends_with("then @notes/hello.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_caps_bytes_and_file_count() {
        let root = temp_root("caps");
        write(&root, "big.txt", &"x".repeat(MENTION_MAX_BYTES + 500));
        for n in 0..MENTION_MAX_FILES + 2 {
            write(&root, &format!("f{n}.txt"), "ok");
        }
        let out = expand("@big.txt @f0.txt @f1.txt @f2.txt @f3.txt @f4.txt", &root);
        assert_eq!(out.matches("<file path=").count(), MENTION_MAX_FILES);
        assert!(out.contains("<file path=\"big.txt\" truncated>"));
        assert!(out.contains("@f4.txt"), "fifth+ mentions stay raw");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_rejects_escapes_and_binary() {
        let root = temp_root("escape");
        write(&root, "inside.txt", "in");
        let out = expand("@../outside.txt @inside.txt", &root);
        assert!(out.starts_with("@../outside.txt "));
        assert!(out.contains("<file path=\"inside.txt\">"));
        // Absolute paths outside the root are ignored (canonicalize may
        // succeed, but containment fails).
        let etc = expand("read @/etc/hosts", &root);
        assert_eq!(etc, "read @/etc/hosts");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expand_is_a_noop_without_root_or_at() {
        let root = Path::new("");
        assert_eq!(expand("plain prompt", root), "plain prompt");
        assert_eq!(expand("@x/y.txt", root), "@x/y.txt");
        let real = temp_root("noop");
        assert_eq!(expand("no mentions here", &real), "no mentions here");
        let _ = std::fs::remove_dir_all(&real);
    }
}
