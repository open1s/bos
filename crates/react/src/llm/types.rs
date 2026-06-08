//! Data types for LLM interactions.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

pub trait ReactSession {
    fn push(&mut self, msg: LlmMessage);
    fn history(&self) -> Option<&[LlmMessage]>;
}

// =============================================================================
// Multimodal Content Types (for images, audio in chat messages)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "binary")]
    Binary { binary: Binary },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BinarySource {
    #[serde(rename = "url")]
    Url(String),
    #[serde(rename = "base64")]
    Base64(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Binary {
    #[serde(rename = "content_type")]
    pub content_type: String,
    pub source: BinarySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Binary {
    pub fn from_base64(content_type: impl Into<String>, content: impl Into<String>, name: Option<String>) -> Self {
        Self {
            content_type: content_type.into(),
            source: BinarySource::Base64(content.into()),
            name,
        }
    }

    pub fn from_url(content_type: impl Into<String>, url: impl Into<String>, name: Option<String>) -> Self {
        Self {
            content_type: content_type.into(),
            source: BinarySource::Url(url.into()),
            name,
        }
    }

    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    pub fn is_audio(&self) -> bool {
        self.content_type.starts_with("audio/")
    }

    pub fn url(&self) -> String {
        match &self.source {
            BinarySource::Url(url) => url.clone(),
            BinarySource::Base64(b64_content) => {
                format!("data:{};base64,{}", self.content_type, b64_content)
            }
        }
    }

    pub fn into_url(self) -> String {
        match self.source {
            BinarySource::Url(url) => url,
            BinarySource::Base64(b64_content) => {
                format!("data:{};base64,{}", self.content_type, b64_content)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        if let Ok(parts) = serde_json::from_str::<Vec<ContentPart>>(&s) {
            return Content::Parts(parts);
        }
        if let Ok(part) = serde_json::from_str::<ContentPart>(&s) {
            return Content::Parts(vec![part]);
        }
        Content::Text(s)
    }
}

impl<'a> From<&'a str> for Content {
    fn from(s: &'a str) -> Self {
        Content::from(s.to_string())
    }
}

impl<'a> From<&'a String> for Content {
    fn from(s: &'a String) -> Self {
        Content::from(s.as_str())
    }
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text(text.into())
    }

    pub fn parts(parts: Vec<ContentPart>) -> Self {
        Content::Parts(parts)
    }

    pub fn image(url: impl Into<String>) -> Self {
        Content::binary("image url".to_string(), url.into())
    }

    pub fn audio(data: impl Into<String>, format: &str) -> Self {
        Content::binary(format!("audio/{}", format), data)
    }

    pub fn audio_url(url: impl Into<String>, format: &str) -> Self {
        Content::binary_url(format!("audio/{}", format), url)
    }

    pub fn binary(content_type: impl Into<String>, data: impl Into<String>) -> Self {
        Content::Parts(vec![ContentPart::Binary {
            binary: Binary::from_base64(content_type, data, None),
        }])
    }

    pub fn binary_url(content_type: impl Into<String>, url: impl Into<String>) -> Self {
        Content::Parts(vec![ContentPart::Binary {
            binary: Binary::from_url(content_type, url, None),
        }])
    }

    pub fn is_text_only(&self) -> bool {
        matches!(self, Content::Text(_))
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s),
            _ => None,
        }
    }
}

pub trait ReactContext {
    fn session_id(&self) -> String;
    fn skills(&self) -> Option<&[Skill]>;
    fn tools(&self) -> Option<&[LlmTool]>;
    fn rules(&self) -> Option<&[Rule]>;
    fn instructions(&self) -> Option<&[Instruction]>;
    fn add_tool(&mut self, tool: LlmTool);

    fn notify_request(&self, _req: &LlmRequest);
    fn notify_response(&self, _resp: &super::LlmResponse);
    fn notify_error(&self, _err: &LlmError);
    fn on_chunk(&self, _chunk: &str);
    fn on_chunk_callback(&self) -> Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>>;
}

impl ReactContext for () {
    fn session_id(&self) -> String {
        "unit_session".to_string()
    }
    fn skills(&self) -> Option<&[Skill]> {
        None
    }
    fn tools(&self) -> Option<&[LlmTool]> {
        None
    }
    fn rules(&self) -> Option<&[Rule]> {
        None
    }
    fn instructions(&self) -> Option<&[Instruction]> {
        None
    }
    fn add_tool(&mut self, _tool: LlmTool) {}

    fn notify_request(&self, __req: &LlmRequest) {}

    fn notify_response(&self, __resp: &super::LlmResponse) {}

    fn notify_error(&self, __err: &LlmError) {}

    fn on_chunk(&self, __chunk: &str) {}

    fn on_chunk_callback(&self) -> Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>> {
        None
    }
}

impl ReactSession for () {
    fn push(&mut self, _msg: LlmMessage) {}
    fn history(&self) -> Option<&[LlmMessage]> {
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
        content: Content,
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
    pub fn user(content: impl Into<Content>) -> Self {
        Self::User {
            content: content.into(),
        }
    }
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User {
            content: Content::Text(content.into()),
        }
    }
    pub fn user_image(url: impl Into<String>) -> Self {
        Self::User {
            content: Content::image(url),
        }
    }
    pub fn user_audio(data: impl Into<String>, format: &str) -> Self {
        Self::User {
            content: Content::audio(data, format),
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

impl From<String> for LlmMessage {
    fn from(s: String) -> Self {
        Self::user(s)
    }
}

impl<'a> From<&'a str> for LlmMessage {
    fn from(s: &'a str) -> Self {
        Self::user(s)
    }
}

impl<'a> From<&'a String> for LlmMessage {
    fn from(s: &'a String) -> Self {
        Self::user(s.as_str())
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
        self.tools.push(tool);
    }

    fn notify_request(&self, _req: &LlmRequest) {}

    fn notify_response(&self, _resp: &super::LlmResponse) {}

    fn notify_error(&self, _err: &LlmError) {}

    fn on_chunk(&self, _chunk: &str) {}

    fn on_chunk_callback(&self) -> Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>> {
        None
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct LlmRequest {
    pub model: String,
    pub input: Content,
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
            input: Content::Text(String::new()),
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
    /// Arc enables O(1) clone for sharing across async boundaries.
    pub history: Arc<Vec<LlmMessage>>,
}

impl LlmSession {
    pub fn new() -> Self {
        Self {
            history: Arc::new(Vec::new()),
        }
    }

    pub fn push(&mut self, msg: LlmMessage) {
        Arc::make_mut(&mut self.history).push(msg);
    }

    pub fn merge(&mut self, other: LlmSession) {
        Arc::make_mut(&mut self.history).extend_from_slice(&other.history);
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

impl ReactSession for LlmSession {
    fn push(&mut self, msg: LlmMessage) {
        Arc::make_mut(&mut self.history).push(msg);
    }

    fn history(&self) -> Option<&[LlmMessage]> {
        Some(&*self.history)
    }
}
