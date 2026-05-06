use crate::agent::hooks::HookRegistry;
use crate::agent::plugin::PluginRegistry;
use crate::OpenAiMessage;
use react::engine::ReactError;
use react::llm::types::{ReactContext, ReactSession};
use react::llm::vendor::ToolCall;
use react::llm::{
    Instruction, LlmMessage as Message, LlmRequest as ReactLlmRequest,
    LlmResponse as ReactLlmResponse, LlmTool, Rule, Skill,
};
use react::runtime::app::ReActApp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::future::Future;
use std::sync::Arc;

/// AgentSession stores conversation history and session state for the ReAct engine.
/// Implements ReactSession trait for integration with the react crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    messages: Vec<Message>,
    context: JsonValue,
    metadata: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
}

impl AgentSession {
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

    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User { content });
        self.update_metadata();
    }

    pub fn add_system(&mut self, profile: String) {
        self.messages.push(Message::System { content: profile });
        self.update_metadata();
    }

    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
        self.update_metadata();
    }

    fn update_metadata(&mut self) {
        self.metadata.updated_at = current_timestamp();
        self.metadata.message_count = self.messages.len();
    }

    pub fn take_messages(&mut self) -> Vec<Message> {
        let msgs = std::mem::take(&mut self.messages);
        self.update_metadata();
        msgs
    }

    pub fn restore_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.update_metadata();
    }

    pub fn history_ref(&self) -> &[Message] {
        &self.messages
    }

    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            match message {
                Message::User { content } => {
                    target.push(OpenAiMessage {
                        role: "user".to_string(),
                        content: Some(content.clone()),
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

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn push(&mut self, msg: Message) {
        self.messages.push(msg);
        self.update_metadata();
    }

    pub fn session_context(&self) -> JsonValue {
        self.context.clone()
    }

    pub fn set_session_context(&mut self, context: JsonValue) {
        self.context = context;
    }

    pub fn clear_session_context(&mut self) {
        self.context = JsonValue::Null;
    }

    pub fn save(&self, path: &str) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    pub fn restore(&mut self, path: &str) -> Result<(), std::io::Error> {
        let json = std::fs::read_to_string(path)?;
        self.restore_from_json(&json)
    }

    pub fn restore_from_json(&mut self, json: &str) -> Result<(), std::io::Error> {
        let restored: AgentSession = serde_json::from_str(json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        self.messages = restored.messages;
        self.context = restored.context;
        self.metadata = restored.metadata;
        Ok(())
    }

    pub fn to_json_string(&self) -> Result<String, std::io::Error> {
        serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    pub fn clear(&mut self) {
        self.messages.retain(|msg| matches!(msg, Message::System { .. }));
        self.context = JsonValue::Null;
        self.metadata.updated_at = current_timestamp();
    }

    pub fn compact(&mut self, keep_recent: usize, max_summary_chars: usize) {
        if self.messages.len() <= keep_recent {
            return;
        }

        let split_at = self.messages.len().saturating_sub(keep_recent);
        let removed = &self.messages[..split_at];
        let recent = self.messages[split_at..].to_vec();

        let summary_input: String = removed
            .iter()
            .filter_map(|msg| match msg {
                Message::System { content }
                | Message::User { content }
                | Message::Assistant { content } => Some(content.clone()),
                Message::AssistantToolCall { name, args, .. } => {
                    Some(format!("Tool call {}: {}", name, args))
                }
                Message::ToolResult { content, .. } => Some(content.clone()),
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

    fn history(&self) -> Option<Vec<Message>> {
        Some(self.messages.clone())
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
#[derive(Debug, Clone, Default)]
pub struct AgentReactContext {
    pub session_id: String,
    pub tools: Vec<LlmTool>,
    pub skills: Vec<Skill>,
    pub rules: Vec<Rule>,
    pub instructions: Vec<Instruction>,
}

impl AgentReactContext {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            tools: Vec::new(),
            skills: Vec::new(),
            rules: Vec::new(),
            instructions: Vec::new(),
        }
    }

    pub fn with_tools(mut self, tools: Vec<LlmTool>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }
}

impl ReactContext for AgentReactContext {
    fn session_id(&self) -> String {
        self.session_id.clone()
    }

    fn skills(&self) -> Option<Vec<Skill>> {
        if self.skills.is_empty() {
            None
        } else {
            Some(self.skills.clone())
        }
    }

    fn tools(&self) -> Option<Vec<LlmTool>> {
        if self.tools.is_empty() {
            None
        } else {
            Some(self.tools.clone())
        }
    }

    fn rules(&self) -> Option<Vec<Rule>> {
        if self.rules.is_empty() {
            None
        } else {
            Some(self.rules.clone())
        }
    }

    fn instructions(&self) -> Option<Vec<Instruction>> {
        if self.instructions.is_empty() {
            None
        } else {
            Some(self.instructions.clone())
        }
    }

    fn add_tool(&mut self, tool: LlmTool) {
        self.tools.push(tool);
    }
}

/// AgentReActApp integrates the Agent's hooks, plugins, and configuration with the ReAct engine.
/// This allows the agent to intercept and react to events during the ReAct loop.
pub struct AgentReActApp {
    hooks: Arc<HookRegistry>,
    agent_name: String,
}

impl AgentReActApp {
    pub fn new(
        hooks: Arc<HookRegistry>,
        _plugins: Arc<PluginRegistry>,
        agent_name: String,
    ) -> Self {
        Self { hooks, agent_name }
    }
}

impl Default for AgentReActApp {
    fn default() -> Self {
        Self {
            hooks: Arc::new(HookRegistry::new()),
            agent_name: "agent".to_string(),
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
    ) -> impl Future<Output = react::runtime::HookDecision> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let model = req.model.clone();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("model", &model);
            self.hooks
                .trigger(crate::agent::hooks::HookEvent::BeforeLlmCall, ctx)
                .await
        }
    }

    #[allow(refining_impl_trait)]
    fn after_llm_response(
        &self,
        _response: &mut ReactLlmResponse,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send + '_ {
        let agent_name = self.agent_name.clone();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("response_type", "react");
            let _ = self
                .hooks
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
    ) -> impl Future<Output = ()> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let response_text = response_text.to_string();
        let had_tool_call = had_tool_call;
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("response_type", "stream");
            ctx.set("response_text", &response_text);
            ctx.set("had_tool_call", &had_tool_call.to_string());
            let _ = self
                .hooks
                .trigger(crate::agent::hooks::HookEvent::AfterLlmCall, ctx)
                .await;
        }
    }

    #[allow(refining_impl_trait)]
    fn before_tool_call(
        &self,
        tool_name: &str,
        args: &mut JsonValue,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = react::runtime::HookDecision> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let tool_name = tool_name.to_string();
        let args_json = args.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("tool_name", &tool_name);
            ctx.set("tool_args", &args_json);
            self.hooks
                .trigger(crate::agent::hooks::HookEvent::BeforeToolCall, ctx)
                .await
        }
    }

    #[allow(refining_impl_trait)]
    fn after_tool_result(
        &self,
        tool_name: &str,
        result: &mut Result<JsonValue, ReactError>,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let tool_name = tool_name.to_string();
        let result_text = result.as_ref().map(|v| v.to_string()).unwrap_or_default();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("tool_name", &tool_name);
            ctx.set("tool_result", &result_text);
            let _ = self
                .hooks
                .trigger(crate::agent::hooks::HookEvent::AfterToolCall, ctx)
                .await;
        }
    }

    #[allow(refining_impl_trait)]
    fn on_thought(
        &self,
        thought: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let thought = thought.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("thought", &thought);
            let _ = self
                .hooks
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
    ) -> impl Future<Output = ()> + Send + '_ {
        let agent_name = self.agent_name.clone();
        let answer = answer.to_string();
        async move {
            let mut ctx = crate::agent::hooks::HookContext::new(&agent_name);
            ctx.set("answer", &answer);
            let _ = self
                .hooks
                .trigger(crate::agent::hooks::HookEvent::OnComplete, ctx)
                .await;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MessageContext {
    pub(crate) messages: Vec<Message>,
}

impl MessageContext {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User { content });
    }

    pub fn add_system(&mut self, profile: String) {
        self.messages.push(Message::System { content: profile });
    }

    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant { content });
    }

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        match self.messages.last_mut() {
            Some(Message::Assistant { content }) => content.push_str(chunk),
            _ => self.messages.push(Message::Assistant {
                content: chunk.to_string(),
            }),
        }
    }

    pub fn add_tool_call(&mut self, tool_call_id: String, name: String, args: serde_json::Value) {
        self.messages.push(Message::AssistantToolCall {
            tool_call_id,
            name,
            args,
        });
    }

    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult {
            tool_call_id: name,
            content,
        });
    }

    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        let mut api_messages = Vec::with_capacity(self.messages.len());
        self.extend_api_format(&mut api_messages);
        api_messages
    }

    pub fn extend_api_format(&self, target: &mut Vec<OpenAiMessage>) {
        target.reserve(self.messages.len());
        for message in &self.messages {
            match message {
                Message::User { content } => {
                    target.push(OpenAiMessage {
                        role: "user".to_string(),
                        content: Some(content.clone()),
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

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}
