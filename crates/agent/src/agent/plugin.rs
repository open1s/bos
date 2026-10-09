//! Plugin hooks that can rewrite LLM requests, responses, tool calls, and
//! stream tokens.

use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use react::llm::Content;

/// Stage at which an LLM plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmStage {
    /// Before the request is sent.
    PreRequest,
    /// After the response is received.
    PostResponse,
}

/// Stage at which a tool plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolStage {
    /// Before the tool runs.
    PreExecute,
    /// After the tool returns.
    PostExecute,
}

/// A plugin-facing view of an LLM request.
#[derive(Debug, Clone)]
pub struct LlmRequestWrapper {
    /// Model identifier.
    pub model: String,
    /// Conversation input.
    pub input: Content,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Optional completion token cap.
    pub max_tokens: Option<u32>,
    /// Optional nucleus sampling parameter.
    pub top_p: Option<f32>,
    /// Optional top-k sampling parameter.
    pub top_k: Option<u32>,
    /// Optional reasoning effort.
    pub reasoning_effort: Option<react::llm::ReasoningEffort>,
    /// API mode to use.
    pub api_mode: react::llm::ApiMode,
    /// Free-form key-value metadata.
    pub metadata: std::collections::HashMap<String, String>,
}

impl LlmRequestWrapper {
    /// Wrap a borrowed [`LlmRequest`](react::llm::LlmRequest).
    pub fn new(request: &react::llm::LlmRequest) -> Self {
        Self {
            model: request.model.clone(),
            input: request.input.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            top_k: request.top_k,
            reasoning_effort: request.reasoning_effort,
            api_mode: request.api_mode,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Convert back into an [`LlmRequest`](react::llm::LlmRequest).
    pub fn into_request(self) -> react::llm::LlmRequest {
        react::llm::LlmRequest {
            model: self.model,
            input: self.input,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            top_k: self.top_k,
            reasoning_effort: self.reasoning_effort,
            api_mode: self.api_mode,
        }
    }
}

/// A plugin-facing view of an LLM response.
#[derive(Debug, Clone)]
pub enum LlmResponseWrapper {
    /// A Chat Completions response.
    OpenAI(react::llm::vendor::ChatCompletionResponse),
    /// A Responses API response.
    Responses(react::llm::vendor::ResponsesResponse),
}

/// A plugin-facing view of one stream token.
#[derive(Debug, Clone)]
pub enum StreamTokenWrapper {
    /// Plain text output.
    Text(String),
    /// Reasoning text emitted alongside the answer.
    ReasoningContent(String),
    /// A tool call request.
    ToolCall {
        /// Tool name.
        name: String,
        /// Tool arguments.
        args: serde_json::Value,
        /// Optional provider-assigned call id.
        id: Option<String>,
    },
    /// A tool call finished executing, with its output and duration.
    ToolResult {
        /// Tool name.
        name: String,
        /// Tool output text.
        output: String,
        /// Wall-clock execution time in milliseconds.
        ms: u64,
    },
    /// Token usage reported by the provider.
    Usage(react::llm::vendor::openaicompatible::Usage),
    /// The stream completed normally.
    Done,
    /// The stream was stopped early.
    Stopped,
}

impl StreamTokenWrapper {
    /// Wrap a borrowed [`StreamToken`](react::llm::StreamToken).
    pub fn new(token: &react::llm::StreamToken) -> Self {
        match token {
            react::llm::StreamToken::Text(s) => StreamTokenWrapper::Text(s.clone()),
            react::llm::StreamToken::ToolCall { name, args, id } => StreamTokenWrapper::ToolCall {
                name: name.clone(),
                args: args.clone(),
                id: id.clone(),
            },
            react::llm::StreamToken::ToolResult { name, output, ms } => {
                StreamTokenWrapper::ToolResult {
                    name: name.clone(),
                    output: output.clone(),
                    ms: *ms,
                }
            }
            react::llm::StreamToken::ReasoningContent(s) => {
                StreamTokenWrapper::ReasoningContent(s.clone())
            }
            react::llm::StreamToken::Usage(u) => StreamTokenWrapper::Usage(u.clone()),
            react::llm::StreamToken::Done => StreamTokenWrapper::Done,
            react::llm::StreamToken::Stopped => StreamTokenWrapper::Stopped,
        }
    }

    /// Convert back into a [`StreamToken`](react::llm::StreamToken).
    pub fn into_token(self) -> react::llm::StreamToken {
        match self {
            StreamTokenWrapper::Text(s) => react::llm::StreamToken::Text(s),
            StreamTokenWrapper::ToolCall { name, args, id } => {
                react::llm::StreamToken::ToolCall { name, args, id }
            }
            StreamTokenWrapper::ToolResult { name, output, ms } => {
                react::llm::StreamToken::ToolResult { name, output, ms }
            }
            StreamTokenWrapper::ReasoningContent(s) => react::llm::StreamToken::ReasoningContent(s),
            StreamTokenWrapper::Usage(u) => react::llm::StreamToken::Usage(u),
            StreamTokenWrapper::Done => react::llm::StreamToken::Done,
            StreamTokenWrapper::Stopped => react::llm::StreamToken::Stopped,
        }
    }
}

impl LlmResponseWrapper {
    /// Wrap a borrowed [`LlmResponse`](react::llm::LlmResponse).
    pub fn new(response: &react::llm::LlmResponse) -> Self {
        match response {
            react::llm::LlmResponse::OpenAI(resp) => LlmResponseWrapper::OpenAI(resp.clone()),
            react::llm::LlmResponse::Responses(resp) => LlmResponseWrapper::Responses(resp.clone()),
        }
    }

    /// Convert back into an [`LlmResponse`](react::llm::LlmResponse).
    pub fn into_response(self) -> react::llm::LlmResponse {
        match self {
            LlmResponseWrapper::OpenAI(resp) => react::llm::LlmResponse::OpenAI(resp),
            LlmResponseWrapper::Responses(resp) => react::llm::LlmResponse::Responses(resp),
        }
    }
}

/// A plugin-facing view of a tool call.
#[derive(Debug, Clone)]
pub struct ToolCallWrapper {
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    pub args: serde_json::Value,
    /// Optional provider-assigned call id.
    pub id: Option<String>,
    /// Free-form key-value metadata.
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolCallWrapper {
    /// Create a tool call wrapper.
    pub fn new(name: impl Into<String>, args: serde_json::Value, id: Option<String>) -> Self {
        Self {
            name: name.into(),
            args,
            id,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Create a wrapper from borrowed tool-call fields.
    pub fn from_tool_call(name: &str, args: &serde_json::Value, id: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            args: args.clone(),
            id: id.map(|s| s.to_string()),
            metadata: std::collections::HashMap::new(),
        }
    }
}

/// A plugin-facing view of a tool result.
#[derive(Debug, Clone)]
pub struct ToolResultWrapper {
    /// Result value, or null on failure.
    pub result: serde_json::Value,
    /// Whether the tool succeeded.
    pub success: bool,
    /// Error message when `success` is false.
    pub error: Option<String>,
    /// Free-form key-value metadata.
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolResultWrapper {
    /// Wrap a successful result value.
    pub fn new(result: serde_json::Value) -> Self {
        Self {
            result,
            success: true,
            error: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Wrap a `Result` from the tool layer.
    pub fn from_result(result: &Result<serde_json::Value, react::tool::ToolError>) -> Self {
        match result {
            Ok(v) => Self::new(v.clone()),
            Err(e) => Self {
                result: serde_json::Value::Null,
                success: false,
                error: Some(e.to_string()),
                metadata: std::collections::HashMap::new(),
            },
        }
    }

    /// Convert back into a `Result`.
    pub fn into_result(self) -> Result<serde_json::Value, react::tool::ToolError> {
        if self.success {
            Ok(self.result)
        } else {
            Err(react::tool::ToolError::Failed(
                self.error.unwrap_or_else(|| "Unknown error".to_string()),
            ))
        }
    }
}

#[async_trait]
/// A hook that can rewrite requests, responses, tool calls, and tokens.
pub trait AgentPlugin: Send + Sync + 'static {
    /// Unique plugin name.
    fn name(&self) -> &str;

    /// Rewrite an LLM request before it is sent; return `None` to veto.
    async fn on_llm_request(&self, request: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
        Some(request)
    }

    /// Rewrite an LLM response; return `None` to veto.
    async fn on_llm_response(&self, response: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
        Some(response)
    }

    /// Rewrite a tool call before execution; return `None` to veto.
    async fn on_tool_call(&self, tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        Some(tool_call)
    }

    /// Rewrite a tool result; return `None` to veto.
    async fn on_tool_result(&self, tool_result: ToolResultWrapper) -> Option<ToolResultWrapper> {
        Some(tool_result)
    }

    /// Rewrite a stream token; return `None` to veto.
    async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> {
        Some(token)
    }
}

#[derive(Default, Clone)]
/// An ordered collection of [`AgentPlugin`]s.
pub struct PluginRegistry {
    plugins: Arc<Mutex<Vec<Arc<dyn AgentPlugin>>>>,
    plugin_count: Arc<AtomicUsize>,
}

impl PluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a plugin.
    pub fn register(&self, plugin: Arc<dyn AgentPlugin>) {
        let mut plugins = self.plugins.lock().unwrap();
        plugins.push(plugin);
        self.plugin_count.store(plugins.len(), Ordering::Release);
    }

    /// Snapshot of the registered plugins.
    pub fn plugins(&self) -> Vec<Arc<dyn AgentPlugin>> {
        self.plugins.lock().unwrap().clone()
    }

    /// Blocking alias for [`PluginRegistry::plugins`].
    pub fn plugins_blocking(&self) -> Vec<Arc<dyn AgentPlugin>> {
        self.plugins()
    }

    /// Names of the registered plugins.
    pub fn plugin_names_blocking(&self) -> Vec<String> {
        self.plugins_blocking()
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    /// Blocking alias for [`PluginRegistry::register`].
    pub fn register_blocking(&self, plugin: Arc<dyn AgentPlugin>) {
        self.register(plugin)
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugin_count.load(Ordering::Acquire)
    }

    /// Whether no plugins are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether at least one plugin is registered.
    pub fn has_plugins(&self) -> bool {
        !self.is_empty()
    }

    /// Remove every plugin.
    pub fn clear(&self) {
        self.plugins.lock().unwrap().clear();
        self.plugin_count.store(0, Ordering::Release);
    }

    /// Blocking alias for [`PluginRegistry::clear`].
    pub fn clear_blocking(&self) {
        self.clear();
    }

    /// Run all plugins' on_llm_request in order. Each plugin's output feeds the next.
    /// Returns Some(request) if the chain completes, None if any plugin vetoes.
    pub async fn on_llm_request(
        &self,
        mut request: LlmRequestWrapper,
    ) -> Option<LlmRequestWrapper> {
        let plugins = self.plugins();
        for plugin in &plugins {
            match plugin.on_llm_request(request).await {
                Some(r) => request = r,
                None => return None,
            }
        }
        Some(request)
    }

    /// Run all plugins' on_llm_response in order.
    pub async fn on_llm_response(
        &self,
        mut response: LlmResponseWrapper,
    ) -> Option<LlmResponseWrapper> {
        let plugins = self.plugins();
        for plugin in &plugins {
            match plugin.on_llm_response(response).await {
                Some(r) => response = r,
                None => return None,
            }
        }
        Some(response)
    }

    /// Run all plugins' on_tool_call in order.
    pub async fn on_tool_call(&self, mut tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        let plugins = self.plugins();
        for plugin in &plugins {
            match plugin.on_tool_call(tool_call).await {
                Some(r) => tool_call = r,
                None => return None,
            }
        }
        Some(tool_call)
    }

    /// Run all plugins' on_tool_result in order.
    pub async fn on_tool_result(
        &self,
        mut tool_result: ToolResultWrapper,
    ) -> Option<ToolResultWrapper> {
        let plugins = self.plugins();
        for plugin in &plugins {
            match plugin.on_tool_result(tool_result).await {
                Some(r) => tool_result = r,
                None => return None,
            }
        }
        Some(tool_result)
    }

    /// Run all plugins' on_stream_token in order.
    pub async fn on_stream_token(
        &self,
        mut token: StreamTokenWrapper,
    ) -> Option<StreamTokenWrapper> {
        let plugins = self.plugins();
        for plugin in &plugins {
            match plugin.on_stream_token(token).await {
                Some(r) => token = r,
                None => return None,
            }
        }
        Some(token)
    }
}
