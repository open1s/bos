use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmStage {
    PreRequest,
    PostResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolStage {
    PreExecute,
    PostExecute,
}

#[derive(Debug, Clone)]
pub struct LlmRequestWrapper {
    pub model: String,
    pub input: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl LlmRequestWrapper {
    pub fn new(request: &react::llm::LlmRequest) -> Self {
        Self {
            model: request.model.clone(),
            input: request.input.clone(),
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            top_k: request.top_k,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn into_request(self) -> react::llm::LlmRequest {
        react::llm::LlmRequest {
            model: self.model,
            input: self.input,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            top_p: self.top_p,
            top_k: self.top_k,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LlmResponseWrapper {
    OpenAI(react::llm::vendor::ChatCompletionResponse),
}

#[derive(Debug, Clone)]
pub enum StreamTokenWrapper {
    Text(String),
    ReasoningContent(String),
    ToolCall {
        name: String,
        args: serde_json::Value,
        id: Option<String>,
    },
    Done,
}

impl StreamTokenWrapper {
    pub fn new(token: &react::llm::StreamToken) -> Self {
        match token {
            react::llm::StreamToken::Text(s) => StreamTokenWrapper::Text(s.clone()),
            react::llm::StreamToken::ToolCall { name, args, id } => StreamTokenWrapper::ToolCall {
                name: name.clone(),
                args: args.clone(),
                id: id.clone(),
            },
            react::llm::StreamToken::ReasoningContent(s) => {
                StreamTokenWrapper::ReasoningContent(s.clone())
            }
            react::llm::StreamToken::Done => StreamTokenWrapper::Done,
        }
    }

    pub fn into_token(self) -> react::llm::StreamToken {
        match self {
            StreamTokenWrapper::Text(s) => react::llm::StreamToken::Text(s),
            StreamTokenWrapper::ToolCall { name, args, id } => {
                react::llm::StreamToken::ToolCall { name, args, id }
            }
            StreamTokenWrapper::ReasoningContent(s) => react::llm::StreamToken::ReasoningContent(s),
            StreamTokenWrapper::Done => react::llm::StreamToken::Done,
        }
    }
}

impl LlmResponseWrapper {
    pub fn new(response: &react::llm::LlmResponse) -> Self {
        match response {
            react::llm::LlmResponse::OpenAI(resp) => LlmResponseWrapper::OpenAI(resp.clone()),
        }
    }

    pub fn into_response(self) -> react::llm::LlmResponse {
        match self {
            LlmResponseWrapper::OpenAI(resp) => react::llm::LlmResponse::OpenAI(resp),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCallWrapper {
    pub name: String,
    pub args: serde_json::Value,
    pub id: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolCallWrapper {
    pub fn new(name: impl Into<String>, args: serde_json::Value, id: Option<String>) -> Self {
        Self {
            name: name.into(),
            args,
            id,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn from_tool_call(name: &str, args: &serde_json::Value, id: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            args: args.clone(),
            id: id.map(|s| s.to_string()),
            metadata: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolResultWrapper {
    pub result: serde_json::Value,
    pub success: bool,
    pub error: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

impl ToolResultWrapper {
    pub fn new(result: serde_json::Value) -> Self {
        Self {
            result,
            success: true,
            error: None,
            metadata: std::collections::HashMap::new(),
        }
    }

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
pub trait AgentPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    async fn on_llm_request(&self, request: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
        Some(request)
    }

    async fn on_llm_response(&self, response: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
        Some(response)
    }

    async fn on_tool_call(&self, tool_call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        Some(tool_call)
    }

    async fn on_tool_result(&self, tool_result: ToolResultWrapper) -> Option<ToolResultWrapper> {
        Some(tool_result)
    }

    async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> {
        Some(token)
    }
}

#[derive(Default, Clone)]
pub struct PluginRegistry {
    plugins: Arc<Mutex<Vec<Arc<dyn AgentPlugin>>>>,
    plugin_count: Arc<AtomicUsize>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, plugin: Arc<dyn AgentPlugin>) {
        let mut plugins = self.plugins.lock().unwrap();
        plugins.push(plugin);
        self.plugin_count.store(plugins.len(), Ordering::Release);
    }

    pub fn plugins(&self) -> Vec<Arc<dyn AgentPlugin>> {
        self.plugins.lock().unwrap().clone()
    }

    pub fn plugins_blocking(&self) -> Vec<Arc<dyn AgentPlugin>> {
        self.plugins()
    }

    pub fn plugin_names_blocking(&self) -> Vec<String> {
        self.plugins_blocking()
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    pub fn register_blocking(&self, plugin: Arc<dyn AgentPlugin>) {
        self.register(plugin)
    }

    pub fn len(&self) -> usize {
        self.plugin_count.load(Ordering::Acquire)
    }

    pub fn has_plugins(&self) -> bool {
        self.len() > 0
    }

    pub fn clear(&self) {
        self.plugins.lock().unwrap().clear();
        self.plugin_count.store(0, Ordering::Release);
    }

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
