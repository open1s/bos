//! The chat data model, persisted as JSON under ~/.bos/bsh/sessions/.
//!
//! Framework-free: the runner reads these records, the shell renders them, and
//! nothing here depends on GPUI or on the agent runtime.

use serde::{Deserialize, Serialize};

pub(crate) use agent::goal::Goal;
pub(crate) use agent::tools::{PlanItem, PlanStatus, PlanStore};

/// Who authored a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    /// The human user.
    User,
    /// The assistant.
    Assistant,
}

/// A tool invocation surfaced while streaming a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolEvent {
    /// Tool name reported by the model.
    pub(crate) name: String,
    /// Serialized tool arguments.
    pub(crate) args: String,
    /// Tool output captured when the call finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<String>,
    /// Wall-clock execution time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ms: Option<u64>,
    /// When the call STARTED, in epoch milliseconds. The trajectory's timeline
    /// needs a start time to place a call on a real time axis; a duration alone
    /// cannot say where in the run it happened. Absent on records written before
    /// this field existed, so the timeline falls back to step order for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) at: Option<u64>,
}

/// What one compaction did, as the command reports it.
///
/// The counts are the commit figures the shipped contract reports: how many
/// history items the checkpoint replaced and how many tokens it freed.
#[derive(Debug, Clone)]
pub(crate) struct CompactionReport {
    /// Transcript rows the checkpoint replaced.
    pub(crate) items: usize,
    /// Estimated tokens in the replaced span, the reference's
    /// `shadowedTokenCount` - not the net difference after the summary lands.
    pub(crate) tokens: usize,
    /// The model-written checkpoint text, unframed.
    pub(crate) summary: String,
    /// Whether a human ran the command rather than the automatic path.
    pub(crate) manual: bool,
}

/// One compaction the session has been through, as a dated record.
///
/// The transcript keeps every row it folded - the checkpoint only replaces the
/// MODEL's view - so this is what lets the reader see that a compaction
/// happened, how much history it condensed, and what the model was told in its
/// place. The reference draws exactly this marker at the checkpoint event -
/// after the retained tail - and expands to the same summary text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompactionRecord {
    /// Unix seconds when the compaction finished.
    pub(crate) at: u64,
    /// How many transcript rows the checkpoint replaced.
    pub(crate) items: usize,
    /// Estimated tokens in the replaced span, the reference's
    /// `shadowedTokenCount` the marker reports.
    pub(crate) tokens: usize,
    /// The model-written checkpoint text, unframed, as markdown.
    pub(crate) summary: String,
    /// Whether a human ran `/compact` rather than the automatic pressure path.
    #[serde(default)]
    pub(crate) manual: bool,
    /// The GLOBAL transcript slot the checkpoint event occupied at commit.
    ///
    /// The reference anchors the marker at the checkpoint event's sequence, so
    /// it sits AFTER the retained tail and before any message added later. bsh
    /// persists `loaded_from + messages.len()`-style global slot at commit so a
    /// restart that loads only the tail can still place it; a record written
    /// before this field existed defaults to zero, which draws it at the end
    /// rather than hiding it above a boundary the reader may not be looking at.
    #[serde(default)]
    pub(crate) after: usize,
}

/// One `/compact` command as the transcript draws it.
///
/// The reference logs `command/run` and `command/done`, so its command card is
/// rebuilt after a reload; this is the one command row bsh keeps, with the same
/// lifecycle. A landed compaction clears it, because the checkpoint marker
/// takes its place - exactly the reference's `CompactionCommandCard` rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompactCommand {
    /// Unix seconds when the command started.
    pub(crate) started_at: u64,
    /// The outcome once one exists; absent while the summarizer runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<CompactOutcome>,
}

/// The text one `/compact` command ended with.
///
/// The wording is the command package's own, resolved through the locale at the
/// moment the result lands. `error` is the reference's command state: a
/// `kind: "error"` result draws its summary in the error colour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CompactOutcome {
    /// The outcome line the transcript shows.
    pub(crate) text: String,
    /// Whether the reference draws this as a failed command.
    pub(crate) error: bool,
}

/// A goal transition recorded above a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LogMarker {
    /// What happened.
    pub(crate) kind: String,
    /// Unix seconds when it happened.
    pub(crate) at: u64,
}

/// What one turn cost, as the provider counted it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MessageUsage {
    /// Prompt tokens billed for this turn.
    pub(crate) prompt_tokens: u64,
    /// Completion tokens billed for this turn.
    pub(crate) completion_tokens: u64,
    /// Prompt tokens the provider served from its cache.
    ///
    /// Older session files predate this field, so it defaults to unknown.
    #[serde(default)]
    pub(crate) cached_tokens: u64,
    /// The model this turn ran against.
    ///
    /// Recorded when the usage lands, so a session that switched models keeps
    /// the figure per turn; older session files load it as empty.
    #[serde(default)]
    pub(crate) model: String,
}

impl MessageUsage {
    /// Prompt plus completion.
    pub(crate) fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// What a whole session cost, in the reference's own prompt-side buckets plus
/// the output side.
///
/// The usage pill and its dialog read these. The reference sums its durable
/// `tokenUsage` projection over the whole log, so its cache hit is
/// session-wide rather than the last turn's. Providers bsh talks to report no
/// separate cache-write bucket, which is why that field has no twin here - the
/// reference hides its own row when the count is zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UsageSummary {
    /// Prompt tokens the provider did not serve from cache.
    pub(crate) uncached_input: u64,
    /// Prompt tokens it served from cache.
    pub(crate) cache_read: u64,
    /// Completion tokens.
    pub(crate) output: u64,
}

impl UsageSummary {
    /// Every prompt-side bucket the provider billed for.
    pub(crate) fn billed_input(&self) -> u64 {
        self.uncached_input + self.cache_read
    }

    /// Billed input plus output: the figure the pill counts.
    pub(crate) fn total(&self) -> u64 {
        self.billed_input() + self.output
    }

    /// No turn reported a usage block.
    pub(crate) fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// The reference's fixed-density token heuristic.
///
/// `dsh-token-meter`'s `estimate.js` prices every model-visible node it cannot
/// bill for exactly: four characters to a token, four tokens of JSON framing per
/// content block, and four more for a message's role field. Its
/// `contextBreakdown` is documented as the context's *composition*, not its
/// provider-billed size, which is why the meter prints these figures with a `~`.
pub(crate) const CHARS_PER_TOKEN: usize = 4;
/// Per-block structural overhead for JSON framing.
pub(crate) const BLOCK_OVERHEAD: u64 = 4;
/// Role-field framing overhead added to every priced message.
pub(crate) const ROLE_OVERHEAD: u64 = 4;
/// The heuristic price of one attached image.
///
/// A provider bills an image by its decoded pixels, not by the length of its
/// data URL, so pricing the base64 here would overstate a screenshot by an
/// order of magnitude. A fixed mid-size image price keeps the meter and the
/// compaction trigger from treating an attachment as free.
pub(crate) const IMAGE_TOKENS: u64 = 1_500;

/// The heuristic price of one present content block.
///
/// The reference measures its string lengths in UTF-16 units; a scalar count is
/// the same number for everything below the astral planes, and unlike a byte
/// count it does not double CJK text.
pub(crate) fn price_block(chars: usize) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN) as u64 + BLOCK_OVERHEAD
}

/// The heuristic price of a rendered system prompt: density plus role framing
/// and no per-block overhead, and exactly zero when there is no prompt.
pub(crate) fn price_system_prompt(chars: usize) -> u64 {
    if chars == 0 {
        0
    } else {
        chars.div_ceil(CHARS_PER_TOKEN) as u64 + ROLE_OVERHEAD
    }
}

/// The heuristic price of a request's tool schemas, from the serialized length
/// of the definitions it carries, and zero when it carries none.
pub(crate) fn price_tools(json_chars: usize) -> u64 {
    if json_chars == 0 {
        0
    } else {
        json_chars.div_ceil(CHARS_PER_TOKEN) as u64 + BLOCK_OVERHEAD
    }
}

/// The heuristic composition of the retained context.
///
/// The reference's `contextBreakdown` projection splits the surface it would
/// send into the system prompt, the tool schemas of the latest request header,
/// and every other visible node. These are the same three buckets under the same
/// heuristic, so the meter's bar and legend have the reference's shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ContextBreakdown {
    /// The rendered system prompt.
    pub(crate) system: u64,
    /// The tool definitions the request carries.
    pub(crate) tools: u64,
    /// Every model-visible message, tool calls and their results included.
    pub(crate) messages: u64,
}

impl ContextBreakdown {
    /// Price one retained surface.
    pub(crate) fn price(
        system_chars: usize,
        tools_json_chars: usize,
        messages: &[ChatMessage],
    ) -> Self {
        Self {
            system: price_system_prompt(system_chars),
            tools: price_tools(tools_json_chars),
            messages: messages.iter().map(ChatMessage::priced_tokens).sum(),
        }
    }

    /// Every bucket.
    pub(crate) fn total(&self) -> u64 {
        self.system + self.tools + self.messages
    }

    /// Nothing visible to price.
    pub(crate) fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// One bucket by the reference's legend order: system, tools, messages.
    pub(crate) fn bucket(&self, index: usize) -> u64 {
        match index {
            0 => self.system,
            1 => self.tools,
            _ => self.messages,
        }
    }
}

/// When one turn ran, in wall-clock milliseconds since the epoch.
///
/// The trajectory places every record on a real time axis, so a message needs
/// its own clock: when the turn opened, when the model's first visible output
/// arrived, and how long the turn took once it settled. All three are absent on
/// records written before this existed, which is why the view reports an
/// unavailable duration rather than a fabricated zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MessageTiming {
    /// When the turn opened, i.e. when its user message was submitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<u64>,
    /// When the turn's first visible output - text, reasoning or a tool call -
    /// arrived. The gap from `start` is the model's time to first activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_output: Option<u64>,
    /// How long the turn ran, from `start` to its terminal event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ms: Option<u64>,
}

/// The reader's own notes about one message, and what it cost or took.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct MessageMeta {
    /// A goal transition to draw above this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) marker: Option<LogMarker>,
    /// "good" or "bad"; absent once taken back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) feedback: Option<String>,
    /// What this turn cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) usage: Option<MessageUsage>,
    /// When this turn ran, for the trajectory's time axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timing: Option<MessageTiming>,
    /// The reader stopped this turn before it produced an answer.
    ///
    /// A stopped turn is recorded so the transcript can say so; it is not
    /// replayed into the next request, because an empty assistant row is not a
    /// thing the model said.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) cancelled: bool,
}

/// One skill a prompt invoked, with the instructions the model received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SkillInvocation {
    /// The skill's own name, from its manifest.
    pub(crate) name: String,
    /// The rendered `<skill_content>` block, exactly what the request carried.
    pub(crate) content: String,
}

/// One segment of a turn, in the order the stream produced it.
///
/// A turn interleaves reasoning, visible text and tool calls - the model thinks,
/// writes, calls a tool, then thinks again - and the transcript draws them in
/// that order. Segments are recorded as they land; a session written before
/// this timeline existed carries none and draws in the older
/// reasoning-then-text-then-steps order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TurnPart {
    /// This many bytes of `reasoning`, consumed in order by Thinking parts.
    Thinking(usize),
    /// This many bytes of `text`, consumed in order by Content parts.
    Content(usize),
    /// The tool call at this index of `tools`.
    Tool(usize),
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    /// Author of the message.
    pub(crate) role: Role,
    /// Visible response text.
    pub(crate) text: String,
    /// Reasoning trace, when the model produced one.
    #[serde(default)]
    pub(crate) reasoning: String,
    /// Tool calls made while producing the response.
    #[serde(default)]
    pub(crate) tools: Vec<ToolEvent>,
    /// The order this turn's thinking, text and tool calls arrived in.
    ///
    /// Empty for a session written before the timeline existed, and for user
    /// messages: the transcript then draws reasoning, text and steps in the
    /// order the older pane did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) parts: Vec<TurnPart>,
    /// Attached images, each a data URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<String>,
    /// Error description when the turn failed.
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// Reader notes: marker, feedback, usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) meta: Option<MessageMeta>,
    /// Skills this prompt invoked, with the rendered block the request carried.
    ///
    /// Display only: the model received each block once, and a later turn
    /// replays the reader's own words (the skill is loaded again when it is
    /// invoked again). The transcript draws the block here so what the model
    /// saw is what the bubble shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) skills: Vec<SkillInvocation>,
    /// Whether this is a mid-turn STEER rather than the turn's own prompt.
    ///
    /// A steered message is a real user turn in the model history - it sits
    /// between the assistant text before it and the continuation after it,
    /// because that is the order the running turn took them. But it does not
    /// OPEN a turn: the rail, the trajectory's turn numbers, the audit's turn
    /// grouping, the usage count and the diff's turn ordinal all belong to the
    /// prompt that started the turn. The flag is what tells those readers
    /// "inside this turn", and it is persisted so a relaunch groups the
    /// transcript the same way.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) steering: bool,
}

/// Skip a false flag in the stored session; see [`ChatMessage::steering`].
fn is_false(value: &bool) -> bool {
    !*value
}

impl ChatMessage {
    /// An empty user message.
    pub(crate) fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
            reasoning: String::new(),
            tools: Vec::new(),
            parts: Vec::new(),
            images: Vec::new(),
            error: None,
            meta: None,
            skills: Vec::new(),
            steering: false,
        }
    }

    /// A user message injected into a turn already running.
    pub(crate) fn steer(text: impl Into<String>) -> Self {
        Self {
            steering: true,
            ..Self::user(text)
        }
    }

    /// An empty assistant message that streaming fills in.
    pub(crate) fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            text: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            parts: Vec::new(),
            images: Vec::new(),
            error: None,
            meta: None,
            skills: Vec::new(),
            steering: false,
        }
    }

    /// Fold a reasoning delta into the timeline, extending the Thinking segment
    /// the stream is still writing.
    pub(crate) fn note_thinking(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        match self.parts.last_mut() {
            Some(TurnPart::Thinking(len)) => *len += bytes,
            _ => self.parts.push(TurnPart::Thinking(bytes)),
        }
    }

    /// Fold a visible-text delta into the timeline the same way.
    pub(crate) fn note_content(&mut self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        match self.parts.last_mut() {
            Some(TurnPart::Content(len)) => *len += bytes,
            _ => self.parts.push(TurnPart::Content(bytes)),
        }
    }

    /// Note a tool call at its place in the timeline.
    pub(crate) fn note_tool(&mut self) {
        let index = self.tools.len().saturating_sub(1);
        self.parts.push(TurnPart::Tool(index));
    }

    /// This message's turn timing, when the record carries one.
    pub(crate) fn timing(&self) -> Option<&MessageTiming> {
        self.meta.as_ref()?.timing.as_ref()
    }

    /// The timing block for this message, created when it is absent.
    pub(crate) fn timing_mut(&mut self) -> &mut MessageTiming {
        self.meta
            .get_or_insert_with(MessageMeta::default)
            .timing
            .get_or_insert_with(MessageTiming::default)
    }

    /// Heuristic tokens this message contributes to the context.
    ///
    /// One price per present content block under the reference's heuristic: the
    /// visible text is a text block, each tool call is a `tool-call` block, and
    /// each finished call's output stands for the tool result the next request
    /// would carry. Reasoning is NOT priced: the engine keeps it for the
    /// transcript but does not send it back, so charging it here would make
    /// every turn look larger than the request it sends. An image is priced at
    /// the fixed [`IMAGE_TOKENS`] estimate. The role field is priced once.
    pub(crate) fn priced_tokens(&self) -> u64 {
        #[cfg(test)]
        PRICED_MESSAGES.with(|priced| priced.set(priced.get() + 1));
        let mut tokens = ROLE_OVERHEAD;
        if !self.text.is_empty() {
            tokens += price_block(self.text.chars().count());
        }
        tokens += IMAGE_TOKENS * self.images.len() as u64;
        for tool in &self.tools {
            tokens += price_block(tool.name.chars().count());
            tokens += price_block(tool.args.chars().count());
            if let Some(output) = &tool.output {
                if !output.is_empty() {
                    tokens += price_block(output.chars().count());
                }
            }
        }
        tokens
    }
}

// How many messages this thread has priced, for the context-meter test.
#[cfg(test)]
thread_local! {
    static PRICED_MESSAGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The messages priced so far on this thread.
#[cfg(test)]
pub(crate) fn priced_messages() -> usize {
    PRICED_MESSAGES.with(|priced| priced.get())
}

/// Persisted metadata for a child session spawned by a subagent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubagentMeta {
    /// Root conversation this run belongs to.
    pub(crate) root_id: String,
    /// Direct parent run id, or the root id for a top-level child.
    pub(crate) parent_id: String,
    /// How deep below the root this run sits.
    pub(crate) depth: usize,
    /// Running, done or failed.
    pub(crate) status: String,
    /// The task handed to the child.
    pub(crate) prompt: String,
    /// Token estimate while usage is unavailable.
    pub(crate) estimated_tokens: u64,
    /// Prompt tokens the child reported.
    pub(crate) prompt_tokens: u64,
    /// Completion tokens the child reported.
    pub(crate) completion_tokens: u64,
    /// Epoch milliseconds when the run started.
    pub(crate) started_at: u64,
    /// Epoch milliseconds when it settled.
    pub(crate) finished_at: Option<u64>,
    /// Final answer, when done.
    pub(crate) result: Option<String>,
    /// Failure text, when failed.
    pub(crate) error: Option<String>,
}

/// One chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    /// Stable identity, also the file name.
    pub(crate) id: String,
    /// Display title (the first prompt, usually).
    pub(crate) title: String,
    /// Unix seconds the session was created.
    pub(crate) created_at: u64,
    /// Unix seconds of the last change.
    pub(crate) updated_at: u64,
    /// Owning workspace id.
    #[serde(default)]
    pub(crate) workspace: String,
    /// The provider this session runs under, as a reference into the global
    /// provider tree. `None` means this chat names no model - there is no
    /// workspace default to take - and the composer refuses to send until one
    /// is chosen here.
    ///
    /// A reference, never a base URL or a key: the credential belongs to the
    /// provider node, so rotating it must not have to rewrite every session.
    #[serde(default)]
    pub(crate) provider: Option<String>,
    /// The model nickname inside that provider.
    ///
    /// The pair is the session's explicit choice and the ONLY model selection,
    /// written only when the composer picks a model in THIS chat. It is
    /// deliberately not copied into a new session: a fresh chat starts with no
    /// model and asks for one instead of inheriting a neighbour's.
    #[serde(default)]
    pub(crate) model: Option<String>,
    /// The preset this session runs, recorded when it is selected.
    ///
    /// A creation fact, and it outlives the blank window: the first turn and
    /// every turn after it run under this composition, so it has to be
    /// reconstructible from the session alone. `None` takes the user's default.
    #[serde(default)]
    pub(crate) agent_preset: Option<String>,
    /// Child-run metadata; absent on a normal conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) subagent: Option<SubagentMeta>,
    /// The transcript.
    #[serde(default)]
    pub(crate) messages: Vec<ChatMessage>,
    /// The agent's working plan.
    #[serde(default)]
    pub(crate) plan: Vec<PlanItem>,
    /// The session goal.
    #[serde(default)]
    pub(crate) goal: Option<Goal>,
    /// Whether this conversation is in plan mode.
    ///
    /// A SESSION fact, not a global preference: it changes what the next turn
    /// asks the model to do - inspect and plan first, act only after approval -
    /// and it travels with the session the way the model choice does.
    #[serde(default)]
    pub(crate) plan_mode: bool,
    /// The GLOBAL transcript index where the last compaction cut the context.
    ///
    /// Counted from the start of the session's log, not from the loaded window:
    /// a restart loads only the tail (`loaded_from`) and paging moves the
    /// window, so a local index would point at a different message after
    /// either. `request_transcript` maps this into the window. Zero means
    /// nothing has been elided. The rows stay on screen; only the request
    /// leaves them out and carries the checkpoint summary in their place.
    #[serde(default)]
    pub(crate) compacted: usize,
    /// The reader's note about this session, recorded by the Feedback command.
    #[serde(default)]
    pub(crate) feedback: Option<String>,
    /// Pinned to the top of its workspace.
    #[serde(default)]
    pub(crate) pinned: bool,
    /// Unix seconds the session was archived, if it was.
    #[serde(default)]
    pub(crate) archived_at: Option<u64>,
    /// Messages moved out by compaction.
    #[serde(default)]
    pub(crate) archived: Vec<serde_json::Value>,
    /// Every compaction this session has been through, oldest first.
    ///
    /// The transcript keeps the folded rows, so this is the record of what
    /// replaced them: the marker row the reader expands, and the trail the
    /// Trajectory tab lists. Empty means the session has never been compacted.
    #[serde(default)]
    pub(crate) compactions: Vec<CompactionRecord>,
    /// The `/compact` command whose progress or outcome the transcript still
    /// shows.
    ///
    /// The reference persists `command/run` and `command/done` in the session
    /// log and replays the command card after a reload; bsh keeps the one card
    /// this crate draws. A landed compaction clears it, because the checkpoint
    /// marker replaces it. Absent on sessions that never ran the command.
    #[serde(default)]
    pub(crate) compact_command: Option<CompactCommand>,
    /// The global index of messages[0] in the session's Parquet history.
    ///
    /// A paged session holds a WINDOW of its transcript in memory, so the
    /// document and the log disagree about where the first row is until this
    /// says where the window starts. Runtime state, never written: the log on
    /// disk is the transcript, the document is metadata.
    #[serde(skip)]
    pub(crate) loaded_from: usize,
    /// How far back the log has been read: the index ABOVE the oldest row this
    /// session has already paged in.
    ///
    /// It is not the same fact as loaded_from. That one says where the window in
    /// MEMORY starts, and it moves forward when old rows are dropped to bound
    /// memory. This one only moves DOWN toward zero, so it is what answers "is
    /// there more history above" - using the window's start for that would stop
    /// the reader at the top of the loaded window instead of the top of the chat.
    #[serde(skip)]
    pub(crate) paged_from: Option<usize>,
    /// Whether the full document has been read.
    ///
    /// Startup reads only metadata into each row; the plan, goal and archived
    /// values are filled by `Store::ensure_document` when a session is selected
    /// or mutated. A metadata-only row never rewrites those fields from empty.
    #[serde(skip)]
    pub(crate) loaded: bool,
}

impl Session {
    /// A fresh, empty session in the given workspace.
    pub(crate) fn new(workspace: &str, title: impl Into<String>) -> Self {
        let at = crate::state::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            created_at: at,
            updated_at: at,
            workspace: workspace.to_string(),
            provider: None,
            model: None,
            agent_preset: None,
            subagent: None,
            messages: Vec::new(),
            plan: Vec::new(),
            goal: None,
            plan_mode: false,
            compacted: 0,
            feedback: None,
            pinned: false,
            archived_at: None,
            archived: Vec::new(),
            compactions: Vec::new(),
            compact_command: None,
            loaded_from: 0,
            paged_from: None,
            loaded: true,
        }
    }

    /// What this session contributes to resolution.
    ///
    /// A half-made model reference is no reference: a provider with no model
    /// would resolve to nothing anyway, so it is reported as unchosen rather
    /// than sent on to fail as an unknown name.
    ///
    /// This is the session's explicit choice, not a record of what a turn ran
    /// against: the per-turn record lives on `MessageUsage::model`, because a
    /// session may switch models between turns.
    pub(crate) fn choice(&self) -> crate::settings::resolve::SessionChoice {
        let named = match (self.provider.as_deref(), self.model.as_deref()) {
            (Some(provider), Some(model))
                if !provider.trim().is_empty() && !model.trim().is_empty() =>
            {
                Some(crate::settings::global::ModelRef {
                    provider: provider.to_string(),
                    model: model.to_string(),
                })
            }
            _ => None,
        };
        crate::settings::resolve::SessionChoice {
            model: named,
            preset: self.agent_preset.clone().filter(|id| !id.trim().is_empty()),
            // The owning workspace, so a turn opened from another workspace
            // still runs in the project this chat was created in.
            workspace: Some(self.workspace.clone()).filter(|id| !id.trim().is_empty()),
        }
    }

    /// The title derived from the first prompt, else the localized default.
    pub(crate) fn title_from(text: &str, lang: crate::i18n::Lang) -> String {
        let line = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        let title: String = line.chars().take(48).collect();
        if title.is_empty() {
            crate::i18n::tr(lang, "row.newChat")
        } else {
            title
        }
    }

    /// Number of user-and-assistant turns.
    ///
    /// A steering row is part of the turn around it, not a turn of its own.
    pub(crate) fn turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == Role::User && !m.steering)
            .count()
    }

    /// Number of tool calls across the transcript.
    pub(crate) fn step_count(&self) -> usize {
        self.messages.iter().map(|m| m.tools.len()).sum()
    }

    /// Sum every turn's usage into the session-wide buckets the usage pill and
    /// its dialog read.
    pub(crate) fn usage_summary(&self) -> UsageSummary {
        let mut summary = UsageSummary::default();
        for usage in self
            .messages
            .iter()
            .filter_map(|message| message.meta.as_ref().and_then(|meta| meta.usage.as_ref()))
        {
            let cached = usage.cached_tokens.min(usage.prompt_tokens);
            summary.uncached_input += usage.prompt_tokens - cached;
            summary.cache_read += cached;
            summary.output += usage.completion_tokens;
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::{
        price_block, price_system_prompt, price_tools, ChatMessage, ContextBreakdown, MessageUsage,
        Session, ToolEvent, TurnPart, UsageSummary, IMAGE_TOKENS, ROLE_OVERHEAD,
    };
    /// The timeline keeps the ORDER the stream produced, merging only runs of
    /// the same kind: thinking, the reply, a tool, then thinking again is four
    /// segments, not three buckets.
    #[test]
    fn the_timeline_orders_and_merges_stream_segments() {
        let mut message = ChatMessage::assistant();
        message.note_thinking(4);
        message.note_thinking(3);
        assert_eq!(message.parts, vec![TurnPart::Thinking(7)], "a run merges");
        message.note_content(2);
        message.tools.push(ToolEvent {
            name: "bash".to_string(),
            args: "{}".to_string(),
            output: None,
            ms: None,
            at: None,
        });
        message.note_tool();
        message.note_thinking(5);
        message.note_content(1);
        message.note_thinking(0);
        assert_eq!(
            message.parts,
            vec![
                TurnPart::Thinking(7),
                TurnPart::Content(2),
                TurnPart::Tool(0),
                TurnPart::Thinking(5),
                TurnPart::Content(1),
            ],
            "thinking, content and the tool keep their arrival order"
        );
    }

    /// A message written before the timeline round-trips with no segments, so
    /// an old session file loads and draws in the older shape.
    #[test]
    fn a_message_without_a_timeline_still_loads() {
        let message: ChatMessage =
            serde_json::from_str(r#"{"role":"assistant","text":"kept"}"#).expect("an old message");
        assert_eq!(message.text, "kept");
        assert!(message.parts.is_empty());
    }

    /// A session file written before the model was recorded still loads.
    #[test]
    fn usage_without_a_model_loads_as_empty() {
        let usage: MessageUsage =
            serde_json::from_str(r#"{"prompt_tokens":7,"completion_tokens":3}"#)
                .expect("old usage");
        assert_eq!(usage.prompt_tokens, 7);
        assert_eq!(usage.cached_tokens, 0);
        assert_eq!(usage.model, "");
        assert_eq!(usage.total(), 10);
    }

    /// The summary bills the prompt side plus the output side.
    #[test]
    fn the_usage_summary_bills_input_plus_output() {
        let summary = UsageSummary {
            uncached_input: 1,
            cache_read: 199,
            output: 50,
        };
        assert_eq!(summary.billed_input(), 200);
        assert_eq!(summary.total(), 250);
        assert!(!summary.is_empty());
        assert!(UsageSummary::default().is_empty());
    }

    /// And the model survives a round trip once it is recorded.
    #[test]
    fn usage_keeps_the_model_it_ran_against() {
        let usage = MessageUsage {
            prompt_tokens: 1,
            completion_tokens: 2,
            cached_tokens: 0,
            model: "deepseek-chat".to_string(),
        };
        let text = serde_json::to_string(&usage).expect("serialize");
        let back: MessageUsage = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(back.model, "deepseek-chat");
    }

    /// The heuristic prices a block, a prompt and a tool array the way the
    /// reference's own `estimate.js` does: four characters to a token, four
    /// tokens of framing, and zero for content that is not there.
    #[test]
    fn the_context_heuristic_prices_like_the_reference() {
        assert_eq!(price_block(0), 4);
        assert_eq!(price_block(4), 5);
        // 5 characters is two tokens, not one: the reference takes the ceiling.
        assert_eq!(price_block(5), 6);
        // An absent prompt or tool array prices at zero rather than at its
        // framing, which is what `estimateSystemMessage` and
        // `estimateToolsTokens` return for empty input.
        assert_eq!(price_system_prompt(0), 0);
        assert_eq!(price_system_prompt(4), 5);
        assert_eq!(price_tools(0), 0);
        assert_eq!(price_tools(8), 6);
    }

    /// The breakdown splits one retained surface into the legend's three
    /// buckets, with the role field priced once per message.
    #[test]
    fn the_context_breakdown_splits_the_surface_into_three_buckets() {
        let user = ChatMessage::user("abcd");
        let assistant = {
            let mut message = ChatMessage::assistant();
            message.text = "abcdefgh".to_string();
            message.tools.push(ToolEvent {
                name: "bash".to_string(),
                args: "{}".to_string(),
                output: Some("ok".to_string()),
                ms: None,
                at: None,
            });
            message
        };
        // user: role 4 + text (1 + 4); assistant: role 4 + text (2 + 4)
        // + name (1 + 4) + args (1 + 4) + output (1 + 4).
        assert_eq!(user.priced_tokens(), 9);
        assert_eq!(assistant.priced_tokens(), 25);
        let breakdown = ContextBreakdown::price(8, 0, &[user, assistant]);
        assert_eq!(breakdown.system, 6);
        assert_eq!(breakdown.tools, 0);
        assert_eq!(breakdown.messages, 34);
        assert_eq!(breakdown.total(), 40);
        assert_eq!(breakdown.bucket(0), breakdown.system);
        assert_eq!(breakdown.bucket(1), breakdown.tools);
        assert_eq!(breakdown.bucket(2), breakdown.messages);
        assert!(!breakdown.is_empty());
        assert!(ContextBreakdown::default().is_empty());
    }

    /// Reasoning is kept for the transcript but never sent back, so charging
    /// it would make every request look larger than it is.
    #[test]
    fn reasoning_is_not_priced_into_the_request() {
        let mut message = ChatMessage::assistant();
        message.text = "abcd".to_string();
        message.reasoning = "x".repeat(4_000);
        assert_eq!(message.priced_tokens(), ROLE_OVERHEAD + price_block(4));
    }

    /// An attachment is billed by the provider, so it is not free here either;
    /// its data URL length is not its token price, hence the fixed estimate.
    #[test]
    fn an_attached_image_is_priced_at_the_fixed_estimate() {
        let mut message = ChatMessage::user("abcd");
        message
            .images
            .push("data:image/png;base64,AAAA".to_string());
        assert_eq!(
            message.priced_tokens(),
            ROLE_OVERHEAD + price_block(4) + IMAGE_TOKENS
        );
    }

    #[test]
    fn a_session_file_written_before_the_reference_fields_still_loads() {
        let json = r#"{"id":"a","title":"t","created_at":1,"updated_at":2,"workspace":"w"}"#;
        let session: Session = serde_json::from_str(json).expect("an older file loads");
        assert_eq!(session.provider, None);
        assert_eq!(session.model, None);
        assert_eq!(session.agent_preset, None);
        // The provider/model/preset are what an older file may lack; the
        // workspace is a fact the file still names, and the choice carries it
        // so a chat opened from another workspace keeps running in its own.
        let choice = session.choice();
        assert_eq!(choice.model, None);
        assert_eq!(choice.preset, None);
        assert_eq!(choice.workspace.as_deref(), Some("w"));
    }

    #[test]
    fn a_half_made_model_reference_is_not_a_choice() {
        let mut session = Session::new("w", "t");
        session.provider = Some("nvidia".to_string());
        assert_eq!(
            session.choice().model,
            None,
            "a provider with no model names nothing, so it is not sent on to \
             fail as an unknown name"
        );
        session.model = Some("glm-5.3-flash".to_string());
        assert_eq!(
            session.choice().model.map(|m| m.model),
            Some("glm-5.3-flash".to_string())
        );
    }

    #[test]
    fn a_blank_preset_id_is_no_preset() {
        let mut session = Session::new("w", "t");
        session.agent_preset = Some("  ".to_string());
        assert_eq!(
            session.choice().preset,
            None,
            "the session passes the id on; resolve() is what applies the default"
        );
    }
}
