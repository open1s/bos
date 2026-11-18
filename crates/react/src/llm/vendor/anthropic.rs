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
    ReactContext, ReactSession, ReasoningEffort, ReasoningState, StreamToken, TokenStream,
    VendorBuilderError,
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
    /// Extended thinking. Omitted unless the reader chose an effort level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinking>,
    /// Whether the response is streamed.
    pub stream: bool,
}

/// Anthropic's extended-thinking switch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicThinking {
    /// `enabled` or `disabled`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Token budget reserved for thinking; the Messages API requires at least
    /// 1024 and a `max_tokens` above it. Absent when disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
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
        let mut consumed = 0;
        // Scan each complete frame in place and drain the consumed prefix
        // once: draining per frame re-copies the rest of the buffer for every
        // frame, which is quadratic when a chunk arrives with many frames.
        while let Some(relative) = self.buf[consumed..].find("\n\n") {
            let end = consumed + relative;
            for line in self.buf[consumed..end].lines() {
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
            consumed = end + 2;
        }
        if consumed > 0 {
            self.buf.drain(..consumed);
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
    /// <https://api.anthropic.com> and <https://api.anthropic.com/v1> work.
    pub fn messages_url(&self) -> String {
        let base = self.endpoint.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{}/messages", base)
        } else {
            format!("{}/v1/messages", base)
        }
    }

    /// Append blocks to the last message when the role repeats.
    ///
    /// Anthropic expects one assistant message carrying the thinking block
    /// together with its tool_use blocks; splitting them into consecutive
    /// messages breaks the reasoning/tool continuity the protocol requires.
    fn push_blocks(messages: &mut Vec<AnthropicMessage>, role: &str, content: Value) {
        let blocks = match content {
            Value::Array(blocks) => blocks,
            other => vec![other],
        };
        if let Some(last) = messages.last_mut() {
            if last.role == role {
                if let Some(existing) = last.content.as_array_mut() {
                    existing.extend(blocks);
                    return;
                }
            }
        }
        messages.push(AnthropicMessage {
            role: role.to_string(),
            content: Value::Array(blocks),
        });
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
        let typed = request.provider_request.as_ref();
        if let Some(provider) = typed {
            // The typed system prompt plus this call's host guidance: Anthropic
            // has a top-level system section, so guidance stays out of the
            // message list (issue #434).
            system_parts.extend(provider.wire_system_prompt());
        } else if let Some(persona) = persona {
            if !persona.is_empty() {
                system_parts.push(persona.clone());
            }
        }
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        let history: Vec<crate::llm::LlmMessage> = if let Some(provider) = typed {
            provider.wire_messages()
        } else {
            session
                .history()
                .map(<[crate::llm::LlmMessage]>::to_vec)
                .unwrap_or_default()
        };
        {
            for message in &history {
                match message {
                    LlmMessage::System { content } => system_parts.push(content.clone()),
                    // Anthropic has no developer role. Keep the instruction in
                    // the system channel but name its weaker origin so it is
                    // not mistaken for the authored policy.
                    LlmMessage::Developer { content } => {
                        system_parts.push(format!("[developer]\n{content}"))
                    }
                    LlmMessage::User { content } => {
                        Self::push_blocks(&mut messages, "user", Self::content_blocks(content))
                    }
                    LlmMessage::Assistant { content } => Self::push_blocks(
                        &mut messages,
                        "assistant",
                        json!([{ "type": "text", "text": content }]),
                    ),
                    LlmMessage::AssistantReasoning {
                        content,
                        provider_state: Some(state),
                    } if state.provider == "anthropic" => {
                        match state.payload.get("type").and_then(Value::as_str) {
                            Some("thinking") => {
                                if let Some(signature) =
                                    state.payload.get("signature").and_then(Value::as_str)
                                {
                                    Self::push_blocks(
                                        &mut messages,
                                        "assistant",
                                        json!([{
                                            "type": "thinking",
                                            "thinking": content,
                                            "signature": signature,
                                        }]),
                                    );
                                }
                            }
                            Some("redacted_thinking") => {
                                if let Some(data) =
                                    state.payload.get("data").and_then(Value::as_str)
                                {
                                    Self::push_blocks(
                                        &mut messages,
                                        "assistant",
                                        json!([{ "type": "redacted_thinking", "data": data }]),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    // Reasoning without a signed Anthropic block stays out of
                    // the wire: inventing a signature is not possible.
                    LlmMessage::AssistantReasoning { .. } => {}
                    LlmMessage::AssistantToolCall {
                        tool_call_id,
                        name,
                        args,
                    } => Self::push_blocks(
                        &mut messages,
                        "assistant",
                        json!([{
                            "type": "tool_use",
                            "id": tool_call_id,
                            "name": name,
                            "input": args,
                        }]),
                    ),
                    LlmMessage::ToolResult {
                        tool_call_id,
                        content,
                    } => Self::push_blocks(
                        &mut messages,
                        "user",
                        json!([{
                            "type": "tool_result",
                            "tool_use_id": tool_call_id,
                            "content": content,
                        }]),
                    ),
                }
            }
        }

        let typed_tools = request
            .provider_request
            .as_ref()
            .map(|provider| provider.tools.as_slice());
        let tools: Vec<AnthropicTool> = typed_tools
            .or_else(|| context.tools())
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

        // Extended thinking is opt-in on this wire. The reader's effort level
        // selects a budget; the completion cap must stay above it, so a small
        // configured cap is raised rather than sending a body Anthropic rejects.
        let thinking = request.reasoning_effort.map(|effort| match effort {
            ReasoningEffort::None => AnthropicThinking {
                kind: "disabled".to_string(),
                budget_tokens: None,
            },
            ReasoningEffort::Low | ReasoningEffort::Medium | ReasoningEffort::High => {
                let budget_tokens = match effort {
                    ReasoningEffort::Low => 1024,
                    ReasoningEffort::Medium => 2048,
                    ReasoningEffort::High => 4096,
                    ReasoningEffort::None => unreachable!("handled above"),
                };
                AnthropicThinking {
                    kind: "enabled".to_string(),
                    budget_tokens: Some(budget_tokens),
                }
            }
        });
        let mut max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        if let Some(budget_tokens) = thinking
            .as_ref()
            .and_then(|thinking| thinking.budget_tokens)
        {
            max_tokens = max_tokens.max(budget_tokens.saturating_add(1024));
        }

        let thinking_on = thinking
            .as_ref()
            .is_some_and(|thinking| thinking.kind == "enabled");
        AnthropicRequest {
            model,
            max_tokens,
            system: if system_parts.is_empty() {
                None
            } else {
                Some(system_parts.join("\n"))
            },
            messages,
            temperature: if thinking_on {
                None
            } else {
                request.temperature
            },
            top_p: if thinking_on { None } else { request.top_p },
            tools: if tools.is_empty() { None } else { Some(tools) },
            thinking,
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
        let mut reasoning = String::new();
        let mut reasoning_state: Option<ReasoningState> = None;
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut usage_in = 0u32;
        let mut usage_out = 0u32;
        while let Some(item) = stream.next().await {
            match item? {
                StreamToken::Text(chunk) => text.push_str(&chunk),
                // The blocking path must not throw away what the streamed path
                // preserves: the turn's reasoning and its signature are what a
                // tool loop replays to Anthropic.
                StreamToken::ReasoningContent(chunk) => reasoning.push_str(&chunk),
                StreamToken::ReasoningState(state) => reasoning_state = Some(state),
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
            let mut message = json!({ "role": "assistant", "content": content });
            if !reasoning.is_empty() {
                message["reasoning_content"] = Value::String(reasoning.clone());
            }
            message
        } else {
            let mut message =
                json!({ "role": "assistant", "content": content, "tool_calls": tool_calls });
            if !reasoning.is_empty() {
                message["reasoning_content"] = Value::String(reasoning.clone());
            }
            message
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
                "total_tokens": usage_in.saturating_add(usage_out),
            },
        });
        let mut parsed: ChatCompletionResponse =
            serde_json::from_value(body).map_err(|e| LlmError::Parse(e.to_string()))?;
        // The Chat wire has no field for the signature; hand it to the engine
        // through the message extension so it can attach a ReasoningState.
        if let Some(state) = reasoning_state {
            if let Some(choice) = parsed.choices.first_mut() {
                choice.message.extra = json!({ "_bos_reasoning_state": state.payload });
            }
        }
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
        let wire = serde_json::to_string(&body).unwrap_or_default();
        if log::log_enabled!(log::Level::Info) {
            // A one-line summary, not the whole body: the full payload rode
            // this INFO line for every call, images included.
            info!("anthropic request: {} chars", wire.chars().count());
        }
        if log::log_enabled!(log::Level::Trace) {
            if crate::llm::vendor::wire_bodies_enabled() {
                log::trace!("anthropic request body: {wire}");
            } else {
                log::trace!(
                    "anthropic request body head: {}",
                    crate::llm::vendor::wire_body_head(&wire)
                );
            }
        }

        let builder = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body);
        let response = super::tap::traced_send(
            "llm",
            "POST",
            &url,
            request.request_id.as_deref(),
            self.client.as_ref(),
            builder,
        )
        .await
        .map_err(|e| {
            let err = LlmError::Other(format!("Request failed: {}", e));
            context.notify_error(&err);
            err
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let err = LlmError::Http(format!("HTTP {}: {} (endpoint: {})", status, text, url));
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
            // A transport chunk may split a multi-byte character; keep the
            // incomplete tail and decode it with the next chunk.
            let mut utf8_tail: Vec<u8> = Vec::new();

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

                utf8_tail.extend_from_slice(&chunk);
                let valid = crate::utils::complete_utf8_prefix(&utf8_tail);
                let text = String::from_utf8_lossy(&utf8_tail[..valid]).into_owned();
                utf8_tail.drain(..valid);
                for frame in decoder.push(&text) {
                    let kind = frame.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "message_start" => {
                            if let Some(n) = frame
                                .pointer("/message/usage/input_tokens")
                                .and_then(Value::as_u64)
                            {
                                input_tokens = u32::try_from(n).unwrap_or(u32::MAX);
                            }
                        }
                        "content_block_start" => {
                            if frame.pointer("/content_block/type").and_then(Value::as_str)
                                == Some("redacted_thinking")
                            {
                                if let Some(data) =
                                    frame.pointer("/content_block/data").and_then(Value::as_str)
                                {
                                    let state = ReasoningState::new(
                                        "anthropic",
                                        json!({ "type": "redacted_thinking", "data": data }),
                                    );
                                    let _ = tx.send(Ok(StreamToken::ReasoningState(state))).await;
                                }
                            }
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
                                "signature_delta" => {
                                    if let Some(signature) =
                                        frame.pointer("/delta/signature").and_then(Value::as_str)
                                    {
                                        let state = ReasoningState::new(
                                            "anthropic",
                                            json!({ "type": "thinking", "signature": signature }),
                                        );
                                        let _ =
                                            tx.send(Ok(StreamToken::ReasoningState(state))).await;
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
                                output_tokens = u32::try_from(n).unwrap_or(u32::MAX);
                            }
                        }
                        "message_stop" => {
                            let _ = tx
                                .send(Ok(StreamToken::Usage(Usage {
                                    prompt_tokens: input_tokens,
                                    completion_tokens: output_tokens,
                                    total_tokens: input_tokens.saturating_add(output_tokens),
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
            Arc::new(crate::llm::vendor::build_http_client()),
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
            request_id: None,
            intent: Default::default(),
            provider_request: None,
            host_guidance: Vec::new(),
            context_blocks: Vec::new(),
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
    fn the_decoder_reads_coalesced_frames_and_keeps_a_partial_one() {
        let mut decoder = AnthropicSseDecoder::default();
        let frames = decoder.push(
            "data: {\"type\":\"one\"}\n\ndata: {\"type\":\"two\"}\n\ndata: {\"type\":\"three\"}",
        );
        assert_eq!(frames.len(), 2, "only terminated frames are emitted");
        assert_eq!(frames[0]["type"], json!("one"));
        assert_eq!(frames[1]["type"], json!("two"));

        let frames = decoder.push("\n\n");
        assert_eq!(frames.len(), 1, "the buffered frame completes");
        assert_eq!(frames[0]["type"], json!("three"));
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

    #[test]
    fn effort_enables_thinking_and_drops_custom_sampling() {
        let vendor = vendor();
        let mut with_effort = request(Some(2048));
        with_effort.reasoning_effort = Some(crate::llm::ReasoningEffort::High);
        with_effort.temperature = Some(0.2);
        with_effort.top_p = Some(0.5);
        let body = vendor.build_request(&None, &with_effort, &(), &());
        let thinking = body.thinking.expect("thinking is enabled");
        assert_eq!(thinking.budget_tokens, Some(4096));
        assert!(
            body.max_tokens > thinking.budget_tokens.unwrap(),
            "the completion cap stays above the thinking budget"
        );
        assert_eq!(body.temperature, None, "thinking requires the default");
        assert_eq!(body.top_p, None, "thinking requires the default");
    }

    #[test]
    fn off_disables_thinking_on_the_messages_wire() {
        let vendor = vendor();
        let mut with_off = request(Some(2048));
        with_off.reasoning_effort = Some(crate::llm::ReasoningEffort::None);
        with_off.temperature = Some(0.2);
        let body = vendor.build_request(&None, &with_off, &(), &());
        let thinking = body.thinking.expect("thinking is explicitly disabled");
        assert_eq!(thinking.kind, "disabled");
        assert_eq!(thinking.budget_tokens, None);
        assert_eq!(
            body.temperature,
            Some(0.2),
            "disabled thinking keeps ordinary sampling"
        );
    }

    /// Untrusted context keeps its explicit envelope inside Anthropic's one
    /// system channel; the host policy section is written first, and the data
    /// is never bare (issue #434 review).
    #[test]
    fn untrusted_context_is_marked_inside_the_system_channel() {
        use crate::llm::{Content, LlmMessage, LlmRequest};
        let vendor = super::tests::vendor();
        let request = LlmRequest {
            model: "claude-3-5-sonnet-latest".to_string(),
            input: Content::text("hello"),
            provider_request: Some(crate::llm::ProviderRequest {
                system_prompt: Some("HOST POLICY".to_string()),
                messages: vec![
                    LlmMessage::system("<untrusted_context source=\"workspace-instructions\" trust=\"untrusted-data\">\nDATA\n</untrusted_context>"),
                    LlmMessage::user("hello"),
                ],
                input: Content::text("hello"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let built = vendor.build_request(&None, &request, &(), &());
        let system = built.system.expect("system");
        let host = system.find("HOST POLICY").expect("host policy");
        let open = system.find("<untrusted_context").expect("envelope");
        let data = system.find("DATA").expect("data");
        let close = system.find("</untrusted_context>").expect("close");
        assert!(host < open && open < data && data < close, "{system}");
        let inside = &system[open..close];
        assert!(!inside.contains("HOST POLICY"));
        let outside = system.replace(inside, "");
        assert!(
            !outside.contains("DATA"),
            "data is only inside the envelope: {system}"
        );
    }

    #[test]
    fn developer_falls_back_to_the_system_channel() {
        use crate::llm::{Content, LlmMessage, LlmRequest};
        let vendor = super::tests::vendor();
        let request = LlmRequest {
            model: "claude-3-5-sonnet-latest".to_string(),
            input: Content::text("question"),
            provider_request: Some(crate::llm::ProviderRequest {
                system_prompt: Some("POLICY".to_string()),
                messages: vec![
                    LlmMessage::developer("HARNESS RULE"),
                    LlmMessage::user("question"),
                ],
                input: Content::text("question"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let built = vendor.build_request(&None, &request, &(), &());
        let system = built.system.expect("system channel");
        assert!(system.contains("POLICY"));
        assert!(system.contains("[developer]\nHARNESS RULE"));
        assert_eq!(built.messages.len(), 1);
        assert_eq!(built.messages[0].role, "user");
    }

    #[test]
    fn signed_thinking_replays_and_unsigned_reasoning_is_skipped() {
        use crate::llm::{Content, LlmMessage, LlmRequest, ReasoningState};
        let vendor = super::tests::vendor();
        let request = |provider_state: Option<ReasoningState>| LlmRequest {
            model: "claude-3-5-sonnet-latest".to_string(),
            input: Content::text("next"),
            provider_request: Some(crate::llm::ProviderRequest {
                messages: vec![
                    LlmMessage::user("think"),
                    LlmMessage::AssistantReasoning {
                        content: "private chain".to_string(),
                        provider_state,
                    },
                    LlmMessage::assistant("answer"),
                    LlmMessage::user("next"),
                ],
                input: Content::text("next"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let signed = vendor.build_request(
            &None,
            &request(Some(ReasoningState::new(
                "anthropic",
                json!({ "type": "thinking", "signature": "sig-1" }),
            ))),
            &(),
            &(),
        );
        let assistant = signed
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .expect("the assistant turn");
        let blocks = assistant.content.as_array().expect("blocks");
        assert!(blocks.iter().any(|block| {
            block["type"] == "thinking"
                && block["thinking"] == "private chain"
                && block["signature"] == "sig-1"
        }));
        assert!(blocks.iter().any(|block| block["type"] == "text"));

        let unsigned = vendor.build_request(&None, &request(None), &(), &());
        let serialized = serde_json::to_string(&unsigned.messages).expect("serializes");
        assert!(
            !serialized.contains("private chain") && !serialized.contains("thinking"),
            "unsigned reasoning is not invented on the wire: {serialized}"
        );

        // A provider switch must not replay another protocol's opaque state.
        let foreign = vendor.build_request(
            &None,
            &request(Some(ReasoningState::new(
                "openai-responses",
                json!({ "id": "rs_1" }),
            ))),
            &(),
            &(),
        );
        let serialized = serde_json::to_string(&foreign.messages).expect("serializes");
        assert!(!serialized.contains("thinking"), "{serialized}");
    }

    #[test]
    fn typed_request_supplies_messages_system_and_tools() {
        use crate::llm::{Content, HostGuidance, LlmMessage, LlmRequest, LlmTool};
        let vendor = super::tests::vendor();
        let request = LlmRequest {
            model: "claude-3-5-sonnet-latest".to_string(),
            input: Content::text("next"),
            provider_request: Some(crate::llm::ProviderRequest {
                system_prompt: Some("SYS".to_string()),
                host_guidance: vec![HostGuidance::new("action-retry", "ACT NOW")],
                messages: vec![
                    LlmMessage::user("hello"),
                    LlmMessage::assistant("hi"),
                    LlmMessage::user("next"),
                ],
                input: Content::text("next"),
                tools: vec![LlmTool {
                    name: "typed-tool".to_string(),
                    description: "from snapshot".to_string(),
                    parameters: serde_json::json!({"type": "object"}),
                    kind: Default::default(),
                    config: None,
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        let built = vendor.build_request(&None, &request, &(), &());
        let system = built.system.as_deref().expect("a system section");
        assert!(system.starts_with("SYS"));
        assert!(
            system.contains("ACT NOW"),
            "host guidance is top-level system policy (issue #434)"
        );
        assert_eq!(built.messages.len(), 3, "guidance is not a message");
        assert_eq!(built.tools.expect("tools")[0].name, "typed-tool");
    }
}
