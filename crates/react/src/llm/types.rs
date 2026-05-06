//! Data types for LLM interactions.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub trait ReactSession {
    fn push(&mut self, msg: LlmMessage);
    fn history(&self) -> Option<Vec<LlmMessage>>;
}

pub trait ReactContext {
    fn session_id(&self) -> String;
    fn skills(&self) -> Option<Vec<Skill>>;
    fn tools(&self) -> Option<Vec<LlmTool>>;
    fn rules(&self) -> Option<Vec<Rule>>;
    fn instructions(&self) -> Option<Vec<Instruction>>;

    fn add_tool(&mut self, _tool: LlmTool) {}

    fn notify_request(&self, _req: &LlmRequest) {}
    fn notify_response(&self, _resp: &super::LlmResponse) {}
    fn notify_error(&self, _err: &LlmError) {}
    fn on_chunk(&self, _chunk: &str) {}
    fn on_chunk_callback(&self) -> Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>> {
        None
    }
}

impl ReactContext for () {
    fn session_id(&self) -> String {
        "unit_session".to_string()
    }
    fn skills(&self) -> Option<Vec<Skill>> {
        None
    }
    fn tools(&self) -> Option<Vec<LlmTool>> {
        None
    }
    fn rules(&self) -> Option<Vec<Rule>> {
        None
    }
    fn instructions(&self) -> Option<Vec<Instruction>> {
        None
    }
    fn add_tool(&mut self, _tool: LlmTool) {}
}

impl ReactSession for () {
    fn push(&mut self, _msg: LlmMessage) {}
    fn history(&self) -> Option<Vec<LlmMessage>> {
        None
    }
}

pub trait Stringfy: Serialize + for<'de> Deserialize<'de> {
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }
    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }
    fn from_value(value: &Value) -> Result<Self, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::from_value(value.clone())
    }
}

#[derive(Debug, Error, Clone)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Request timed out")]
    Timeout,
    #[error("API key is missing")]
    ApiKeyMissing,
    #[error("Rate limited")]
    RateLimited,
    #[error("LLM error: {0}")]
    Other(String),
}

#[derive(Debug, Error, Clone)]
pub enum VendorBuilderError {
    #[error("API key is required")]
    MissingApiKey,
    #[error("Model is required")]
    MissingModel,
    #[error("Endpoint URL is required")]
    MissingEndpoint,
    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            LlmError::Timeout
        } else {
            LlmError::Http(e.to_string())
        }
    }
}

// =============================================================================
// Message Types
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum LlmMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
    },
    AssistantToolCall {
        tool_call_id: String,
        name: String,
        args: Value,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }
    pub fn assistant_tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        args: Value,
    ) -> Self {
        Self::AssistantToolCall {
            tool_call_id: id.into(),
            name: name.into(),
            args,
        }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
}

impl<T: Into<String>> From<T> for LlmMessage {
    fn from(s: T) -> Self {
        Self::user(s)
    }
}

// =============================================================================
// Request/Context Types
// =============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Skill {
    pub category: String,
    pub description: String,
    pub name: String,
}

impl Stringfy for Skill {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl Stringfy for LlmTool {}

pub fn load_skill_tool() -> LlmTool {
    LlmTool {
        name: "load_skill".to_string(),
        description: "Load skill instructions by name. Returns the skill's instructions which you should use to answer the user's question.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to load"
                }
            },
            "required": ["name"]
        }),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub content: String,
}

impl Stringfy for Rule {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Instruction {
    pub instruction: String,
    pub description: String,
    pub name: String,
    pub dependon: Option<Vec<String>>,
}

impl Stringfy for Instruction {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmContext {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<LlmTool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Skill>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<LlmMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<Instruction>,
}

impl ReactContext for LlmContext {
    fn session_id(&self) -> String {
        "llm_context_session".to_string()
    }
    fn skills(&self) -> Option<Vec<Skill>> {
        Some(self.skills.clone())
    }
    fn tools(&self) -> Option<Vec<LlmTool>> {
        Some(self.tools.clone())
    }
    fn rules(&self) -> Option<Vec<Rule>> {
        Some(self.rules.clone())
    }
    fn instructions(&self) -> Option<Vec<Instruction>> {
        Some(self.instructions.clone())
    }
    fn add_tool(&mut self, tool: LlmTool) {
        self.tools.push(tool);
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct LlmRequest {
    pub model: String,
    pub input: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl LlmRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            input: String::new(),
            temperature: Some(0.7),
            max_tokens: None,
            top_p: None,
            top_k: None,
        }
    }

    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }
}

impl Default for LlmRequest {
    fn default() -> Self {
        Self::new("gpt-4")
    }
}

// =============================================================================
// Session for LLM clients
// =============================================================================

/// Session state carried across LLM calls - holds accumulated message history.
#[derive(Debug, Clone, Default)]
pub struct LlmSession {
    /// Accumulated message history from prior LLM calls.
    /// Vendors append to this and/or read it to build requests.
    pub history: Vec<LlmMessage>,
}

impl LlmSession {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    pub fn push(&mut self, msg: LlmMessage) {
        self.history.push(msg);
    }

    pub fn merge(&mut self, other: LlmSession) {
        self.history.extend(other.history);
    }
}

impl ReactSession for LlmSession {
    fn push(&mut self, msg: LlmMessage) {
        self.history.push(msg);
    }

    fn history(&self) -> Option<Vec<LlmMessage>> {
        Some(self.history.clone())
    }
}
