//! The Anthropic Messages API vendor client.
//!
//! Wire contract: POST {endpoint}/v1/messages with the x-api-key and
//! anthropic-version headers, a required max_tokens field, the system prompt at
//! the top level rather than as a message role, tools shaped as name /
//! description / input_schema, and images as base64 source blocks.
//!
//! Streaming is Server-Sent Events whose frames carry an event: line beside the
//! data: one, and whose tool arguments arrive as a JSON string split across
//! input_json_delta fragments - so they are accumulated per content block and
//! parsed only at content_block_stop, never per fragment. The full contract is
//! written down in crates/bsh/docs/anthropic-vendor-plan.md.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::openaicompatible::{ChatCompletionResponse, Usage};
use crate::llm::types::{BinarySource, LlmMessage};
use crate::llm::{
    Content, ContentPart, LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult,
    ReactContext, ReactSession, StreamToken, TokenStream, VendorBuilderError,
};

/// The Anthropic API version this client speaks.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Token cap used when a request carries none.
///
/// The Messages API requires the field, so a request without one is sent with
/// this cap rather than with a null the provider would reject.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// One message in an Anthropic conversation.
///
/// The content is a list of typed blocks (text, image, tool_use, tool_result),
/// which is why it is carried as a JSON value: only this protocol has those
/// block names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    /// Either user or assistant.
    pub role: String,
    /// A list of typed content blocks.
    pub content: Value,
}

/// One tool offered to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON schema for the tool arguments.
    pub input_schema: Value,
}

/// A request body for POST /v1/messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicRequest {
    /// Model identifier.
    pub model: String,
    /// Response token cap. Required by the wire.
    pub max_tokens: u32,
    /// The system prompt, at the top level rather than as a message role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The conversation.
    pub messages: Vec<AnthropicMessage>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Tools offered to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    /// Whether the response is streamed.
    pub stream: bool,
}

/// Splits an Anthropic SSE byte stream into complete data frames.
///
/// Anthropic frames carry an event: line beside the data: one, and a frame can
/// be split across transport chunks, so bytes are buffered until a blank line
/// closes a frame and only then parsed.
#[derive(Default)]
pub struct AnthropicSseDecoder {
    buf: String,
}

impl AnthropicSseDecoder {
    /// Push a raw chunk and take the JSON payload of every complete frame.
    pub fn push(&mut self, chunk: &str) -> Vec<Value> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find("\n\n") {
            let frame: String = self.buf.drain(..pos + 2).collect();
            for line in frame.lines() {
                let Some(rest) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = rest.trim();
                if payload.is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(payload) {
                    out.push(value);
                }
            }
        }
        out
    }
}

/// A tool call whose arguments are still streaming in.
struct PendingTool {
    id: Option<String>,
    name: String,
    json: String,
}

/// Accumulates streamed tool-input fragments per content block.
///
/// This is the protocol's one real trap: input_json_delta carries a fragment of
/// a JSON string, so the arguments are only valid once the block closes.
#[derive(Default)]
pub struct AnthropicBlockAccumulator {
    pending: HashMap<u64, PendingTool>,
}

impl AnthropicBlockAccumulator {
    /// Record a tool_use block that started at an index.
    pub fn start(&mut self, index: u64, id: Option<String>, name: String) {
        self.pending.insert(
            index,
            PendingTool {
                id,
                name,
                json: String::new(),
            },
        );
    }

    /// Append an input_json_delta fragment to the block at an index.
    pub fn push_json(&mut self, index: u64, fragment: &str) {
        if let Some(pending) = self.pending.get_mut(&index) {
            pending.json.push_str(fragment);
        }
    }

    /// Close the block at an index, parsing its accumulated arguments.
    pub fn finish(&mut self, index: u64) -> Option<(String, Value, Option<String>)> {
        let pending = self.pending.remove(&index)?;
        if pending.name.is_empty() {
            return None;
        }
        let args = if pending.json.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&pending.json).unwrap_or(Value::Null)
        };
        Some((pending.name, args, pending.id))
    }
}

/// An Anthropic Messages API vendor client.
pub struct AnthropicVendor {
    client: Arc<Client>,
    endpoint: Arc<String>,
    model: String,
    api_key: Arc<String>,
}

impl Clone for AnthropicVendor {
    fn clone(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            endpoint: self.endpoint.clone(),
            model: self.model.clone(),
            api_key: Arc::clone(&self.api_key),
        }
    }
}

impl AnthropicVendor {
    /// Build a vendor from an HTTP client, an endpoint, a default model and an
    /// API key.
    pub fn new(
        client: Arc<Client>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            client,
            endpoint: Arc::new(endpoint.into()),
            model: model.into(),
            api_key: Arc::new(api_key.into()),
        }
    }

    /// The Messages endpoint URL.
    ///
    /// An endpoint that already ends in /v1 is respected, so both
    /// https://api.anthropic.com and https://api.anthropic.com/v1 work.
    pub fn messages_url(&self) -> String {
        let base = self.endpoint.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{}/messages", base)
        } else {
            format!("{}/v1/messages", base)
        }
    }

    /// Map the library message content onto Anthropic content blocks.
    fn content_blocks(content: &Content) -> Value {
        match content {
            Content::Text(text) => json!([{ "type": "text", "text": text }]),
            Content::Parts(parts) => Value::Array(
                parts
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text } => json!({ "type": "text", "text": text }),
                        ContentPart::Binary { binary } => match &binary.source {
                            BinarySource::Base64(data) => json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": binary.content_type,
                                    "data": data,
                                }
                            }),
                            BinarySource::Url(url) => json!({
                                "type": "image",
                                "source": { "type": "url", "url": url }
                            }),
                        },
                    })
                    .collect(),
            ),
        }
    }

    /// Build the request body for one streaming call.
    fn build_request<S: ReactSession, C: ReactContext>(
        &self,
        persona: &Option<String>,
        request: &LlmRequest,
        session: &S,
        context: &C,
    ) -> AnthropicRequest {
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };

        let mut system_parts: Vec<String> = Vec::new();
        if let Some(persona) = persona {
            if !persona.is_empty() {
                system_parts.push(persona.clone());
            }
        }
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        if let Some(history) = session.history() {
            for message in history {
                match message {
                    LlmMessage::System { content } => system_parts.push(content.clone()),
                    LlmMessage::User { content } => messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: Self::content_blocks(content),
                    }),
                    LlmMessage::Assistant { content } => messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: json!([{ "type": "text", "text": content }]),
                    }),
                    LlmMessage::AssistantToolCall {
                        tool_call_id,
                        name,
                        args,
                    } => messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: json!([{
                            "type": "tool_use",
                            "id": tool_call_id,
                            "name": name,
                            "input": args,
                        }]),
                    }),
                    LlmMessage::ToolResult {
                        tool_call_id,
                        content,
                    } => messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: json!([{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }]),
                    }),
                }
            }
        }

        let tools: Vec<AnthropicTool> = context
            .tools()
            .map(|tools| {
                tools
                    .iter()
                    .map(|tool| AnthropicTool {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.parameters.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        AnthropicRequest {
            model,
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system: if system_parts.is_empty() {
                None
            } else {
                Some(system_parts.join("\n"))
            },
            messages,
            temperature: request.temperature,
            top_p: request.top_p,
            tools: if tools.is_empty() { None } else { Some(tools) },
            stream: true,
        }
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for AnthropicVendor
{
    async fn complete(
        &self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };

        let mut stream = self
            .stream_complete(persona, request, &mut *session, &mut *context)
            .await?;

        let mut text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut usage_in = 0u32;
        let mut usage_out = 0u32;
        while let Some(item) = stream.next().await {
            match item? {
                StreamToken::Text(chunk) => text.push_str(&chunk),
                StreamToken::ToolCall { name, args, id } => tool_calls.push(json!({
                    "id": id.unwrap_or_default(),
                    "type": "function",
                    "function": { "name": name, "arguments": args.to_string() }
                })),
                StreamToken::Usage(usage) => {
                    usage_in = usage.prompt_tokens;
                    usage_out = usage.completion_tokens;
                }
                _ => {}
            }
        }

        let content = if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        };
        let finish = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        let message = if tool_calls.is_empty() {
            json!({ "role": "assistant", "content": content })
        } else {
            json!({ "role": "assistant", "content": content, "tool_calls": tool_calls })
        };
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let body = json!({
            "id": "anthropic-message",
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "message": message,
                "finish_reason": finish,
            }],
            "usage": {
                "prompt_tokens": usage_in,
                "completion_tokens": usage_out,
                "total_tokens": usage_in + usage_out,
            },
        });
        let parsed: ChatCompletionResponse =
            serde_json::from_value(body).map_err(|e| LlmError::Parse(e.to_string()))?;
        let response = LlmResponse::OpenAI(parsed);
        context.notify_response(&response);
        Ok(response)
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        if request.model.is_empty() {
            request.model = self.model.clone();
        }
        if self.api_key.is_empty() {
            let err = LlmError::ApiKeyMissing;
            context.notify_error(&err);
            return Err(err);
        }

        context.notify_request(&request);

        let body = self.build_request(&persona, &request, session, context);
        let url = self.messages_url();
        info!(
            "anthropic request: {}",
            serde_json::to_string(&body).unwrap_or_default()
        );

        let response = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let err = LlmError::Other(format!("Request failed: {}", e));
                context.notify_error(&err);
                err
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let err = LlmError::Http(format!("HTTP {}: {}", status, text));
            context.notify_error(&err);
            return Err(err);
        }

        let (tx, rx) = mpsc::channel(32);
        let on_chunk = context.on_chunk_callback();

        tokio::spawn(async move {
            let mut bytes = response.bytes_stream();
            let mut decoder = AnthropicSseDecoder::default();
            let mut blocks = AnthropicBlockAccumulator::default();
            let mut input_tokens = 0u32;
            let mut output_tokens = 0u32;

            while let Some(chunk) = bytes.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        let _ = tx
                            .send(Err(LlmError::Other(format!("stream failed: {}", e))))
                            .await;
                        return;
                    }
                };

                for frame in decoder.push(&String::from_utf8_lossy(&chunk)) {
                    let kind = frame.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "message_start" => {
                            if let Some(n) = frame
                                .pointer("/message/usage/input_tokens")
                                .and_then(Value::as_u64)
                            {
                                input_tokens = n as u32;
                            }
                        }
                        "content_block_start" => {
                            if frame.pointer("/content_block/type").and_then(Value::as_str)
                                == Some("tool_use")
                            {
                                let index = frame.get("index").and_then(Value::as_u64).unwrap_or(0);
                                let name = frame
                                    .pointer("/content_block/name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                let id = frame
                                    .pointer("/content_block/id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string);
                                blocks.start(index, id, name);
                            }
                        }
                        "content_block_delta" => {
                            let index = frame.get("index").and_then(Value::as_u64).unwrap_or(0);
                            let delta = frame
                                .pointer("/delta/type")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            match delta {
                                "text_delta" => {
                                    if let Some(text) =
                                        frame.pointer("/delta/text").and_then(Value::as_str)
                                    {
                                        if !text.is_empty() {
                                            on_chunk.as_ref().inspect(|cb| cb(text));
                                            let _ = tx
                                                .send(Ok(StreamToken::Text(text.to_string())))
                                                .await;
                                        }
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(text) =
                                        frame.pointer("/delta/thinking").and_then(Value::as_str)
                                    {
                                        if !text.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamToken::ReasoningContent(
                                                    text.to_string(),
                                                )))
                                                .await;
                                        }
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(fragment) =
                                        frame.pointer("/delta/partial_json").and_then(Value::as_str)
                                    {
                                        blocks.push_json(index, fragment);
                                    }
                                }
                                _ => {}
                            }
                        }
                        "content_block_stop" => {
                            let index = frame.get("index").and_then(Value::as_u64).unwrap_or(0);
                            if let Some((name, args, id)) = blocks.finish(index) {
                                on_chunk.as_ref().inspect(|cb| cb(&name));
                                let _ = tx.send(Ok(StreamToken::ToolCall { name, args, id })).await;
                            }
                        }
                        "message_delta" => {
                            if let Some(n) = frame
                                .pointer("/usage/output_tokens")
                                .and_then(Value::as_u64)
                            {
                                output_tokens = n as u32;
                            }
                        }
                        "message_stop" => {
                            let _ = tx
                                .send(Ok(StreamToken::Usage(Usage {
                                    prompt_tokens: input_tokens,
                                    completion_tokens: output_tokens,
                                    total_tokens: input_tokens + output_tokens,
                                    prompt_tokens_details: None,
                                })))
                                .await;
                            let _ = tx.send(Ok(StreamToken::Done)).await;
                            return;
                        }
                        "error" => {
                            let message = frame
                                .pointer("/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("anthropic stream error");
                            let _ = tx.send(Err(LlmError::Other(message.to_string()))).await;
                            return;
                        }
                        _ => {}
                    }
                }
            }

            let _ = tx.send(Ok(StreamToken::Done)).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "anthropic"
    }
}

/// Builder for [`AnthropicVendor`].
pub struct AnthropicVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl Default for AnthropicVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicVendorBuilder {
    /// Create a builder with Anthropic defaults.
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.anthropic.com".to_string(),
            model: "claude-3-5-sonnet-latest".to_string(),
            api_key: None,
        }
    }

    /// Set the API endpoint.
    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Set the default model id.
    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    /// Build the vendor.
    pub fn build(self) -> Result<AnthropicVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(AnthropicVendor::new(
            Arc::new(Client::new()),
            self.endpoint,
            self.model,
            api_key,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnthropicBlockAccumulator, AnthropicSseDecoder, AnthropicVendor, DEFAULT_MAX_TOKENS,
    };
    use crate::llm::{ApiMode, Content, LlmRequest};
    use serde_json::json;
    use std::sync::Arc;

    fn vendor() -> AnthropicVendor {
        AnthropicVendor::new(
            Arc::new(reqwest::Client::new()),
            "https://api.anthropic.com",
            "claude-3-5-sonnet-latest",
            "sk-test",
        )
    }

    fn request(max_tokens: Option<u32>) -> LlmRequest {
        LlmRequest {
            model: String::new(),
            input: Content::text("hello"),
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens,
            reasoning_effort: None,
            api_mode: ApiMode::Anthropic,
        }
    }

    #[test]
    fn the_endpoint_gains_v1_messages_once() {
        assert_eq!(
            vendor().messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
        let with_v1 = AnthropicVendor::new(
            Arc::new(reqwest::Client::new()),
            "https://api.anthropic.com/v1",
            "m",
            "k",
        );
        assert_eq!(
            with_v1.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn the_decoder_reads_data_lines_across_a_split_frame() {
        let mut decoder = AnthropicSseDecoder::default();
        let first = decoder.push("event: message_start\ndata: {\"type\":\"message_start\"");
        assert!(first.is_empty());
        let frames = decoder.push("}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["type"], json!("message_start"));
    }

    #[test]
    fn tool_input_fragments_accumulate_until_the_block_closes() {
        let mut blocks = AnthropicBlockAccumulator::default();
        blocks.start(1, Some("toolu_1".to_string()), "add".to_string());
        blocks.push_json(1, "{\"a\":");
        assert!(blocks.pending.contains_key(&1));
        blocks.push_json(1, "2}");
        let (name, args, id) = blocks.finish(1).expect("the block yields one call");
        assert_eq!(name, "add");
        assert_eq!(args, json!({ "a": 2 }));
        assert_eq!(id.as_deref(), Some("toolu_1"));
        assert!(blocks.finish(1).is_none());
    }

    #[test]
    fn the_body_puts_the_system_prompt_top_level_and_caps_tokens() {
        let vendor = vendor();
        let persona = Some("be brief".to_string());
        let body = vendor.build_request(&persona, &request(None), &(), &());
        assert_eq!(body.system.as_deref(), Some("be brief"));
        assert_eq!(body.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(body.model, "claude-3-5-sonnet-latest");
        assert!(body.stream);
        assert!(body.tools.is_none());
        assert!(body.messages.is_empty());

        let capped = vendor.build_request(&None, &request(Some(64)), &(), &());
        assert_eq!(capped.max_tokens, 64);
        assert!(capped.system.is_none());
    }
}
