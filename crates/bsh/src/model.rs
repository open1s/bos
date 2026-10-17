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

/// The reader's own notes about one message.
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
    /// Attached images, each a data URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<String>,
    /// Error description when the turn failed.
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// Reader notes: marker, feedback, usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) meta: Option<MessageMeta>,
}

impl ChatMessage {
    /// An empty user message.
    pub(crate) fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
            reasoning: String::new(),
            tools: Vec::new(),
            images: Vec::new(),
            error: None,
            meta: None,
        }
    }

    /// An empty assistant message that streaming fills in.
    pub(crate) fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            text: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            images: Vec::new(),
            error: None,
            meta: None,
        }
    }

    /// Heuristic tokens this message contributes to the context.
    ///
    /// One price per present content block under the reference's heuristic: the
    /// visible text and the reasoning trace are text blocks, each tool call is a
    /// `tool-call` block, and each finished call's output stands for the tool
    /// result the next request would carry. The role field is priced once.
    pub(crate) fn priced_tokens(&self) -> u64 {
        let mut tokens = ROLE_OVERHEAD;
        if !self.text.is_empty() {
            tokens += price_block(self.text.chars().count());
        }
        if !self.reasoning.is_empty() {
            tokens += price_block(self.reasoning.chars().count());
        }
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
    /// provider tree. `None` takes the workspace's default.
    ///
    /// A reference, never a base URL or a key: the credential belongs to the
    /// provider node, so rotating it must not have to rewrite every session.
    #[serde(default)]
    pub(crate) provider: Option<String>,
    /// The model nickname inside that provider. `None` takes the workspace's.
    #[serde(default)]
    pub(crate) model: Option<String>,
    /// The preset this session runs, recorded when it is selected.
    ///
    /// A creation fact, and it outlives the blank window: the first turn and
    /// every turn after it run under this composition, so it has to be
    /// reconstructible from the session alone. `None` takes the user's default.
    #[serde(default)]
    pub(crate) agent_preset: Option<String>,
    /// The transcript.
    #[serde(default)]
    pub(crate) messages: Vec<ChatMessage>,
    /// The agent's working plan.
    #[serde(default)]
    pub(crate) plan: Vec<PlanItem>,
    /// The session goal.
    #[serde(default)]
    pub(crate) goal: Option<Goal>,
    /// Pinned to the top of its workspace.
    #[serde(default)]
    pub(crate) pinned: bool,
    /// Unix seconds the session was archived, if it was.
    #[serde(default)]
    pub(crate) archived_at: Option<u64>,
    /// Messages moved out by compaction.
    #[serde(default)]
    pub(crate) archived: Vec<serde_json::Value>,
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
            messages: Vec::new(),
            plan: Vec::new(),
            goal: None,
            pinned: false,
            archived_at: None,
            archived: Vec::new(),
        }
    }

    /// What this session contributes to resolution.
    ///
    /// A half-made model reference is no reference: a provider with no model
    /// would resolve to nothing anyway, so it is reported as unchosen rather
    /// than sent on to fail as an unknown name.
    ///
    /// In flight: the caps switch is what calls this. Until it lands the
    /// method has no caller, so the allow marks an unfinished migration
    /// rather than a decision.
    #[allow(dead_code)]
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
    pub(crate) fn turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role == Role::User)
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
        Session, ToolEvent, UsageSummary,
    };

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

    #[test]
    fn a_session_file_written_before_the_reference_fields_still_loads() {
        let json = r#"{"id":"a","title":"t","created_at":1,"updated_at":2,"workspace":"w"}"#;
        let session: Session = serde_json::from_str(json).expect("an older file loads");
        assert_eq!(session.provider, None);
        assert_eq!(session.model, None);
        assert_eq!(session.agent_preset, None);
        assert_eq!(
            session.choice(),
            crate::settings::resolve::SessionChoice::default(),
            "a session that names nothing contributes nothing"
        );
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
