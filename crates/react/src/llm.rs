use std::fmt;
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

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

pub trait Stringfy: Serialize + for<'de> Deserialize<'de> {
    fn yaml(&self) -> String;
    fn json(&self) -> String;

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

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            LlmError::Timeout
        } else {
            LlmError::Http(e.to_string())
        }
    }
}

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

pub type OpenAiMessage = LlmMessage;

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

#[derive(Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub model: String,
    pub context: LlmContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Skill {
    pub category: String,
    pub description: String,
    pub name: String,
}

impl Stringfy for Skill {
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }

    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl Stringfy for LlmTool {
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }

    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub content: String,
}

impl Stringfy for Rule {
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }

    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Instruction {
    pub instruction: String,
    pub description: String,
    pub name: String,
    pub dependon: Option<Vec<String>>,
}

impl Stringfy for Instruction {
    fn yaml(&self) -> String {
        serde_yaml::to_string(&self).unwrap()
    }

    fn json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
}

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

impl LlmRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            context: LlmContext {
                tools: Vec::new(),
                skills: Vec::new(),
                conversations: Vec::new(),
                rules: Vec::new(),
                instructions: Vec::new(),
            },
            temperature: Some(0.7),
            max_tokens: None,
            top_p: None,
            top_k: None,
        }
    }

    pub fn with_user(model: impl Into<String>, content: impl Into<String>) -> Self {
        Self::new(model).user_message(content)
    }

    pub fn message(mut self, msg: LlmMessage) -> Self {
        self.context.conversations.push(msg);
        self
    }

    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.context.conversations.push(LlmMessage::user(content));
        self
    }

    pub fn system_message(mut self, content: impl Into<String>) -> Self {
        self.context.conversations.push(LlmMessage::system(content));
        self
    }

    pub fn messages(mut self, msgs: impl IntoIterator<Item = LlmMessage>) -> Self {
        self.context.conversations.extend(msgs);
        self
    }

    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    pub fn tools(mut self, tools: Vec<LlmTool>) -> Self {
        self.context.tools = tools;
        self
    }

    pub fn skills(mut self, skills: Vec<Skill>) -> Self {
        self.context.skills = skills;
        self
    }

    pub fn rules(mut self, rules: Vec<Rule>) -> Self {
        self.context.rules = rules;
        self
    }

    pub fn instructions(mut self, instructions: Vec<Instruction>) -> Self {
        self.context.instructions = instructions;
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

impl fmt::Debug for LlmRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[derive(Serialize)]
        struct PrettyRequest<'a> {
            model: &'a str,
            temperature: f64,
            max_tokens: Option<u32>,
            context: &'a LlmContext,
        }

        let rounded_temperature = self
            .temperature
            .map(|t| (t as f64 * 1000.0).round() / 1000.0);
        let payload = PrettyRequest {
            model: &self.model,
            temperature: rounded_temperature.unwrap_or(0.7),
            max_tokens: self.max_tokens,
            context: &self.context,
        };

        match serde_yaml::to_string(&payload) {
            Ok(yaml) => write!(f, "{}", yaml.trim_end()),
            Err(_) => f
                .debug_struct("LlmRequest")
                .field("model", &self.model)
                .field("temperature", &self.temperature)
                .field("top_p", &self.top_p)
                .field("top_k", &self.top_k)
                .field("max_tokens", &self.max_tokens)
                .field("context", &self.context)
                .field("rules", &self.context.rules)
                .field("instructions", &self.context.instructions)
                .finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum LlmResponse {
    Text(String),
    Partial(String),
    ToolCall {
        name: String,
        args: Value,
        id: Option<String>,
    },
    Done,
}

#[derive(Debug, Clone)]
pub enum StreamToken {
    Text(String),
    ToolCall {
        name: String,
        args: Value,
        id: Option<String>,
    },
    Done,
}

/// Accumulates tool call data and invokes handler with raw accumulated data.
/// Handler is responsible for parsing and returning StreamToken.
pub struct StreamResponseAccumulator<F, T = StreamToken> {
    response: String,
    index: usize,
    handler: F,
    _marker: std::marker::PhantomData<T>,
}

impl<F, T> StreamResponseAccumulator<F, T>
where
    F: FnMut(&str, usize) -> (usize, Option<Vec<T>>),
{
    pub fn new(handler: F) -> Self {
        Self {
            response: String::new(),
            index: 0,
            handler,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Push a chunk of tool call data. Handler is called to try parse accumulated arguments.
    pub fn push(&mut self, chunk: &str) -> Option<Vec<T>> {
        self.response.push_str(chunk);
        let (index, token) = (self.handler)(&self.response, self.index);
        self.index = index;
        token
    }

    pub fn reset(&mut self) {
        self.response.clear();
        self.index = 0;
    }
}

pub type LlmResponseResult = Result<LlmResponse, LlmError>;
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>;

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest) -> LlmResponseResult;
    async fn stream_complete(&self, req: LlmRequest) -> Result<TokenStream, LlmError>;
    fn supports_tools(&self) -> bool;
    fn provider_name(&self) -> &'static str;
}

pub type LlmResponseResultFuture<'a> = Pin<Box<dyn Future<Output = LlmResponseResult> + Send + 'a>>;

pub struct ModelFallback {
    primary: Box<dyn LlmClient>,
    fallback: Box<dyn LlmClient>,
    fallback_on_error: bool,
}

impl ModelFallback {
    pub fn new(primary: Box<dyn LlmClient>, fallback: Box<dyn LlmClient>) -> Self {
        Self {
            primary,
            fallback,
            fallback_on_error: true,
        }
    }

    pub fn with_fallback_enabled(mut self, enabled: bool) -> Self {
        self.fallback_on_error = enabled;
        self
    }
}

#[async_trait]
impl LlmClient for ModelFallback {
    async fn complete(&self, req: LlmRequest) -> LlmResponseResult {
        let result = self.primary.complete(req.clone()).await;
        if result.is_err() && self.fallback_on_error {
            self.fallback.complete(req).await
        } else {
            result
        }
    }

    async fn stream_complete(&self, req: LlmRequest) -> Result<TokenStream, LlmError> {
        let result = self.primary.stream_complete(req.clone()).await;
        if result.is_err() && self.fallback_on_error {
            self.fallback.stream_complete(req).await
        } else {
            result
        }
    }

    fn supports_tools(&self) -> bool {
        self.primary.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        "model_fallback"
    }
}

pub mod vendor;
