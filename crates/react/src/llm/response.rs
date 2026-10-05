//! Provider-agnostic responses and streamed tokens.

//! Provider-agnostic responses and streamed tokens.

use crate::telemetry::TokenUsage;
use futures::Stream;
use serde::Serialize;
use serde_json::Value;
use std::pin::Pin;

use super::types::LlmError;

/// Result of an LLM call.
pub type LlmResponseResult = Result<LlmResponse, LlmError>;
/// A stream of response tokens.
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>;

#[derive(Debug, Clone, Serialize)]
/// A provider response in one of the supported wire formats.
pub enum LlmResponse {
    /// An OpenAI chat-completions response.
    OpenAI(ChatCompletionResponse),
    /// An OpenAI Responses API response.
    Responses(ResponsesResponse),
}

impl LlmResponse {
    /// Token usage reported by the provider, if any.
    pub fn usage(&self) -> Option<TokenUsage> {
        match self {
            LlmResponse::OpenAI(rsp) => rsp
                .usage
                .as_ref()
                .map(|u| TokenUsage::new(u.prompt_tokens, u.completion_tokens)),
            LlmResponse::Responses(rsp) => rsp
                .chat_usage()
                .map(|u| TokenUsage::new(u.prompt_tokens, u.completion_tokens)),
        }
    }
}

#[derive(Debug, Clone)]
/// One token or control event in a streamed response.
pub enum StreamToken {
    /// A chunk of assistant text.
    Text(String),
    /// A chunk of reasoning text.
    ReasoningContent(String),
    /// A complete tool call.
    ToolCall {
        /// Tool name.
        name: String,
        /// Tool arguments.
        args: Value,
        /// Provider-assigned tool call id.
        id: Option<String>,
    },
    /// Final token usage for the response.
    Usage(super::vendor::openaicompatible::Usage),
    /// The stream finished normally.
    Done,
    /// The stream was stopped by the caller.
    Stopped,
}

/// Accumulates streamed text and yields parsed items.
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
    /// Create an accumulator with a parse handler.
    pub fn new(handler: F) -> Self {
        Self {
            response: String::new(),
            index: 0,
            handler,
            _marker: std::marker::PhantomData,
        }
    }
    /// Current byte index into the accumulated response.
    pub fn index(&self) -> usize {
        self.index
    }
    /// Append `chunk` and return any items the handler produced.
    pub fn push(&mut self, chunk: &str) -> Option<Vec<T>> {
        self.response.push_str(chunk);
        let (index, token) = (self.handler)(&self.response, self.index);
        self.index = index;
        token
    }
    /// Clear the accumulated response and index.
    pub fn reset(&mut self) {
        self.response.clear();
        self.index = 0;
    }
}

pub use crate::llm::vendor::openaicompatible::{
    ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice, ChunkChoice, Delta,
    FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, ToolCall, ToolCallDelta, Usage,
};
pub use crate::llm::vendor::responses::ResponsesResponse;
