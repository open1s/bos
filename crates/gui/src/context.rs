//! Context accounting: token metering, tool-output spill, and compaction state.
//!
//! Three related controls live here, all read-only except for the spill files:
//!
//! * a **token meter** that estimates what a session's messages cost and
//!   reconciles that estimate against a provider's reported usage;
//! * **retention/spill**, which moves an oversized tool output out of the
//!   transcript and into `<store>/spill/<id>.txt`, where the UI can list and
//!   page through it without holding the whole thing in memory;
//! * **compaction status** over the existing [`crate::compact`] rules, so the
//!   UI can show what `/compact` would do before asking it to run.
//!
//! The estimates deliberately reuse [`agent::estimate_tokens`] so the number
//! the UI shows is the same heuristic the send-side budget trims against.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::compact;
use crate::session::{ChatMessage, Role, SessionRecord, ToolEvent};

/// Tool output at or above this size is a candidate for spilling to disk.
pub(crate) const SPILL_THRESHOLD_BYTES: usize = 16 * 1024;

/// Hard ceiling on one spill file; a larger output is truncated on the way in.
pub(crate) const MAX_SPILL_BYTES: usize = 4 * 1024 * 1024;

/// How much of a spill file `spill_read` returns by default.
pub(crate) const DEFAULT_SPILL_READ_BYTES: usize = 64 * 1024;

/// Hard ceiling on how much of a spill file a single read returns.
pub(crate) const MAX_SPILL_READ_BYTES: usize = 1024 * 1024;

/// Relative error allowed before an estimate is called out of tolerance.
const RECONCILE_TOLERANCE_DIVISOR: usize = 10;

/// Smallest absolute error tolerated, so tiny prompts are not "off by 1".
const RECONCILE_MIN_TOLERANCE: usize = 8;

/// Estimated token cost of one message.
///
/// The visible text, the reasoning trace, every tool call (name, arguments,
/// and any captured output), and the error are all part of what a provider
/// bills for; the constant covers role framing and message delimiters.
pub(crate) fn message_tokens(message: &ChatMessage) -> usize {
    let mut total = agent::estimate_tokens(&message.text)
        + agent::estimate_tokens(&message.reasoning)
        + if message.is_blank() { 0 } else { 4 };
    for tool in &message.tools {
        total += agent::estimate_tokens(&tool.name) + agent::estimate_tokens(&tool.args);
        if let Some(output) = &tool.output {
            total += agent::estimate_tokens(output);
        }
    }
    if let Some(error) = &message.error {
        total += agent::estimate_tokens(error);
    }
    total
}

/// Estimated token cost of a whole message list.
pub(crate) fn estimate_messages(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_tokens).sum()
}

/// Token cost of one message, as reported to the UI.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessageTokens {
    /// Index of the message in the live transcript.
    pub(crate) index: usize,
    /// `"user"` or `"assistant"`.
    pub(crate) role: &'static str,
    /// Estimated tokens this message contributes.
    pub(crate) tokens: usize,
}

/// The meter's answer for one session.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TokenMeterReport {
    /// Session the report describes.
    pub(crate) session_id: String,
    /// Live messages counted.
    pub(crate) messages: usize,
    /// Estimated tokens across the live transcript.
    pub(crate) estimated: usize,
    /// Estimated tokens across the folded (`archived`) turns, kept apart.
    pub(crate) archived_estimated: usize,
    /// Provider-reported prompt tokens, when the caller has them.
    pub(crate) provider: Option<usize>,
    /// `provider - estimated` (positive means the estimate ran low).
    pub(crate) delta: Option<i64>,
    /// Whether the delta is inside the reconciliation tolerance.
    pub(crate) within_tolerance: Option<bool>,
    /// Per-message breakdown, oldest first.
    pub(crate) per_message: Vec<MessageTokens>,
}

/// Reconcile an estimate against a provider's reported usage.
///
/// Tolerance grows with the estimate (10%) and never drops below a small
/// absolute floor, because a one-token estimate cannot be 10% accurate.
pub(crate) fn reconcile(estimated: usize, provider: usize) -> (i64, bool) {
    let delta = provider as i64 - estimated as i64;
    let tolerance = (estimated / RECONCILE_TOLERANCE_DIVISOR).max(RECONCILE_MIN_TOLERANCE) as i64;
    (delta, delta.abs() <= tolerance)
}

/// Token-cost names for a role.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Build the meter's report for one session.
pub(crate) fn meter(record: &SessionRecord, provider: Option<usize>) -> TokenMeterReport {
    let per_message: Vec<MessageTokens> = record
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| MessageTokens {
            index,
            role: role_name(message.role),
            tokens: message_tokens(message),
        })
        .collect();
    let estimated: usize = per_message.iter().map(|m| m.tokens).sum();
    let (delta, within_tolerance) = match provider {
        Some(provider) => {
            let (delta, ok) = reconcile(estimated, provider);
            (Some(delta), Some(ok))
        }
        None => (None, None),
    };
    TokenMeterReport {
        session_id: record.id.clone(),
        messages: record.messages.len(),
        estimated,
        archived_estimated: estimate_messages(&record.archived),
        provider,
        delta,
        within_tolerance,
        per_message,
    }
}

/// Clip `text` to at most `max` bytes, never splitting a character.
///
/// Returns the clipped text and whether anything was dropped.
pub(crate) fn clip_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_string(), true)
}

/// The directory spill files live in, beside the session files.
///
/// It is a subdirectory rather than a sibling so it travels with the store,
/// and it holds only `.txt` files, so the session loader never sees it.
pub(crate) fn spill_dir(store_dir: &Path) -> PathBuf {
    store_dir.join("spill")
}

/// Validate a spill id before it becomes a filename.
fn safe_id(id: &str) -> Result<&str, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("a spill id must not be empty".to_string());
    }
    if id == "." || id == ".." || id.contains("..") {
        return Err(format!("'{id}' is not a valid spill id"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!("'{id}' is not a valid spill id"));
    }
    Ok(id)
}

/// Outcome of one spill write.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpillWrite {
    /// Id the file is filed under.
    pub(crate) id: String,
    /// Absolute path written.
    pub(crate) file: String,
    /// Bytes actually written.
    pub(crate) bytes: usize,
    /// Size of the content before the cap was applied.
    pub(crate) original_bytes: usize,
    /// Whether the content was cut at [`MAX_SPILL_BYTES`].
    pub(crate) truncated: bool,
}

/// Write one tool output to the spill file for `id`.
pub(crate) fn spill_write(store_dir: &Path, id: &str, content: &str) -> Result<SpillWrite, String> {
    let id = safe_id(id)?;
    let dir = spill_dir(store_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let (body, truncated) = clip_bytes(content, MAX_SPILL_BYTES);
    let path = dir.join(format!("{id}.txt"));
    std::fs::write(&path, body.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(SpillWrite {
        id: id.to_string(),
        file: path.to_string_lossy().into_owned(),
        bytes: body.len(),
        original_bytes: content.len(),
        truncated,
    })
}

/// Clean a tool name into something safe to use as a filename.
fn sanitize_id(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "tool".to_string()
    } else {
        cleaned
    }
}

/// A short stable hash of `text`, used to keep spill ids from colliding across
/// turns that repeat the same tool name.
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash >> 32) as u32)
}

/// The marker left in a transcript where an oversized output was spilled.
pub(crate) fn spill_marker(id: &str, bytes: usize) -> String {
    format!("[spilled to spill/{id}.txt — {bytes} bytes]")
}

/// Move every oversized tool output in `tools` to the spill directory.
///
/// Returns how many outputs were moved. A moved output is replaced by a short
/// marker naming its file, so the persisted transcript stays small while the
/// full text stays retrievable through [`spill_read`]. A write that fails is
/// skipped rather than aborting the turn: losing the spill is better than
/// losing the reply.
pub(crate) fn spill_oversized_tools(store_dir: &Path, tools: &mut [ToolEvent]) -> usize {
    let mut moved = 0;
    for (index, tool) in tools.iter_mut().enumerate() {
        let Some(output) = tool.output.as_deref() else {
            continue;
        };
        if output.len() < SPILL_THRESHOLD_BYTES {
            continue;
        }
        let id = format!("{index}-{}-{}", sanitize_id(&tool.name), short_hash(output));
        let Ok(written) = spill_write(store_dir, &id, output) else {
            continue;
        };
        tool.output = Some(spill_marker(&written.id, written.bytes));
        moved += 1;
    }
    moved
}

/// One spilled output as the list reports it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpillEntry {
    /// Id the file is filed under.
    pub(crate) id: String,
    /// Absolute path of the file.
    pub(crate) file: String,
    /// Size on disk in bytes.
    pub(crate) bytes: usize,
    /// First line of the file, clipped, so the list is readable.
    pub(crate) preview: String,
}

/// Every spilled output, sorted by id so the list is stable.
pub(crate) fn spill_list(store_dir: &Path) -> Vec<SpillEntry> {
    let dir = spill_dir(store_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let bytes = entry.metadata().map(|m| m.len() as usize).unwrap_or(0);
        let preview = std::fs::read(&path)
            .ok()
            .map(|raw| {
                let text = String::from_utf8_lossy(&raw);
                let first = text.lines().next().unwrap_or("");
                clip_bytes(first, 200).0
            })
            .unwrap_or_default();
        out.push(SpillEntry {
            id: stem.to_string(),
            file: path.to_string_lossy().into_owned(),
            bytes,
            preview,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// One bounded read of a spill file.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SpillRead {
    /// Id that was read.
    pub(crate) id: String,
    /// Absolute path of the file.
    pub(crate) file: String,
    /// Total size of the file in bytes.
    pub(crate) bytes: usize,
    /// Whether the returned text is shorter than the file.
    pub(crate) truncated: bool,
    /// The text itself, at most the requested cap.
    pub(crate) text: String,
}

/// Read up to `max_bytes` of a spill file, keeping the head.
pub(crate) fn spill_read(
    store_dir: &Path,
    id: &str,
    max_bytes: Option<usize>,
) -> Result<SpillRead, String> {
    let id = safe_id(id)?;
    let path = spill_dir(store_dir).join(format!("{id}.txt"));
    let Ok(raw) = std::fs::read(&path) else {
        return Err(format!("no spilled output named '{id}'"));
    };
    let cap = max_bytes
        .unwrap_or(DEFAULT_SPILL_READ_BYTES)
        .min(MAX_SPILL_READ_BYTES);
    let text = String::from_utf8_lossy(&raw);
    let (text, truncated) = clip_bytes(&text, cap);
    Ok(SpillRead {
        id: id.to_string(),
        file: path.to_string_lossy().into_owned(),
        bytes: raw.len(),
        truncated,
        text,
    })
}

/// What `/compact` would do for one session right now.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CompactionStatus {
    /// Session the status describes.
    pub(crate) session_id: String,
    /// Session title, so the UI does not need a second lookup.
    pub(crate) title: String,
    /// Total live messages.
    pub(crate) messages: usize,
    /// Non-blank live messages (what `/compact` counts).
    pub(crate) live_messages: usize,
    /// Folded turns kept from an earlier compaction.
    pub(crate) archived_messages: usize,
    /// Estimated tokens of the live transcript.
    pub(crate) estimated_tokens: usize,
    /// The configured send-side token ceiling (0 = trimming disabled).
    pub(crate) budget_tokens: usize,
    /// Whether the live transcript is over that ceiling.
    pub(crate) over_budget: bool,
    /// Whether `/compact` would run (enough history and not already running).
    pub(crate) can_compact: bool,
    /// Whether a compaction is running for this session right now.
    pub(crate) compacting: bool,
    /// Why `/compact` would refuse, when it would.
    pub(crate) refusal: Option<String>,
}

/// Build the compaction status for one session.
pub(crate) fn compaction_status(
    record: &SessionRecord,
    budget_tokens: usize,
    compacting: bool,
) -> CompactionStatus {
    let estimated = estimate_messages(&record.messages);
    let refusal = compact::refusal(&record.messages);
    CompactionStatus {
        session_id: record.id.clone(),
        title: record.title.clone(),
        messages: record.messages.len(),
        live_messages: record.messages.iter().filter(|m| !m.is_blank()).count(),
        archived_messages: record.archived.len(),
        estimated_tokens: estimated,
        budget_tokens,
        over_budget: budget_tokens > 0 && estimated > budget_tokens,
        can_compact: refusal.is_none() && !compacting,
        compacting,
        refusal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ToolEvent;

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text)
    }

    fn assistant(text: &str) -> ChatMessage {
        let mut message = ChatMessage::assistant();
        message.text = text.to_string();
        message
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bos-gui-context-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// More messages never cost fewer tokens.
    #[test]
    fn the_estimate_grows_with_the_transcript() {
        let mut record = SessionRecord::new();
        let mut previous = estimate_messages(&record.messages);
        assert_eq!(previous, 0, "an empty transcript costs nothing");
        for index in 0..25 {
            record
                .messages
                .push(assistant(&format!("reply number {index} with some words")));
            let now = estimate_messages(&record.messages);
            assert!(
                now > previous,
                "appending message {index} must raise the estimate ({previous} -> {now})"
            );
            previous = now;
        }
        // Blank messages still cost their framing, so the total stays monotone.
        record.messages.push(ChatMessage::assistant());
        assert!(estimate_messages(&record.messages) >= previous);
        // A longer message costs more than a shorter one.
        assert!(
            message_tokens(&user("a much longer message indeed")) > message_tokens(&user("hi"))
        );
    }

    /// CJK is not free: the estimator charges them closer to one token each.
    #[test]
    fn non_ascii_text_is_not_underestimated() {
        let cjk = "这是一段中文文本，用来测试估算".repeat(10);
        let estimate = message_tokens(&user(&cjk));
        assert!(
            estimate >= cjk.chars().count(),
            "each non-ASCII character costs at least one token ({estimate})"
        );
    }

    /// Tool output and errors count toward the message they belong to.
    #[test]
    fn tools_and_errors_are_counted() {
        let bare = assistant("done");
        let mut with_tool = assistant("done");
        with_tool.tools.push(ToolEvent {
            name: "read".into(),
            args: "{\"path\":\"a very long path indeed\"}".into(),
            output: Some("x".repeat(400)),
            ms: Some(3),
        });
        assert!(message_tokens(&with_tool) > message_tokens(&bare));
        let mut failed = assistant("done");
        failed.error = Some("the tool exploded loudly".to_string());
        assert!(message_tokens(&failed) > message_tokens(&bare));
    }

    /// Reconciliation reports both numbers and the delta, within tolerance.
    #[test]
    fn reconciliation_reports_both_numbers_and_a_tolerance() {
        let (delta, ok) = reconcile(1_000, 1_050);
        assert_eq!(delta, 50);
        assert!(ok, "5% is inside the 10% tolerance");
        let (delta, ok) = reconcile(1_000, 1_400);
        assert_eq!(delta, 400);
        assert!(!ok, "40% is outside the tolerance");
        let (delta, ok) = reconcile(1_000, 900);
        assert_eq!(delta, -100);
        assert!(ok, "the tolerance is symmetric");
        // The absolute floor protects tiny prompts.
        let (delta, ok) = reconcile(10, 16);
        assert_eq!(delta, 6);
        assert!(ok, "6 tokens is inside the minimum tolerance");
    }

    /// The meter reports the estimate, the provider number, and the delta.
    #[test]
    fn the_meter_combines_estimate_and_provider_usage() {
        let mut record = SessionRecord::new();
        record
            .messages
            .push(user("hello there, please explain the design"));
        record.messages.push(assistant("sure, here is the design"));
        let without = meter(&record, None);
        assert!(without.estimated > 0);
        assert_eq!(without.provider, None);
        assert_eq!(without.delta, None);
        assert_eq!(without.within_tolerance, None);
        assert_eq!(without.per_message.len(), 2);
        assert_eq!(without.per_message[0].role, "user");
        assert_eq!(without.per_message[1].role, "assistant");

        let provider = without.estimated + 3;
        let with_provider = meter(&record, Some(provider));
        assert_eq!(with_provider.provider, Some(provider));
        assert_eq!(with_provider.delta, Some(3));
        assert_eq!(with_provider.within_tolerance, Some(true));
        // Folded turns are reported separately, never folded into the estimate.
        let mut archived = record.clone();
        archived.archived.push(assistant("an old folded turn"));
        let report = meter(&archived, None);
        assert_eq!(report.estimated, without.estimated);
        assert!(report.archived_estimated > 0);
    }

    /// Spill write/list/read round-trips the content.
    #[test]
    fn spill_writes_lists_and_reads_back() {
        let dir = scratch("spill");
        let content = "line one\nline two\nline three\n";
        let written = spill_write(&dir, "call-7", content).expect("write");
        assert_eq!(written.id, "call-7");
        assert_eq!(written.bytes, content.len());
        assert_eq!(written.original_bytes, content.len());
        assert!(!written.truncated);
        assert!(Path::new(&written.file).starts_with(spill_dir(&dir)));

        let listed = spill_list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "call-7");
        assert_eq!(listed[0].bytes, content.len());
        assert_eq!(listed[0].preview, "line one");

        let read = spill_read(&dir, "call-7", None).expect("read");
        assert_eq!(read.text, content);
        assert_eq!(read.bytes, content.len());
        assert!(!read.truncated);

        // A bounded read keeps the head and says it cut.
        let bounded = spill_read(&dir, "call-7", Some(8)).expect("bounded read");
        assert_eq!(bounded.text, "line one");
        assert!(bounded.truncated);
        assert_eq!(bounded.bytes, content.len());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An oversized output is truncated on the way in, and the write says so.
    #[test]
    fn oversize_spill_reports_truncation() {
        let dir = scratch("oversize");
        let huge = "y".repeat(MAX_SPILL_BYTES + 1_000);
        let written = spill_write(&dir, "big", &huge).expect("write");
        assert!(written.truncated, "the cap must be reported");
        assert_eq!(written.bytes, MAX_SPILL_BYTES);
        assert_eq!(written.original_bytes, huge.len());
        // Reading it back is still bounded by the read cap.
        let read = spill_read(&dir, "big", Some(MAX_SPILL_READ_BYTES * 4)).expect("read");
        assert!(read.truncated);
        assert_eq!(read.text.len(), MAX_SPILL_READ_BYTES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Oversized tool outputs move to disk and leave a marker behind.
    #[test]
    fn oversized_tool_outputs_are_spilled_out_of_the_transcript() {
        let dir = scratch("spill-tools");
        let big = "z".repeat(SPILL_THRESHOLD_BYTES + 5);
        let mut tools = vec![
            ToolEvent {
                name: "read_file".into(),
                args: "{}".into(),
                output: Some("small output".into()),
                ms: Some(1),
            },
            ToolEvent {
                name: "bash/cat".into(),
                args: "{}".into(),
                output: Some(big.clone()),
                ms: Some(9),
            },
        ];
        assert_eq!(spill_oversized_tools(&dir, &mut tools), 1, "one moved");
        assert_eq!(tools[0].output.as_deref(), Some("small output"));
        let marker = tools[1].output.as_deref().expect("marker");
        assert!(marker.starts_with("[spilled to spill/"), "{marker}");
        assert!(marker.contains(&big.len().to_string()));

        // The full text is retrievable, and the id is filename-safe.
        let listed = spill_list(&dir);
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].id.contains('/'), "{}", listed[0].id);
        let read = spill_read(&dir, &listed[0].id, Some(MAX_SPILL_READ_BYTES)).expect("read");
        assert_eq!(read.text, big);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unusable ids and unknown names fail cleanly rather than escaping.
    #[test]
    fn spill_ids_are_validated_and_missing_files_are_errors() {
        let dir = scratch("ids");
        for bad in ["", "  ", "..", "../escape", "a/b", "a\\b", "nul\0"] {
            assert!(
                spill_write(&dir, bad, "x").is_err(),
                "{bad:?} must be refused"
            );
            assert!(
                spill_read(&dir, bad, None).is_err(),
                "{bad:?} must be refused"
            );
        }
        let err = spill_read(&dir, "absent", None).expect_err("missing file");
        assert!(err.contains("absent"), "{err}");
        assert!(spill_list(&dir).is_empty(), "nothing was written");
        assert!(
            !spill_dir(&dir).join("../escape.txt").exists(),
            "no path escaped the spill directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The compaction status reports the shape the UI needs, and flips flags.
    #[test]
    fn compaction_status_has_the_expected_shape() {
        let mut record = SessionRecord::new();
        record.title = "compaction probe".to_string();
        record.messages = vec![user("a"), assistant("b"), user("c"), assistant("d")];
        let status = compaction_status(&record, 32_768, false);
        assert_eq!(status.session_id, record.id);
        assert_eq!(status.title, "compaction probe");
        assert_eq!(status.messages, 4);
        assert_eq!(status.live_messages, 4);
        assert_eq!(status.archived_messages, 0);
        assert!(status.estimated_tokens > 0);
        assert_eq!(status.budget_tokens, 32_768);
        assert!(!status.over_budget, "four short turns fit the budget");
        assert!(status.can_compact, "four live messages is enough");
        assert!(!status.compacting);
        assert!(status.refusal.is_none());

        // Short history refuses, and running marks it busy.
        let mut short = SessionRecord::new();
        short.messages = vec![user("only one live turn")];
        let status = compaction_status(&short, 32_768, false);
        assert!(!status.can_compact);
        assert!(status
            .refusal
            .as_deref()
            .unwrap()
            .contains("nothing to compact"));

        let busy = compaction_status(&record, 32_768, true);
        assert!(busy.compacting);
        assert!(!busy.can_compact, "already running cannot start again");

        // A tiny budget reports over_budget; zero disables the check.
        let over = compaction_status(&record, 1, false);
        assert!(over.over_budget);
        let off = compaction_status(&record, 0, false);
        assert!(
            !off.over_budget,
            "zero means trimming is off, not over budget"
        );
    }

    /// The status is serializable in the flat shape the frontend reads.
    #[test]
    fn compaction_status_serializes_flat() {
        let record = SessionRecord::new();
        let json = serde_json::to_value(compaction_status(&record, 100, false)).unwrap();
        for key in [
            "session_id",
            "title",
            "messages",
            "live_messages",
            "archived_messages",
            "estimated_tokens",
            "budget_tokens",
            "over_budget",
            "can_compact",
            "compacting",
        ] {
            assert!(json.get(key).is_some(), "missing {key} in {json}");
        }
        assert!(
            json.get("refusal").is_some(),
            "the refusal key is always present"
        );
    }
}
