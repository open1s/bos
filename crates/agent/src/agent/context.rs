//! ReAct session and context implementations plus the message accumulator.

use crate::agent::hooks::HookRegistry;
use crate::agent::plugin::PluginRegistry;
use crate::OpenAiMessage;
use react::engine::ReactError;
use react::llm::types::{ReactContext, ReactSession};
use react::llm::vendor::ToolCall;
use react::llm::{
    Content, ContentPart, Instruction, LlmMessage as Message, LlmRequest as ReactLlmRequest,
    LlmResponse as ReactLlmResponse, LlmTool, Rule, Skill,
};
use react::runtime::app::ReActApp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::future::Future;
use std::sync::Arc;

fn content_to_string(content: &Content) -> String {
    match content {
        Content::Text(s) => s.clone(),
        Content::Parts(parts) => serde_json::to_string(parts).unwrap_or_default(),
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

    /// Append a system message.
    pub fn add_system(&mut self, profile: String) {
        self.messages.push(Message::System { content: profile });
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

    /// Borrow the conversation history.
    pub fn history_ref(&self) -> &[Message] {
        &self.messages
    }

    /// Render the history as OpenAI chat messages.
    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    /// Append the history to `target` as OpenAI chat messages.
    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            match message {
                Message::User { content } => {
                    target.push(OpenAiMessage {
                        role: "user".to_string(),
                        content: Some(content_to_string(content)),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::System { content } => {
                    target.push(OpenAiMessage {
                        role: "system".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::Assistant { content } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: None,
                        tool_calls: Some(vec![ToolCall {
                            id: tool_call_id.clone(),
                            r#type: "function".to_string(),
                            function: react::llm::vendor::FunctionCall {
                                name: Some(name.clone()),
                                arguments: Some(args.to_string()),
                            },
                        }]),
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::ToolResult {
                    tool_call_id: _,
                    content,
                } => {
                    target.push(OpenAiMessage {
                        role: "tool".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
            }
        }
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

    /// Summarize all but the most recent `keep_recent` messages and prepend a summary.
    pub fn compact(&mut self, keep_recent: usize, max_summary_chars: usize) {
        if self.messages.len() <= keep_recent {
            return;
        }

        let split_at = self.messages.len().saturating_sub(keep_recent);
        let removed = &self.messages[..split_at];
        let recent = self.messages[split_at..].to_vec();

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
        let mut compacted = vec![summary_message];
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
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let other = text.chars().count().saturating_sub(ascii);
    if ascii + other == 0 {
        0
    } else {
        ascii.div_ceil(4) + other
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
                let body = match msg {
                    Message::System { content } => content.clone(),
                    Message::User { content } => content_to_summary_string(content),
                    Message::Assistant { content } => content.clone(),
                    Message::AssistantToolCall { name, args, .. } => format!("{name} {args}"),
                    Message::ToolResult { content, .. } => content.clone(),
                };
                // +4 per message covers role framing and message delimiters.
                estimate_tokens(&body) + 4
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

/// AgentReActApp integrates the Agent's hooks, plugins, and configuration with the ReAct engine.
/// This allows the agent to intercept and react to events during the ReAct loop.
#[derive(Default)]
pub struct AgentReActApp {
    hooks: Arc<HookRegistry>,
    plugins: Arc<PluginRegistry>,
    agent_name: String,
}

impl AgentReActApp {
    /// Create an app bridging `hooks` and `plugins` into the ReAct loop.
    pub fn new(hooks: Arc<HookRegistry>, plugins: Arc<PluginRegistry>, agent_name: String) -> Self {
        Self {
            hooks,
            plugins,
            agent_name,
        }
    }
}

impl ReActApp for AgentReActApp {
    type Session = AgentSession;
    type Context = AgentReactContext;

    fn name(&self) -> &str {
        &self.agent_name
    }

    #[allow(refining_impl_trait)]
    fn before_llm_call(
        &self,
        req: &mut ReactLlmRequest,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = react::runtime::HookDecision> + Send {
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
        let result_text = result.as_ref().map(|v| v.to_string()).unwrap_or_default();
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

    /// Append a system message.
    pub fn add_system(&mut self, profile: String) {
        self.messages.push(Message::System { content: profile });
    }

    /// Append an assistant message.
    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
    }

    /// Append `chunk` to the trailing assistant message, or start one.
    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        match self.messages.last_mut() {
            Some(Message::Assistant { content }) => content.push_str(chunk),
            _ => self.messages.push(Message::Assistant {
                content: chunk.to_string(),
            }),
        }
    }

    /// Record an assistant tool call.
    pub fn add_tool_call(&mut self, tool_call_id: String, name: String, args: serde_json::Value) {
        self.messages.push(Message::AssistantToolCall {
            tool_call_id,
            name,
            args,
        });
    }

    /// Record a tool result.
    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult {
            tool_call_id: name,
            content,
        });
    }

    /// Render the history as OpenAI chat messages.
    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    /// Append the history to `target` as OpenAI chat messages.
    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            match message {
                Message::User { content } => {
                    target.push(OpenAiMessage {
                        role: "user".to_string(),
                        content: Some(content_to_string(content)),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::System { content } => {
                    target.push(OpenAiMessage {
                        role: "system".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::Assistant { content } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => {
                    target.push(OpenAiMessage {
                        role: "assistant".to_string(),
                        content: None,
                        tool_calls: Some(vec![ToolCall {
                            id: tool_call_id.clone(),
                            r#type: "function".to_string(),
                            function: react::llm::vendor::FunctionCall {
                                name: Some(name.clone()),
                                arguments: Some(args.to_string()),
                            },
                        }]),
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
                Message::ToolResult {
                    tool_call_id: _,
                    content,
                } => {
                    target.push(OpenAiMessage {
                        role: "tool".to_string(),
                        content: Some(content.clone()),
                        tool_calls: None,
                        function_call: None,
                        reasoning_content: None,
                        extra: serde_json::Value::Object(serde_json::Map::new()),
                    });
                }
            }
        }
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
