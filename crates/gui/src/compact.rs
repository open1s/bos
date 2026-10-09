//! Manual `/compact`: fold a long conversation into an LLM-written summary.
//!
//! The send-side [`ContextBudget`](agent::ContextBudget) trims mechanically
//! on every turn while the full transcript is kept in the UI. `/compact` is
//! the deliberate, *persistent* counterpart (codex parity): it asks the model
//! to summarize the stored history and rewrites the session to that summary
//! anchored on the original opening request, so future turns genuinely start
//! small. The working plan and session title survive the rewrite.

use std::sync::Arc;
use std::time::Duration;

use crate::approval::ApprovalBroker;
use crate::caps;
use crate::runner::Runner;
use crate::session::{ChatMessage, Role, SessionRecord};
use crate::settings::Settings;

/// Per-message character cap fed to the summarizer (long pastes truncate).
const MAX_MESSAGE_CHARS: usize = 4_096;
/// Total transcript character cap fed to the summarizer.
const MAX_TRANSCRIPT_CHARS: usize = 96 * 1024;
/// Deadline for the summarization turn.
const SUMMARIZE_TIMEOUT: Duration = Duration::from_secs(120);
/// Non-blank messages required before `/compact` bothers running.
pub(crate) const MIN_MESSAGES_TO_COMPACT: usize = 4;

/// System prompt for the summarization turn (tools, skills, memory, and
/// project instructions are all disabled for this probe agent).
pub(crate) const SUMMARIZER_SYSTEM: &str = "\
You compress chat histories. You are given a user/assistant conversation; \
reply with one dense markdown summary of it and nothing else — no preamble, \
no closing remarks, no tool calls. Preserve the user's goals, decisions and \
constraints; file paths, identifiers, commands and numbers exactly; what was \
already accomplished; and every unfinished item with its next step, so the \
conversation can continue from the summary alone. Prefer bullet points. Stay \
well under 400 lines.";

/// Why `/compact` should not run for this history, or `None` when it may.
pub(crate) fn refusal(messages: &[ChatMessage]) -> Option<String> {
    let live = messages.iter().filter(|m| !m.is_blank()).count();
    if live < MIN_MESSAGES_TO_COMPACT {
        return Some(format!(
            "nothing to compact yet ({live} message{}; \
             at least {MIN_MESSAGES_TO_COMPACT} needed)",
            if live == 1 { "" } else { "s" }
        ));
    }
    None
}

/// Render `messages` as a role-labelled transcript for the summarizer.
///
/// Blank placeholders and reasoning traces are skipped; each message is
/// capped at [`MAX_MESSAGE_CHARS`] and the whole rendering at
/// [`MAX_TRANSCRIPT_CHARS`], with an explicit truncation marker either way.
pub(crate) fn transcript(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        if message.is_blank() {
            continue;
        }
        let label = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        let raw = if message.text.is_empty() && !message.reasoning.is_empty() {
            message.reasoning.as_str()
        } else {
            message.text.as_str()
        };
        let body = if raw.chars().count() > MAX_MESSAGE_CHARS {
            let mut head: String = raw.chars().take(MAX_MESSAGE_CHARS).collect();
            head.push_str("… [truncated]");
            head
        } else {
            raw.to_string()
        };
        let entry = format!("{label}: {body}\n\n");
        if out.chars().count() + entry.chars().count() > MAX_TRANSCRIPT_CHARS {
            out.push_str("… [transcript truncated]\n");
            break;
        }
        out.push_str(&entry);
    }
    out
}

/// The summarization user message: rules plus the rendered transcript.
pub(crate) fn prompt(messages: &[ChatMessage]) -> String {
    format!(
        "Compact this conversation into a single summary:\n\n{}",
        transcript(messages)
    )
}

/// Replace `record`'s history with the original opening request plus the
/// summary message, returning `(before, after)` message counts.
///
/// The summary is embedded as one assistant message so the next turn seeds
/// both the anchor question and the compacted context; an empty summary is
/// refused so a flaky provider can never destroy the history.
pub(crate) fn apply(record: &mut SessionRecord, summary: String) -> Result<(usize, usize), String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("the summarizer returned no text — history kept".to_string());
    }
    let anchor = record
        .messages
        .iter()
        .find(|m| m.role == Role::User && !m.text.trim().is_empty())
        .map(|m| m.text.clone())
        .ok_or_else(|| "no user message to anchor the compacted history".to_string())?;
    let before = record.messages.len();
    let mut summary_msg = ChatMessage::assistant();
    summary_msg.text = format!("**Compacted** — earlier turns folded by `/compact`:\n\n{summary}");
    record.messages = vec![ChatMessage::user(anchor), summary_msg];
    Ok((before, record.messages.len()))
}

/// Run one tool-less summarization turn over `messages` and return the
/// summary text.
///
/// The probe agent reuses the endpoint/model from `settings` but registers
/// no bash, file, memory, MCP, or skill tools and no project instructions —
/// compaction must never side-effect the workspace.
pub(crate) fn summarize(settings: &Settings, messages: &[ChatMessage]) -> Result<String, String> {
    let mut probe = settings.clone();
    probe.system_prompt = SUMMARIZER_SYSTEM.to_string();
    probe.project_instructions = false;
    probe.bash_enabled = false;
    probe.file_tools_enabled = false;
    probe.memory_enabled = false;
    probe.skills_dir.clear();
    probe.mcp_servers.clear();
    probe.require_approval = false;

    let agent = caps::build_agent(&probe, Arc::new(ApprovalBroker::default()));
    let mut runner = Runner::new();
    let rx = runner.spawn("__compact__", agent, prompt(messages), Vec::new(), 0);
    let text = runner.collect(&rx, SUMMARIZE_TIMEOUT)?;
    let summary = text.trim();
    if summary.is_empty() {
        return Err("the summarizer returned no text".to_string());
    }
    Ok(summary.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    fn assistant(text: &str) -> ChatMessage {
        let mut msg = ChatMessage::assistant();
        msg.text = text.to_string();
        msg
    }

    #[test]
    fn refusal_counts_only_live_messages() {
        let few: Vec<ChatMessage> = vec![user("a"), assistant("b")];
        let reason = refusal(&few).expect("two messages must refuse");
        assert!(reason.contains("2 message"), "{reason}");

        let blanks: Vec<ChatMessage> = vec![
            user("a"),
            assistant("b"),
            user("c"),
            ChatMessage::assistant(),
        ];
        assert!(
            refusal(&blanks).is_some(),
            "blank placeholder must not count"
        );

        let enough: Vec<ChatMessage> = vec![user("a"), assistant("b"), user("c"), assistant("d")];
        assert!(refusal(&enough).is_none());
    }

    #[test]
    fn transcript_labels_roles_and_caps_long_messages() {
        let long: String = "x".repeat(MAX_MESSAGE_CHARS + 50);
        let text = transcript(&[user(&long), assistant("hi")]);
        assert!(text.starts_with("User: xxxx"), "got {text:?}");
        assert!(text.contains("… [truncated]"));
        assert!(text.contains("Assistant: hi"));
    }

    #[test]
    fn transcript_stops_at_total_cap() {
        let bulk: String = "y".repeat(16 * 1024);
        let messages: Vec<ChatMessage> = (0..40).map(|i| user(&format!("{bulk} {i}"))).collect();
        let text = transcript(&messages);
        assert!(
            text.chars().count() <= MAX_TRANSCRIPT_CHARS + 64,
            "rendering must stay bounded ({} chars)",
            text.chars().count()
        );
        assert!(text.contains("[transcript truncated]"));
    }

    #[test]
    fn prompt_embeds_instruction_and_transcript() {
        let text = prompt(&[user("fix the bug"), assistant("done")]);
        assert!(text.starts_with("Compact this conversation"));
        assert!(text.contains("User: fix the bug"));
        assert!(text.contains("Assistant: done"));
    }

    #[test]
    fn apply_replaces_history_with_anchor_and_summary() {
        let mut record = SessionRecord::new();
        record.messages = vec![
            user("the original task"),
            assistant("step one"),
            user("step two"),
            assistant("step three"),
        ];
        let (before, after) =
            apply(&mut record, "  condensed summary  ".to_string()).expect("apply succeeds");
        assert_eq!((before, after), (4, 2));
        assert_eq!(record.messages[0].text, "the original task");
        assert_eq!(record.messages[0].role, Role::User);
        assert_eq!(record.messages[1].role, Role::Assistant);
        assert!(
            record.messages[1].text.contains("condensed summary"),
            "summary text kept"
        );
        assert!(
            record.messages[1].text.contains("/compact"),
            "summary carries its marker"
        );
    }

    #[test]
    fn apply_refuses_empty_summary() {
        let mut record = SessionRecord::new();
        record.messages = vec![user("a"), assistant("b"), user("c"), assistant("d")];
        let err = apply(&mut record, "   \n".to_string()).expect_err("empty refused");
        assert!(err.contains("history kept"), "{err}");
        assert_eq!(record.messages.len(), 4, "history untouched");
    }

    #[test]
    fn apply_requires_a_user_anchor() {
        let mut record = SessionRecord::new();
        record.messages = vec![
            assistant("orphan 1"),
            assistant("orphan 2"),
            user("   "),
            assistant("orphan 3"),
        ];
        let err = apply(&mut record, "s".to_string()).expect_err("no anchor");
        assert!(err.contains("anchor"), "{err}");
    }
}
