//! OpenAI Responses API (`/v1/responses`) support.
//!
//! The Responses API is OpenAI's successor to Chat Completions. Its wire
//! format differs from Chat Completions:
//! - requests carry an `input` array of typed *items* plus `instructions`,
//! - responses carry an `output` array of typed items (messages, function
//!   calls, hosted-tool calls such as `web_search_call`),
//! - streaming emits typed SSE events (`response.output_text.delta`,
//!   `response.function_call_arguments.done`, `response.completed`, ...).
//!
//! This module provides the request/response types, request building from the
//! crate's own `LlmMessage` history, hosted-tool serialization, and an SSE
//! extractor/accumulator that normalize the stream back into [`StreamToken`]s.
//!
//! It is protocol-agnostic about the endpoint host, so any vendor that speaks
//! an OpenAI-compatible Responses API (OpenAI, OpenRouter, ...) can reuse it.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::llm::{
    Content, ContentPart, LlmError, LlmMessage, LlmRequest, LlmResponse, LlmResponseResult,
    LlmTool, LlmToolKind, ReactContext, ReactSession, StreamToken, TokenStream,
};
use crate::utils::{JsonExtractor, StreamExtractor};

// =============================================================================
// Request
// =============================================================================

#[derive(Debug, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesInputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<ResponsesContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(rename = "call_id")]
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        #[serde(rename = "call_id")]
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesContentPart {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "output_text")]
    OutputText {
        text: String,
        #[serde(default)]
        annotations: Vec<Value>,
    },
    #[serde(rename = "input_image")]
    InputImage { image_url: String },
    #[serde(rename = "input_audio")]
    InputAudio { input_audio: ResponsesInputAudio },
    #[serde(rename = "output_image")]
    OutputImage {
        #[serde(default)]
        image_url: String,
    },
    #[serde(rename = "output_audio")]
    OutputAudio {
        #[serde(default)]
        transcript: String,
    },
    #[serde(rename = "refusal")]
    Refusal {
        #[serde(default)]
        refusal: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponsesInputAudio {
    pub data: String,
    pub format: String,
}

// =============================================================================
// Response
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: String,
    pub model: String,
    #[serde(default)]
    pub output: Vec<ResponsesItem>,
    #[serde(default)]
    pub usage: Option<ResponsesUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesItem {
    #[serde(rename = "message")]
    Message {
        #[serde(default)]
        id: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        content: Vec<ResponsesContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default)]
        id: String,
        #[serde(rename = "call_id", default)]
        call_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        #[serde(rename = "call_id", default)]
        call_id: String,
        #[serde(default)]
        output: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        summary: Vec<ResponsesReasoningSummary>,
    },
    #[serde(rename = "web_search_call")]
    WebSearchCall {
        #[serde(default)]
        id: String,
    },
    #[serde(rename = "file_search_call")]
    FileSearchCall {
        #[serde(default)]
        id: String,
    },
    #[serde(rename = "computer_call")]
    ComputerCall {
        #[serde(default)]
        id: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesReasoningSummary {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    #[serde(default)]
    pub input_tokens_details: Option<Value>,
    pub output_tokens: u32,
    #[serde(default)]
    pub output_tokens_details: Option<Value>,
    pub total_tokens: u32,
}

impl ResponsesResponse {
    /// Concatenate all `output_text` parts of `message` items in order.
    pub fn output_text(&self) -> String {
        self.output
            .iter()
            .filter_map(|item| match item {
                ResponsesItem::Message { content, .. } => {
                    Some(content.iter().filter_map(|part| match part {
                        ResponsesContentPart::OutputText { text, .. } => Some(text.as_str()),
                        _ => None,
                    }))
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// Map Responses API usage onto the Chat-Completions-shaped [`crate::llm::vendor::openaicompatible::Usage`].
    pub fn chat_usage(&self) -> Option<super::openaicompatible::Usage> {
        self.usage.as_ref().map(|u| super::openaicompatible::Usage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
            prompt_tokens_details: None,
        })
    }
}

// =============================================================================
// Request building
// =============================================================================

fn serialize_args(args: &Value) -> String {
    if args.is_null() || !args.is_object() {
        "{}".to_string()
    } else {
        args.to_string()
    }
}

fn serialize_content_part(part: &ContentPart) -> Option<ResponsesContentPart> {
    match part {
        ContentPart::Text { text } => Some(ResponsesContentPart::InputText { text: text.clone() }),
        ContentPart::Binary { binary } => {
            if binary.is_image() {
                Some(ResponsesContentPart::InputImage {
                    image_url: binary.url(),
                })
            } else if binary.is_audio() {
                let format = binary
                    .content_type
                    .rsplit('/')
                    .next()
                    .unwrap_or("mp3")
                    .to_string();
                Some(ResponsesContentPart::InputAudio {
                    input_audio: ResponsesInputAudio {
                        data: binary.url(),
                        format,
                    },
                })
            } else {
                None
            }
        }
    }
}

fn serialize_input_content(content: &Content) -> Vec<ResponsesContentPart> {
    match content {
        Content::Text(s) => {
            if let Ok(parts) = serde_json::from_str::<Vec<ContentPart>>(s) {
                parts.iter().filter_map(serialize_content_part).collect()
            } else if let Ok(part) = serde_json::from_str::<ContentPart>(s) {
                serialize_content_part(&part).into_iter().collect()
            } else {
                vec![ResponsesContentPart::InputText { text: s.clone() }]
            }
        }
        Content::Parts(parts) => parts.iter().filter_map(serialize_content_part).collect(),
    }
}

fn build_input(history: Option<&[LlmMessage]>, input: &Content) -> Vec<ResponsesInputItem> {
    let mut items = Vec::new();
    if let Some(history) = history {
        for message in history {
            match message {
                // System prompts are folded into `instructions`, not `input`.
                LlmMessage::System { .. } => {}
                LlmMessage::User { content } => items.push(ResponsesInputItem::Message {
                    role: "user".to_string(),
                    content: serialize_input_content(content),
                }),
                LlmMessage::Assistant { content } => {
                    items.push(ResponsesInputItem::Message {
                        role: "assistant".to_string(),
                        content: vec![ResponsesContentPart::OutputText {
                            text: content.clone(),
                            annotations: Vec::new(),
                        }],
                    });
                }
                LlmMessage::AssistantToolCall {
                    tool_call_id,
                    name,
                    args,
                } => items.push(ResponsesInputItem::FunctionCall {
                    call_id: tool_call_id.clone(),
                    name: name.clone(),
                    arguments: serialize_args(args),
                }),
                LlmMessage::ToolResult {
                    tool_call_id,
                    content,
                } => items.push(ResponsesInputItem::FunctionCallOutput {
                    call_id: tool_call_id.clone(),
                    output: content.clone(),
                }),
            }
        }
    }

    // A bare request (no history) still carries the user input.
    if items.is_empty() {
        items.push(ResponsesInputItem::Message {
            role: "user".to_string(),
            content: serialize_input_content(input),
        });
    }
    items
}

fn serialize_tools(tools: Option<&[LlmTool]>) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    if let Some(tools) = tools {
        for tool in tools {
            match tool.kind {
                LlmToolKind::Function => {
                    out.push(serde_json::json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }));
                }
                LlmToolKind::WebSearch => {
                    let mut v = serde_json::json!({ "type": "web_search" });
                    if let Some(cfg) = &tool.config {
                        v["web_search"] = cfg.clone();
                    }
                    out.push(v);
                }
                LlmToolKind::FileSearch => {
                    let mut v = serde_json::json!({ "type": "file_search" });
                    if let Some(cfg) = &tool.config {
                        v["file_search"] = cfg.clone();
                    }
                    out.push(v);
                }
                LlmToolKind::ComputerUse => {
                    let mut v = serde_json::json!({ "type": "computer_use" });
                    if let Some(cfg) = &tool.config {
                        v["computer_use"] = cfg.clone();
                    }
                    out.push(v);
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn build_instructions(
    persona: Option<String>,
    session: &impl ReactSession,
    context: &impl ReactContext,
) -> Option<String> {
    let mut instructions = String::new();

    if let Some(history) = session.history() {
        for message in history {
            if let LlmMessage::System { content } = message {
                instructions.push_str(content);
                instructions.push('\n');
            }
        }
    }

    if let Some(p) = &persona {
        instructions.push_str(&format!("Persona: {}\n", p));
    }

    if let Some(skills) = context.skills() {
        if !skills.is_empty() {
            let skill_names = skills
                .iter()
                .map(|s| format!("- {}: {}", s.name, s.description))
                .collect::<Vec<_>>()
                .join("\n");
            instructions.push_str(&format!(
                "Available Skills (call load_skill to read instructions when needed):\n{}\n",
                skill_names
            ));
        }
    }

    if let Some(rules) = context.rules() {
        if !rules.is_empty() {
            let rule_texts = rules
                .iter()
                .map(|r| r.content.clone())
                .collect::<Vec<_>>()
                .join("\n");
            instructions.push_str(&format!("You should follow below rules:\n{}\n", rule_texts));
        }
    }

    if let Some(inst) = context.instructions() {
        if !inst.is_empty() {
            let inst_texts = inst
                .iter()
                .map(|i| format!("- {}: {}", i.name, i.description))
                .collect::<Vec<_>>()
                .join("\n");
            instructions.push_str(&format!("Instructions:\n{}\n", inst_texts));
        }
    }

    if instructions.trim().is_empty() {
        None
    } else {
        Some(instructions)
    }
}

pub fn build_request(
    persona: Option<String>,
    req: &LlmRequest,
    session: &impl ReactSession,
    context: &impl ReactContext,
    stream: bool,
) -> ResponsesRequest {
    ResponsesRequest {
        model: req.model.clone(),
        instructions: build_instructions(persona, session, context),
        input: build_input(session.history(), &req.input),
        tools: serialize_tools(context.tools()),
        temperature: req.temperature,
        top_p: req.top_p,
        max_output_tokens: req.max_tokens,
        stream: Some(stream),
    }
}

// =============================================================================
// Streaming events
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        item_id: String,
        output_index: u32,
        text: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryDelta {
        item_id: String,
        output_index: u32,
        delta: String,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: u32, item: Value },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: u32, item: Value },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        item_id: String,
        output_index: u32,
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        item_id: String,
        output_index: u32,
        arguments: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: ResponsesResponse },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponsesResponse },
    #[serde(rename = "response.failed")]
    Failed { response: ResponsesResponse },
    #[serde(rename = "error")]
    Error {
        message: String,
        code: Option<String>,
    },
    #[serde(other)]
    Other,
}

impl ResponsesStreamEvent {
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            ResponsesStreamEvent::Completed { .. }
                | ResponsesStreamEvent::Incomplete { .. }
                | ResponsesStreamEvent::Failed { .. }
                | ResponsesStreamEvent::Error { .. }
        )
    }
}

pub struct ResponsesExtractor {
    inner: JsonExtractor,
}

impl ResponsesExtractor {
    pub fn new(inner: JsonExtractor) -> Self {
        Self { inner }
    }
}

impl StreamExtractor for ResponsesExtractor {
    type Item<'a> = ResponsesStreamEvent;

    fn push<'a>(&mut self, chunk: &str) -> Option<Vec<Self::Item<'a>>> {
        let spans = self.inner.push(chunk)?;
        let mut events = Vec::new();
        for span in spans.iter() {
            if span.is_root() {
                let json_str = self.inner.extract(span);
                if let Ok(event) = serde_json::from_slice::<ResponsesStreamEvent>(json_str) {
                    if !matches!(event, ResponsesStreamEvent::Other) {
                        events.push(event);
                    }
                }
            }
        }
        if events.is_empty() {
            None
        } else {
            Some(events)
        }
    }

    fn extract<'a>(&'a self, span: &crate::utils::StreamSpan) -> &'a [u8] {
        self.inner.extract(span)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// Accumulates streamed Responses API function calls.
///
/// The Responses API identifies items by `item_id` and streams the arguments
/// in `response.function_call_arguments.delta` events; the tool name and
/// `call_id` arrive earlier inside `response.output_item.added`/`done` events.
/// This accumulator joins them per `item_id` so a completed call can be
/// emitted as a [`StreamToken::ToolCall`].
#[derive(Debug, Default)]
pub struct ResponsesFunctionCallAccumulator {
    pending: HashMap<String, PendingResponsesFunctionCall>,
}

#[derive(Debug, Default)]
struct PendingResponsesFunctionCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ResponsesFunctionCallAccumulator {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Feed a raw `response.output_item.added`/`done` item. If it is a
    /// `function_call` item its name and call_id are captured by `item_id`.
    pub fn add_item(&mut self, item: &Value) {
        let item_type = item.get("type").and_then(|v| v.as_str());
        let item_id = item.get("id").and_then(|v| v.as_str());
        if item_type == Some("function_call") {
            if let Some(item_id) = item_id {
                let entry = self.pending.entry(item_id.to_string()).or_default();
                if entry.name.is_none() {
                    entry.name = item.get("name").and_then(|v| v.as_str()).map(String::from);
                }
                if entry.call_id.is_none() {
                    entry.call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
        }
    }

    pub fn push_args_delta(&mut self, item_id: &str, delta: &str) {
        self.pending
            .entry(item_id.to_string())
            .or_default()
            .arguments
            .push_str(delta);
    }

    /// Finalize a call from `response.function_call_arguments.done`. The event
    /// carries the complete `arguments` string; accumulated deltas are used as
    /// a fallback for providers that only send deltas.
    pub fn done(
        &mut self,
        item_id: &str,
        final_arguments: Option<String>,
    ) -> Option<(String, Value, Option<String>)> {
        let entry = self.pending.get_mut(item_id)?;
        if let Some(args) = &final_arguments {
            if !args.is_empty() {
                entry.arguments = args.clone();
            }
        }
        let name = entry.name.clone().unwrap_or_default();
        let call_id = entry.call_id.clone();
        let arguments = std::mem::take(&mut entry.arguments);
        self.pending.remove(item_id);

        if name.is_empty() {
            return None;
        }
        let args_val = serde_json::from_str(&arguments).unwrap_or_else(|_| serde_json::json!({}));
        let id = call_id
            .filter(|s| !s.is_empty())
            .or_else(|| Some(uuid::Uuid::new_v4().to_string()));
        Some((name, args_val, id))
    }

    /// Drain all pending calls (stream end / error). Incomplete arguments fall
    /// back to `{}`.
    pub fn drain(&mut self) -> Vec<(String, Value, Option<String>)> {
        let keys: Vec<String> = self.pending.keys().cloned().collect();
        keys.into_iter()
            .filter_map(|key| self.done(&key, None))
            .collect()
    }
}

// =============================================================================
// Transport
// =============================================================================

/// Minimal HTTP client for the Responses API. Vendors construct it from their
/// own endpoint/api-key and delegate both complete and streaming calls.
pub struct ResponsesTransport {
    client: Arc<Client>,
    api_key: Arc<String>,
    endpoint: Arc<String>,
}

impl ResponsesTransport {
    pub fn new(client: Arc<Client>, api_key: Arc<String>, endpoint: Arc<String>) -> Self {
        Self {
            client,
            api_key,
            endpoint,
        }
    }

    pub async fn complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut (impl ReactSession + Send),
        context: &mut (impl ReactContext + Send),
    ) -> LlmResponseResult {
        context.notify_request(&request);

        if request.model.is_empty() {
            request.model = "gpt-4".to_string();
        }

        let responses_req = build_request(persona, &request, session, context, false);

        let url = format!("{}/responses", self.endpoint);

        info!(
            "Req: {}",
            serde_json::to_string(&responses_req)
                .unwrap_or_else(|_| "Failed to serialize request".into())
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&responses_req)
            .send()
            .await
            .map_err(|e| {
                let err = LlmError::Http(e.to_string());
                context.notify_error(&err);
                err
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = LlmError::Http(format!("HTTP {}: {}", status, body));
            context.notify_error(&err);
            return Err(err);
        }

        let body: ResponsesResponse = response.json().await.map_err(|e| {
            let err = LlmError::Parse(e.to_string());
            context.notify_error(&err);
            err
        })?;

        let resp = LlmResponse::Responses(body);
        context.notify_response(&resp);
        Ok(resp)
    }

    pub async fn stream_complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut (impl ReactSession + Send),
        context: &mut (impl ReactContext + Send),
    ) -> Result<TokenStream, LlmError> {
        context.notify_request(&request);

        if request.model.is_empty() {
            request.model = "gpt-4".to_string();
        }

        let responses_req = build_request(persona, &request, session, context, true);

        let url = format!("{}/responses", self.endpoint);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&responses_req)
            .send()
            .await
            .map_err(|e| {
                let err = LlmError::Other(format!("Request failed: {}", e));
                context.notify_error(&err);
                err
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = LlmError::Other(format!("HTTP {}: {}", status, body));
            context.notify_error(&err);
            return Err(err);
        }

        let (tx, rx) = mpsc::channel(32);
        let on_chunk = context.on_chunk_callback();

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut extractor = ResponsesExtractor::new(JsonExtractor::default());
            let mut func_calls = ResponsesFunctionCallAccumulator::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let text = match chunk_result {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(e) => {
                        let err = LlmError::Other(format!("Stream error: {}", e));
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                };

                if let Some(events) = extractor.push(&text) {
                    for event in events {
                        match event {
                            ResponsesStreamEvent::OutputTextDelta { delta, .. } => {
                                if !delta.is_empty() {
                                    on_chunk.as_ref().map(|cb| cb(&delta));
                                    let _ = tx.send(Ok(StreamToken::Text(delta))).await;
                                }
                            }
                            ResponsesStreamEvent::ReasoningSummaryDelta { delta, .. } => {
                                if !delta.is_empty() {
                                    let _ = tx.send(Ok(StreamToken::ReasoningContent(delta))).await;
                                }
                            }
                            ResponsesStreamEvent::OutputItemAdded { item, .. }
                            | ResponsesStreamEvent::OutputItemDone { item, .. } => {
                                func_calls.add_item(&item);
                            }
                            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                                item_id,
                                delta,
                                ..
                            } => {
                                func_calls.push_args_delta(&item_id, &delta);
                            }
                            ResponsesStreamEvent::FunctionCallArgumentsDone {
                                item_id,
                                arguments,
                                ..
                            } => {
                                if let Some((name, args_val, id)) =
                                    func_calls.done(&item_id, Some(arguments))
                                {
                                    on_chunk.as_ref().map(|cb| cb(&name));
                                    let _ = tx
                                        .send(Ok(StreamToken::ToolCall {
                                            name,
                                            args: args_val,
                                            id,
                                        }))
                                        .await;
                                }
                            }
                            ResponsesStreamEvent::Completed { response }
                            | ResponsesStreamEvent::Incomplete { response } => {
                                if let Some(usage) = response.chat_usage() {
                                    let _ = tx.send(Ok(StreamToken::Usage(usage))).await;
                                }
                                for (name, args_val, id) in func_calls.drain() {
                                    let _ = tx
                                        .send(Ok(StreamToken::ToolCall {
                                            name,
                                            args: args_val,
                                            id,
                                        }))
                                        .await;
                                }
                                let _ = tx.send(Ok(StreamToken::Done)).await;
                                return;
                            }
                            ResponsesStreamEvent::Failed { response } => {
                                for (name, args_val, id) in func_calls.drain() {
                                    let _ = tx
                                        .send(Ok(StreamToken::ToolCall {
                                            name,
                                            args: args_val,
                                            id,
                                        }))
                                        .await;
                                }
                                let err = LlmError::Other(format!(
                                    "Responses API failed (status={})",
                                    response.status
                                ));
                                let _ = tx.send(Err(err)).await;
                                return;
                            }
                            ResponsesStreamEvent::Error { message, .. } => {
                                for (name, args_val, id) in func_calls.drain() {
                                    let _ = tx
                                        .send(Ok(StreamToken::ToolCall {
                                            name,
                                            args: args_val,
                                            id,
                                        }))
                                        .await;
                                }
                                let err =
                                    LlmError::Other(format!("Responses API error: {}", message));
                                let _ = tx.send(Err(err)).await;
                                return;
                            }
                            ResponsesStreamEvent::Other
                            | ResponsesStreamEvent::OutputTextDone { .. } => {}
                        }
                    }
                }
            }

            for (name, args_val, id) in func_calls.drain() {
                let _ = tx
                    .send(Ok(StreamToken::ToolCall {
                        name,
                        args: args_val,
                        id,
                    }))
                    .await;
            }
            let _ = tx.send(Ok(StreamToken::Done)).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}
