//! The reader's own editor, for a prompt too long for the composer.
//!
//! The desktop's composer grows with whatever is typed into it; a terminal's is
//! the few rows between the transcript and the footer, and a long prompt
//! written there scrolls away from its own beginning. Every terminal reader
//! already has the answer: `$EDITOR`. opencode binds the same gesture, and so
//! does every shell that has carried `^X^E` for forty years.
//!
//! So `/editor` and `^X` write the prompt to a file, run the editor the reader
//! has already configured, and take the buffer back as the prompt. Nothing here
//! touches the Host: the prompt is client state until Enter sends it, and an
//! edit that is cancelled leaves it word for word as it was.

use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Open the reader's editor on `text`, and answer what the buffer holds when
/// they leave it.
///
/// `Ok(None)` is a reader who cancelled: an editor that exits in failure is the
/// same change of mind `:cq` spells, so the caller keeps the prompt it had. A
/// program that could not be started is an error instead, because an editor
/// that never opened would otherwise look exactly like one that opened and
/// saved nothing.
pub(crate) fn run(text: &str) -> io::Result<Option<String>> {
    run_with(&program(), text)
}

/// The editor this build opens: the reader's own choice, or this platform's
/// fallback when they have made none.
fn program() -> Vec<String> {
    program_from(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

/// The editor `$VISUAL` names, else the one `$EDITOR` names, else the fallback.
///
/// `$VISUAL` wins because it is the variable for a full-screen editor, which is
/// what this gesture opens. A variable that names a program *and* its arguments
/// keeps both - `code --wait` is one editor, and half of it would return before
/// the reader had typed anything. A variable set to nothing is not a choice, so
/// a shell that exports `EDITOR=` still leaves the reader with a working editor
/// rather than one called `""`.
fn program_from(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    visual
        .and_then(words)
        .or_else(|| editor.and_then(words))
        .unwrap_or_else(|| vec![fallback().to_string()])
}

/// One program variable split into the words a command line carries, or `None`
/// when the variable names nothing.
fn words(variable: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = variable.split_whitespace().map(str::to_string).collect();
    (!parts.is_empty()).then_some(parts)
}

/// The editor a platform has when the reader configured none.
#[cfg(windows)]
fn fallback() -> &'static str {
    "notepad"
}

/// The editor a platform has when the reader configured none.
#[cfg(not(windows))]
fn fallback() -> &'static str {
    "vi"
}

/// Open `program` on `text`, and read the buffer back.
fn run_with(program: &[String], text: &str) -> io::Result<Option<String>> {
    let Some((command, arguments)) = program.split_first() else {
        return Ok(None);
    };
    let path = temp_path();
    std::fs::write(&path, text)?;
    // Inherited stdio is the point: the editor draws on the reader's terminal,
    // which the caller has already handed back to it.
    let outcome = Command::new(command).args(arguments).arg(&path).status();
    let answered = match outcome {
        Ok(status) if status.success() => {
            std::fs::read_to_string(&path).map(|buffer| Some(trimmed(&buffer)))
        }
        Ok(_) => Ok(None),
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_file(&path);
    answered
}

/// A file in the system's temporary directory, named for this process.
///
/// The name carries the process id and a tally so two prompts edited in a row -
/// or two terminals of this build on the same machine - never share a buffer. A
/// `.md` suffix is what tells the reader's editor to highlight the prompt as
/// prose instead of as plain text.
fn temp_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tally = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("bsh-prompt-{}-{tally}.md", std::process::id()))
}

/// The buffer as a prompt.
///
/// An editor's final newline is its own punctuation rather than a blank line
/// the reader typed, so one is dropped and the rest are kept: a reader who left
/// two means the blank line they see.
fn trimmed(buffer: &str) -> String {
    let buffer = buffer.strip_suffix('\n').unwrap_or(buffer);
    let buffer = buffer.strip_suffix('\r').unwrap_or(buffer);
    buffer.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readers_own_editor_is_the_one_opened() {
        assert_eq!(program_from(Some("vim"), Some("nano")), vec!["vim"]);
        // A variable naming a program with arguments keeps both: `code --wait`
        // is one editor, and half of it would not wait.
        assert_eq!(
            program_from(None, Some("code --wait")),
            vec!["code", "--wait"]
        );
        // An empty variable is not a choice.
        assert_eq!(program_from(Some("  "), Some("nano")), vec!["nano"]);
        assert_eq!(program_from(None, Some("")), vec![fallback()]);
        assert_eq!(program_from(None, None), vec![fallback()]);
    }

    #[test]
    fn a_buffer_loses_only_the_newline_its_editor_added() {
        assert_eq!(trimmed("hello\n"), "hello");
        // A terminal's editor may write the pair.
        assert_eq!(trimmed("hello\r\n"), "hello");
        assert_eq!(trimmed("two\nlines\n"), "two\nlines");
        // A blank line the reader did type is theirs to keep.
        assert_eq!(trimmed("hello\n\n"), "hello\n");
        assert_eq!(trimmed(""), "");
    }

    /// Two prompts edited in a row never share a buffer.
    #[test]
    fn every_edit_opens_its_own_file() {
        let first = temp_path();
        let second = temp_path();
        assert_ne!(first, second);
        assert!(first.starts_with(std::env::temp_dir()));
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("md"));
    }

    /// The whole round trip, with a stand-in for the reader's editor: what the
    /// program writes is what the prompt becomes.
    #[cfg(unix)]
    #[test]
    fn an_editor_writes_the_prompt_that_comes_back() {
        let editor = |script: &str| -> Vec<String> {
            ["sh", "-c", script, "sh"]
                .iter()
                .map(|word| word.to_string())
                .collect()
        };
        // `$1` is the file this build hands over.
        let saved = editor("printf 'edited\\n' > \"$1\"");
        assert_eq!(
            run_with(&saved, "typed").expect("the stand-in editor runs"),
            Some("edited".to_string())
        );
        // An editor that exits in failure is a cancelled edit, not an error.
        let cancelled = editor("exit 1");
        assert_eq!(
            run_with(&cancelled, "typed").expect("a cancelled edit is not an error"),
            None
        );
        // A program that does not exist is the error the reader has to see.
        assert!(run_with(&["bsh-no-such-editor".to_string()], "typed").is_err());
    }
}
