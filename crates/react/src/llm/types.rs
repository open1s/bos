//! Data types for LLM interactions.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use thiserror::Error;

/// Conversation history shared with an LLM client.
pub trait ReactSession {
    /// Append a message to the history.
    fn push(&mut self, msg: LlmMessage);
    /// The accumulated messages, if the session keeps any.
    fn history(&self) -> Option<&[LlmMessage]>;
    /// Estimate the tokens this history contributes to the next request.
    ///
    /// `None` means the session cannot price itself; the engine then skips
    /// mid-turn compaction rather than guessing. The estimate is a heuristic,
    /// the same way [`crate::engine`] prices a budget.
    fn estimate_tokens(&self) -> Option<usize> {
        None
    }
    /// Mechanically condense the oldest safe span, keeping the newest messages.
    ///
    /// Returns whether the history actually shrank. The default does nothing,
    /// so a session that cannot compact keeps the pre-existing behaviour.
    fn compact_mechanical(&mut self, _keep_recent: usize, _max_summary_chars: usize) -> bool {
        false
    }
}

// =============================================================================
// Multimodal Content Types (for images, audio in chat messages)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
/// One part of a multimodal message.
pub enum ContentPart {
    /// A text part.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },
    /// A binary part, such as an image or audio.
    #[serde(rename = "binary")]
    Binary {
        /// The binary payload.
        binary: Binary,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Where a [`Binary`] payload comes from.
pub enum BinarySource {
    /// A URL to fetch the payload from.
    #[serde(rename = "url")]
    Url(String),
    /// Base64-encoded payload bytes.
    #[serde(rename = "base64")]
    Base64(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// A binary payload attached to a message.
pub struct Binary {
    /// MIME type, for example `image/png`.
    #[serde(rename = "content_type")]
    pub content_type: String,
    /// Where the payload comes from.
    pub source: BinarySource,
    /// Optional file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Binary {
    /// Build a binary from base64-encoded `content`.
    pub fn from_base64(
        content_type: impl Into<String>,
        content: impl Into<String>,
        name: Option<String>,
    ) -> Self {
        Self {
            content_type: content_type.into(),
            source: BinarySource::Base64(content.into()),
            name,
        }
    }

    /// Build a binary from a `url`.
    pub fn from_url(
        content_type: impl Into<String>,
        url: impl Into<String>,
        name: Option<String>,
    ) -> Self {
        Self {
            content_type: content_type.into(),
            source: BinarySource::Url(url.into()),
            name,
        }
    }

    /// Whether the content type is an image.
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// Whether the content type is audio.
    pub fn is_audio(&self) -> bool {
        self.content_type.starts_with("audio/")
    }

    /// The payload as a URL, inlining base64 data as a data URL.
    pub fn url(&self) -> String {
        match &self.source {
            BinarySource::Url(url) => url.clone(),
            BinarySource::Base64(b64_content) => {
                format!("data:{};base64,{}", self.content_type, b64_content)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
/// Message content: plain text or multimodal parts.
pub enum Content {
    /// Plain text.
    Text(String),
    /// A sequence of multimodal parts.
    Parts(Vec<ContentPart>),
}

/// Decode a JSON content-parts payload, if `text` can possibly be one.
///
/// Only a JSON object or array can decode as a `ContentPart`/parts list, so
/// ordinary prompt text skips the two parse attempts that used to run on
/// every message construction and every request build.
pub(crate) fn parse_content_parts(text: &str) -> Option<Vec<ContentPart>> {
    if !matches!(text.trim_start().as_bytes().first(), Some(b'[' | b'{')) {
        return None;
    }
    if let Ok(parts) = serde_json::from_str::<Vec<ContentPart>>(text) {
        return Some(parts);
    }
    serde_json::from_str::<ContentPart>(text)
        .ok()
        .map(|part| vec![part])
}

impl From<String> for Content {
    fn from(s: String) -> Self {
        match parse_content_parts(&s) {
            Some(parts) => Content::Parts(parts),
            None => Content::Text(s),
        }
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
    /// Create text content.
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text(text.into())
    }

    /// Create content from parts.
    pub fn parts(parts: Vec<ContentPart>) -> Self {
        Content::Parts(parts)
    }

    /// Create content holding one image URL.
    pub fn image(url: impl Into<String>) -> Self {
        Content::binary_url("image/jpeg".to_string(), url)
    }

    /// Create content holding base64 audio of `format`.
    pub fn audio(data: impl Into<String>, format: &str) -> Self {
        Content::binary(format!("audio/{}", format), data)
    }

    /// Create content holding an audio URL of `format`.
    pub fn audio_url(url: impl Into<String>, format: &str) -> Self {
        Content::binary_url(format!("audio/{}", format), url)
    }

    /// Create content holding one base64 binary of `content_type`.
    pub fn binary(content_type: impl Into<String>, data: impl Into<String>) -> Self {
        Content::Parts(vec![ContentPart::Binary {
            binary: Binary::from_base64(content_type, data, None),
        }])
    }

    /// Create content holding one binary URL of `content_type`.
    pub fn binary_url(content_type: impl Into<String>, url: impl Into<String>) -> Self {
        Content::Parts(vec![ContentPart::Binary {
            binary: Binary::from_url(content_type, url, None),
        }])
    }

    /// The text, if this is plain text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Content::Text(s) => Some(s),
            _ => None,
        }
    }
}

/// Callback invoked with each streamed chunk of text, when one is installed.
pub type ChunkCallback = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Per-request context: session id, tools, skills, rules, and hooks.
pub trait ReactContext {
    /// Identifier for the current session.
    fn session_id(&self) -> String;
    /// Skills available to the model.
    fn skills(&self) -> Option<&[Skill]>;
    /// Tools available to the model.
    fn tools(&self) -> Option<&[LlmTool]>;
    /// Rules constraining the model.
    fn rules(&self) -> Option<&[Rule]>;
    /// Instructions prepended to the prompt.
    fn instructions(&self) -> Option<&[Instruction]>;
    /// Add a tool to the context.
    fn add_tool(&mut self, tool: LlmTool);

    /// Called before a request is sent.
    fn notify_request(&self, _req: &LlmRequest);
    /// Called after a response is received.
    fn notify_response(&self, _resp: &super::LlmResponse);
    /// Called when a call fails.
    fn notify_error(&self, _err: &LlmError);
    /// Called with each streamed chunk of text.
    fn on_chunk(&self, _chunk: &str);
    /// The installed chunk callback, if any.
    fn on_chunk_callback(&self) -> Option<ChunkCallback>;
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

    fn on_chunk_callback(&self) -> Option<ChunkCallback> {
        None
    }
}

impl ReactSession for () {
    fn push(&mut self, _msg: LlmMessage) {}
    fn history(&self) -> Option<&[LlmMessage]> {
        None
    }
}

/// Serialization helpers for config-like types.
pub trait Stringfy: Serialize + for<'de> Deserialize<'de> {
    /// Serialize to YAML.
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }
    /// Serialize to JSON.
    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    /// Serialize to a [`Value`].
    fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }
    /// Deserialize from a [`Value`].
    fn from_value(value: &Value) -> Result<Self, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::from_value(value.clone())
    }
}

#[derive(Debug, Error, Clone)]
/// Errors raised by an LLM client.
pub enum LlmError {
    /// The HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(String),
    /// The response could not be parsed.
    #[error("Parse error: {0}")]
    Parse(String),
    /// The request timed out.
    #[error("Request timed out")]
    Timeout,
    /// No API key was configured.
    #[error("API key is missing")]
    ApiKeyMissing,
    /// The provider rate-limited the request.
    #[error("Rate limited")]
    RateLimited,
    /// Any other error, carrying the whole sentence.
    ///
    /// The message already names what happened ("Stream error: ...",
    /// "HTTP 500: ...") and every layer that renders it also names the LLM
    /// call, so a second "LLM error:" here stacked two labels on one failure.
    #[error("{0}")]
    Other(String),
}

impl LlmError {
    /// Whether the error is worth retrying.
    ///
    /// Rate limits, timeouts, and server-side or connection failures are
    /// transient; parse errors and missing credentials are not. HTTP
    /// failures are classified by status code, so a 4xx response is never
    /// retried even when its body mentions a retryable condition.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::RateLimited | LlmError::Timeout => true,
            LlmError::Http(message) | LlmError::Other(message) => retryable_message(message),
            LlmError::Parse(_) | LlmError::ApiKeyMissing => false,
        }
    }
}

/// Fragments that mark a provider message as transient when it carries
/// no HTTP status code.
const RETRYABLE_FRAGMENTS: &[&str] = &[
    "timed out",
    "timeout",
    "connection refused",
    "connection reset",
    "connection closed",
    "service unavailable",
    "bad gateway",
    "gateway timeout",
    "temporarily unavailable",
    "too many requests",
    "rate limit",
];

/// Extract the status code from a provider message like `"HTTP 503: ..."`.
fn http_status(message: &str) -> Option<u16> {
    let rest = message.strip_prefix("HTTP ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether a provider message describes a transient failure.
fn retryable_message(message: &str) -> bool {
    if let Some(status) = http_status(message) {
        return matches!(status, 408 | 425 | 429) || (500..=599).contains(&status);
    }
    let lower = message.to_ascii_lowercase();
    RETRYABLE_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
}

#[derive(Debug, Error, Clone)]
/// Errors raised while building a vendor.
pub enum VendorBuilderError {
    /// No API key was supplied.
    #[error("API key is required")]
    MissingApiKey,
    /// No model was supplied.
    #[error("Model is required")]
    MissingModel,
    /// No endpoint was supplied.
    #[error("Endpoint URL is required")]
    MissingEndpoint,
    /// The configuration was invalid.
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
/// A message in an LLM conversation.
pub enum LlmMessage {
    /// A system message.
    System {
        /// The system prompt.
        content: String,
    },
    /// A user message.
    User {
        /// The user content.
        content: Content,
    },
    /// An assistant message.
    Assistant {
        /// The assistant text.
        content: String,
    },
    /// An assistant request to call a tool.
    AssistantToolCall {
        /// Id linking this call to its result.
        tool_call_id: String,
        /// Tool name.
        name: String,
        /// Tool arguments.
        args: Value,
    },
    /// The result of a tool call.
    ToolResult {
        /// Id of the call this answers.
        tool_call_id: String,
        /// The tool output.
        content: String,
    },
}

impl LlmMessage {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }
    /// Create a user message with arbitrary content.
    pub fn user(content: impl Into<Content>) -> Self {
        Self::User {
            content: content.into(),
        }
    }
    /// Create a user message from plain text.
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User {
            content: Content::Text(content.into()),
        }
    }
    /// Create a user message holding an image URL.
    pub fn user_image(url: impl Into<String>) -> Self {
        Self::User {
            content: Content::image(url),
        }
    }
    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }
    /// Create an assistant tool-call message.
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
    /// Create a tool-result message.
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
/// A skill the model can load.
pub struct Skill {
    /// Skill category.
    pub category: String,
    /// Short description.
    pub description: String,
    /// Skill name.
    pub name: String,
}

impl Stringfy for Skill {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// The kind of tool passed to the provider.
pub enum LlmToolKind {
    /// A normal callable function tool.
    #[default]
    Function,
    /// A hosted web-search tool.
    WebSearch,
    /// A hosted file-search tool.
    FileSearch,
    /// A hosted computer-use tool.
    ComputerUse,
}

impl LlmToolKind {
    /// The wire name of this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            LlmToolKind::Function => "function",
            LlmToolKind::WebSearch => "web_search",
            LlmToolKind::FileSearch => "file_search",
            LlmToolKind::ComputerUse => "computer_use",
        }
    }
}

fn is_function_kind(kind: &LlmToolKind) -> bool {
    *kind == LlmToolKind::Function
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// A tool definition passed to the provider.
pub struct LlmTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON schema for the arguments.
    pub parameters: Value,
    /// Tool kind. `Function` (default) is the normal callable tool. The other
    /// variants map to OpenAI Responses API hosted tools (`web_search`,
    /// `file_search`, `computer_use`) which run server-side.
    #[serde(default, skip_serializing_if = "is_function_kind")]
    pub kind: LlmToolKind,
    /// Hosted-tool configuration, e.g. `{"max_results": 5}` for web_search or
    /// `{"vector_store_ids": ["..."]}` for file_search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

impl LlmTool {
    /// Create a normal function tool.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            kind: LlmToolKind::Function,
            config: None,
        }
    }

    /// Create a hosted web-search tool.
    pub fn web_search(config: Option<Value>) -> Self {
        Self {
            name: "web_search".into(),
            description: "Search the web for up-to-date information.".into(),
            parameters: serde_json::json!({}),
            kind: LlmToolKind::WebSearch,
            config,
        }
    }

    /// Create a hosted file-search tool.
    pub fn file_search(config: Option<Value>) -> Self {
        Self {
            name: "file_search".into(),
            description: "Search files in the attached vector stores.".into(),
            parameters: serde_json::json!({}),
            kind: LlmToolKind::FileSearch,
            config,
        }
    }

    /// Create a hosted computer-use tool.
    pub fn computer_use(config: Option<Value>) -> Self {
        Self {
            name: "computer_use".into(),
            description: "Operate a virtual computer (screenshots, clicks, typing).".into(),
            parameters: serde_json::json!({}),
            kind: LlmToolKind::ComputerUse,
            config,
        }
    }
}

impl Stringfy for LlmTool {}

/// The built-in tool that loads skill instructions.
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
        kind: LlmToolKind::Function,
        config: None,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// A rule that constrains the model.
pub struct Rule {
    /// Rule name.
    pub name: String,
    /// Rule text.
    pub content: String,
}

impl Stringfy for Rule {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// An instruction prepended to the prompt.
pub struct Instruction {
    /// The instruction text.
    pub instruction: String,
    /// Short description.
    pub description: String,
    /// Instruction name.
    pub name: String,
    /// Names of instructions that must run first.
    pub dependon: Option<Vec<String>>,
}

impl Stringfy for Instruction {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// A ready-made [`ReactContext`] with tools, skills, rules, and history.
pub struct LlmContext {
    /// Tools available to the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<LlmTool>,
    /// Skills available to the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<Skill>,
    /// Prior conversation messages.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<LlmMessage>,
    /// Rules constraining the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    /// Instructions prepended to the prompt.
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

    fn on_chunk_callback(&self) -> Option<ChunkCallback> {
        None
    }
}

#[derive(Clone, Deserialize, Serialize)]
/// A request to an LLM provider.
pub struct LlmRequest {
    /// Model identifier.
    pub model: String,
    /// The prompt content.
    pub input: Content,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling cutoff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Completion token cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Reasoning effort for supported reasoning models (`low`/`medium`/`high`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Which OpenAI API protocol to use. `Chat` (default) targets
    /// `/chat/completions`; `Responses` targets the newer `/v1/responses` API.
    #[serde(default)]
    pub api_mode: ApiMode,
}

/// Selects the protocol used for a single LLM request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiMode {
    /// OpenAI Chat Completions API (`/chat/completions`).
    #[default]
    Chat,
    /// OpenAI Responses API (`/v1/responses`).
    Responses,
    /// Anthropic Messages API (`/v1/messages`).
    Anthropic,
}

impl ApiMode {
    /// The wire name of this mode.
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiMode::Chat => "chat",
            ApiMode::Responses => "responses",
            ApiMode::Anthropic => "anthropic",
        }
    }

    /// Parse `s`, defaulting to chat.
    pub fn from_name(s: &str) -> Self {
        match s {
            "responses" => ApiMode::Responses,
            "anthropic" => ApiMode::Anthropic,
            _ => ApiMode::Chat,
        }
    }
}

/// Reasoning effort for reasoning-capable models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
}

impl ReasoningEffort {
    /// The wire name of this effort.
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }

    /// Parse `s`, defaulting to medium.
    pub fn from_name(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "low" => ReasoningEffort::Low,
            "high" => ReasoningEffort::High,
            _ => ReasoningEffort::Medium,
        }
    }
}

impl LlmRequest {
    /// Create a request for `model` with default sampling.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            input: Content::Text(String::new()),
            temperature: Some(0.7),
            max_tokens: None,
            top_p: None,
            top_k: None,
            reasoning_effort: None,
            api_mode: ApiMode::Chat,
        }
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set the completion token cap.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set nucleus sampling probability.
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set the top-k sampling cutoff.
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    /// Set the reasoning effort.
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// Set the API protocol.
    pub fn api_mode(mut self, mode: ApiMode) -> Self {
        self.api_mode = mode;
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
    /// Create an empty session.
    pub fn new() -> Self {
        Self {
            history: Arc::new(Vec::new()),
        }
    }

    /// Append a message to the history.
    pub fn push(&mut self, msg: LlmMessage) {
        Arc::make_mut(&mut self.history).push(msg);
    }

    /// Append another session history.
    pub fn merge(&mut self, other: LlmSession) {
        Arc::make_mut(&mut self.history).extend_from_slice(&other.history);
    }

    /// Number of messages in the history.
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Whether the history is empty.
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

/// How many skills keep their full description in one request.
///
/// A machine accumulates skills, and a measured BSH install carried 123 of
/// them. Injecting every description cost 36 KB of a 41 KB system prompt on
/// every step of every turn - for tasks that used none of them. Selection needs
/// the few skills that could apply and the NAMES of the rest, because
/// load_skill reads the instructions on demand.
pub const SKILL_CATALOG_DESCRIBED: usize = 8;

/// Hard byte budget for the whole rendered catalogue.
pub const SKILL_CATALOG_BUDGET: usize = 4_096;

/// The longest description a described skill keeps.
const SKILL_DESCRIPTION_CHARS: usize = 240;

/// Words too common to say what a request is about.
const SKILL_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "your", "when", "use", "using",
    "are", "was", "were", "will", "would", "should", "can", "could", "have", "has", "had", "not",
    "but", "you", "our", "its", "their", "them", "they", "than", "then", "there", "here", "also",
    "any", "all", "some", "such", "each", "every", "work", "works", "working", "task", "tasks",
    "thing", "things", "need", "needs", "want", "wants", "make", "makes", "making", "get", "gets",
    "one", "two", "new", "add", "adds", "adding", "create", "creates", "creating", "file", "files",
];

/// Render the catalogue of available skills for one request.
///
/// The catalogue is RANKED against the request, not listed in full: a skill
/// whose name or category shares a word with the request keeps its description
/// (up to SKILL_CATALOG_DESCRIBED of them), every other skill is named only, and
/// the whole block is bounded by SKILL_CATALOG_BUDGET. The model still discovers
/// everything by name and can open any of them with load_skill; what it no
/// longer pays for is a 36 KB catalogue of skills unrelated to the turn.
pub fn render_skill_catalog(skills: &[Skill], query: &str) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let wanted = catalog_words(query);
    let mut ranked: Vec<(usize, usize)> = skills
        .iter()
        .enumerate()
        .map(|(index, skill)| {
            let head = format!("{} {}", skill.name, skill.category);
            (catalog_score(&wanted, &head, &skill.description), index)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let described: Vec<usize> = ranked
        .iter()
        .filter(|(score, _)| *score > 0)
        .take(SKILL_CATALOG_DESCRIBED)
        .map(|(_, index)| *index)
        .collect();

    let mut text = format!(
        "Available skills ({}; call load_skill with a name to read its instructions):\n",
        skills.len()
    );
    // A listed skill is a candidate, not an instruction: the only installed
    // skill must not be loaded just because it is the only one.
    text.push_str("Load a skill only when its description matches the request; being listed is not a reason to load it.\n");
    // The reader can invoke one by name in the composer (the `/name`
    // gesture), in which case its instructions arrive as a block of their
    // own. Saying so is the reference catalogue's own sentence, retargeted
    // to this tool's name, and it stops a second read of a skill that is
    // already in the conversation.
    text.push_str("A user may also invoke one directly; its <skill_content> block then appears in the conversation. Follow it, and do not call load_skill again for that skill.\n");
    for index in &described {
        let skill = &skills[*index];
        text.push_str(&format!(
            "- {}: {}\n",
            skill.name,
            clip(&skill.description, SKILL_DESCRIPTION_CHARS)
        ));
    }
    let rest: Vec<&str> = skills
        .iter()
        .enumerate()
        .filter(|(index, _)| !described.contains(index))
        .map(|(_, skill)| skill.name.as_str())
        .collect();
    if !rest.is_empty() {
        text.push_str("- other skills (name only): ");
        text.push_str(&rest.join(", "));
    }
    if text.len() > SKILL_CATALOG_BUDGET {
        text = clip(&text, SKILL_CATALOG_BUDGET);
    }
    text
}

/// The words a request is about, lowercased and without the filler.
fn catalog_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(|word| word.to_lowercase())
        .filter(|word| !SKILL_STOPWORDS.contains(&word.as_str()))
        .collect()
}

/// How strongly one skill answers the request; 0 means "not this one".
fn catalog_score(wanted: &[String], head: &str, description: &str) -> usize {
    if wanted.is_empty() {
        return 0;
    }
    let head = catalog_words(head);
    let body = catalog_words(description);
    wanted
        .iter()
        .map(|word| {
            if head.iter().any(|candidate| candidate == word) {
                3
            } else if body.iter().any(|candidate| candidate == word) {
                1
            } else {
                0
            }
        })
        .sum()
}

/// Truncate on a character boundary, marking that it was cut.
fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut cut = limit.saturating_sub(4).min(text.len());
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} ...", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::{parse_content_parts, ApiMode, Content, ContentPart, LlmError};

    #[test]
    fn content_from_json_still_decodes_with_leading_whitespace() {
        let content = Content::from("  [{\"type\":\"text\",\"text\":\"hi\"}]".to_string());
        match content {
            Content::Parts(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], ContentPart::Text { text } if text == "hi"));
            }
            other => panic!("expected parts, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_is_never_mistaken_for_content_parts() {
        let text = "Explain {a: b} in prose";
        assert!(parse_content_parts(text).is_none());
        assert!(matches!(Content::from(text.to_string()), Content::Text(t) if t == text));
    }

    /// One failure wears one label.
    ///
    /// `LlmError::Other` carries the provider's whole sentence ("Stream error:
    /// ..."), and the engine's wrapper supplies the only "LLM error:" - the two
    /// used to stack, so the reader saw the label twice and the cause nowhere.
    #[test]
    fn an_llm_failure_is_labelled_once() {
        let error = crate::engine::ReactError::Llm(LlmError::Other(
            "Stream error: error decoding response body: operation timed out".to_string(),
        ));
        assert_eq!(
            error.to_string(),
            "LLM error: Stream error: error decoding response body: operation timed out"
        );
    }

    #[test]
    fn api_mode_round_trips_through_its_wire_name() {
        for mode in [ApiMode::Chat, ApiMode::Responses, ApiMode::Anthropic] {
            assert_eq!(ApiMode::from_name(mode.as_str()), mode);
        }
        // An unknown name falls back to chat rather than failing.
        assert_eq!(ApiMode::from_name("nonsense"), ApiMode::Chat);
    }

    fn skill(name: &str, description: &str) -> super::Skill {
        super::Skill {
            category: "general".to_string(),
            description: description.to_string(),
            name: name.to_string(),
        }
    }

    /// A catalogue of skills that have nothing to do with the turn is NAMES,
    /// not descriptions. The measured BSH install carried 123 skills and their
    /// descriptions were 87% of the system prompt.
    #[test]
    fn an_unrelated_catalogue_costs_names_not_descriptions() {
        let skills: Vec<super::Skill> = (0..200)
            .map(|index| {
                skill(
                    &format!("threejs-procedural-thing-{index}"),
                    "Author physically based procedural Three.js materials with                      derivative normals, atlas filtering and specular antialiasing.",
                )
            })
            .collect();
        let catalog = super::render_skill_catalog(&skills, "create AGENTS.md");
        assert!(
            catalog.len() <= super::SKILL_CATALOG_BUDGET,
            "bounded: {} bytes",
            catalog.len()
        );
        assert!(
            !catalog.contains("derivative normals"),
            "no unrelated description rides along: {catalog}"
        );
        assert!(
            catalog.contains("threejs-procedural-thing-0"),
            "every skill is still discoverable by name"
        );
    }

    /// A skill that answers the request keeps its description, because that is
    /// the one thing the model needs to decide to open it.
    #[test]
    fn a_matching_skill_keeps_its_description() {
        let skills = vec![
            skill("threejs-bloom", "Implement production bloom in Three.js."),
            skill(
                "rust-testing",
                "Write and run Rust tests, then fix the failures they report.",
            ),
        ];
        let catalog = super::render_skill_catalog(&skills, "the rust tests are failing, fix them");
        assert!(
            catalog.contains("Write and run Rust tests"),
            "the matching skill is described: {catalog}"
        );
        assert!(
            !catalog.contains("Implement production bloom"),
            "the unrelated one is a name only: {catalog}"
        );
    }

    /// The budget wins over any skill count.
    #[test]
    fn the_catalogue_is_bounded_even_with_hundreds_of_skills() {
        let skills: Vec<super::Skill> = (0..500)
            .map(|index| skill(&format!("rust-tests-{index}"), &"x".repeat(400)))
            .collect();
        let catalog = super::render_skill_catalog(&skills, "rust tests");
        assert!(
            catalog.len() <= super::SKILL_CATALOG_BUDGET,
            "the catalogue stays inside its budget: {} bytes",
            catalog.len()
        );
    }

    /// A skill the READER invoked by name arrives as a block in the
    /// conversation, so the catalogue must say the model does not load it
    /// again - the reference catalogue's own sentence.
    #[test]
    fn the_catalogue_says_a_direct_invocation_needs_no_second_load() {
        let catalog = super::render_skill_catalog(&[skill("alpha", "does alpha")], "alpha");
        assert!(
            catalog.contains("<skill_content>"),
            "the block the gesture produces is named: {catalog}"
        );
        assert!(
            catalog.contains("do not call load_skill again"),
            "a direct invocation is not loaded twice: {catalog}"
        );
    }
}
