use crate::telemetry::TokenUsage;
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;

use super::types::LlmError;

pub type LlmResponseResult = Result<LlmResponse, LlmError>;
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>;

#[derive(Debug, Clone)]
pub enum LlmResponse {
    OpenAI(ChatCompletionResponse),
}

impl LlmResponse {
    pub fn usage(&self) -> Option<TokenUsage> {
        match self {
            LlmResponse::OpenAI(rsp) => rsp
                .usage
                .as_ref()
                .map(|u| TokenUsage::new(u.prompt_tokens, u.completion_tokens)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamToken {
    Text(String),
    ReasoningContent(String),
    ToolCall {
        name: String,
        args: Value,
        id: Option<String>,
    },
    Done,
}

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

pub use crate::llm::vendor::openaicompatible::{
    ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice, ChunkChoice, Delta,
    FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, ToolCall, ToolCallDelta, Usage,
};
