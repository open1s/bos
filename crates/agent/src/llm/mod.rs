use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

pub(crate) use crate::error::LlmError;

pub mod client;

pub use client::OpenAiClient;

#[derive(Debug, Clone)]
pub enum LlmResponse {
    Text(String),
    Patial(String),
    ToolCall { name: String, args: serde_json::Value },
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum OpenAiMessage {
    System { content: String },
    User { content: String },
    Assistant { content: String },
    ToolResult { tool_call_id: String, content: String },
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    pub tools: Option<Arc<Vec<serde_json::Value>>>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

#[derive(Debug)]
pub enum StreamToken {
    Text(String),
    ToolCall { name: String, args: serde_json::Value },
    Done,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    fn stream_complete(
        &self,
        req: LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send + '_>>;
    fn supports_tools(&self) -> bool;
    fn provider_name(&self) -> &'static str;
}
