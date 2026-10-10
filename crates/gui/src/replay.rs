//! Deterministic session replay.
//!
//! A [`ReplayReport`] is a session rendered as an ordered transcript plus the
//! metrics that can be derived from the stored record. Nothing in it is read
//! from the wall clock, the file system, or the process: replaying the same
//! record twice produces byte-identical JSON, which is what makes a report
//! comparable between runs and safe to diff in a test.
//!
//! Every metric comes from data the record already holds — message counts,
//! tool names and arguments, and the per-tool `ms` the streaming layer
//! recorded. Where the record has no timing (older sessions), the metric is
//! simply absent rather than invented.

use serde::Serialize;

use crate::session::{ChatMessage, Role, SessionRecord};

/// One tool call, as it appears in the replayed transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayTool {
    /// Tool name reported by the model.
    pub(crate) name: String,
    /// Serialized arguments as recorded.
    pub(crate) args: String,
    /// Whether the recorded output looks like a failure.
    pub(crate) failed: bool,
    /// Recorded execution time, when the record has one.
    pub(crate) ms: Option<u64>,
}

/// One transcript turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayEntry {
    /// Position in the replayed order (folded turns first, then live ones).
    pub(crate) index: usize,
    /// `"user"` or `"assistant"`.
    pub(crate) role: &'static str,
    /// Visible text, verbatim.
    pub(crate) text: String,
    /// Reasoning trace, verbatim (empty when none).
    pub(crate) reasoning: String,
    /// Tool calls made in this turn, in order.
    pub(crate) tools: Vec<ReplayTool>,
    /// Turn-level error, when the turn failed.
    pub(crate) error: Option<String>,
    /// Whether this turn came from the folded (`archived`) history.
    pub(crate) archived: bool,
}

/// Counts and durations derived from a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayMetrics {
    /// Total turns in the report (folded plus live).
    pub(crate) turns: usize,
    /// User turns.
    pub(crate) user_turns: usize,
    /// Assistant turns.
    pub(crate) assistant_turns: usize,
    /// Turns that came from the folded history.
    pub(crate) archived_turns: usize,
    /// Tool calls across every turn.
    pub(crate) tool_calls: usize,
    /// Tool calls whose recorded output looks like a failure.
    pub(crate) tool_failures: usize,
    /// Sum of the recorded per-tool durations.
    pub(crate) tool_ms_total: u64,
    /// Tool calls that carried no recorded duration.
    pub(crate) tool_ms_missing: usize,
    /// Turns that recorded a turn-level error.
    pub(crate) errors: usize,
    /// Blank turns carried over from streaming placeholders.
    pub(crate) blank_turns: usize,
    /// Estimated tokens of the replayed transcript.
    pub(crate) estimated_tokens: usize,
}

/// A whole session, replayed deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReplayReport {
    /// Session the report replays.
    pub(crate) session_id: String,
    /// Its title, verbatim.
    pub(crate) title: String,
    /// Its workspace, verbatim.
    pub(crate) workspace: String,
    /// Ordered transcript entries.
    pub(crate) entries: Vec<ReplayEntry>,
    /// Counts and durations.
    pub(crate) metrics: ReplayMetrics,
}

/// Whether a tool output reads as a failure.
///
/// The streaming layer records failures as either a tool error raised by the
/// gate (`"denied by user"`, `"approval timed out"`) or a `Failed`-prefixed
/// message from the tool itself. The check is on that recorded text so it stays
/// deterministic and needs no live state.
pub(crate) fn tool_failed(output: &str) -> bool {
    let text = output.trim_start();
    text.starts_with("Failed")
        || text.contains("denied by user")
        || text.contains("approval timed out")
}

/// Render one turn for the report.
fn entry(index: usize, message: &ChatMessage, archived: bool) -> ReplayEntry {
    ReplayEntry {
        index,
        role: match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        },
        text: message.text.clone(),
        reasoning: message.reasoning.clone(),
        tools: message
            .tools
            .iter()
            .map(|tool| ReplayTool {
                name: tool.name.clone(),
                args: tool.args.clone(),
                failed: tool.output.as_deref().is_some_and(tool_failed),
                ms: tool.ms,
            })
            .collect(),
        error: message.error.clone(),
        archived,
    }
}

/// Replay `record` into a deterministic report.
pub(crate) fn replay(record: &SessionRecord) -> ReplayReport {
    // Folded turns are older than what replaced them, so they come first.
    let ordered: Vec<(&ChatMessage, bool)> = record
        .archived
        .iter()
        .map(|message| (message, true))
        .chain(record.messages.iter().map(|message| (message, false)))
        .collect();
    let entries: Vec<ReplayEntry> = ordered
        .iter()
        .enumerate()
        .map(|(index, (message, archived))| entry(index, message, *archived))
        .collect();

    let mut metrics = ReplayMetrics {
        turns: entries.len(),
        user_turns: 0,
        assistant_turns: 0,
        archived_turns: record.archived.len(),
        tool_calls: 0,
        tool_failures: 0,
        tool_ms_total: 0,
        tool_ms_missing: 0,
        errors: 0,
        blank_turns: 0,
        estimated_tokens: 0,
    };
    for (message, _) in &ordered {
        match message.role {
            Role::User => metrics.user_turns += 1,
            Role::Assistant => metrics.assistant_turns += 1,
        }
        if message.is_blank() {
            metrics.blank_turns += 1;
        }
        if message.error.is_some() {
            metrics.errors += 1;
        }
        for tool in &message.tools {
            metrics.tool_calls += 1;
            match tool.ms {
                Some(ms) => metrics.tool_ms_total += ms,
                None => metrics.tool_ms_missing += 1,
            }
            if tool.output.as_deref().is_some_and(tool_failed) {
                metrics.tool_failures += 1;
            }
        }
        metrics.estimated_tokens += crate::context::message_tokens(message);
    }

    ReplayReport {
        session_id: record.id.clone(),
        title: record.title.clone(),
        workspace: record.workspace.clone(),
        entries,
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionStore, ToolEvent};

    /// A fixture session exercising every branch the report renders.
    fn fixture() -> SessionRecord {
        let mut record = SessionRecord::new();
        record.title = "replay fixture".to_string();
        record.workspace = "/work".to_string();
        record
            .archived
            .push(ChatMessage::user("an older folded ask"));
        let mut folded = ChatMessage::assistant();
        folded.text = "an older folded answer".to_string();
        record.archived.push(folded);
        record.messages.push(ChatMessage::user("first live ask"));
        let mut good = ChatMessage::assistant();
        good.text = "ran the tool".to_string();
        good.reasoning = "because it seemed right".to_string();
        good.tools.push(ToolEvent {
            name: "read".into(),
            args: "{\"path\":\"a.txt\"}".into(),
            output: Some("file contents".into()),
            ms: Some(5),
        });
        record.messages.push(good);
        let mut bad = ChatMessage::assistant();
        bad.text = "tried and failed".to_string();
        bad.tools.push(ToolEvent {
            name: "bash".into(),
            args: "{\"command\":\"false\"}".into(),
            output: Some("Failed: exit status 1".into()),
            ms: Some(7),
        });
        bad.tools.push(ToolEvent {
            name: "write_file".into(),
            args: "{\"path\":\"b.txt\"}".into(),
            output: None,
            ms: None,
        });
        bad.error = Some("the write was refused".into());
        record.messages.push(bad);
        record
    }

    /// Every count and duration is derived from the record.
    #[test]
    fn metrics_count_turns_tools_and_durations() {
        let record = fixture();
        let report = replay(&record);
        assert_eq!(report.session_id, record.id);
        assert_eq!(report.title, "replay fixture");
        assert_eq!(report.workspace, "/work");
        assert_eq!(report.entries.len(), 5, "two folded plus three live");
        assert_eq!(report.entries[0].role, "user");
        assert!(report.entries[0].archived);
        assert!(!report.entries[2].archived, "live turns are not archived");

        let metrics = &report.metrics;
        assert_eq!(metrics.turns, 5);
        assert_eq!(metrics.archived_turns, 2);
        assert_eq!(metrics.user_turns, 2);
        assert_eq!(metrics.assistant_turns, 3);
        assert_eq!(metrics.tool_calls, 3);
        assert_eq!(metrics.tool_failures, 1, "only the failed bash call");
        assert_eq!(metrics.tool_ms_total, 12);
        assert_eq!(metrics.tool_ms_missing, 1, "the un-timed call is counted");
        assert_eq!(metrics.errors, 1);
        assert_eq!(metrics.blank_turns, 0);
        assert!(metrics.estimated_tokens > 0);

        // Per-tool detail is preserved verbatim.
        let good = &report.entries[3];
        assert_eq!(good.reasoning, "because it seemed right");
        assert_eq!(good.tools[0].name, "read");
        assert!(!good.tools[0].failed);
        assert!(report.entries[4].tools[0].failed);
    }

    /// Replaying the same session twice yields byte-identical JSON.
    #[test]
    fn replay_is_byte_identical_across_runs() {
        let record = fixture();
        let first = serde_json::to_string(&replay(&record)).expect("json");
        let second = serde_json::to_string(&replay(&record)).expect("json");
        assert_eq!(first, second, "no wall-clock value may leak in");
        // Nothing that looks like a timestamp field is present.
        assert!(!first.contains("created_at"));
        assert!(!first.contains("updated_at"));
        assert!(!first.contains("started_at"));
        assert!(!first.contains("generated"));
    }

    /// The same session, read back from the store, replays identically.
    #[test]
    fn replay_round_trips_through_a_written_fixture() {
        let dir = std::env::temp_dir().join(format!("bos-gui-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SessionStore::new(&dir);
        let record = fixture();
        store.save(&record).expect("save fixture");

        let loaded = store.load(&record.id).expect("load fixture");
        let first = serde_json::to_string(&replay(&loaded)).expect("json");
        let second = serde_json::to_string(&replay(&loaded)).expect("json");
        assert_eq!(first, second);
        assert_eq!(replay(&loaded), replay(&record), "load is lossless");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty session replays to an empty, well-formed report.
    #[test]
    fn an_empty_session_replays_cleanly() {
        let report = replay(&SessionRecord::new());
        assert!(report.entries.is_empty());
        assert_eq!(report.metrics.turns, 0);
        assert_eq!(report.metrics.tool_calls, 0);
        assert_eq!(report.metrics.tool_ms_total, 0);
        assert_eq!(report.metrics.estimated_tokens, 0);
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["entries"].as_array().unwrap().is_empty());
        assert_eq!(json["metrics"]["turns"], 0);
    }

    /// The failure classifier only fires on recorded failure text.
    #[test]
    fn the_failure_classifier_matches_recorded_failures() {
        assert!(tool_failed("Failed: exit status 1"));
        assert!(tool_failed("bash: denied by user"));
        assert!(tool_failed("read: approval timed out"));
        assert!(!tool_failed("file contents"));
        assert!(!tool_failed(""));
        assert!(!tool_failed("it failed gracefully in prose"));
    }
}
