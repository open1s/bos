//! The terminal's `/export`: the open session as one Markdown file.
//!
//! The desktop's Export row saves a ZIP of the session log through a native
//! save dialog, and the reference terminal's own `/export` writes the
//! conversation as Markdown and reports the path it used. A terminal has no
//! file dialog, so the path comes from the command line: `/export <path>`
//! writes where the reader says, and `/export` alone writes under the Host's
//! data directory.
//!
//! The file is built from the projection the client already holds - that is the
//! only transcript a client may read - so an export asks the Host for nothing
//! and changes nothing durable. What it contains is exactly what the terminal
//! shows: the timeline of messages and tool calls, the failure of a turn that
//! did not finish, and the session's own configuration and goal.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::runtime::contract::{
    ExecutionStatus, ExecutionView, GoalState, MessageRole, MessageView, PlanItemStatus,
    SessionProjection, ToolCallStatus, ToolCallView,
};

/// How much of one tool call's output the file keeps.
///
/// A tool result is often a whole file, and this file is read beside the
/// session rather than instead of it. The cut is marked, so a reader learns
/// that more existed instead of believing the output ended there.
const MAX_OUTPUT_CHARS: usize = 4000;

/// The file name an `/export` with no path suggests, from the session's title.
///
/// The desktop's `export_name` rule with the terminal's extension: keep
/// alphanumerics and `-`/`_`, fold every other character to a space, join runs
/// with hyphens, cap the stem at 60 characters, and never produce an empty one.
#[must_use]
pub(crate) fn file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                ' '
            }
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join("-");
    let stem: String = cleaned.chars().take(60).collect();
    let stem = if stem.is_empty() {
        "session".to_string()
    } else {
        stem
    };
    format!("{stem}.md")
}

/// The whole session as Markdown: a header of durable facts, then the
/// transcript in the same order the terminal paints it.
#[must_use]
pub(crate) fn markdown(session: &SessionProjection) -> String {
    let mut out = String::new();
    let title = session.title.trim();
    let title = if title.is_empty() {
        "untitled session"
    } else {
        title
    };
    let _ = writeln!(out, "# {title}\n");
    let _ = writeln!(out, "- **session** `{}`", session.session_id);
    let _ = writeln!(out, "- **created** {}", session.created_at);
    let _ = writeln!(out, "- **updated** {}", session.updated_at);
    let model = session
        .config
        .model_ref()
        .map(|(provider, model)| format!("{provider}/{model}"))
        .unwrap_or_else(|| "machine default".to_string());
    let _ = writeln!(out, "- **model** `{model}`");
    let _ = writeln!(out, "- **permission** `{}`", session.config.permission);
    if let Some(preset) = session.config.preset.as_deref() {
        let _ = writeln!(out, "- **preset** `{preset}`");
    }
    if session.config.plan_mode {
        let _ = writeln!(out, "- **plan** on");
    }
    if let Some(goal) = session.goal.as_ref() {
        let mut line = format!(
            "- **goal** {} · round {}/{} — {}",
            goal_state(goal.state),
            goal.rounds,
            goal.max_rounds,
            goal.objective
        );
        if let Some(reason) = goal.blocked_reason.as_deref() {
            let _ = write!(line, " · blocked: {reason}");
        }
        let _ = writeln!(out, "{line}");
    }
    if let Some(note) = session.feedback.as_deref() {
        let note = note.trim();
        if !note.is_empty() {
            let _ = writeln!(out, "- **feedback** {note}");
        }
    }
    let _ = writeln!(
        out,
        "- **transcript** {} message(s) · {} tool call(s)\n",
        session.messages.len(),
        session.tool_calls.len()
    );
    // The model's working plan is durable session state, so the file carries
    // it: a reader resuming later sees which steps were done, not only what the
    // conversation said about them.
    if !session.plan.is_empty() {
        out.push_str("## Plan\n\n");
        for item in &session.plan {
            let _ = writeln!(out, "- {} {}", plan_box(item.status), item.text);
        }
        out.push('\n');
    }

    out.push_str("## Transcript\n\n");
    let mut rows: Vec<Row> = session
        .messages
        .iter()
        .map(Row::Message)
        .chain(session.tool_calls.iter().map(Row::Tool))
        .chain(
            session
                .executions
                .iter()
                .filter(|execution| is_failure(execution.status))
                .map(Row::Execution),
        )
        .collect();
    // The terminal sorts by committed time, so a call made between two
    // messages reads in the order it happened rather than after the whole
    // conversation. A tie keeps messages ahead of calls, as the sort is stable
    // and messages were collected first.
    rows.sort_by_key(Row::at);
    if rows.is_empty() {
        out.push_str("_This session has no transcript yet._\n");
    }
    for row in rows {
        match row {
            Row::Message(message) => message_block(&mut out, message),
            Row::Tool(call) => tool_block(&mut out, call),
            Row::Execution(execution) => execution_block(&mut out, execution),
        }
    }
    out
}

/// Write one export file, replacing the target in one step.
///
/// The parent directories are created because a reader typing `out/talk.md`
/// should not have to make `out` first. The write goes through a temporary
/// beside the target and a rename, so a failure leaves no half-written file
/// where a previous export used to be.
pub(crate) fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let suffix = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("md");
    let temporary = path.with_extension(format!("{suffix}.part"));
    fs::write(&temporary, text)?;
    crate::atomic_replace::replace(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

/// The absolute path a reader's `path` actually names.
///
/// A relative path resolves against `base` - the workspace the Host serves, so
/// the terminal writes where the desktop window would - and falls back to the
/// reader's own directory when the client was given no workspace. A path that
/// names a directory takes the session's own file name, which is what makes
/// `/export notes/` mean "this conversation, into `notes`".
#[must_use]
pub(crate) fn resolve(path: &str, base: Option<&Path>, title: &str) -> PathBuf {
    let named = match base {
        Some(base) if !Path::new(path).is_absolute() => base.join(path),
        _ => PathBuf::from(path),
    };
    let trailing_separator = path.ends_with(std::path::MAIN_SEPARATOR)
        || (std::path::MAIN_SEPARATOR != '/' && path.ends_with('/'));
    if trailing_separator || named.is_dir() {
        named.join(file_name(title))
    } else {
        named
    }
    .components()
    .collect()
}

/// One row of the exported timeline.
enum Row<'a> {
    Message(&'a MessageView),
    Tool(&'a ToolCallView),
    Execution(&'a ExecutionView),
}

impl Row<'_> {
    fn at(&self) -> chrono::DateTime<chrono::Utc> {
        match self {
            Self::Message(message) => message.at,
            Self::Tool(call) => call.started_at,
            Self::Execution(execution) => execution.started_at,
        }
    }
}

/// Whether a turn stopped without finishing, and so earned a row of its own.
/// One plan step as a Markdown checkbox.
///
/// Checkboxes are the file's form of the panel's glyphs: a reader who opens the
/// export in any Markdown viewer sees which steps were finished, and a step the
/// model is working on reads as neither done nor untouched.
fn plan_box(status: PlanItemStatus) -> &'static str {
    match status {
        PlanItemStatus::Pending => "[ ]",
        PlanItemStatus::InProgress => "[~]",
        PlanItemStatus::Completed => "[x]",
    }
}

/// Whether one execution ended without producing its answer.
fn is_failure(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Failed | ExecutionStatus::Interrupted | ExecutionStatus::NeedsIntervention
    )
}

/// The goal's state in the reader's lowercase words, the same vocabulary the
/// terminal's `/goal` report uses.
fn goal_state(state: GoalState) -> String {
    match state {
        GoalState::Active => "active".to_string(),
        GoalState::Paused => "paused".to_string(),
        GoalState::Completed => "completed".to_string(),
        GoalState::Blocked => "blocked".to_string(),
    }
}

/// The tool call's state in the terminal's own words.
fn tool_state(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Running => "running",
        ToolCallStatus::Completed => "ok",
        ToolCallStatus::Failed => "failed",
    }
}

/// One message: the speaker the terminal paints, then the text verbatim.
fn message_block(out: &mut String, message: &MessageView) {
    let speaker = match (message.compaction, message.role) {
        // A checkpoint is the condensed history the Host kept, not something a
        // participant said, so it is labelled as the terminal labels it.
        (true, _) => "checkpoint",
        (false, MessageRole::User) => "you",
        (false, MessageRole::Assistant) => "bsh",
    };
    let _ = writeln!(out, "### {speaker} · {}\n", message.at);
    out.push_str(message.text.trim_end());
    out.push_str("\n\n");
}

/// One tool call: its name and verdict, the one-line summary, the arguments
/// and the output the model received.
fn tool_block(out: &mut String, call: &ToolCallView) {
    let duration = call
        .duration_ms
        .map(|millis| format!(" · {millis} ms"))
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "### tool `{}` · {}{duration}\n",
        call.tool,
        tool_state(call.status)
    );
    if !call.summary.trim().is_empty() {
        out.push_str(call.summary.trim());
        out.push_str("\n\n");
    }
    if !call.args.trim().is_empty() {
        out.push_str(&block("json", &call.args));
        out.push('\n');
    }
    if let Some(output) = call
        .output
        .as_deref()
        .filter(|output| !output.trim().is_empty())
    {
        out.push_str(&block("text", &capped(output)));
        out.push('\n');
    }
}

/// One turn that stopped without finishing; the terminal shows the same
/// sentence, so the file explains a failure the transcript's messages cannot.
fn execution_block(out: &mut String, execution: &ExecutionView) {
    let state = format!("{:?}", execution.status).to_lowercase();
    let detail = execution.detail.as_deref().unwrap_or("no detail");
    let _ = writeln!(out, "> **{state}** · {detail}\n");
}

/// The first [`MAX_OUTPUT_CHARS`] characters of `text`, with the cut marked and
/// the remainder counted rather than silently dropped.
fn capped(text: &str) -> String {
    let total = text.chars().count();
    if total <= MAX_OUTPUT_CHARS {
        return text.trim_end().to_string();
    }
    let head: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    format!(
        "{}\n\n_(truncated: {} more character(s))_",
        head.trim_end(),
        total - MAX_OUTPUT_CHARS
    )
}

/// One fenced block whose fence is longer than any backtick run inside it.
///
/// Arguments and outputs are arbitrary text: an output that quotes Markdown
/// carries its own fences, and a fixed three-backtick fence would close on
/// them and corrupt the rest of the file.
fn block(label: &str, body: &str) -> String {
    let longest = body
        .split(|character: char| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let body = body.trim_end();
    match label.is_empty() {
        true => format!("{fence}\n{body}\n{fence}\n"),
        false => format!("{fence}{label}\n{body}\n{fence}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::contract::{ApprovalView, PlanItemView, SessionConfig, SessionGoalView};
    use crate::runtime::ids::{ExecutionId, MessageId, SessionId};
    use chrono::{TimeZone as _, Utc};

    fn at(second: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 5, 12, 10, 33, second)
            .single()
            .expect("a real instant")
    }

    fn message(role: MessageRole, text: &str, second: u32) -> MessageView {
        MessageView {
            usage: None,
            message_id: MessageId::new(),
            role,
            text: text.to_string(),
            at: at(second),
            execution_id: Some(ExecutionId::new()),
            compaction: false,
        }
    }

    fn session() -> SessionProjection {
        SessionProjection {
            session_id: SessionId::new(),
            version: 7,
            title: "Release the hounds".to_string(),
            created_at: at(0),
            updated_at: at(30),
            messages: vec![
                message(MessageRole::User, "ship the release", 1),
                message(MessageRole::Assistant, "on it", 3),
            ],
            tool_calls: vec![ToolCallView {
                execution_id: Some(ExecutionId::new()),
                step: 1,
                tool: "shell".to_string(),
                summary: "cargo test".to_string(),
                args: r#"{"command":"cargo test"}"#.to_string(),
                status: ToolCallStatus::Completed,
                output: Some("ok. 46 passed".to_string()),
                duration_ms: Some(42),
                started_at: at(2),
                finished_at: Some(at(2)),
            }],
            executions: Vec::<ExecutionView>::new(),
            approvals: Vec::<ApprovalView>::new(),
            // The plan the model last wrote, one step of each state: the file
            // has to carry what the panel would draw.
            plan: vec![
                PlanItemView {
                    text: "read the RFC".to_string(),
                    status: PlanItemStatus::Completed,
                },
                PlanItemView {
                    text: "land the plan block".to_string(),
                    status: PlanItemStatus::InProgress,
                },
                PlanItemView {
                    text: "verify the export".to_string(),
                    status: PlanItemStatus::Pending,
                },
            ],
            config: SessionConfig::default(),
            compaction: None,
            goal: Some(SessionGoalView {
                objective: "mirror the desktop".to_string(),
                state: GoalState::Active,
                rounds: 8,
                max_rounds: 256,
                blocked_reason: None,
            }),
            feedback: Some("the transcript mirrors the desktop".to_string()),
            trashed: false,
            pinned: false,
            archived: false,
        }
    }

    #[test]
    fn the_file_name_survives_any_title() {
        assert_eq!(file_name("Release the hounds!"), "Release-the-hounds.md");
        assert_eq!(file_name("  "), "session.md");
        assert_eq!(file_name("a  b___c"), "a-b___c.md");
        let long = file_name(&"x".repeat(200));
        assert_eq!(long.chars().count(), "x".repeat(60).chars().count() + 3);
    }

    #[test]
    fn the_markdown_keeps_the_timeline_order_and_the_session_facts() {
        let text = markdown(&session());
        assert!(text.starts_with("# Release the hounds\n"));
        assert!(text.contains("- **goal** active · round 8/256 — mirror the desktop"));
        // The reader's note is durable session state, so the file carries it
        // beside the goal instead of leaving it behind in the window.
        assert!(text.contains("- **feedback** the transcript mirrors the desktop"));
        assert!(text.contains("### you · "));
        assert!(text.contains("### tool `shell` · ok · 42 ms"));
        assert!(text.contains("```json\n{\"command\":\"cargo test\"}\n```"));
        assert!(text.contains("ok. 46 passed"));
        // The call happened between the two messages, so it is written between
        // them rather than after the assistant's answer.
        let you = text.find("### you").expect("the user row");
        let tool = text.find("### tool").expect("the call row");
        let bsh = text.find("### bsh").expect("the answer row");
        assert!(
            you < tool && tool < bsh,
            "timeline order: {you} < {tool} < {bsh}"
        );
    }

    #[test]
    fn a_long_output_is_cut_and_says_so() {
        let mut call = session().tool_calls.remove(0);
        call.output = Some("x".repeat(MAX_OUTPUT_CHARS + 10));
        let mut out = String::new();
        tool_block(&mut out, &call);
        assert!(out.contains("_(truncated: 10 more character(s))_"));
        // The cut is measured on the run itself: a bare count of the filler
        // character would also count the `text` fence label's own x.
        assert!(out.contains(&"x".repeat(MAX_OUTPUT_CHARS)));
        assert!(!out.contains(&"x".repeat(MAX_OUTPUT_CHARS + 1)));
    }

    #[test]
    fn a_fence_outgrows_the_backticks_it_quotes() {
        let out = block("text", "before\n```\nquoted fence\n```\nafter");
        assert!(out.starts_with("````text\n"), "got {out}");
        assert!(out.trim_end().ends_with("````"));
        assert!(out.contains("```\nquoted fence\n```"));
    }

    #[test]
    fn a_relative_path_lands_under_the_workspace() {
        let base = Path::new("/work/project");
        assert_eq!(
            resolve("out/talk.md", Some(base), "t"),
            PathBuf::from("/work/project/out/talk.md")
        );
        assert_eq!(
            resolve("talk", Some(base), "t"),
            PathBuf::from("/work/project/talk")
        );
        // A directory takes the session's own name; so does a trailing
        // separator, which cannot be checked against the filesystem here.
        assert_eq!(
            resolve("out/", Some(base), "Release the hounds"),
            PathBuf::from("/work/project/out/Release-the-hounds.md")
        );
        // An absolute path ignores the base.
        assert_eq!(
            resolve("/tmp/talk.md", Some(base), "t"),
            PathBuf::from("/tmp/talk.md")
        );
    }

    #[test]
    fn a_write_replaces_the_target_and_leaves_no_temporary() {
        let root = std::env::temp_dir().join(format!("bsh-tui-export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let target = root.join("nested").join("talk.md");
        write(&target, "# one\n").expect("the first export");
        assert_eq!(fs::read_to_string(&target).unwrap(), "# one\n");
        write(&target, "# two\n").expect("the replacement");
        assert_eq!(fs::read_to_string(&target).unwrap(), "# two\n");
        assert!(
            !target.with_extension("md.part").exists(),
            "the temporary is renamed away"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
