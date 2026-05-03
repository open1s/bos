use crate::{JsonExtractor, StreamExtractor, StreamSpan};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String, // "function"
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>, // JSON string
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub audio_tokens: Option<u32>,
    #[serde(default)]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogProbs {
    pub content: Option<Vec<LogProbContent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogProbContent {
    pub token: String,
    pub logprob: f32,
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    pub name: Option<String>,
    pub arguments: Option<String>, // streamed JSON fragments
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: Option<u32>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    pub function_call: Option<FunctionCallDelta>,
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
    pub logprobs: Option<LogProbs>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,

    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,

    #[serde(default)]
    pub function_call: Option<FunctionCall>,

    /// Reasoning content from models like Claude/DeepSeek (non-streaming)
    #[serde(default)]
    pub reasoning_content: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<u32>,
    pub logprobs: Option<LogProbs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String, // "chat.completion"
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
    pub system_fingerprint: Option<String>,
    #[serde(default)]
    pub nvext: Option<serde_json::Value>,
}

pub struct OpenAIExtractor {
    inner: JsonExtractor,
}

impl OpenAIExtractor {
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
}
