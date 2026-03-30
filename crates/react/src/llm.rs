use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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
        id: String,
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
            id: id.into(),
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

#[derive(Clone)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<LlmMessage>,
    pub tools: Option<Arc<Vec<Value>>>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

impl LlmRequest {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: Vec::new(),
            tools: None,
            temperature: 0.7,
            max_tokens: None,
        }
    }

    pub fn with_user(model: impl Into<String>, content: impl Into<String>) -> Self {
        Self::new(model).user_message(content)
    }

    pub fn message(mut self, msg: LlmMessage) -> Self {
        self.messages.push(msg);
        self
    }

    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(LlmMessage::user(content));
        self
    }

    pub fn system_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(LlmMessage::system(content));
        self
    }

    pub fn messages(mut self, msgs: impl IntoIterator<Item = LlmMessage>) -> Self {
        self.messages.extend(msgs);
        self
    }

    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    pub fn tools(mut self, tools: Vec<Value>) -> Self {
        self.tools = Some(Arc::new(tools));
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
            messages: &'a [LlmMessage],
            tools: Option<&'a [Value]>,
        }

        let rounded_temperature = (self.temperature as f64 * 1000.0).round() / 1000.0;
        let payload = PrettyRequest {
            model: &self.model,
            temperature: rounded_temperature,
            max_tokens: self.max_tokens,
            messages: &self.messages,
            tools: self.tools.as_ref().map(|t| t.as_ref().as_slice()),
        };

        match serde_yaml::to_string(&payload) {
            Ok(yaml) => write!(f, "{}", yaml.trim_end()),
            Err(_) => f
                .debug_struct("LlmRequest")
                .field("model", &self.model)
                .field("temperature", &self.temperature)
                .field("max_tokens", &self.max_tokens)
                .field("messages", &self.messages)
                .field("tools", &self.tools)
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

pub mod vendor;
