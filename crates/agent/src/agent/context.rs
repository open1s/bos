//! ReAct session and context implementations plus the message accumulator.

use crate::agent::hooks::HookRegistry;
use crate::agent::plugin::PluginRegistry;
use react::engine::ReactError;
use react::llm::types::{ReactContext, ReactSession};
use react::llm::{
    Content, ContentPart, Instruction, LlmMessage as Message, LlmRequest as ReactLlmRequest,
    LlmResponse as ReactLlmResponse, LlmTool, Rule, Skill,
};
use react::runtime::app::ReActApp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Whether every tool call opened before `cut` is answered before it.
///
/// Compaction must never cut between an assistant tool call and its result: a
/// request whose history opens on an unanswered call is rejected by the
/// provider, and one that ends on a call whose result was folded away asks for
/// work it can never see the answer to. A cut is balanced when the calls opened
/// strictly before it are exactly the results seen strictly before it, which is
/// what the id-linked form below decides.
///
/// An orphan result - one whose call is not in the history - is treated as
/// balanced rather than corrupt: the transport may be reading a window that
/// starts mid-turn, and refusing to compact would be worse than a cut that
/// keeps the window consistent.
pub fn cut_is_balanced(messages: &[Message], cut: usize) -> bool {
    let cut = cut.min(messages.len());
    let mut open: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for message in &messages[..cut] {
        match message {
            Message::AssistantToolCall { tool_call_id, .. } => {
                open.insert(tool_call_id.as_str());
            }
            Message::ToolResult { tool_call_id, .. } => {
                open.remove(tool_call_id.as_str());
            }
            _ => {}
        }
    }
    open.is_empty()
}

/// The surface position the model cannot lose: a system prompt leading the
/// history.
///
/// The contract never shadows a `system/message` at surface node 0, because
/// folding the system prompt into a summary would drop the instructions the
/// whole session runs under. A later system message is ordinary history and may
/// be folded like anything else.
pub fn system_head_len(messages: &[Message]) -> usize {
    usize::from(matches!(messages.first(), Some(Message::System { .. })))
}

/// What one on-demand compaction did.
///
/// The counts are what the command reports, so the reader sees the same shape
/// the compaction contract describes: how many history items the checkpoint
/// replaced and how many tokens the session shed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionOutcome {
    /// Messages the checkpoint replaced.
    pub replaced: usize,
    /// Estimated tokens in the replaced span.
    pub before_tokens: usize,
    /// Estimated tokens in the session after the checkpoint was installed.
    pub after_tokens: usize,
    /// The model-written checkpoint text, unframed.
    pub summary: String,
}

impl CompactionOutcome {
    /// Tokens this compaction freed, saturating at zero.
    pub fn freed_tokens(&self) -> usize {
        self.before_tokens.saturating_sub(self.after_tokens)
    }
}

/// Why one compaction attempt did not land.
///
/// The variants exist so a caller can report the REAL cause instead of one
/// generic sentence: a provider failure carries the provider's own text, an
/// empty answer is not a shrink-gate refusal, and a summary that would grow the
/// context names both token counts. The display strings keep the wording the
/// older string errors used, because callers and tests already read them.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactionFailure {
    /// The summarizer request itself failed.
    ///
    /// `detail` is the provider's message verbatim. `over_window` is set when
    /// that message names a context/token limit, so the UI can say the request
    /// was too large rather than calling it a useless summary.
    #[error("compaction request failed: {detail}")]
    Provider {
        /// The provider's own error text.
        detail: String,
        /// Whether the error names a context-window or token-limit overflow.
        over_window: bool,
    },
    /// The provider rejected the request for a missing API key.
    #[error("compaction request failed: {detail}")]
    MissingCredential {
        /// The provider's own error text.
        detail: String,
    },
    /// The summarizer answered with no text at all.
    #[error("summarization produced no text summary content")]
    EmptySummary,
    /// The framed checkpoint is not smaller than the span it would shadow.
    #[error("summary is not smaller than the shadowed content ({framed_tokens} >= {span_tokens})")]
    NotSmaller {
        /// Estimated tokens in the framed checkpoint.
        framed_tokens: usize,
        /// Estimated tokens in the span it would replace.
        span_tokens: usize,
    },
}

impl CompactionFailure {
    /// Classify one failed summarizer request.
    ///
    /// The whole provider text is preserved; only the CATEGORY is derived.
    pub fn from_llm_error(error: &react::llm::LlmError) -> Self {
        let detail = error.to_string();
        match error {
            react::llm::LlmError::ApiKeyMissing => CompactionFailure::MissingCredential { detail },
            react::llm::LlmError::Http(_)
            | react::llm::LlmError::Other(_)
            | react::llm::LlmError::Parse(_)
            | react::llm::LlmError::Timeout
            | react::llm::LlmError::RateLimited => CompactionFailure::Provider {
                over_window: looks_like_context_overflow(&detail),
                detail,
            },
        }
    }
}

/// Whether a provider message names a context-window or token-limit overflow.
///
/// Providers word this differently (`context length`, `maximum context`,
/// `too many tokens`, `input is too long`), so the check is deliberately a
/// fragment match over the lowercased text rather than one exact sentence.
pub fn looks_like_context_overflow(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    const FRAGMENTS: &[&str] = &[
        "context length",
        "context_length",
        "context window",
        "context_window",
        "maximum context",
        "max context",
        "too many tokens",
        "token limit",
        "input is too long",
        "prompt is too long",
        "request too large",
        "exceeds the maximum",
        "exceeded the maximum",
    ];
    FRAGMENTS.iter().any(|fragment| lower.contains(fragment))
}

/// The completion cap for one summarizer request.
///
/// A checkpoint is extraction, not writing: capping the output keeps the
/// request's input + reserved output inside a modest model window, which is the
/// other half of a provider's context-length rejection. It is deliberately much
/// smaller than a normal turn's completion budget.
pub const COMPACTION_MAX_OUTPUT_TOKENS: u32 = 4_096;

/// The marker a bounded digest puts where it omitted the span's middle.
pub const COMPACTION_OMISSION: &str =
    "[... the middle of this span was omitted so the compaction request fits the summarizer budget ...]";

/// Render the shadowed span as the plain text a summarizer reads.
///
/// A span that fits `budget` estimated tokens is rendered in full, message by
/// message, exactly as before. A larger span is reduced to its OLDEST
/// `budget / 2` tokens plus its NEWEST `budget - budget / 2` tokens with the
/// omission marker between them: the head carries the original intent and the
/// tail carries the work in progress, which is what a checkpoint needs. A
/// zero budget means "unbounded" and keeps the old whole-span behaviour for
/// callers that do not configure one.
pub fn render_compaction_digest(span: &[Message], budget: usize) -> String {
    let rendered: Vec<String> = span.iter().map(render_for_summary).collect();
    let whole = rendered.join("\n");
    if budget == 0 || estimate_tokens(&whole) <= budget {
        return whole;
    }
    let marker_tokens = estimate_tokens(COMPACTION_OMISSION) + 2;
    let content_budget = budget.saturating_sub(marker_tokens);
    if content_budget == 0 {
        // The budget cannot even pay for the marker. Reporting the omission is
        // still bounded and truthful; returning the whole span would not be.
        return COMPACTION_OMISSION.to_string();
    }
    let head_budget = content_budget / 2;
    let tail_budget = content_budget - head_budget;
    let head_end = token_prefix_end(&whole, head_budget);
    let tail_start = token_suffix_start(&whole, tail_budget);
    if tail_start <= head_end || tail_start >= whole.len() {
        // The two slices would overlap or the tail could not be paid for at
        // all: keep the oldest part alone rather than duplicating or spilling
        // the whole span.
        return format!("{}\n{COMPACTION_OMISSION}", &whole[..head_end]);
    }
    format!(
        "{}\n{COMPACTION_OMISSION}\n{}",
        &whole[..head_end],
        &whole[tail_start..]
    )
}

/// The byte end of the longest prefix priced at or below `max_tokens`.
fn token_prefix_end(text: &str, max_tokens: usize) -> usize {
    let mut ascii = 0usize;
    let mut other = 0usize;
    let mut end = 0usize;
    for (index, ch) in text.char_indices() {
        if ch.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
        if ascii.div_ceil(4) + other > max_tokens {
            break;
        }
        end = index + ch.len_utf8();
    }
    end
}

/// The byte start of the longest suffix priced at or below `max_tokens`.
fn token_suffix_start(text: &str, max_tokens: usize) -> usize {
    let mut ascii = 0usize;
    let mut other = 0usize;
    let mut start = text.len();
    for (index, ch) in text.char_indices().rev() {
        if ch.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
        if ascii.div_ceil(4) + other > max_tokens {
            break;
        }
        start = index;
    }
    start
}

/// Estimated tokens for one message, using the same heuristic as
/// [`ContextBudget`].
pub fn estimate_message_tokens(message: &Message) -> usize {
    match message {
        Message::System { content } | Message::Assistant { content } => estimate_tokens(content),
        Message::User { content } => estimate_tokens(&content_to_summary_string(content)),
        Message::AssistantToolCall { name, args, .. } => {
            estimate_tokens(name) + estimate_tokens(&args.to_string())
        }
        Message::ToolResult { content, .. } => estimate_tokens(content),
    }
}

/// The checkpoint output of an LLM response, provider-agnostic.
pub fn response_text(response: &ReactLlmResponse) -> String {
    match response {
        ReactLlmResponse::OpenAI(rsp) => rsp
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .unwrap_or_default(),
        ReactLlmResponse::Responses(rsp) => rsp.output_text(),
    }
}

/// The preamble the compaction contract puts above a checkpoint.
///
/// Mirrors the shipped backend's own copy: the reader is told this is an
/// automatically generated checkpoint, to treat it as background, and not to
/// acknowledge it.
pub const CHECKPOINT_PREAMBLE: &str = "This is an automatically generated checkpoint condensing an earlier span of the conversation to free up context. Treat the captured context as established background and build on it without restating it. Continue the task directly from the messages that follow, without acknowledging this checkpoint.";

/// The instruction the summarizer call carries as its final user message.
///
/// The section list is fixed, so the checkpoint shape does not drift between
/// runs, and the rules pin the things a resume actually needs: exact paths,
/// commands, error strings and identifiers survive verbatim.
pub const COMPACTION_INSTRUCTION: &str = "You are now acting as a compaction engine for this AI coding assistant. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.

Output EXACTLY the Markdown structure below: keep every section, in order. Use terse bullets, not prose paragraphs. Write \"(none)\" for an empty section \u{2014} never drop a section.

## Primary Request and Intent
- [the user's original and evolving goals; quote verbatim where the exact wording matters]

## Key Technical Concepts
- [technologies, frameworks, patterns, and conventions in play]

## Files and Code
- [exact path: why it matters, key changes or snippets]

## Errors and Fixes
- [error: how it was resolved, plus any related user feedback]

## Pending Jobs
- [explicitly requested work not yet completed]

## Current Work
- [precisely what was in progress at this checkpoint]

## Next Step
- [the single next action, directly in line with the most recent request, or \"(none)\"]

## Critical Context
- [decisions and their rationale, constraints, user preferences, open questions, data needed to continue]

Rules:
- Write concise English engineering prose. Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.
- Capture user feedback and explicit instructions faithfully, especially corrections.
- Do NOT mention this summarization request or that the context was compacted.
- Output only the checkpoint text: do not call any tool or take any other action.
- If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint. Do not copy it forward verbatim: preserve still-true facts, drop stale ones, and merge newer information into a single consolidated summary under the same structure.";

/// Frame one summary as the checkpoint message the model reads.
pub fn render_checkpoint(summary: &str) -> String {
    // The shipped backend frames the replacement user message as three text
    // blocks - preamble + open tag, the summary blocks, the close tag - and the
    // adapter concatenates them with no separator. Keep the tags flush against
    // the body so the bytes match what DSH persisted.
    format!("{CHECKPOINT_PREAMBLE}\n\n<compacted-summary>{summary}</compacted-summary>")
}

/// Render one message the way the summarizer reads it.
///
/// The summarizer replays the shadowed span as plain text, so a tool call and
/// its result stay legible instead of collapsing into an opaque structure.
pub fn render_for_summary(message: &Message) -> String {
    match message {
        Message::System { content } => format!("[system] {content}"),
        Message::User { content } => {
            format!("[user] {}", content_to_summary_string(content))
        }
        Message::Assistant { content } => format!("[assistant] {content}"),
        Message::AssistantToolCall { name, args, .. } => {
            format!("[assistant tool call] {name}: {args}")
        }
        Message::ToolResult { content, .. } => format!("[tool result] {content}"),
    }
}

fn content_to_summary_string(content: &Content) -> String {
    match content {
        Content::Text(s) => s.clone(),
        Content::Parts(parts) => parts
            .iter()
            .filter_map(|part| {
                if let ContentPart::Text { text } = part {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// AgentSession stores conversation history and session state for the ReAct engine.
/// Implements ReactSession trait for integration with the react crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    messages: Vec<Message>,
    context: JsonValue,
    metadata: SessionMetadata,
}

/// Timestamps and counters stored with an [`AgentSession`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unix timestamp when the session was created.
    pub created_at: u64,
    /// Unix timestamp of the last update.
    pub updated_at: u64,
    /// Number of messages recorded so far.
    pub message_count: usize,
}

impl AgentSession {
    /// Create an empty session.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            context: JsonValue::Null,
            metadata: SessionMetadata {
                created_at: current_timestamp(),
                updated_at: current_timestamp(),
                message_count: 0,
            },
        }
    }

    /// Append a user message.
    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User {
            content: Content::Text(content),
        });
        self.update_metadata();
    }

    /// Append a user message holding arbitrary [`Content`].
    ///
    /// Multimodal turns (text plus images) arrive as
    /// [`Content::Parts`]; vendors render each part the way their
    /// API expects, and [`ContextBudget::estimate_session`] keeps
    /// counting only the text, so a pasted screenshot does not
    /// masquerade as a prompt of base64.
    pub fn add_user_content(&mut self, content: Content) {
        self.messages.push(Message::User { content });
        self.update_metadata();
    }

    /// Append an assistant message.
    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
        self.update_metadata();
    }

    fn update_metadata(&mut self) {
        self.metadata.updated_at = current_timestamp();
        self.metadata.message_count = self.messages.len();
    }

    /// Take the history, leaving the session empty.
    pub fn take_messages(&mut self) -> Vec<Message> {
        let msgs = std::mem::take(&mut self.messages);
        self.update_metadata();
        msgs
    }

    /// Replace the conversation history.
    pub fn restore_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.update_metadata();
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether there are no messages.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Borrow the conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Append a message of any kind.
    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
        self.update_metadata();
    }

    /// The opaque session context value.
    pub fn session_context(&self) -> JsonValue {
        self.context.clone()
    }

    /// Replace the session context value.
    pub fn set_session_context(&mut self, context: JsonValue) {
        self.context = context;
    }

    /// Reset the session context to null.
    pub fn clear_session_context(&mut self) {
        self.context = JsonValue::Null;
    }

    /// Write the session to `path` as pretty JSON.
    pub fn save(&self, path: &str) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Load the session from a JSON file at `path`.
    pub fn restore(&mut self, path: &str) -> Result<(), std::io::Error> {
        let json = std::fs::read_to_string(path)?;
        self.restore_from_json(&json)
    }

    /// Load the session from a JSON string.
    pub fn restore_from_json(&mut self, json: &str) -> Result<(), std::io::Error> {
        let restored: AgentSession = serde_json::from_str(json).map_err(std::io::Error::other)?;
        self.messages = restored.messages;
        self.context = restored.context;
        self.metadata = restored.metadata;
        Ok(())
    }

    /// Serialize the session to pretty JSON.
    pub fn to_json_string(&self) -> Result<String, std::io::Error> {
        serde_json::to_string_pretty(self).map_err(std::io::Error::other)
    }

    /// Drop all messages except system messages and reset the context.
    pub fn clear(&mut self) {
        self.messages
            .retain(|msg| matches!(msg, Message::System { .. }));
        self.context = JsonValue::Null;
        self.metadata.updated_at = current_timestamp();
    }

    /// Split the history for a compaction that retains a priced recent tail.
    ///
    /// This is the contract scheme: walk back from the newest message
    /// accumulating prices until `retain_tokens` is reached, then walk that
    /// boundary FORWARD until it sits on a tool-pairing balanced cut, so a cut
    /// never lands between a call and its result. A system prompt at the head is
    /// never folded, because the instructions the session runs under are not
    /// history.
    ///
    /// `retain_tokens` of zero means "condense everything safe", which is what
    /// the manual command asks for: it prices the tail anyway and then keeps
    /// walking the boundary back to the head.
    ///
    /// Returns the older span and the tail, or `None` when nothing can be
    /// folded: an empty history, everything already needed, or a boundary that
    /// would reach the head.
    pub fn split_for_compaction_at(
        &self,
        retain_tokens: usize,
    ) -> Option<(Vec<Message>, Vec<Message>)> {
        let len = self.messages.len();
        if len == 0 {
            return None;
        }
        let first = system_head_len(&self.messages);
        // Walk back from the newest end, pricing as we go, until the tail is
        // worth `retain_tokens`. At zero this settles on the last message, so
        // the boundary then walks forward from there under the pairing rule.
        let mut accumulated = 0usize;
        let mut keep_from = len;
        for index in (0..len).rev() {
            accumulated += estimate_message_tokens(&self.messages[index]);
            keep_from = index;
            if accumulated >= retain_tokens {
                break;
            }
        }
        if keep_from <= first {
            return None;
        }
        // Never split a call from its result: move the boundary forward (toward
        // the newest end) until the cut is balanced.
        while keep_from > first && !cut_is_balanced(&self.messages, keep_from) {
            keep_from -= 1;
        }
        if keep_from <= first {
            return None;
        }
        Some((
            self.messages[first..keep_from].to_vec(),
            self.messages[keep_from..].to_vec(),
        ))
    }

    /// Split the history keeping the newest `keep_recent` messages verbatim.
    ///
    /// The message-count form of [`Self::split_for_compaction_at`], for callers
    /// with no price for the tail. The same head protection and pairing rule
    /// apply, so a boundary chosen this way is as safe as a priced one.
    pub fn split_for_compaction(&self, keep_recent: usize) -> Option<(Vec<Message>, Vec<Message>)> {
        let len = self.messages.len();
        if len <= keep_recent {
            return None;
        }
        let first = system_head_len(&self.messages);
        let mut keep_from = len.saturating_sub(keep_recent);
        if keep_from <= first {
            return None;
        }
        while keep_from > first && !cut_is_balanced(&self.messages, keep_from) {
            keep_from -= 1;
        }
        if keep_from <= first {
            return None;
        }
        Some((
            self.messages[first..keep_from].to_vec(),
            self.messages[keep_from..].to_vec(),
        ))
    }

    /// Install a model-written checkpoint over the older span, keeping the
    /// recent tail verbatim.
    ///
    /// This is the real condensation the `/compact` command performs: the
    /// caller serializes the returned older span, asks the model for a
    /// structured summary, and hands the text back here. The checkpoint
    /// replaces that span as one user-role message framed in
    /// `<compacted-summary>` tags, which is the shape the compaction contract
    /// specifies, and the recent tail follows it untouched.
    ///
    /// Returns the number of messages the checkpoint replaced. Zero means the
    /// history was not compactable and nothing was written.
    ///
    /// The system prompt at the head, when there is one, stays ahead of the
    /// checkpoint: it is the session instructions, not history, and folding it
    /// away would drop the rules the conversation runs under.
    pub fn install_checkpoint(&mut self, keep_recent: usize, summary: &str) -> usize {
        self.install_checkpoint_at(keep_recent, summary)
    }

    /// Install a checkpoint selected by message count, keeping `keep_recent`.
    ///
    /// The priced form is [`Self::install_priced_checkpoint`], for callers that
    /// measure the tail in tokens rather than messages.
    pub fn install_checkpoint_at(&mut self, keep_recent: usize, summary: &str) -> usize {
        let Some((removed, recent)) = self.split_for_compaction(keep_recent) else {
            return 0;
        };
        self.replace_span_with_checkpoint(removed, recent, summary)
    }

    /// Install a checkpoint over a token-priced span.
    ///
    /// Prefer this when the caller prices the tail: `retain_tokens` is the same
    /// budget [`Self::split_for_compaction_at`] selected with, so the span the
    /// model was asked about is exactly the span this replaces.
    pub fn install_priced_checkpoint(&mut self, retain_tokens: usize, summary: &str) -> usize {
        let Some((removed, recent)) = self.split_for_compaction_at(retain_tokens) else {
            return 0;
        };
        self.replace_span_with_checkpoint(removed, recent, summary)
    }

    /// Put one checkpoint, framed in tags, in place of `removed` and ahead of
    /// the retained tail.
    fn replace_span_with_checkpoint(
        &mut self,
        removed: Vec<Message>,
        recent: Vec<Message>,
        summary: &str,
    ) -> usize {
        let checkpoint = Message::User {
            content: Content::Text(render_checkpoint(summary)),
        };
        // Any leading system prompt is instructions, not history: it is kept in
        // front of the checkpoint rather than folded into it.
        let head = system_head_len(&self.messages);
        let mut compacted: Vec<Message> = self.messages[..head].to_vec();
        compacted.push(checkpoint);
        compacted.extend(recent);
        self.messages = compacted;

        match &mut self.context {
            JsonValue::Object(map) => {
                map.insert(
                    "compacted_summary".to_string(),
                    JsonValue::String(summary.to_string()),
                );
            }
            ctx if !ctx.is_null() => {
                self.context = serde_json::json!({
                    "compacted_summary": summary,
                    "previous_context": ctx.clone(),
                });
            }
            _ => {
                self.context = serde_json::json!({"compacted_summary": summary});
            }
        }
        self.update_metadata();
        removed.len()
    }

    /// Summarize all but the most recent `keep_recent` messages and prepend a summary.
    ///
    /// This is the MECHANICAL path: the summary is the dropped messages
    /// truncated, not a model-written checkpoint, and it prepends a system
    /// message rather than replacing the span with a user one. It backs the
    /// automatic pre-send trim, where a request must be made to fit without a
    /// second model call.
    ///
    /// The split is the same safe selection the checkpoint path uses: a leading
    /// system prompt is never folded, and the cut never lands between an
    /// assistant tool call and its result.
    pub fn compact(&mut self, keep_recent: usize, max_summary_chars: usize) {
        let Some((removed, recent)) = self.split_for_compaction(keep_recent) else {
            return;
        };
        // The system prompt at the head stays where it is; the mechanical
        // summary follows it.
        let head = system_head_len(&self.messages);

        let summary_input: String = removed
            .iter()
            .map(|msg| match msg {
                Message::System { content } => content.clone(),
                Message::User { content } => content_to_summary_string(content),
                Message::Assistant { content } => content.clone(),
                Message::AssistantToolCall { name, args, .. } => {
                    format!("Tool call {}: {}", name, args)
                }
                Message::ToolResult { content, .. } => content.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let summary = if summary_input.is_empty() {
            "Prior conversation history has been compacted.".to_string()
        } else {
            let summary_text: String = summary_input.chars().take(max_summary_chars).collect();
            format!(
                "Prior conversation history has been compacted. Summary: {}",
                summary_text
            )
        };

        let summary_message = Message::system(summary.clone());
        let mut compacted: Vec<Message> = self.messages[..head].to_vec();
        compacted.push(summary_message);
        compacted.extend(recent);
        self.messages = compacted;

        match &mut self.context {
            JsonValue::Object(map) => {
                map.insert("compacted_summary".to_string(), JsonValue::String(summary));
            }
            ctx if !ctx.is_null() => {
                self.context = serde_json::json!({
                    "compacted_summary": summary.clone(),
                    "previous_context": ctx.clone(),
                });
            }
            _ => {
                self.context = serde_json::json!({"compacted_summary": summary.clone()});
            }
        }
        self.update_metadata();
    }
}

/// Rough token estimate for `text`: ASCII averages ~4 characters per token,
/// while CJK and other non-ASCII characters average closer to one each.
///
/// The estimate intentionally runs a little high so a budget trips before the
/// provider rejects the request. It is a heuristic for [`ContextBudget`],
/// not a billing-grade count.
pub fn estimate_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    if ascii + other == 0 {
        0
    } else {
        ascii.div_ceil(4) + other
    }
}

/// Character counts backing a token estimate without materializing the text.
#[derive(Default)]
struct CharCounts {
    ascii: usize,
    other: usize,
}

impl CharCounts {
    fn add(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_ascii() {
                self.ascii += 1;
            } else {
                self.other += 1;
            }
        }
    }

    fn tokens(&self) -> usize {
        if self.ascii + self.other == 0 {
            0
        } else {
            self.ascii.div_ceil(4) + self.other
        }
    }
}

impl std::fmt::Write for CharCounts {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.add(s);
        Ok(())
    }
}

/// Token estimate for a [`Content`] without joining its text parts.
///
/// [`content_to_summary_string`] joins text parts with a single space; this
/// mirrors that join by counting one extra ASCII separator per boundary, so
/// the estimate is identical without allocating the joined string.
fn estimate_content_tokens(content: &Content) -> usize {
    match content {
        Content::Text(text) => estimate_tokens(text),
        Content::Parts(parts) => {
            let mut counts = CharCounts::default();
            let mut texts = 0usize;
            for part in parts {
                if let ContentPart::Text { text } = part {
                    counts.add(text);
                    texts += 1;
                }
            }
            if texts == 0 {
                return 0;
            }
            // The separators `content_to_summary_string` inserts between parts.
            counts.ascii += texts - 1;
            counts.tokens()
        }
    }
}

/// What changed when a [`ContextBudget`] trimmed a session's outgoing context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionReport {
    /// Estimated prompt tokens before compaction.
    pub before_tokens: usize,
    /// Estimated prompt tokens after compaction.
    pub after_tokens: usize,
    /// How many messages were folded into the summary before the turn.
    pub dropped_messages: usize,
}

/// Send-side context budget: when the estimated outgoing prompt would exceed
/// `max_tokens`, the oldest messages are compacted before the turn starts.
///
/// `max_tokens == 0` disables compaction entirely. [`AgentSession::compact`]
/// folds dropped messages into one summary system message while callers keep
/// their full transcript, so the budget is re-applied deterministically on
/// every turn from the same history — nothing is ever destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    /// Token ceiling for the estimated outgoing prompt (0 = never compact).
    pub max_tokens: usize,
    /// Most recent messages kept verbatim after compaction.
    pub keep_recent: usize,
    /// Cap on the mechanical summary that replaces dropped messages.
    pub max_summary_chars: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_tokens: 32_768,
            keep_recent: 8,
            max_summary_chars: 4_096,
        }
    }
}

impl ContextBudget {
    /// A budget with the given token ceiling and default trimming policy.
    pub fn with_max_tokens(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            ..Self::default()
        }
    }

    /// Estimate the tokens a session's messages contribute to the next prompt.
    pub fn estimate_session(&self, session: &AgentSession) -> usize {
        session
            .messages()
            .iter()
            .map(|msg| {
                // Estimate over borrowed text: this runs on every turn, and a
                // history full of tool results must not be copied just to be
                // measured.
                let body = match msg {
                    Message::System { content } => estimate_tokens(content),
                    Message::User { content } => estimate_content_tokens(content),
                    Message::Assistant { content } => estimate_tokens(content),
                    Message::AssistantToolCall { name, args, .. } => {
                        let mut counts = CharCounts::default();
                        let _ =
                            std::fmt::Write::write_fmt(&mut counts, format_args!("{name} {args}"));
                        counts.tokens()
                    }
                    Message::ToolResult { content, .. } => estimate_tokens(content),
                };
                // +4 per message covers role framing and message delimiters.
                body + 4
            })
            .sum()
    }

    /// Whether the next send would exceed the budget.
    pub fn needs_compaction(&self, session: &AgentSession) -> bool {
        self.max_tokens > 0 && self.estimate_session(session) > self.max_tokens
    }

    /// Compact `session` when it is over budget, reporting what changed.
    ///
    /// Returns `None` when the budget is disabled, the session already fits,
    /// or trimming had nothing left to remove (the process always converges:
    /// each pass reduces the message count toward `keep_recent`).
    pub fn apply(&self, session: &mut AgentSession) -> Option<CompactionReport> {
        if !self.needs_compaction(session) {
            return None;
        }
        let before_messages = session.messages().len();
        let before_tokens = self.estimate_session(session);
        session.compact(self.keep_recent, self.max_summary_chars);
        let after_messages = session.messages().len();
        // Net-shrink gate: `compact` leaves the session unchanged when there
        // is nothing left to trim, so repeated applications converge.
        if after_messages >= before_messages {
            return None;
        }
        Some(CompactionReport {
            before_tokens,
            after_tokens: self.estimate_session(session),
            // Everything folded into the summary system message, which is the
            // one new message compact() inserts.
            dropped_messages: before_messages + 1 - after_messages,
        })
    }
}

impl Default for AgentSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactSession for AgentSession {
    fn push(&mut self, msg: Message) {
        self.messages.push(msg);
        self.update_metadata();
    }

    fn history(&self) -> Option<&[Message]> {
        Some(&self.messages)
    }

    fn estimate_tokens(&self) -> Option<usize> {
        Some(self.messages.iter().map(estimate_message_tokens).sum())
    }

    fn compact_mechanical(&mut self, keep_recent: usize, max_summary_chars: usize) -> bool {
        let before = self.estimate_tokens().unwrap_or(0);
        self.compact(keep_recent, max_summary_chars);
        self.estimate_tokens().is_some_and(|after| after < before)
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// AgentReactContext holds tools, skills, rules, and instructions for the ReAct engine.
/// Implements ReactContext trait for integration with the react crate.
/// Tools, skills, rules, and instructions for one ReAct run.
#[derive(Debug, Clone, Default)]
pub struct AgentReactContext {
    /// Identifier for the session this context belongs to.
    pub session_id: String,
    /// Tools offered to the model.
    pub tools: Vec<LlmTool>,
    /// Skills offered to the model.
    pub skills: Vec<Skill>,
    /// Rules constraining the model.
    pub rules: Vec<Rule>,
    /// Extra instructions prepended to the prompt.
    pub instructions: Vec<Instruction>,
}

impl AgentReactContext {
    /// Create an empty context for `session_id`.
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            tools: Vec::new(),
            skills: Vec::new(),
            rules: Vec::new(),
            instructions: Vec::new(),
        }
    }

    /// Replace the tool list.
    pub fn with_tools(mut self, tools: Vec<LlmTool>) -> Self {
        self.tools = tools;
        self
    }

    /// Replace the skill list.
    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }
}

impl ReactContext for AgentReactContext {
    fn session_id(&self) -> String {
        self.session_id.clone()
    }

    fn skills(&self) -> Option<&[Skill]> {
        if self.skills.is_empty() {
            None
        } else {
            Some(&self.skills)
        }
    }

    fn tools(&self) -> Option<&[LlmTool]> {
        if self.tools.is_empty() {
            None
        } else {
            Some(&self.tools)
        }
    }

    fn rules(&self) -> Option<&[Rule]> {
        if self.rules.is_empty() {
            None
        } else {
            Some(&self.rules)
        }
    }

    fn instructions(&self) -> Option<&[Instruction]> {
        if self.instructions.is_empty() {
            None
        } else {
            Some(&self.instructions)
        }
    }

    fn add_tool(&mut self, tool: LlmTool) {
        if self.tools.iter().any(|t| t.name == tool.name) {
            return;
        }
        self.tools.push(tool);
    }

    fn notify_request(&self, _req: &ReactLlmRequest) {}
    fn notify_response(&self, _resp: &ReactLlmResponse) {}
    fn notify_error(&self, _err: &react::llm::LlmError) {}
    fn on_chunk(&self, _chunk: &str) {}
    fn on_chunk_callback(&self) -> Option<react::llm::ChunkCallback> {
        None
    }
}

/// The largest number of messages one running turn keeps queued for its next
/// step.
///
/// The bound exists so a host that steers faster than the model steps cannot
/// grow the turn without limit, but it must not be a product limit: a reader
/// can queue a whole message list and steer it in one gesture, so this is a
/// guard well above any queue a person builds between two steps. The
/// reference's inbox is bounded by the session rather than by a constant.
pub const MAX_STEER_DEPTH: usize = 64;

/// One host message waiting for a running turn's next step.
///
/// The content is what the model receives - it is appended as a user turn at
/// the point the step begins - and the ticket is the host's identity for the
/// message, reported back when the step takes it.
#[derive(Debug, Clone)]
pub struct SteerMessage {
    /// What the model receives as a user turn.
    pub content: Content,
    /// The host's identity for this message.
    pub ticket: u64,
}

#[derive(Default)]
struct SteerState {
    queue: VecDeque<SteerMessage>,
    sink: Option<Arc<dyn Fn(u64) + Send + Sync>>,
}

/// A bounded mailbox between the host and a RUNNING turn.
///
/// The reference's `Agent::steer` puts a message on the session's next-step
/// inbox and the loop takes it before deriving its next request. The BOS engine
/// exposes the same point as `ReActApp::before_llm_call`, which runs at the top
/// of every ReAct step (both the blocking and the streaming loop); this mailbox
/// is what that hook drains.
///
/// It is a mailbox and not a session message because a message pushed while a
/// request is in flight must not appear in THAT request: the drain happens
/// before the next step derives its request, so the message lands in the next
/// request and is recorded in the session at the point it was taken.
#[derive(Default)]
pub struct SteerMailbox {
    state: Mutex<SteerState>,
}

impl SteerMailbox {
    /// An empty mailbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one message for the next step.
    ///
    /// Returns false - and queues nothing - when the mailbox already holds
    /// [`MAX_STEER_DEPTH`] messages.
    pub fn push(&self, message: SteerMessage) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.queue.len() >= MAX_STEER_DEPTH {
            return false;
        }
        state.queue.push_back(message);
        true
    }

    /// Take every queued message, oldest first.
    ///
    /// The sink, when one is installed, is told every ticket after the queue is
    /// empty and outside the lock: a sink that calls back into the mailbox (or a
    /// host that steers again from it) cannot deadlock the drain.
    pub fn take(&self) -> Vec<SteerMessage> {
        let (messages, sink) = {
            let mut state = self.state.lock().unwrap();
            (
                state.queue.drain(..).collect::<Vec<_>>(),
                state.sink.clone(),
            )
        };
        if let Some(sink) = sink {
            for message in &messages {
                sink(message.ticket);
            }
        }
        messages
    }

    /// Drop every queued message and return its ticket, oldest first.
    ///
    /// The host calls this when a turn ends before reaching another step: the
    /// model never saw those messages, so they go back to the host's queue
    /// instead of being lost with the turn.
    pub fn withdraw(&self) -> Vec<u64> {
        let mut state = self.state.lock().unwrap();
        state
            .queue
            .drain(..)
            .map(|message| message.ticket)
            .collect()
    }

    /// How many messages wait for the next step.
    pub fn depth(&self) -> usize {
        self.state.lock().unwrap().queue.len()
    }

    /// Install, or clear with `None`, the callback told each ticket as a step
    /// takes it.
    pub fn set_sink(&self, sink: Option<Arc<dyn Fn(u64) + Send + Sync>>) {
        self.state.lock().unwrap().sink = sink;
    }
}

/// AgentReActApp integrates the Agent's hooks, plugins, and configuration with the ReAct engine.
/// This allows the agent to intercept and react to events during the ReAct loop.
#[derive(Default)]
pub struct AgentReActApp {
    hooks: Arc<HookRegistry>,
    plugins: Arc<PluginRegistry>,
    agent_name: String,
    steers: Option<Arc<SteerMailbox>>,
}

impl AgentReActApp {
    /// Create an app bridging `hooks` and `plugins` into the ReAct loop.
    pub fn new(hooks: Arc<HookRegistry>, plugins: Arc<PluginRegistry>, agent_name: String) -> Self {
        Self {
            hooks,
            plugins,
            agent_name,
            steers: None,
        }
    }

    /// Attach the mailbox this app drains at the top of every step.
    ///
    /// An app built by [`AgentReActApp::new`] alone drains nothing and behaves
    /// exactly as it did before steering existed.
    pub fn with_steers(mut self, steers: Arc<SteerMailbox>) -> Self {
        self.steers = Some(steers);
        self
    }
}

impl ReActApp for AgentReActApp {
    type Session = AgentSession;
    type Context = AgentReactContext;

    fn name(&self) -> &str {
        &self.agent_name
    }

    /// A steered message extends the turn instead of waiting for the next one.
    ///
    /// The engine asks this before it closes a step with no tool call. The
    /// reference's loop does the same with `inbox.nextStep.length`: the turn
    /// stays open, the next `before_llm_call` appends the message, and the
    /// model answers it inside the SAME turn. Nothing is taken here - this is
    /// only the keep-open decision, so a message pushed at the same instant
    /// cannot be dropped by an answer that never reaches a step.
    fn has_pending_step_input(&self) -> bool {
        self.steers
            .as_ref()
            .is_some_and(|steers| steers.depth() > 0)
    }

    #[allow(refining_impl_trait)]
    fn before_llm_call(
        &self,
        req: &mut ReactLlmRequest,
        session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = react::runtime::HookDecision> + Send {
        // The host's steered messages become user turns HERE, at the top of the
        // step and before the request is derived: a message pushed while the
        // previous request streamed is carried by THIS request, and one pushed
        // while this request streams waits for the next. This is the engine's
        // equivalent of the reference's next-step inbox, and it runs through
        // the same BeforeLlmCall hook a turn's own input passes, so any host
        // policy on that hook sees the step either way.
        if let Some(steers) = &self.steers {
            for message in steers.take() {
                session.push(Message::user(message.content));
            }
        }
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let plugins = self.plugins.clone();
        async move {
            if plugins.has_plugins() {
                let wrapper = crate::agent::plugin::LlmRequestWrapper::new(&*req);
                if let Some(modified) = plugins.on_llm_request(wrapper).await {
                    req.model = modified.model;
                    req.input = modified.input;
                    req.temperature = modified.temperature;
                    req.max_tokens = modified.max_tokens;
                    req.top_p = modified.top_p;
                    req.top_k = modified.top_k;
                }
            }
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("model", &req.model);
            hooks
                .trigger(crate::agent::hooks::HookEvent::BeforeLlmCall, ctx)
                .await
        }
    }

    #[allow(refining_impl_trait)]
    fn after_llm_response(
        &self,
        response: &mut ReactLlmResponse,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let plugins = self.plugins.clone();
        async move {
            if plugins.has_plugins() {
                let wrapper = crate::agent::plugin::LlmResponseWrapper::new(&*response);
                if let Some(modified) = plugins.on_llm_response(wrapper).await {
                    *response = modified.into_response();
                }
            }
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("response_type", "react");
            let _ = hooks
                .trigger(crate::agent::hooks::HookEvent::AfterLlmCall, ctx)
                .await;
        }
    }

    #[allow(refining_impl_trait)]
    fn after_llm_response_step(
        &self,
        response_text: &str,
        had_tool_call: bool,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let response_text = response_text.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("response_type", "stream");
            ctx.set("response_text", &response_text);
            ctx.set("had_tool_call", had_tool_call.to_string());
            let _ = hooks
                .trigger(crate::agent::hooks::HookEvent::AfterLlmCall, ctx)
                .await;
        }
    }

    #[allow(refining_impl_trait)]
    fn before_tool_call(
        &self,
        tool_name: &str,
        args: &mut JsonValue,
        call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = react::runtime::HookDecision> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let tool_name = tool_name.to_string();
        let call_id = call_id.to_string();
        let plugins = self.plugins.clone();
        async move {
            if plugins.has_plugins() {
                let wrapper =
                    crate::agent::plugin::ToolCallWrapper::new(&tool_name, args.clone(), None);
                if let Some(modified) = plugins.on_tool_call(wrapper).await {
                    *args = modified.args;
                }
            }
            // Serializing the arguments is only worth it when a hook reads
            // them; tool calls happen on every step of the ReAct loop.
            if !hooks.has_hooks(&crate::agent::hooks::HookEvent::BeforeToolCall) {
                return react::runtime::HookDecision::Continue;
            }
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("tool_name", &tool_name);
            ctx.set("call_id", &call_id);
            ctx.set("tool_args", args.to_string());
            hooks
                .trigger(crate::agent::hooks::HookEvent::BeforeToolCall, ctx)
                .await
        }
    }

    #[allow(refining_impl_trait)]
    fn after_tool_result(
        &self,
        tool_name: &str,
        result: &mut Result<JsonValue, ReactError>,
        call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = react::runtime::HookDecision> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let tool_name = tool_name.to_string();
        let call_id = call_id.to_string();
        let plugins = self.plugins.clone();
        async move {
            if plugins.has_plugins() {
                let tool_result = match &*result {
                    Ok(v) => crate::agent::plugin::ToolResultWrapper::new(v.clone()),
                    Err(e) => crate::agent::plugin::ToolResultWrapper {
                        result: serde_json::Value::Null,
                        success: false,
                        error: Some(e.to_string()),
                        metadata: std::collections::HashMap::new(),
                    },
                };
                if let Some(modified) = plugins.on_tool_result(tool_result).await {
                    if modified.success {
                        *result = Ok(modified.result);
                    } else {
                        *result = Err(ReactError::ToolError(
                            modified.error.unwrap_or_else(|| "Plugin error".to_string()),
                        ));
                    }
                }
            }
            if !hooks.has_hooks(&crate::agent::hooks::HookEvent::AfterToolCall) {
                return react::runtime::HookDecision::Continue;
            }
            // Tool results can be large; only render them into the hook
            // context when a hook is actually registered.
            let result_text = result.as_ref().map(|v| v.to_string()).unwrap_or_default();
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("tool_name", &tool_name);
            ctx.set("call_id", &call_id);
            ctx.set("tool_result", &result_text);
            hooks
                .trigger(crate::agent::hooks::HookEvent::AfterToolCall, ctx)
                .await
        }
    }

    #[allow(refining_impl_trait)]
    fn on_thought(
        &self,
        thought: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let thought = thought.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("thought", &thought);
            let _ = hooks
                .trigger(crate::agent::hooks::HookEvent::OnMessage, ctx)
                .await;
        }
    }

    #[allow(refining_impl_trait)]
    fn on_final_answer(
        &self,
        answer: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        let agent_name = self.agent_name.clone();
        let hooks = self.hooks.clone();
        let answer = answer.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("answer", &answer);
            let _ = hooks
                .trigger(crate::agent::hooks::HookEvent::OnComplete, ctx)
                .await;
        }
    }
}

/// A simple accumulator for building a message list.
#[derive(Debug, Clone, Default)]
pub struct MessageContext {
    pub(crate) messages: Vec<Message>,
}

impl MessageContext {
    /// Create an empty accumulator.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Append a user message.
    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User {
            content: Content::Text(content),
        });
    }

    /// Append an assistant message.
    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether there are no messages.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn a_multimodal_user_turn_holds_its_parts() {
        let mut session = AgentSession::new();
        session.add_user_content(Content::parts(vec![
            ContentPart::Text {
                text: "what is in this picture?".into(),
            },
            ContentPart::Binary {
                binary: react::llm::Binary::from_base64("image/png", "aGVsbG8=", None),
            },
        ]));
        let [Message::User { content }] = session.messages() else {
            panic!("one user message expected");
        };
        match content {
            Content::Parts(parts) => {
                assert_eq!(parts.len(), 2, "text and image travel together");
                assert!(matches!(parts[1], ContentPart::Binary { .. }));
            }
            other => panic!("parts expected, got {other:?}"),
        }
        // The budget counts only the text, never the base64 payload.
        let budget = ContextBudget::default();
        let est = budget.estimate_session(&session);
        assert!(
            est < 40,
            "base64 must not inflate the estimate: {est} tokens"
        );
    }

    /// A session of `messages` alternating user/assistant turns of `body`
    /// ASCII characters each.
    fn chat(messages: usize, body: usize) -> AgentSession {
        let mut session = AgentSession::new();
        for i in 0..messages {
            let filler = "x".repeat(body);
            if i % 2 == 0 {
                session.add_user(format!("u{i} {filler}"));
            } else {
                session.add_assistant(format!("a{i} {filler}"));
            }
        }
        session
    }

    /// Build a history whose middle holds one complete tool call and result.
    fn chat_with_tool_traffic() -> AgentSession {
        let mut session = AgentSession::new();
        session.add_user("u0 first".to_string());
        session.add_assistant("a0 reply".to_string());
        session.add_user("u1 run a tool".to_string());
        session.add_assistant("a1 calling".to_string());
        session.push(Message::AssistantToolCall {
            tool_call_id: "call-1".to_string(),
            name: "read_file".to_string(),
            args: serde_json::json!({"path": "src/main.rs"}),
        });
        session.push(Message::ToolResult {
            tool_call_id: "call-1".to_string(),
            content: "file body".to_string(),
        });
        session.add_assistant("a2 summarized the file".to_string());
        session.add_user("u2 next ask".to_string());
        session.add_assistant("a3 final".to_string());
        session
    }

    /// A cut landing on a call/result pair is refused; the boundary walks
    /// forward until the pair is whole. Without this, a request opens on a
    /// result whose call was folded away, or ends on a call the model can
    /// never see answered.
    #[test]
    fn a_cut_never_splits_a_tool_call_from_its_result() {
        let session = chat_with_tool_traffic();
        let messages = session.messages();
        // Index 4 is the AssistantToolCall, 5 its result.
        assert!(
            !cut_is_balanced(messages, 5),
            "a cut between call and result is unbalanced"
        );
        assert!(
            cut_is_balanced(messages, 4),
            "a cut before the call is balanced"
        );
        assert!(
            cut_is_balanced(messages, 6),
            "a cut after the result is balanced"
        );

        // Every boundary the selection returns is balanced, whatever the
        // budget, and the retained tail is never empty.
        for retain in [0usize, 1, 8, 40, 400] {
            if let Some((_, tail)) = session.split_for_compaction_at(retain) {
                assert!(cut_is_balanced(messages, messages.len() - tail.len()));
                assert!(!tail.is_empty());
            }
        }
    }

    /// The selection keeps a priced tail: a larger budget keeps more, and the
    /// boundary never reaches into the retained tail.
    #[test]
    fn a_larger_retain_budget_keeps_a_longer_tail() {
        // Each message is ~40 ascii chars, so ~10 tokens. A 20-message
        // history is about 200 tokens, and these budgets sit inside it.
        let session = chat(20, 40);
        let small = session.split_for_compaction_at(1).expect("a span");
        let large = session.split_for_compaction_at(80).expect("a span");
        assert!(
            small.0.len() > large.0.len(),
            "a bigger retain budget folds less: {} vs {}",
            small.0.len(),
            large.0.len()
        );
        assert!(small.1.len() < large.1.len(), "and it keeps a longer tail");
        // A budget larger than the whole history folds nothing.
        assert!(session.split_for_compaction_at(100_000).is_none());
    }

    /// A zero budget is the manual command: fold everything safely foldable,
    /// which is what makes `/compact` condense below the pressure threshold.
    #[test]
    fn a_zero_budget_folds_everything_safe() {
        let session = chat(12, 16);
        let (removed, kept) = session.split_for_compaction_at(0).expect("a span");
        assert_eq!(removed.len(), 11, "all but the last message folds");
        assert_eq!(kept.len(), 1);
    }

    /// A leading system prompt is the session instructions, not history: it
    /// stays in front of the checkpoint.
    #[test]
    fn a_leading_system_prompt_is_never_folded() {
        let mut session = AgentSession::new();
        session.push(Message::system("you are the session rules"));
        for i in 0..8 {
            session.add_user(format!("u{i}"));
            session.add_assistant(format!("a{i}"));
        }
        let before = session.messages().len();
        let replaced = session.install_priced_checkpoint(0, "checkpoint body");
        assert!(replaced > 0, "history was folded");
        // The head, the checkpoint, and whatever zero-budget retained: the
        // last message, which is all a balanced cut can leave.
        assert_eq!(
            session.messages().len(),
            3,
            "the system head plus the checkpoint and one tail message"
        );
        assert!(
            matches!(session.messages()[1], Message::User { .. }),
            "the checkpoint follows the system head"
        );
        let Message::System { content } = &session.messages()[0] else {
            panic!("the system prompt stays first");
        };
        assert_eq!(content, "you are the session rules");
        assert!(before > session.messages().len());
    }

    /// A history that is nothing but a system prompt has no compactable span.
    #[test]
    fn a_system_only_history_has_nothing_to_fold() {
        let mut session = AgentSession::new();
        session.push(Message::system("rules"));
        assert!(session.split_for_compaction_at(0).is_none());
        assert!(session.split_for_compaction(0).is_none());
    }

    #[test]
    fn a_checkpoint_replaces_the_older_span_and_keeps_the_tail() {
        let mut session = chat(8, 16);
        let before = session.messages().len();
        let (removed, kept) = session.split_for_compaction(2).expect("an older span");
        assert_eq!(removed.len(), 6);
        assert_eq!(kept.len(), 2);

        let replaced = session.install_checkpoint(2, "## Current Work\n- the checkpoint");
        assert_eq!(replaced, 6, "the checkpoint reports the span it replaced");
        // One checkpoint message plus the verbatim tail.
        assert_eq!(session.messages().len(), 3);
        assert!(before > session.messages().len(), "the session shrank");

        let first = &session.messages()[0];
        let Message::User { content } = first else {
            panic!("the checkpoint is a user message, got {first:?}");
        };
        let Content::Text(text) = content else {
            panic!("the checkpoint is plain text");
        };
        assert!(text.starts_with(CHECKPOINT_PREAMBLE));
        assert!(text.contains("<compacted-summary>"));
        assert!(text.contains("</compacted-summary>"));
        assert!(text.contains("the checkpoint"));
        // The retained tail is byte-for-byte what it was.
        assert_eq!(session.messages()[1..].len(), 2);
    }

    /// The compaction instruction is a fixed contract: every section, in order,
    /// and the rules a resume depends on. A deleted line fails here before it
    /// can quietly change how a checkpoint is written.
    #[test]
    fn the_compaction_instruction_pins_the_checkpoint_shape() {
        let sections = [
            "## Primary Request and Intent",
            "## Key Technical Concepts",
            "## Files and Code",
            "## Errors and Fixes",
            "## Pending Jobs",
            "## Current Work",
            "## Next Step",
            "## Critical Context",
        ];
        let mut cursor = 0;
        for section in sections {
            let at = COMPACTION_INSTRUCTION[cursor..]
                .find(section)
                .unwrap_or_else(|| panic!("the instruction keeps {section}"));
            cursor += at + section.len();
        }
        for rule in [
            "Output EXACTLY the Markdown structure below",
            "Write \"(none)\" for an empty section",
            "Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.",
            "Do NOT mention this summarization request or that the context was compacted.",
            "Output only the checkpoint text: do not call any tool or take any other action.",
            "If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint.",
        ] {
            assert!(
                COMPACTION_INSTRUCTION.contains(rule),
                "the instruction keeps the rule: {rule}"
            );
        }
        assert!(CHECKPOINT_PREAMBLE.starts_with(
            "This is an automatically generated checkpoint condensing an earlier span of the conversation"
        ));
    }

    #[test]
    fn a_history_inside_the_keep_window_has_nothing_to_fold() {
        let session = chat(4, 16);
        assert!(session.split_for_compaction(4).is_none());
        let mut session = chat(4, 16);
        assert_eq!(session.install_checkpoint(4, "unused"), 0);
        assert_eq!(session.messages().len(), 4, "and it is left alone");
    }

    /// The summarizer reads a span as text, so a tool call and its result stay
    /// legible instead of collapsing into an opaque structure.
    #[test]
    fn the_span_renders_tool_traffic_as_text() {
        let call = Message::AssistantToolCall {
            name: "read_file".to_string(),
            args: serde_json::json!({"path": "src/main.rs"}),
            tool_call_id: String::new(),
        };
        let rendered = render_for_summary(&call);
        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("src/main.rs"));
        assert!(render_for_summary(&Message::user("hello")).contains("hello"));
    }

    /// The checkpoint text the model wrote is what a later request carries, so
    /// it must survive a session round-trip: a restart may not lose it.
    #[test]
    fn a_checkpoint_survives_a_serialization_round_trip() {
        let mut session = chat(6, 16);
        session.install_checkpoint(2, "summary body");
        let json = session.to_json_string().expect("serializes");
        let mut back = AgentSession::new();
        back.restore_from_json(&json).expect("restores");
        assert_eq!(back.messages().len(), session.messages().len());
        assert!(format!("{:?}", back.messages()[0]).contains("summary body"));
    }

    /// More history than the keep window means the command spends a request;
    /// the gate must agree with what the summarizer would be handed.
    #[test]
    fn the_split_is_the_boundary_the_request_starts_at() {
        let session = chat(16, 16);
        let (removed, kept) = session.split_for_compaction(10).expect("an older span");
        assert_eq!(removed.len(), 6);
        assert_eq!(kept.len(), 10);
        assert_eq!(removed.len() + kept.len(), session.messages().len());
    }

    #[test]
    fn estimate_tokens_splits_ascii_and_non_ascii() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2); // 5 ascii → ceil(5/4)
        assert_eq!(estimate_tokens("你好"), 2); // CJK ≈ one token each
        assert_eq!(estimate_tokens("a你"), 2);
        assert_eq!(estimate_tokens("ab你cd"), 2); // 4 ascii → 1, CJK → 1
    }

    #[test]
    fn disabled_budget_never_trims() {
        let mut session = chat(4, 32);
        let budget = ContextBudget {
            max_tokens: 0,
            ..ContextBudget::default()
        };
        assert!(!budget.needs_compaction(&session));
        assert!(budget.apply(&mut session).is_none());
        assert_eq!(session.messages().len(), 4);
    }

    #[test]
    fn generous_budget_leaves_session_untouched() {
        let mut session = chat(6, 16);
        assert!(ContextBudget::default().apply(&mut session).is_none());
        assert_eq!(session.messages().len(), 6);
    }

    #[test]
    fn over_budget_compacts_reports_and_converges() {
        let mut session = chat(20, 400);
        let budget = ContextBudget::with_max_tokens(400);
        assert!(budget.needs_compaction(&session));

        let report = budget.apply(&mut session).expect("budget trips");
        assert!(report.dropped_messages > 0);
        assert!(report.after_tokens < report.before_tokens);
        // The oldest messages collapse into one summary; the recent window
        // plus that summary is what gets sent.
        assert_eq!(session.messages().len(), budget.keep_recent + 1);

        // Further passes converge instead of looping forever.
        let mut passes = 0;
        while budget.apply(&mut session).is_some() {
            passes += 1;
            assert!(passes < 10, "compaction failed to converge");
        }
    }

    /// The trait the engine sees prices the history and can condense it
    /// without a model call, which is what makes a mid-turn budget check cheap.
    #[test]
    fn the_session_prices_itself_and_mechanically_compacts() {
        use react::llm::ReactSession as _;
        let mut session = chat(20, 400);
        let before = session.estimate_tokens().expect("the app prices itself");
        assert!(session.compact_mechanical(4, 1_000));
        let after = session.estimate_tokens().expect("the app prices itself");
        assert!(
            after < before,
            "the mechanical path shrank: {before} -> {after}"
        );
    }

    /// The allocation-free part estimate must equal the joined-text one.
    ///
    /// [`estimate_session`](ContextBudget::estimate_session) runs on every turn,
    /// so it counts text parts in place instead of building the joined string;
    /// this pins the two accountings together.
    #[test]
    fn parts_estimate_equals_the_joined_text_without_joining() {
        let mut session = AgentSession::new();
        session.add_user_content(Content::parts(vec![
            ContentPart::Text { text: "ab".into() },
            ContentPart::Text {
                text: "c你好".into(),
            },
        ]));
        assert_eq!(
            ContextBudget::default().estimate_session(&session),
            estimate_tokens("ab c你好") + 4
        );
    }

    /// A rendered tool call is measured through `fmt`, never a built String.
    #[test]
    fn tool_call_estimate_matches_its_rendered_form() {
        let args = serde_json::json!({"path": "a.txt"});
        let mut session = AgentSession::new();
        session.push(Message::AssistantToolCall {
            tool_call_id: "call-1".into(),
            name: "read_file".into(),
            args: args.clone(),
        });
        assert_eq!(
            ContextBudget::default().estimate_session(&session),
            estimate_tokens(&format!("read_file {args}")) + 4
        );
    }

    #[test]
    fn compaction_never_touches_the_stored_history_semantics() {
        // The summary message carries what was dropped.
        let mut session = chat(10, 200);
        let budget = ContextBudget::with_max_tokens(100);
        let report = budget.apply(&mut session).expect("budget trips");
        // The 2 oldest messages were folded into the summary system message.
        assert_eq!(report.dropped_messages, 10 - budget.keep_recent);
        let summary = session
            .messages()
            .first()
            .and_then(|m| match m {
                Message::System { content } => Some(content.clone()),
                _ => None,
            })
            .expect("summary system message");
        assert!(summary.contains("compacted"));
    }
}
