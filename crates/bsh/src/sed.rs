//! The sed tool's engine: one bounded sed program over a workspace file.
//!
//! The engine is uutils sed (the sed crate), the Rust reimplementation of POSIX
//! sed with its GNU extensions. Its public library surface is the compiler and
//! the file processor, not the CLI: uumain returns an exit code, installs
//! process-global signal handlers and prints errors to stderr, all wrong inside
//! a GUI process. This module therefore compiles the script itself and drives
//! process_all_files over a context whose in-place mode is on, so the engine's
//! entire output goes to a file this module owns and nothing reaches stdout.
//!
//! The wrapper keeps GNU's in-place output semantics: the automatic print and
//! the explicit output commands (p, P, =, l, F and the s///p flag) all write to
//! the edited file, which is what makes "quiet + /pattern/p" a filter. F is the
//! one name that needs care: it would otherwise print the private copy's path,
//! so its bytes are rewritten to the target the caller named.
//!
//! The compile-time sandbox is always on. It refuses e, r, R, w, W and the e/w
//! substitution flags before any side effect, so a sed script cannot reach a
//! file or a process that the tool's policy never saw. That is GNU sed's own
//! --sandbox, not a second dialect.
//!
//! One GNU flag is deliberately absent: uutils sed 0.3.0 parses -z but never
//! reads ProcessingContext.null_data, so a null_data option here would be a
//! silent no-op. The tool does not advertise a switch that does nothing.
//!
//! An edit never runs on the reader's file: the program processes a private
//! copy beside the target and the copy replaces the target by rename only when
//! its bytes differ (or when a backup suffix asks for GNU's always-replace
//! semantics). A failed or no-op script therefore leaves the original
//! untouched, inode and mtime included.
//!
//! The target is resolved through symlinks, as every BSH file tool does. That is
//! GNU's --follow-symlinks arm; GNU's default of replacing the link itself is
//! deliberately not offered, because the link can name a file outside the
//! policy's read scope while the link path alone looks local.
//!
//! This module owns the engine and its bounds - script size, input size, the
//! preview cap and the refusals - and nothing about permissions. The caller
//! resolves the path, asks the policy, takes the file lock and records the
//! review snapshot. An edit also reports a bounded line diff, which is the only
//! source the transcript has for what a sed program actually changed.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::rc::Rc;
use std::sync::Mutex;

use sed::sed::command::{Command, CommandData, ProcessingContext};
use sed::sed::compiler::compile;
use sed::sed::processor::process_all_files;
use sed::sed::script_line_provider::ScriptValue;

use crate::caps::MAX_EDIT_BYTES;

/// Serializes the sed runs that read uucore's process-global exit status.
///
/// Only q/Q write that global; the lock is what makes one call's status
/// readable by that call and no other.
static EXIT_CODE_LOCK: Mutex<()> = Mutex::new(());

/// The most bytes one sed script may carry.
///
/// A script is a program, not a document: this is forty times the longest
/// script in sed's own test corpus, and past it the request is more likely a
/// pasted file than a program.
pub(crate) const MAX_SCRIPT_BYTES: usize = 8 * 1024;

/// The most changed lines the result carries for the transcript card.
///
/// The card is a preview, and the whole tool result rides under the agent's
/// output cap: twenty 160-character lines stay well inside it while the counts
/// remain exact.
pub(crate) const MAX_DIFF_LINES: usize = 20;

/// The widest changed line the result carries, in characters.
const MAX_DIFF_LINE_CHARS: usize = 160;

/// The most bytes a dry run returns as text.
///
/// A preview is read to decide whether to keep the edit, so the head of a large
/// result is enough; the full text is one in-place call away.
pub(crate) const MAX_PREVIEW_BYTES: usize = 64 * 1024;

/// One sed request, after the caller parsed its arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request<'a> {
    /// The sed program, as one string (newlines and semicolons both work).
    pub(crate) script: &'a str,
    /// Match with POSIX extended regular expressions (-E).
    pub(crate) extended: bool,
    /// Disable GNU extensions (--posix).
    pub(crate) posix: bool,
    /// Suppress the automatic print (-n), like GNU sed.
    pub(crate) quiet: bool,
    /// Wrap the l command at this column (-l), defaulting to GNU's 70.
    pub(crate) line_length: Option<usize>,
    /// Keep the original at this path before replacing it (-iSUFFIX).
    pub(crate) backup: Option<&'a Path>,
}

/// What one run did.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// Whether the bytes on disk (or in the copy) differ from the input.
    pub(crate) changed: bool,
    /// The input size, in bytes.
    pub(crate) before_bytes: usize,
    /// The result size, in bytes.
    pub(crate) after_bytes: usize,
    /// The result text on a dry run, when it is UTF-8.
    pub(crate) content: Option<String>,
    /// Whether the preview was clipped at [MAX_PREVIEW_BYTES].
    pub(crate) truncated: bool,
    /// Whether the result on a dry run is not UTF-8 text.
    pub(crate) binary: bool,
    /// The status GNU's q/Q set for this run; 0 when neither ran.
    pub(crate) exit_code: i32,
    /// The changed lines, bounded for the transcript's changed-files card.
    pub(crate) diff: Vec<(char, String)>,
    /// The full number of inserted lines, even past the diff cap.
    pub(crate) added: usize,
    /// The full number of deleted lines, even past the diff cap.
    pub(crate) deleted: usize,
    /// Whether the diff list omits changed lines past the cap.
    pub(crate) diff_truncated: bool,
}

/// The compiled command graph the engine executes.
type Program = Option<Rc<RefCell<Command>>>;

/// A compiled program, the context that compiled it, and the one output fact
/// this wrapper has to correct.
struct CompiledProgram {
    program: Program,
    context: ProcessingContext,
    /// Whether the script names F, whose output is the input path.
    prints_filename: bool,
}

/// A sed request that could not run, split by whose mistake it is.
#[derive(Debug)]
pub(crate) enum Error {
    /// The script or the target cannot be used as written.
    Refused(String),
    /// The engine or the filesystem failed while running.
    Failed(String),
}

impl Error {
    /// The sentence the tool reports, for the tests that assert on refusals.
    #[cfg(test)]
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Refused(message) | Self::Failed(message) => message,
        }
    }
}

/// Apply one sed program to target.
///
/// A dry run processes a private copy in the system temp directory and removes
/// it. An edit processes a private copy BESIDE the target and renames that copy
/// over the target only when its bytes differ, or when a backup suffix asks for
/// GNU's always-replace semantics. A failed or no-op script leaves the original
/// file - inode and mtime included - untouched.
pub(crate) fn apply(target: &Path, request: &Request, dry_run: bool) -> Result<Outcome, Error> {
    if request.script.trim().is_empty() {
        return Err(Error::Refused("script must not be empty".to_string()));
    }
    if request.script.len() > MAX_SCRIPT_BYTES {
        return Err(Error::Refused(format!(
            "script is {} bytes, past the {MAX_SCRIPT_BYTES} byte ceiling; split it into several calls",
            request.script.len()
        )));
    }
    if dry_run && request.backup.is_some() {
        return Err(Error::Refused(
            "a dry run writes nothing, so it cannot keep a backup".to_string(),
        ));
    }
    let CompiledProgram {
        program,
        mut context,
        prints_filename,
    } = compile_program(request)?;
    let before = read_bounded(target)?;
    let before_bytes = before.len();

    let temporary = CopyTarget::new(target, dry_run)?;
    // q/Q set a process-global exit status. Sed calls are serialized by this
    // lock so one call's q cannot be read by another; every other use of that
    // global in this process is unrelated to sed.
    let exit_guard = EXIT_CODE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    uucore::error::set_exit_code(0);
    let processed = process_all_files(program, vec![temporary.path().to_path_buf()], &mut context)
        .map_err(|error| {
            // The engine names the path it processed, which is the private
            // copy. The request was about the target, so rewrite the copy's
            // name to the file the caller named.
            let mut message = error.to_string();
            if let Some(copy) = temporary.path().to_str() {
                message = message.replace(copy, &target.display().to_string());
            }
            Error::Failed(message)
        });
    let exit_code = uucore::error::get_exit_code();
    drop(exit_guard);
    processed?;

    let mut after = read_bounded(temporary.path())?;
    if prints_filename {
        // GNU's F writes the CURRENT input path. Here that is the private copy,
        // so the bytes are rewritten to the target before the result is judged
        // or kept. A dry run only needs the in-memory text; an edit must also
        // correct the copy that is about to replace the target.
        let remapped = remap_input_name(&after, temporary.path(), target);
        if !dry_run && remapped != after {
            rewrite_private(temporary.path(), &remapped)?;
        }
        after = remapped;
    }
    let changed = after != before;
    let (content, truncated, binary) = if dry_run {
        preview(&after)
    } else {
        (None, false, false)
    };
    // The card's preview exists only for an edit that actually ran; a dry run's
    // text already travels as content.
    let (diff, added, deleted, diff_truncated) = if dry_run || !changed {
        (Vec::new(), 0, 0, false)
    } else {
        line_diff(&before, &after)
    };
    // GNU's -i always replaces the file and moves the original to the backup;
    // without a backup this tool keeps the stronger no-op rule and leaves an
    // unchanged file alone.
    if !dry_run {
        match request.backup {
            Some(backup) => temporary.replace_with_backup(target, backup)?,
            None if changed => temporary.replace(target)?,
            None => {}
        }
    }
    Ok(Outcome {
        changed,
        before_bytes,
        after_bytes: after.len(),
        content,
        truncated,
        binary,
        exit_code,
        diff,
        added,
        deleted,
        diff_truncated,
    })
}

/// Compile the script under the same flags the processor will use, and note
/// whether it names F.
///
/// The compiled program and the context travel together: compile fills the
/// context with the label map and ranges the processor reads.
fn compile_program(request: &Request) -> Result<CompiledProgram, Error> {
    let mut context = context(request)?;
    let program = compile(
        vec![ScriptValue::StringVal(request.script.to_string())],
        &mut context,
    )
    .map_err(|error| Error::Refused(error.to_string()))?;
    let prints_filename = prints_filename(&program);
    Ok(CompiledProgram {
        program,
        context,
        prints_filename,
    })
}

/// Build the context the compiler and the processor share.
///
/// The flags, the character mode, the sandbox and in-place output are what this
/// tool decides; the rest is the engine's own initial state.
fn context(request: &Request) -> Result<ProcessingContext, Error> {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| {
            let value = std::env::var(name).ok()?;
            (!value.is_empty()).then_some(value)
        })
        .unwrap_or_else(|| "C".to_string());
    let character_mode = sed::sed::character_mode_for_locale(&locale)
        .map_err(|error| Error::Failed(error.to_string()))?;
    Ok(ProcessingContext {
        regex_extended: request.extended,
        in_place: true,
        quiet: request.quiet,
        length: request.line_length.unwrap_or(70),
        posix: request.posix,
        sandbox: true,
        character_mode,
        ..ProcessingContext::default()
    })
}

/// Whether the script names GNU's F, which writes the current input path.
///
/// In-place output sends that path to the edited file, and here the path is the
/// private copy; knowing about F up front is what lets the result carry the
/// target's name instead.
fn prints_filename(root: &Program) -> bool {
    let mut pending = vec![root.clone()];
    let mut seen: HashSet<*const RefCell<Command>> = HashSet::new();
    while let Some(head) = pending.pop() {
        let mut current = head;
        while let Some(command) = current {
            let pointer = Rc::as_ptr(&command);
            if !seen.insert(pointer) {
                break;
            }
            let command = command.borrow();
            if command.code == 'F' {
                return true;
            }
            if command.code == '{' {
                if let CommandData::BranchTarget(Some(body)) = &command.data {
                    pending.push(Some(Rc::clone(body)));
                }
            }
            current = command.next.clone();
        }
    }
    false
}

/// Rewrite F's copy path to the target path, byte for byte.
///
/// F writes the path exactly as the engine received it, so the needle and the
/// replacement are the OS-encoded path bytes, not a display string.
fn remap_input_name(bytes: &[u8], copy: &Path, target: &Path) -> Vec<u8> {
    let needle = copy.as_os_str().as_encoded_bytes();
    if needle.is_empty() || memchr::memmem::find(bytes, needle).is_none() {
        return bytes.to_vec();
    }
    let replacement = target.as_os_str().as_encoded_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut last = 0;
    for at in memchr::memmem::find_iter(bytes, needle) {
        out.extend_from_slice(&bytes[last..at]);
        out.extend_from_slice(replacement);
        last = at + needle.len();
    }
    out.extend_from_slice(&bytes[last..]);
    out
}

/// Replace a private copy's bytes atomically, keeping its mode.
///
/// F's output must be corrected before an edit is kept, and the copy may carry
/// the target's read-only mode; a fresh temp plus rename writes the correction
/// without asking the read-only file for write permission.
fn rewrite_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".bsh-sed-rewrite-").suffix(".tmp");
    let mut created = builder
        .tempfile_in(parent)
        .map_err(|error| Error::Failed(format!("rewriting {}: {error}", path.display())))?;
    created
        .write_all(bytes)
        .and_then(|()| created.flush())
        .map_err(|error| Error::Failed(format!("rewriting {}: {error}", path.display())))?;
    let metadata = fs::metadata(path)
        .map_err(|error| Error::Failed(format!("{}: {error}", path.display())))?;
    fs::set_permissions(created.path(), metadata.permissions())
        .map_err(|error| Error::Failed(format!("{}: {error}", path.display())))?;
    let temporary = created.into_temp_path();
    let kept = temporary
        .keep()
        .map_err(|error| Error::Failed(format!("rewriting {}: {error}", path.display())))?;
    if let Err(error) = fs::rename(&kept, path) {
        let _ = fs::remove_file(&kept);
        return Err(Error::Failed(format!(
            "rewriting {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

/// A private copy the engine can rewrite and this module can discard or keep.
struct CopyTarget {
    path: tempfile::TempPath,
}

impl CopyTarget {
    /// Copy source into a private temp file.
    ///
    /// An edit's copy sits beside the target so the kept result can replace the
    /// target by rename on the same filesystem; a dry run's copy lives in the
    /// system temp directory.
    fn new(source: &Path, dry_run: bool) -> Result<Self, Error> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(".bsh-sed-").suffix(".tmp");
        let created = if dry_run {
            builder.tempfile()
        } else {
            let parent = source
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            builder.tempfile_in(parent)
        };
        let created = created.map_err(|error| {
            Error::Failed(format!(
                "creating a private copy of {}: {error}",
                source.display()
            ))
        })?;
        let path = created.into_temp_path();
        fs::copy(source, &path)
            .map_err(|error| Error::Failed(format!("copying {}: {error}", source.display())))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the processed copy and move it over the original in one rename.
    fn replace(self, target: &Path) -> Result<(), Error> {
        let path = self.keep(target)?;
        if let Err(error) = fs::rename(&path, target) {
            let _ = fs::remove_file(&path);
            return Err(Error::Failed(format!(
                "replacing {}: {error}",
                target.display()
            )));
        }
        Ok(())
    }

    /// Move the original to backup, then the processed copy over the target.
    ///
    /// This is GNU's -iSUFFIX order: the backup is the original inode, mode and
    /// mtime (a rename, not a copy), and the target is the processed copy. A
    /// failed second rename restores the backup so the target is never lost.
    fn replace_with_backup(self, target: &Path, backup: &Path) -> Result<(), Error> {
        let path = self.keep(target)?;
        if let Err(error) = fs::rename(target, backup) {
            let _ = fs::remove_file(&path);
            return Err(Error::Failed(format!(
                "moving {} to {}: {error}",
                target.display(),
                backup.display()
            )));
        }
        if let Err(error) = fs::rename(&path, target) {
            // Put the original back before reporting; a stranded backup is
            // recoverable, a missing target is not.
            let _ = fs::rename(backup, target);
            let _ = fs::remove_file(&path);
            return Err(Error::Failed(format!(
                "replacing {}: {error}",
                target.display()
            )));
        }
        Ok(())
    }

    /// Persist the temp path, naming the target when persistence fails.
    fn keep(self, target: &Path) -> Result<std::path::PathBuf, Error> {
        self.path.keep().map_err(|error| {
            Error::Failed(format!(
                "keeping the processed copy of {}: {error}",
                target.display()
            ))
        })
    }
}

/// Read a file that the tool is about to edit, bounded by the edit ceiling.
fn read_bounded(target: &Path) -> Result<Vec<u8>, Error> {
    let metadata = fs::metadata(target)
        .map_err(|error| Error::Failed(format!("{}: {error}", target.display())))?;
    if !metadata.is_file() {
        return Err(Error::Refused(format!(
            "{} is not a regular file",
            target.display()
        )));
    }
    if metadata.len() > MAX_EDIT_BYTES {
        return Err(Error::Refused(format!(
            "{} is {} bytes, too large for sed (the ceiling is {MAX_EDIT_BYTES} bytes)",
            target.display(),
            metadata.len()
        )));
    }
    let file = fs::File::open(target)
        .map_err(|error| Error::Failed(format!("{}: {error}", target.display())))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_EDIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Failed(format!("{}: {error}", target.display())))?;
    if bytes.len() as u64 > MAX_EDIT_BYTES {
        return Err(Error::Refused(format!(
            "{} grew past the {MAX_EDIT_BYTES} byte ceiling while it was read",
            target.display()
        )));
    }
    Ok(bytes)
}

/// The changed lines between two UTF-8 versions, with full counts.
///
/// A binary pair has no line diff; the counts stay zero and the transcript omits
/// the card's preview rather than pretending bytes are lines. Context lines are
/// left out because the existing card previews only the lines a step changed.
fn line_diff(before: &[u8], after: &[u8]) -> (Vec<(char, String)>, usize, usize, bool) {
    let (Ok(before), Ok(after)) = (std::str::from_utf8(before), std::str::from_utf8(after)) else {
        return (Vec::new(), 0, 0, false);
    };
    let diff = similar::TextDiff::from_lines(before, after);
    let mut lines = Vec::new();
    let mut added = 0;
    let mut deleted = 0;
    let mut truncated = false;
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Insert => '+',
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Equal => continue,
        };
        if sign == '+' {
            added += 1;
        } else {
            deleted += 1;
        }
        if lines.len() >= MAX_DIFF_LINES {
            truncated = true;
            continue;
        }
        lines.push((sign, clip_diff_line(change.value())));
    }
    (lines, added, deleted, truncated)
}

/// One diff line, clipped to the card's width on a character boundary.
fn clip_diff_line(value: &str) -> String {
    let line = value.trim_end_matches(['\n', '\r']);
    let mut chars = line.chars();
    let mut out: String = chars.by_ref().take(MAX_DIFF_LINE_CHARS).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// The text a dry run reports: lossless when UTF-8, clipped at the preview cap.
fn preview(bytes: &[u8]) -> (Option<String>, bool, bool) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return (None, false, true);
    };
    if text.len() <= MAX_PREVIEW_BYTES {
        return (Some(text.to_string()), false, false);
    }
    let mut cut = MAX_PREVIEW_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (Some(text[..cut].to_string()), true, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bsh-sed-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn request(script: &str) -> Request<'_> {
        Request {
            script,
            extended: false,
            posix: false,
            quiet: false,
            line_length: None,
            backup: None,
        }
    }

    #[test]
    fn a_substitution_edits_the_file_in_place() {
        let dir = scratch("substitute");
        let file = dir.join("a.txt");
        fs::write(&file, "one two one\n").expect("the input");
        let outcome = apply(&file, &request("s/one/1/g"), false).expect("sed runs");
        assert!(outcome.changed);
        assert_eq!(outcome.before_bytes, 12);
        assert_eq!(outcome.after_bytes, 8);
        assert_eq!(fs::read_to_string(&file).expect("the result"), "1 two 1\n");
        assert!(outcome.content.is_none(), "an edit returns no preview");
        assert_eq!(outcome.added, 1);
        assert_eq!(outcome.deleted, 1);
        assert_eq!(
            outcome.diff,
            vec![
                ('-', "one two one".to_string()),
                ('+', "1 two 1".to_string())
            ]
        );
        assert!(!outcome.diff_truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn posix_refuses_a_gnu_only_substitution_flag() {
        let dir = scratch("posix");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let request = Request {
            script: "s/alpha/beta/I",
            extended: false,
            posix: true,
            quiet: false,
            line_length: None,
            backup: None,
        };
        let error = apply(&file, &request, false).expect_err("posix rejects I");
        assert!(!error.message().is_empty());
        assert_eq!(fs::read_to_string(&file).expect("untouched"), "alpha\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_large_diff_is_bounded_but_still_counted() {
        let dir = scratch("diff-cap");
        let file = dir.join("a.txt");
        let content: String = (0..30).map(|index| format!("line {index}\n")).collect();
        fs::write(&file, content).expect("the input");
        let outcome = apply(&file, &request("s/^/x /"), false).expect("sed runs");
        assert!(outcome.changed);
        assert_eq!(outcome.added, 30);
        assert_eq!(outcome.deleted, 30);
        assert_eq!(outcome.diff.len(), MAX_DIFF_LINES);
        assert!(outcome.diff_truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_q_command_reports_gnus_exit_status() {
        let dir = scratch("exit-code");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let outcome = apply(&file, &request("s/alpha/beta/;q 7"), false).expect("sed runs");
        assert!(outcome.changed);
        assert_eq!(outcome.exit_code, 7);
        assert_eq!(fs::read_to_string(&file).expect("the edit"), "beta\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quiet_filter_keeps_only_matching_lines() {
        let dir = scratch("quiet-filter");
        let file = dir.join("a.txt");
        fs::write(&file, "one\ntwo\nthree\n").expect("the input");
        let quiet = Request {
            script: "/two/p",
            extended: false,
            posix: false,
            quiet: true,
            line_length: None,
            backup: None,
        };
        let outcome = apply(&file, &quiet, false).expect("the filter runs");
        assert!(outcome.changed);
        assert_eq!(outcome.after_bytes, 4);
        assert_eq!(fs::read_to_string(&file).expect("the filter"), "two\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_reports_the_text_and_leaves_the_file_alone() {
        let dir = scratch("dry-run");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\nbeta\n").expect("the input");
        let outcome = apply(&file, &request("/beta/d"), true).expect("sed runs");
        assert!(outcome.changed);
        assert_eq!(outcome.content.as_deref(), Some("alpha\n"));
        assert_eq!(
            fs::read_to_string(&file).expect("the original"),
            "alpha\nbeta\n"
        );
        assert!(!outcome.truncated);
        assert!(!outcome.binary);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_script_that_changes_nothing_reports_unchanged() {
        let dir = scratch("unchanged");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let outcome = apply(&file, &request("s/missing/present/"), false).expect("sed runs");
        assert!(!outcome.changed);
        assert_eq!(fs::read_to_string(&file).expect("the original"), "alpha\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_commands_write_to_the_edited_file_like_gnu_sed() {
        let dir = scratch("output-commands");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        // GNU sed -i 'p' duplicates every line into the file.
        let outcome = apply(&file, &request("p"), false).expect("p runs");
        assert!(outcome.changed);
        assert_eq!(
            fs::read_to_string(&file).expect("the edit"),
            "alpha\nalpha\n"
        );

        // GNU sed -i -n '/beta/p' is the filter idiom.
        fs::write(&file, "alpha\nbeta\n").expect("the input");
        let quiet = Request {
            script: "/beta/p",
            extended: false,
            posix: false,
            quiet: true,
            line_length: None,
            backup: None,
        };
        let outcome = apply(&file, &quiet, false).expect("the filter runs");
        assert!(outcome.changed);
        assert_eq!(fs::read_to_string(&file).expect("the filter"), "beta\n");

        // The substitution's p flag writes the substituted line too.
        fs::write(&file, "alpha\n").expect("the input");
        let outcome = apply(&file, &request("s/alpha/beta/p"), false).expect("s///p runs");
        assert!(outcome.changed);
        assert_eq!(fs::read_to_string(&file).expect("the edit"), "beta\nbeta\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_l_command_uses_the_requested_width() {
        let dir = scratch("line-length");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let quiet = Request {
            script: "l",
            extended: false,
            posix: false,
            quiet: true,
            line_length: Some(20),
            backup: None,
        };
        apply(&file, &quiet, false).expect("l runs");
        assert_eq!(fs::read_to_string(&file).expect("the listing"), "alpha$\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_f_command_names_the_target_not_the_private_copy() {
        let dir = scratch("filename");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let quiet = Request {
            script: "F",
            extended: false,
            posix: false,
            quiet: true,
            line_length: None,
            backup: None,
        };
        let compiled = compile_program(&quiet).expect("the script compiles");
        assert!(
            compiled.prints_filename,
            "F is detected in the compiled graph"
        );
        apply(&file, &quiet, false).expect("F runs");
        let content = fs::read_to_string(&file).expect("the name");
        assert_eq!(content, format!("{}\n", file.display()));
        assert!(!content.contains(".bsh-sed-"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backup_suffix_keeps_the_original_before_replacing() {
        let dir = scratch("backup");
        let file = dir.join("a.txt");
        let backup = dir.join("a.txt.bak");
        fs::write(&file, "alpha\n").expect("the input");
        let original_mtime = fs::metadata(&file)
            .expect("the input")
            .modified()
            .expect("a modification time");
        let request = Request {
            script: "s/alpha/beta/",
            extended: false,
            posix: false,
            quiet: false,
            line_length: None,
            backup: Some(&backup),
        };
        let outcome = apply(&file, &request, false).expect("the edit runs");
        assert!(outcome.changed);
        assert_eq!(fs::read_to_string(&file).expect("the edit"), "beta\n");
        assert_eq!(fs::read_to_string(&backup).expect("the backup"), "alpha\n");
        assert_eq!(
            fs::metadata(&backup)
                .expect("the backup")
                .modified()
                .expect("a modification time"),
            original_mtime,
            "the backup is the original inode, so its mtime is preserved"
        );

        // A no-op with a backup still replaces GNU-style: the backup exists and
        // the target keeps its bytes.
        fs::write(&file, "alpha\n").expect("the input");
        let request = Request {
            script: "s/missing/present/",
            ..request
        };
        let outcome = apply(&file, &request, false).expect("the no-op runs");
        assert!(!outcome.changed);
        assert_eq!(fs::read_to_string(&file).expect("the target"), "alpha\n");
        assert_eq!(fs::read_to_string(&backup).expect("the backup"), "alpha\n");

        // A dry run cannot produce a backup.
        let request = Request {
            script: "s/alpha/beta/",
            ..request
        };
        assert!(
            apply(&file, &request, true).is_err(),
            "a dry run has no backup"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sandbox_refuses_file_and_process_commands() {
        let dir = scratch("sandbox");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        for script in ["e echo hi", "w out.txt", "r other.txt", "s/alpha/echo hi/e"] {
            let error = apply(&file, &request(script), false).expect_err(script);
            assert!(
                error.message().contains("sandbox"),
                "{script}: {}",
                error.message()
            );
        }
        assert_eq!(fs::read_to_string(&file).expect("untouched"), "alpha\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_invalid_script_is_refused_without_a_write() {
        let dir = scratch("invalid");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let error = apply(&file, &request("s/a/x"), false).expect_err("unterminated s");
        assert!(!error.message().is_empty());
        assert_eq!(fs::read_to_string(&file).expect("untouched"), "alpha\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_script_is_refused() {
        let dir = scratch("empty");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let error = apply(&file, &request("   "), false).expect_err("empty");
        assert!(error.message().contains("must not be empty"), "{error:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_script_and_file_ceilings_are_named() {
        let dir = scratch("ceilings");
        let file = dir.join("a.txt");
        fs::write(&file, "alpha\n").expect("the input");
        let long = "s/a/b/".repeat(MAX_SCRIPT_BYTES);
        let error = apply(&file, &request(&long), false).expect_err("long script");
        assert!(error.message().contains("ceiling"), "{error:?}");

        let big = dir.join("big.txt");
        let oversized = vec![b'a'; MAX_EDIT_BYTES as usize + 1];
        fs::write(&big, &oversized).expect("the oversized input");
        let error = apply(&big, &request("s/a/b/"), true).expect_err("large file");
        assert!(error.message().contains("too large"), "{error:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extended_expression_uses_the_extended_engine() {
        let dir = scratch("extended");
        let file = dir.join("a.txt");
        fs::write(&file, "ab\n").expect("the input");
        let outcome = apply(
            &file,
            &Request {
                script: "s/(a)(b)/\\2\\1/",
                extended: true,
                posix: false,
                quiet: false,
                line_length: None,
                backup: None,
            },
            true,
        )
        .expect("the extended expression compiles");
        assert_eq!(outcome.content.as_deref(), Some("ba\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_of_binary_content_reports_no_preview() {
        let dir = scratch("binary");
        let file = dir.join("a.bin");
        fs::write(&file, b"a\xffb\n").expect("the input");
        let outcome = apply(&file, &request("s/a/z/"), true).expect("sed runs");
        assert!(outcome.binary);
        assert!(outcome.content.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_preview_is_clipped_on_a_character_boundary() {
        let dir = scratch("preview");
        let file = dir.join("a.txt");
        let line = "é".repeat(MAX_PREVIEW_BYTES / 2 + 8);
        fs::write(&file, format!("{line}\n")).expect("the input");
        let outcome = apply(&file, &request("s/missing/x/"), true).expect("sed runs");
        let content = outcome.content.expect("a preview");
        assert!(outcome.truncated);
        assert!(content.len() <= MAX_PREVIEW_BYTES);
        assert!(content.is_char_boundary(content.len()));
        let _ = fs::remove_dir_all(&dir);
    }
}
