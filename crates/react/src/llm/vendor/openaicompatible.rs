//! OpenAI-compatible chat-completion wire types and streaming extractors.

use crate::{JsonExtractor, StreamExtractor, StreamSpan};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A tool call requested by the model.
pub struct ToolCall {
    /// Provider-assigned tool call id.
    pub id: String,
    /// Call type, normally `function`.
    #[serde(rename = "type")]
    pub r#type: String, // "function"
    /// The function to call.
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A function name and its JSON-encoded arguments.
pub struct FunctionCall {
    /// Function name.
    pub name: Option<String>,
    /// JSON-encoded arguments.
    pub arguments: Option<String>, // JSON string
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Token usage for a chat completion.
pub struct Usage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens in the completion.
    pub completion_tokens: u32,
    /// Prompt plus completion tokens.
    pub total_tokens: u32,
    /// Breakdown of prompt tokens, when reported.
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Breakdown of prompt token usage.
pub struct PromptTokensDetails {
    /// Audio input tokens.
    #[serde(default)]
    pub audio_tokens: Option<u32>,
    /// Prompt tokens served from cache.
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Log probabilities for a response.
pub struct LogProbs {
    /// Per-token log probabilities.
    pub content: Option<Vec<LogProbContent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A log probability for one token.
pub struct LogProbContent {
    /// The token.
    pub token: String,
    /// Log probability of the token.
    pub logprob: f32,
    /// UTF-8 bytes of the token.
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
/// A streamed fragment of a function call.
pub struct FunctionCallDelta {
    /// Function name, when present.
    pub name: Option<String>,
    /// A fragment of the JSON arguments.
    pub arguments: Option<String>, // streamed JSON fragments
}

#[derive(Debug, Serialize, Deserialize)]
/// A streamed fragment of a tool call.
pub struct ToolCallDelta {
    /// Index of the tool call within the message.
    pub index: Option<u32>,
    /// Provider-assigned tool call id.
    pub id: Option<String>,
    /// Call type, normally `function`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Function fragment.
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Serialize, Deserialize)]
/// The incremental content of a streamed chunk.
pub struct Delta {
    /// Role of the message, when it first appears.
    pub role: Option<String>,
    /// A fragment of assistant text.
    pub content: Option<String>,
    /// Tool call fragments.
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    /// Legacy function call fragment.
    pub function_call: Option<FunctionCallDelta>,
    /// A fragment of reasoning text.
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
/// One choice within a streamed chunk.
pub struct ChunkChoice {
    /// Index of the choice.
    pub index: u32,
    /// Incremental content.
    pub delta: Delta,
    /// Why generation stopped, when finished.
    pub finish_reason: Option<String>,
    /// Log probabilities, when requested.
    pub logprobs: Option<LogProbs>,
}

#[derive(Debug, Serialize, Deserialize)]
/// One SSE chunk of a chat completion.
pub struct ChatCompletionChunk {
    /// Completion id.
    pub id: String,
    /// Object type, normally `chat.completion.chunk`.
    pub object: String,
    /// Unix creation timestamp.
    pub created: u64,
    /// Model that served the request.
    pub model: String,
    /// Choices in this chunk.
    pub choices: Vec<ChunkChoice>,
    /// Token usage, usually only in the final chunk.
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A chat message in an OpenAI request or response.
pub struct ChatMessage {
    /// Message role.
    pub role: String,
    /// Message text, if any.
    pub content: Option<String>,

    /// Tool calls requested by the model.
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,

    /// Legacy function call.
    #[serde(default)]
    pub function_call: Option<FunctionCall>,

    /// Reasoning content from models like Claude/DeepSeek (non-streaming)
    #[serde(default)]
    pub reasoning_content: Option<String>,

    /// Any additional provider fields.
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// One completion choice.
pub struct Choice {
    /// Index of the choice.
    pub index: u32,
    /// The generated message.
    pub message: ChatMessage,
    /// Why generation stopped.
    pub finish_reason: Option<String>,
    /// Provider-specific numeric stop reason.
    #[serde(default)]
    pub stop_reason: Option<u32>,
    /// Log probabilities, when requested.
    pub logprobs: Option<LogProbs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A non-streaming chat completion response.
pub struct ChatCompletionResponse {
    /// Completion id.
    pub id: String,
    /// Object type, normally `chat.completion`.
    pub object: String, // "chat.completion"
    /// Unix creation timestamp.
    pub created: u64,
    /// Model that served the request.
    pub model: String,
    /// Generated choices.
    pub choices: Vec<Choice>,
    /// Token usage.
    pub usage: Option<Usage>,
    /// Backend configuration fingerprint.
    pub system_fingerprint: Option<String>,
    /// NVIDIA-specific extension fields.
    #[serde(default)]
    pub nvext: Option<serde_json::Value>,
}

/// Extracts chat completion chunks from an SSE stream.
pub struct OpenAIExtractor {
    inner: JsonExtractor,
}

impl OpenAIExtractor {
    /// Wrap a JSON extractor.
    pub fn new(inner: JsonExtractor) -> Self {
        Self { inner }
    }
}

impl StreamExtractor for OpenAIExtractor {
    type Item<'a> = ChatCompletionChunk;

    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        let spans = self.inner.push(chunk)?;
        let mut chats = Vec::new();

        for span in spans.iter() {
            if span.is_root() {
                let json_str = self.inner.extract(span);
                if let Ok(chat) = serde_json::from_slice::<ChatCompletionChunk>(json_str) {
                    chats.push(chat);
                }
            }
        }

        if chats.is_empty() {
            None
        } else {
            Some(chats)
        }
    }

    fn extract<'a>(&'a self, span: &StreamSpan) -> &'a [u8] {
        self.inner.extract(span)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// Check if raw SSE text contains the `[DONE]` stream termination signal.
///
/// The SSE protocol defines `[DONE]` as a standalone line: `data: [DONE]`.
/// We check that after stripping `data:` and trimming, the payload is exactly `[DONE]`.
pub fn sse_has_done_signal(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let payload = trimmed.strip_prefix("data:").unwrap_or(trimmed).trim();
        payload == "[DONE]"
    })
}

/// Accumulates streaming tool call arguments that arrive as partial JSON fragments.
///
/// OpenAI-compatible streaming splits `tool_calls[N].function.arguments` across
/// multiple SSE chunks. This accumulator concatenates the fragments by tool call
/// index and emits the completed call once arguments form valid JSON.
#[derive(Debug, Default)]
pub struct StreamToolCallAccumulator {
    pending: HashMap<u32, PendingToolCall>,
}

/// A single pending (incomplete) streaming tool call.
#[derive(Debug)]
/// A single pending (incomplete) streaming tool call.
pub struct PendingToolCall {
    /// Tool name.
    pub name: Option<String>,
    /// Tool call id.
    pub id: Option<String>,
    /// Accumulated JSON argument fragments.
    pub arguments: String,
}

impl StreamToolCallAccumulator {
    /// Create an empty accumulator.
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Push one delta fragment for a tool call.
    ///
    /// Returns `Some((name, args_json, id))` when the accumulated arguments
    /// parse as valid JSON. Once emitted, the entry is removed from pending.
    pub fn push_delta(
        &mut self,
        index: u32,
        id: Option<String>,
        name: Option<String>,
        args_delta: &str,
    ) -> Option<(String, serde_json::Value, Option<String>)> {
        let entry = self
            .pending
            .entry(index)
            .or_insert_with(|| PendingToolCall {
                name: None,
                id: None,
                arguments: String::new(),
            });
        if let Some(n) = name {
            entry.name = Some(n);
        }
        if let Some(i) = id {
            entry.id = Some(i);
        }
        entry.arguments.push_str(args_delta);

        if let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&entry.arguments) {
            if let (Some(n), Some(i)) = (entry.name.as_ref(), entry.id.as_ref()) {
                let result = (n.clone(), args_val, Some(i.clone()));
                self.pending.remove(&index);
                return Some(result);
            }
        }
        None
    }

    /// Drain all pending tool calls (finish_reason or stream end).
    /// Incomplete arguments fall back to `{}`.
    pub fn drain(&mut self) -> Vec<(String, serde_json::Value, Option<String>)> {
        let mut results = Vec::new();
        for (_, entry) in self.pending.drain() {
            let name = entry.name.unwrap_or_default();
            let id = entry.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let args_val = serde_json::from_str(&entry.arguments).unwrap_or(serde_json::json!({}));
            results.push((name, args_val, Some(id)));
        }
        results
    }

    /// Whether any tool calls are pending.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Number of pending tool calls.
    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(content: &str, finish_reason: Option<&str>) -> String {
        serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "delta": { "content": content },
                "finish_reason": finish_reason,
                "logprobs": null
            }]
        })
        .to_string()
    }

    #[test]
    fn extracts_all_coalesced_sse_json_chunks() {
        let mut extractor = OpenAIExtractor::new(JsonExtractor::default());
        let payload = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\n",
            chunk("The", None),
            chunk(" result is 100.", None),
            chunk("", Some("stop"))
        );

        let chunks = extractor.push(&payload).unwrap();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("The"));
        assert_eq!(
            chunks[1].choices[0].delta.content.as_deref(),
            Some(" result is 100.")
        );
        assert_eq!(chunks[2].choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn accumulator_single_chunk_completes_tool_call() {
        let mut acc = StreamToolCallAccumulator::new();
        let result = acc.push_delta(
            0,
            Some("call_abc".into()),
            Some("get_weather".into()),
            r#"{"location": "NYC"}"#,
        );
        assert!(result.is_some());
        let (name, args, id) = result.unwrap();
        assert_eq!(name, "get_weather");
        assert_eq!(args, serde_json::json!({"location": "NYC"}));
        assert_eq!(id, Some("call_abc".into()));
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_multiple_chunks_accumulate_until_complete() {
        let mut acc = StreamToolCallAccumulator::new();

        let r1 = acc.push_delta(0, Some("call_abc".into()), Some("get_weather".into()), "");
        assert!(r1.is_none());
        assert_eq!(acc.len(), 1);

        let r2 = acc.push_delta(0, None, None, r#"{"loc"#);
        assert!(r2.is_none());
        assert_eq!(acc.len(), 1);

        let r3 = acc.push_delta(0, None, None, r#"ation": "NYC"}"#);
        assert!(r3.is_some());
        let (name, args, _) = r3.unwrap();
        assert_eq!(name, "get_weather");
        assert_eq!(args, serde_json::json!({"location": "NYC"}));
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_drain_flushes_incomplete_calls() {
        let mut acc = StreamToolCallAccumulator::new();
        acc.push_delta(
            0,
            Some("call_abc".into()),
            Some("get_weather".into()),
            r#"{"loc"#,
        );
        assert_eq!(acc.len(), 1);

        let drained = acc.drain();
        assert_eq!(drained.len(), 1);
        let (name, args, id) = &drained[0];
        assert_eq!(name, "get_weather");
        assert_eq!(id.as_deref(), Some("call_abc"));
        // Incomplete args fall back to {}
        assert_eq!(args, &serde_json::json!({}));
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_multiple_parallel_tool_calls() {
        let mut acc = StreamToolCallAccumulator::new();

        // Two tool calls interleaved at different indices
        acc.push_delta(
            0,
            Some("call_0".into()),
            Some("get_weather".into()),
            r#"{"loc"#,
        );
        acc.push_delta(1, Some("call_1".into()), Some("search".into()), r#"{"q"#);
        assert_eq!(acc.len(), 2);

        let r0 = acc.push_delta(0, None, None, r#"ation": "NYC"}"#);
        assert!(r0.is_some());
        assert_eq!(acc.len(), 1);

        let r1 = acc.push_delta(1, None, None, r#"uery": "rust"}"#);
        assert!(r1.is_some());
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_drain_assigns_uuid_when_id_missing() {
        let mut acc = StreamToolCallAccumulator::new();
        acc.push_delta(0, None, Some("no_id_tool".into()), r#"{}"#);
        let drained = acc.drain();
        assert_eq!(drained.len(), 1);
        let (name, _, id) = &drained[0];
        assert_eq!(name, "no_id_tool");
        assert!(id.is_some());
        assert!(!id.as_deref().unwrap().is_empty());
    }

    #[test]
    fn extracts_usage_from_final_chunk() {
        let mut extractor = OpenAIExtractor::new(JsonExtractor::default());
        let usage_chunk = serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "test-model",
            "choices": [],
            "usage": {
                "prompt_tokens": 320,
                "completion_tokens": 58,
                "total_tokens": 378,
                "prompt_tokens_details": { "cached_tokens": 16 }
            }
        })
        .to_string();
        let payload = format!("data: {}\n\n", usage_chunk);

        let chunks = extractor.push(&payload).unwrap();

        assert_eq!(chunks.len(), 1);
        let usage = chunks[0].usage.as_ref().expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 320);
        assert_eq!(usage.completion_tokens, 58);
        assert_eq!(usage.total_tokens, 378);
        assert_eq!(
            usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens),
            Some(16)
        );
    }
}
