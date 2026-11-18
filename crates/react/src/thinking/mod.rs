//! Thinking summaries: extracting durable decisions from a reasoning stream
//! and injecting a compact note into later provider calls.
//!
//! The raw reasoning stream is private model output: it is shown to the reader,
//! but it is not conversation history and must not be replayed as one. This
//! module keeps a small, typed summary of what the reasoning established -
//! decisions and their rationale, unresolved questions, and the next action -
//! and hands that summary to the engine as host guidance. The summary is
//! derived from the model's own reasoning plus observed tool results, never
//! from a synthetic user or assistant turn, so it cannot masquerade as
//! something the reader said.
//!
//! Three rules keep the note honest:
//!
//! - A decision extracted from reasoning is always `inferred` until an observed
//!   tool result corroborates it. Nothing the model merely thought is rendered
//!   as a fact.
//! - Superseded or completed state is never injected: the injection path sees
//!   only active decisions, unresolved questions and a live next action.
//! - The note is not stored in the message list. It rides on
//!   [`crate::llm::HostGuidance`] for one call and is filtered against the
//!   history, so it neither duplicates a turn nor changes the history.
//!
//! [`ThinkingState`] is the lifecycle owner: it ingests reasoning and tool
//! outcomes, updates or retires decisions, and emits the injection note. A
//! [`ThinkingSnapshot`] is serializable, so a host that saves its session can
//! restore the still-valid summary after an interruption.

mod extract;
mod lifecycle;
mod summary;

pub use lifecycle::ThinkingState;
pub use summary::{
    PendingTool, ThinkingConfidence, ThinkingDecision, ThinkingOpenQuestion, ThinkingOrigin,
    ThinkingPhase, ThinkingSnapshot, ThinkingSummary,
};

/// The stable source label for the injected note.
///
/// It reaches the request diagnostics and the host-guidance purpose, so a
/// reader (or a log) can tell the note apart from reader input and from any
/// other harness guidance.
pub const THINKING_SOURCE: &str = "thinking-summary";

/// The snapshot schema this build writes and can read.
pub const THINKING_SCHEMA_VERSION: u32 = 1;

/// The longest single item kept in a summary (statement, rationale or action).
pub(crate) const ITEM_CHARS: usize = 320;

/// The longest question kept in a summary.
pub(crate) const QUESTION_CHARS: usize = 240;

/// The longest reasoning tail one observation parses.
///
/// A reasoning model can emit tens of thousands of tokens in one step; keeping
/// only the tail bounds the parse cost and keeps the summary about the work in
/// front of the model rather than its opening thoughts.
pub(crate) const REASONING_TAIL_CHARS: usize = 16_000;

/// Append streamed reasoning while keeping only a bounded tail.
///
/// The buffer is trimmed at twice the parse limit, so repeated appends stay
/// amortized O(total) instead of shifting a full window on every chunk.
pub(crate) fn push_reasoning_tail(buffer: &mut String, text: &str) {
    buffer.push_str(text);
    if buffer.chars().count() <= REASONING_TAIL_CHARS * 2 {
        return;
    }
    let skip = buffer.chars().count() - REASONING_TAIL_CHARS;
    let byte = buffer
        .char_indices()
        .nth(skip)
        .map(|(index, _)| index)
        .unwrap_or(0);
    buffer.drain(..byte);
}

/// Truncate `text` to at most `limit` characters, marking the cut.
pub(crate) fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// Collapse whitespace so one item renders on one line.
pub(crate) fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// Lowercase and collapse whitespace for containment checks.
pub(crate) fn normalize_text(text: &str) -> String {
    one_line(text).to_lowercase()
}

/// Whether a character belongs to a CJK writing system.
pub(crate) fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

/// Words too common to identify what a statement is about.
pub(crate) const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "your", "when", "use", "using",
    "are", "was", "were", "will", "would", "should", "can", "could", "have", "has", "had", "not",
    "but", "you", "our", "its", "their", "them", "they", "than", "then", "there", "here", "also",
    "any", "all", "some", "such", "each", "every", "work", "works", "working", "task", "tasks",
    "thing", "things", "need", "needs", "want", "wants", "make", "makes", "making", "get", "gets",
    "one", "two", "new", "add", "adds", "adding", "create", "creates", "creating", "file", "files",
    "because", "since", "now", "just", "like", "about", "after", "before", "more", "most", "very",
    "still", "only", "even", "well", "what", "which", "how", "why", "where", "who", "been",
    "being", "does", "did",
];

/// The significant tokens of `text`, used for similarity.
///
/// Latin words are lowercased; CJK runs are broken into character bigrams so
/// two Chinese sentences about the same subject compare meaningfully instead of
/// collapsing into one opaque token.
pub(crate) fn significant_tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for word in lower.split(|ch: char| !ch.is_alphanumeric()) {
        let count = word.chars().count();
        if count < 2 {
            continue;
        }
        if word.chars().any(is_cjk) {
            let chars: Vec<char> = word.chars().filter(|ch| is_cjk(*ch)).collect();
            for pair in chars.windows(2) {
                out.push(pair.iter().collect());
            }
        } else if !STOPWORDS.contains(&word) {
            out.push(word.to_string());
        }
    }
    out
}

/// Jaccard similarity of two token sets, in `0.0..=1.0`.
pub(crate) fn similarity(left: &str, right: &str) -> f32 {
    let left_tokens = significant_tokens(left);
    let right_tokens = significant_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let left_set: std::collections::BTreeSet<&str> =
        left_tokens.iter().map(String::as_str).collect();
    let right_set: std::collections::BTreeSet<&str> =
        right_tokens.iter().map(String::as_str).collect();
    let intersection = left_set.intersection(&right_set).count();
    let union = left_set.union(&right_set).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::{clip_chars, similarity};

    #[test]
    fn clip_respects_character_boundaries() {
        let text = "决策".repeat(10);
        let clipped = clip_chars(&text, 5);
        assert_eq!(clipped.chars().count(), 5);
        assert!(clipped.ends_with("..."));
    }

    #[test]
    fn the_streaming_buffer_keeps_only_a_bounded_tail() {
        let mut buffer = String::new();
        for _ in 0..5_000 {
            super::push_reasoning_tail(&mut buffer, "abcde");
        }
        assert!(buffer.chars().count() <= super::REASONING_TAIL_CHARS * 2);
        assert!(buffer.ends_with("abcde"));
    }

    #[test]
    fn cjk_similarity_uses_character_bigrams() {
        let left = "修复解析器中的失败测试";
        let right = "解析器里的失败测试需要修复";
        assert!(similarity(left, right) > 0.4, "{}", similarity(left, right));
        assert_eq!(similarity(left, "部署到生产环境"), 0.0);
    }
}
