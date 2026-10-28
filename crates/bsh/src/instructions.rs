//! Workspace instruction discovery (`AGENTS.md`), aligned with the shipped
//! DeepSeek Harness `dsh-agent-instructions` plugin.
//!
//! The reference discovers instruction files on every turn and delivers them as
//! a **user** message framed by `<system-reminder>` - never as a system
//! section - because a file can change while a session lives and the system
//! prompt is cached. BSH reproduces that shape: the turn build calls
//! [`render`], which returns the bounded, framed text (or none when the
//! workspace has no instruction file), and the runner injects it ahead of the
//! turn's own input.
//!
//! Measured from `dsh-agent-instructions__lib__index.js`:
//!
//! - the project root is the first directory at or above the working directory
//!   that contains a `.git` marker (falling back to the working directory);
//! - discovery walks that root DOWN to the working directory, broadest first,
//!   and within one directory takes `AGENTS.md`, `CLAUDE.md`, then
//!   `AGENTS.local.md`, `CLAUDE.local.md`;
//! - byte-identical (trimmed) duplicates inside one directory collapse to the
//!   earlier candidate;
//! - the framing strings are `AGENT_INSTRUCTIONS_INTRO` and
//!   `Instructions from: <path>\n\n<content>`, and a single source is capped
//!   at 1,048,576 bytes. The rendered baseline is capped at 16,384 bytes rather
//!   than the reference's 65,536 (see [`MAX_RENDERED_BYTES`]): BSH sends the
//!   text on every turn, so size is a per-request cost here, not a cached one.
//!
//! Two deliberate differences, both stated rather than silent:
//!
//! - the reference also reads a user-global `AGENTS.md` under its harness
//!   home. BSH has no user-global instruction scope (its machine state is
//!   `~/.bos`, and a second prompt source would compete with the workspace
//!   persona), so discovery is workspace-only;
//! - a file that cannot be read, is not UTF-8, or exceeds the source cap is
//!   still named in the injected text. The reference drops such a file, which
//!   would let a broken `AGENTS.md` disappear without a trace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// The reference's baseline framing sentence.
///
/// `dsh-agent-instructions__lib__index.js` `AGENT_INSTRUCTIONS_INTRO`; the
/// bytes are the reference's.
pub(crate) const INSTRUCTIONS_INTRO: &str = "The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";

/// The reference's compact replacement used when even the most specific file
/// had to be truncated to fit the budget.
const COMPACT_INTRO: &str =
    "Workspace instructions were omitted or truncated to fit the configured byte budget.";

/// The rendered budget for one baseline.
///
/// The reference uses `maxBytes: 65536` for its standard preset, but it renders
/// that text into the system prompt it caches; BSH prepends it to the turn as a
/// user message that is rebuilt from the stored transcript on every turn, and
/// the same request is re-sent on every tool step of that turn. At 65,536 bytes
/// a large instruction file costs roughly 16k prompt tokens per request for
/// text the model mostly does not need to act.
///
/// 16,384 bytes is the measured compromise: it keeps a normal `AGENTS.md` whole
/// (the common case), and a larger one is truncated by the most specific file
/// first with the named marker below, so the model knows the rest exists and
/// can `read_file` it when a rule matters.
pub(crate) const MAX_RENDERED_BYTES: usize = 16_384;

/// The reference's per-source cap (`maxSourceBytes: 1048576`).
pub(crate) const MAX_SOURCE_BYTES: u64 = 1_048_576;

/// Instruction file names tried in every directory, broadest applicability
/// first. The reference's `DEFAULT_INSTRUCTION_FILE_CANDIDATES`.
pub(crate) const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Local (usually git-ignored) instruction file names tried after the shared
/// ones. The reference's `DEFAULT_LOCAL_INSTRUCTION_FILE_CANDIDATES`.
pub(crate) const LOCAL_INSTRUCTION_FILES: &[&str] = &["AGENTS.local.md", "CLAUDE.local.md"];

/// The marker whose presence identifies the project root.
///
/// The reference's `DEFAULT_PROJECT_ROOT_MARKERS`; it probes any filesystem
/// entry, not only a directory, because a worktree may record `.git` as a file.
const PROJECT_ROOT_MARKERS: &[&str] = &[".git"];

const FRAME_OPEN: &str = "<system-reminder>";
const FRAME_CLOSE: &str = "</system-reminder>";

/// One instruction file that was discovered and (attempted to be) read.
#[derive(Debug, Clone)]
struct LoadedFile {
    /// Project-root-relative path the model sees.
    display: String,
    /// The exact text, or the reason it could not be read.
    content: Result<String, String>,
}

/// A file as it contributes to the rendered baseline.
#[derive(Debug, Clone)]
struct RenderedFile {
    display: String,
    /// The bytes this file contributed to the render.
    content: String,
    /// Bytes the source actually held, for the truncation marker.
    original_bytes: usize,
}

/// The cached answer for one path.
#[derive(Debug, Clone)]
struct CachedFile {
    modified: Option<SystemTime>,
    len: u64,
    content: String,
    /// Reads performed for this path. Test-only probe for "a turn does not
    /// re-read every file"; a cache hit must not move it.
    reads: usize,
}

/// The per-path content cache, keyed by the path and validated by
/// `(mtime, len)`.
///
/// A process-wide cache is safe here because it stores *content*, never a
/// decision: every caller still re-stats the file, so a change is picked up on
/// the next turn regardless of which session wrote it. Failures are not cached
/// (a permission fix changes no mtime), which keeps an unreadable file
/// retryable.
fn cache() -> &'static Mutex<HashMap<PathBuf, CachedFile>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFile>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read one file, reusing the cached text while its `(mtime, len)` is
/// unchanged.
fn read_cached(path: &Path) -> Result<String, String> {
    let stored = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let len = stored.len();
    let modified = stored.modified().ok();
    let previous = cache().lock().unwrap().get(path).cloned();
    if let Some(previous) = previous {
        if previous.modified == modified && previous.len == len {
            return Ok(previous.content);
        }
    }
    let content = if len > MAX_SOURCE_BYTES {
        Err(format!(
            "the file is larger than the {MAX_SOURCE_BYTES}-byte source cap"
        ))
    } else {
        match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|_| "not valid UTF-8".to_string()),
            Err(error) => Err(error.to_string()),
        }
    };
    let reads = cache()
        .lock()
        .unwrap()
        .get(path)
        .map(|entry| entry.reads + 1)
        .unwrap_or(1);
    if let Ok(text) = &content {
        cache().lock().unwrap().insert(
            path.to_path_buf(),
            CachedFile {
                modified,
                len,
                content: text.clone(),
                reads,
            },
        );
    }
    content
}

/// How many times this path was actually read. A cache hit leaves it alone.
#[cfg(test)]
pub(crate) fn read_count(path: &Path) -> usize {
    // Discovery canonicalises the workspace, so the cache key is the
    // canonical path even when the caller built it from a symlinked temp dir.
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    cache()
        .lock()
        .unwrap()
        .get(&path)
        .map(|entry| entry.reads)
        .unwrap_or(0)
}

/// Drop the cache. Tests use unique directories, so this exists for the probes
/// that must start from an empty map.
#[cfg(test)]
pub(crate) fn clear_cache() {
    cache().lock().unwrap().clear();
}

/// The first directory at or above `cwd` holding a project-root marker.
fn project_root(cwd: &Path) -> PathBuf {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut current = start.clone();
    loop {
        if PROJECT_ROOT_MARKERS
            .iter()
            .any(|marker| current.join(marker).exists())
        {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return start,
        }
    }
}

/// Every directory from `root` down to `cwd`, broadest first.
fn ancestor_chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut current = cwd.to_path_buf();
    while current != root {
        chain.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    chain.push(root.to_path_buf());
    chain.reverse();
    chain
}

/// Every present instruction candidate, in the reference's precedence order.
fn discover(cwd: &Path) -> Vec<(PathBuf, PathBuf)> {
    let root = project_root(cwd);
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut found = Vec::new();
    for directory in ancestor_chain(&root, &cwd) {
        for candidates in [INSTRUCTION_FILES, LOCAL_INSTRUCTION_FILES] {
            for name in candidates {
                let path = directory.join(name);
                let present = std::fs::metadata(&path)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false);
                if present {
                    found.push((path, root.clone()));
                }
            }
        }
    }
    found
}

/// A project-root-relative display path, using `/` on every platform because
/// the model reads it as prose.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Load the discovered files, collapsing a trimmed-content duplicate inside one
/// directory (the reference keeps the earlier candidate and its exact bytes).
fn load(cwd: &Path) -> Vec<LoadedFile> {
    let mut files: Vec<LoadedFile> = Vec::new();
    let mut seen: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for (path, root) in discover(cwd) {
        let directory = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(PathBuf::new);
        let content = read_cached(&path);
        if let Ok(text) = &content {
            let digests = seen.entry(directory).or_default();
            let trimmed = text.trim();
            if digests.iter().any(|kept| kept == trimmed) {
                continue;
            }
            digests.push(trimmed.to_string());
        }
        files.push(LoadedFile {
            display: display_path(&root, &path),
            content,
        });
    }
    files
}

/// The reference's UTF-8-safe byte truncation.
fn truncate_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Escape the frame's closing tag inside the body, as the reference does, so a
/// file cannot close the `<system-reminder>` frame early.
fn escape_frame_body(body: &str) -> String {
    body.replace(FRAME_CLOSE, "<\\/system-reminder>")
}

fn section(file: &RenderedFile) -> String {
    format!("Instructions from: {}\n\n{}", file.display, file.content)
}

fn marker(omitted: &[RenderedFile], truncated: Option<(&RenderedFile, usize)>) -> String {
    if omitted.is_empty() && truncated.is_none() {
        return String::new();
    }
    let mut parts = Vec::new();
    if !omitted.is_empty() {
        let names = omitted
            .iter()
            .map(|file| file.display.clone())
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("omitted {names}"));
    }
    if let Some((file, included)) = truncated {
        parts.push(format!(
            "truncated {} from {} to {} bytes",
            file.display, file.original_bytes, included
        ));
    }
    format!(
        "Workspace instruction budget {MAX_RENDERED_BYTES} bytes: {}",
        parts.join("; ")
    )
}

/// One rendered batch: optional budget marker, the intro, then every file in
/// order, all inside the reference's frame.
fn assemble(
    files: &[RenderedFile],
    omitted: &[RenderedFile],
    truncated: Option<(&RenderedFile, usize)>,
    intro: &str,
) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let marker = marker(omitted, truncated);
    if !marker.is_empty() {
        blocks.push(marker);
    }
    if !intro.is_empty() {
        blocks.push(intro.to_string());
    }
    blocks.extend(files.iter().map(section));
    let body = escape_frame_body(&blocks.join("\n\n"));
    format!("{FRAME_OPEN}\n{body}\n{FRAME_CLOSE}")
}

/// Truncate the most specific file until the whole frame fits.
fn truncate_to_fit(
    file: &RenderedFile,
    omitted: &[RenderedFile],
    intro: &str,
) -> Option<(RenderedFile, usize)> {
    let original = file.content.len();
    let (mut low, mut high) = (0usize, original);
    let mut best: Option<(RenderedFile, usize)> = None;
    while low <= high {
        let mid = (low + high) / 2;
        let candidate = RenderedFile {
            display: file.display.clone(),
            content: truncate_bytes(&file.content, mid),
            original_bytes: file.original_bytes,
        };
        let included = candidate.content.len();
        let text = assemble(
            std::slice::from_ref(&candidate),
            omitted,
            Some((file, included)),
            intro,
        );
        if text.len() <= MAX_RENDERED_BYTES {
            best = Some((candidate, included));
            low = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            high = mid - 1;
        }
    }
    best
}

/// Render the discovered baseline, or none when the workspace has no
/// instruction file.
///
/// `cwd` is the workspace root a turn runs in. The reference's broadest file
/// is dropped first when the batch is over budget, and the most specific file
/// is truncated as the last resort; a file that could not be read is rendered
/// as a named, explicit note instead of vanishing.
pub(crate) fn render(cwd: &Path) -> Option<String> {
    if cwd.as_os_str().is_empty() {
        return None;
    }
    let loaded = load(cwd);
    if loaded.is_empty() {
        return None;
    }
    let rendered: Vec<RenderedFile> = loaded
        .into_iter()
        .map(|file| {
            let content = match file.content {
                Ok(text) => text,
                Err(reason) => format!(
                    "This file could not be read, so its contents are not included: {reason}."
                ),
            };
            RenderedFile {
                display: file.display,
                original_bytes: content.len(),
                content,
            }
        })
        .collect();
    let full = assemble(&rendered, &[], None, INSTRUCTIONS_INTRO);
    if full.len() <= MAX_RENDERED_BYTES {
        return Some(full);
    }
    for start in 1..rendered.len() {
        let text = assemble(
            &rendered[start..],
            &rendered[..start],
            None,
            INSTRUCTIONS_INTRO,
        );
        if text.len() <= MAX_RENDERED_BYTES {
            return Some(text);
        }
    }
    let (last, omitted) = rendered.split_last()?;
    for intro in [INSTRUCTIONS_INTRO, COMPACT_INTRO] {
        if let Some((file, _)) = truncate_to_fit(last, omitted, intro) {
            return Some(assemble(
                std::slice::from_ref(&file),
                omitted,
                Some((&file, file.content.len())),
                intro,
            ));
        }
    }
    // Unreachable with the shipped 65,536-byte budget (an empty section is far
    // smaller), kept so a future budget cannot make the render vanish silently.
    Some(format!(
        "{FRAME_OPEN}\n{}\n{FRAME_CLOSE}",
        escape_frame_body(COMPACT_INTRO)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch(label: &str) -> PathBuf {
        let unique = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "bsh-instructions-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("a project root");
        dir
    }

    /// A workspace with one AGENTS.md renders the reference's frame, its intro,
    /// and the file labelled by its project-relative path.
    #[test]
    fn one_agents_file_is_injected_with_the_reference_framing() {
        let root = scratch("one");
        std::fs::write(root.join("AGENTS.md"), "Prefer small diffs.").expect("the file");
        let text = render(&root).expect("the file is discovered");
        assert!(text.starts_with("<system-reminder>\n"), "{text}");
        assert!(text.ends_with("</system-reminder>"), "{text}");
        assert!(text.contains(INSTRUCTIONS_INTRO), "{text}");
        assert!(
            text.contains("Instructions from: AGENTS.md\n\nPrefer small diffs."),
            "{text}"
        );
        assert!(
            !text.contains(".git"),
            "the frame is not the project marker: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two files nest in the reference's order: project root first, then the
    /// workspace subdirectory, both labelled root-relative.
    #[test]
    fn nested_files_render_broadest_first_with_their_paths() {
        let root = scratch("nested");
        std::fs::create_dir_all(root.join("crate")).expect("crate");
        std::fs::write(root.join("AGENTS.md"), "root rules").expect("root file");
        std::fs::write(root.join("crate").join("AGENTS.md"), "crate rules").expect("crate file");
        let text = render(&root.join("crate")).expect("both files are discovered");
        let root_at = text
            .find("Instructions from: AGENTS.md")
            .expect("root file");
        let crate_at = text
            .find("Instructions from: crate/AGENTS.md")
            .expect("crate file");
        assert!(root_at < crate_at, "broadest first: {text}");
        assert!(text.contains("root rules") && text.contains("crate rules"));
        // The in-directory candidate order is the reference's: AGENTS.md,
        // CLAUDE.md, then the local pair.
        std::fs::write(root.join("CLAUDE.md"), "claude rules").expect("claude file");
        std::fs::write(root.join("AGENTS.local.md"), "local rules").expect("local file");
        let text = render(&root.join("crate")).expect("still discovered");
        let agents = text.find("Instructions from: AGENTS.md").expect("agents");
        let claude = text.find("Instructions from: CLAUDE.md").expect("claude");
        let local = text
            .find("Instructions from: AGENTS.local.md")
            .expect("local");
        assert!(
            agents < claude && claude < local,
            "candidate order within one directory: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The second turn does not re-read an unchanged file, and a changed one is
    /// re-read on the next turn because the cache is keyed by (path, mtime).
    #[test]
    fn an_unchanged_file_is_not_re_read_and_an_edit_is_picked_up() {
        let root = scratch("mtime");
        let path = root.join("AGENTS.md");
        std::fs::write(&path, "first body").expect("the file");
        clear_cache();
        let first = render(&root).expect("discovered");
        assert!(first.contains("first body"));
        assert_eq!(read_count(&path), 1, "the first render reads the file");

        let second = render(&root).expect("discovered");
        assert!(second.contains("first body"));
        assert_eq!(
            read_count(&path),
            1,
            "an unchanged (path, mtime) is served from the cache"
        );

        std::fs::write(&path, "second body, a different length").expect("rewrite");
        let third = render(&root).expect("discovered");
        assert!(third.contains("second body"), "{third}");
        assert_eq!(read_count(&path), 2, "the edit invalidated the cache");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file that cannot be decoded is named in the injected text instead of
    /// being dropped silently.
    #[test]
    fn an_unreadable_file_is_reported_in_the_text() {
        let root = scratch("unreadable");
        std::fs::write(root.join("AGENTS.md"), [0xff, 0xfe, 0x00]).expect("the invalid bytes");
        let text = render(&root).expect("the file is still rendered");
        assert!(
            text.contains("Instructions from: AGENTS.md"),
            "the path is labelled: {text}"
        );
        assert!(
            text.contains("could not be read") && text.contains("not valid UTF-8"),
            "the reason is stated: {text}"
        );
        assert!(text.contains("<system-reminder>"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No instruction file means NO message, not an empty one.
    #[test]
    fn a_workspace_without_instructions_renders_nothing() {
        let root = scratch("empty");
        assert_eq!(render(&root), None);
        assert_eq!(render(Path::new("")), None, "no workspace, no render");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The rendered batch is bounded by the reference's 65,536-byte budget, and
    /// the marker names what was truncated.
    #[test]
    fn the_rendered_batch_is_bounded_and_names_the_truncation() {
        let root = scratch("budget");
        let body = "x".repeat(MAX_RENDERED_BYTES + 4_096);
        std::fs::write(root.join("AGENTS.md"), &body).expect("the large file");
        let text = render(&root).expect("still rendered");
        assert!(
            text.len() <= MAX_RENDERED_BYTES,
            "the rendered text stays inside the budget: {} bytes",
            text.len()
        );
        assert!(
            text.contains(&format!(
                "Workspace instruction budget {MAX_RENDERED_BYTES} bytes"
            )) && text.contains("truncated AGENTS.md"),
            "the truncation is named: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A trim-identical sibling in one directory collapses behind the earlier
    /// candidate; a different directory keeps its own copy.
    #[test]
    fn a_trim_identical_sibling_collapses_within_its_directory() {
        let root = scratch("dedup");
        std::fs::create_dir_all(root.join(".git")).expect(".git");
        std::fs::write(root.join("AGENTS.md"), "same body").expect("agents");
        std::fs::write(root.join("CLAUDE.md"), "\n same body \n").expect("claude");
        let text = render(&root).expect("rendered");
        assert!(
            !text.contains("Instructions from: CLAUDE.md"),
            "the identical sibling collapses: {text}"
        );
        assert_eq!(text.matches("same body").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The display path never carries the scratch root: the model reads the
    /// project-relative name the reference uses.
    #[test]
    fn the_display_path_is_project_relative() {
        let root = scratch("display");
        std::fs::create_dir_all(root.join(".git")).expect(".git");
        std::fs::create_dir_all(root.join("a").join("b")).expect("nested");
        std::fs::write(root.join("a").join("b").join("AGENTS.md"), "deep").expect("deep file");
        let text = render(&root.join("a").join("b")).expect("discovered");
        assert!(text.contains("Instructions from: a/b/AGENTS.md"), "{text}");
        assert!(!text.contains(&*root.to_string_lossy()), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The project-root marker is `.git`, and a directory without one falls
    /// back to the working directory itself.
    #[test]
    fn the_project_root_is_the_nearest_git_marker_or_the_workspace() {
        let base = std::env::temp_dir().join(format!(
            "bsh-instructions-marker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let nested = base.join("one").join("two");
        std::fs::create_dir_all(&nested).expect("nested");
        assert_eq!(
            project_root(&nested),
            nested.canonicalize().expect("the canonical scratch root"),
            "no marker anywhere"
        );
        std::fs::create_dir_all(base.join(".git")).expect(".git");
        assert_eq!(
            project_root(&nested),
            base.canonicalize().expect("the canonical marker root"),
            "the marker wins"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
