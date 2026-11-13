//! The OpenAI vendor and its OpenAI-compatible HTTP client.

use std::sync::Arc;

use crate::{
    llm::vendor::openaicompatible::{
        sse_has_done_signal, ChatCompletionResponse, OpenAIExtractor, StreamToolCallAccumulator,
        Usage,
    },
    utils::{JsonExtractor, StreamExtractor},
};
use async_trait::async_trait;
use futures::StreamExt;
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::llm::vendor::responses::ResponsesTransport;
use crate::llm::{
    ApiMode, Content, ContentPart, LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult,
    ReactContext, ReactSession, StreamToken, TokenStream, VendorBuilderError,
};

/// An OpenAI-compatible vendor client.
///
/// One endpoint, three wires: the request's [`ApiMode`] dispatches Chat to
/// `/chat/completions`, Responses to `/responses`, and Anthropic Messages to
/// the Messages vendor at the same endpoint and key — so a custom gateway
/// speaks whichever protocol its provider configuration names.
pub struct OpenAiVendor {
    client: Arc<Client>,
    endpoint: Arc<String>,
    model: String,
    api_key: Arc<String>,
}

impl Clone for OpenAiVendor {
    fn clone(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            endpoint: self.endpoint.clone(),
            model: self.model.clone(),
            api_key: Arc::clone(&self.api_key),
        }
    }
}

/// Log one streamed response's timing and token usage.
///
/// The streaming path used to log nothing after the request, so the turn's
/// first-token latency and billed tokens were invisible.
fn log_stream_result(
    started: std::time::Instant,
    first_token: Option<std::time::Duration>,
    usage: Option<&Usage>,
) {
    let ttft = first_token
        .map(|elapsed| format!("{}ms", elapsed.as_millis()))
        .unwrap_or_else(|| "unreported".to_string());
    match usage {
        Some(usage) => info!(
            "Resp: ttft={} total={}ms prompt={} completion={} cached={}",
            ttft,
            started.elapsed().as_millis(),
            usage.prompt_tokens,
            usage.completion_tokens,
            usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0)
        ),
        None => info!(
            "Resp: ttft={} total={}ms usage=unreported",
            ttft,
            started.elapsed().as_millis()
        ),
    }
}

/// Log one request: a one-line summary at INFO, the full payload at TRACE.
///
/// The full JSON used to be an INFO line, which put every file body, every
/// skill and every base64 image into the log for every step.
fn log_request(request: &OpenAiRequest) {
    let Ok(json) = serde_json::to_string(request) else {
        return;
    };
    if log::log_enabled!(log::Level::Trace) {
        // The full payload used to ride every TRACE step, which put a whole
        // turn into the log for every request. Bodies are opt-in; the head
        // keeps a bounded fingerprint either way.
        if crate::llm::vendor::wire_bodies_enabled() {
            log::trace!("Req: {json}");
        } else {
            log::trace!("Req: {}", crate::llm::vendor::wire_body_head(&json));
        }
    }
    if log::log_enabled!(log::Level::Info) {
        info!(
            "Req: model={} messages={} tools={} chars={}",
            request.model,
            request.messages.len(),
            request.tools.as_ref().map_or(0, Vec::len),
            json.len()
        );
    }
}

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessageJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<crate::llm::ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    /// Ask a streaming provider to end the stream with a usage-only chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
}

/// The `stream_options` object a streaming request carries.
#[derive(Serialize, Clone)]
struct OpenAiStreamOptions {
    /// End the stream with one chunk whose `usage` is populated.
    include_usage: bool,
}

#[derive(Serialize, Clone)]
struct OpenAiMessageJson {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallJson>>,
}

fn serialize_args(args: &serde_json::Value) -> String {
    if args.is_null() || !args.is_object() {
        "{}".to_string()
    } else {
        args.to_string()
    }
}

fn serialize_content(content: &Content) -> serde_json::Value {
    fn serialize_part(part: &ContentPart) -> serde_json::Value {
        match part {
            ContentPart::Text { text } => serde_json::json!({
                "type": "text",
                "text": text
            }),
            ContentPart::Binary { binary } => {
                let url = binary.url();
                if binary.is_image() {
                    serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": url }
                    })
                } else if binary.is_audio() {
                    serde_json::json!({
                        "type": "audio_url",
                        "audio_url": { "url": url }
                    })
                } else {
                    serde_json::json!({
                        "type": "binary",
                        "binary": { "url": url }
                    })
                }
            }
        }
    }

    match content {
        Content::Text(s) => match crate::llm::types::parse_content_parts(s) {
            Some(parts) => serde_json::Value::Array(parts.iter().map(serialize_part).collect()),
            None => serde_json::Value::String(s.clone()),
        },
        Content::Parts(parts) => {
            serde_json::Value::Array(parts.iter().map(serialize_part).collect())
        }
    }
}

#[derive(Serialize, Clone)]
struct ToolCallJson {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: FunctionCallJson,
}

#[derive(Serialize, Clone)]
struct FunctionCallJson {
    name: String,
    arguments: String,
}

impl OpenAiVendor {
    /// Create a vendor with an explicit endpoint, model, and key.
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        let client = Arc::new(crate::llm::vendor::build_http_client());

        Self {
            client,
            endpoint: Arc::new(endpoint),
            model,
            api_key: Arc::new(api_key),
        }
    }

    /// Start building an OpenAI vendor.
    pub fn builder() -> OpenAiVendorBuilder {
        OpenAiVendorBuilder::new()
    }

    fn convert_request(
        &self,
        persona: Option<String>,
        req: &LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> OpenAiRequest {
        let mut messages = Vec::new();
        if let Some(history) = session.history() {
            for message in history {
                let json_msg = match message {
                    crate::llm::LlmMessage::System { content } => OpenAiMessageJson {
                        role: "system",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::User { content } => OpenAiMessageJson {
                        role: "user",
                        content: Some(serialize_content(content)),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::Assistant { content } => OpenAiMessageJson {
                        role: "assistant",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::AssistantToolCall {
                        tool_call_id: id,
                        name,
                        args,
                    } => OpenAiMessageJson {
                        role: "assistant",
                        content: None,
                        tool_call_id: Some(id.clone()),
                        tool_calls: Some(vec![ToolCallJson {
                            id: id.clone(),
                            call_type: "function",
                            function: FunctionCallJson {
                                name: name.clone(),
                                arguments: serialize_args(args),
                            },
                        }]),
                    },
                    crate::llm::LlmMessage::ToolResult {
                        tool_call_id,
                        content,
                    } => OpenAiMessageJson {
                        role: "tool",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: Some(tool_call_id.clone()),
                        tool_calls: None,
                    },
                };
                messages.push(json_msg);
            }
        }

        if messages.is_empty() {
            messages.push(OpenAiMessageJson {
                role: "user",
                content: Some(serialize_content(&req.input)),
                tool_call_id: None,
                tool_calls: None,
            });
        }

        let tools: Vec<serde_json::Value> = context
            .tools()
            .map(|tools| {
                tools
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters
                            }
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let tools = if tools.is_empty() { None } else { Some(tools) };

        let mut extra_system_prompt = String::new();

        if let Some(p) = &persona {
            extra_system_prompt.push_str(&format!("Persona: {}\n", p));
        }

        if let Some(skills) = context.skills() {
            // The catalogue is ranked against THIS request and bounded; see
            // render_skill_catalog. Listing every skill's description cost a
            // measured 36 KB of a 41 KB prompt on every step.
            extra_system_prompt.push_str(&crate::llm::render_skill_catalog(
                skills,
                req.input.as_text().unwrap_or_default(),
            ));
        }

        if let Some(rules) = context.rules() {
            if !rules.is_empty() {
                let rule_texts = rules
                    .iter()
                    .map(|r| r.content.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                extra_system_prompt
                    .push_str(&format!("You should follow below rules:\n{}\n", rule_texts));
            }
        }

        if let Some(instructions) = context.instructions() {
            if !instructions.is_empty() {
                let inst_texts = instructions
                    .iter()
                    .map(|i| format!("- {}: {}", i.name, i.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                extra_system_prompt.push_str(&format!("Instructions:\n{}\n", inst_texts));
            }
        }

        messages.insert(
            0,
            OpenAiMessageJson {
                role: "system",
                content: Some(serde_json::Value::String(extra_system_prompt)),
                tool_call_id: None,
                tool_calls: None,
            },
        );

        let max_tokens = req
            .max_tokens
            .unwrap_or(crate::llm::vendor::DEFAULT_COMPLETION_CAP);

        OpenAiRequest {
            model: req.model.clone(),
            messages,
            tools,
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            max_tokens: Some(max_tokens),
            reasoning_effort: req.reasoning_effort,
            stream: Some(false),
            stream_options: None,
        }
    }

    fn build_stream_request(
        &self,
        persona: Option<String>,
        req: &LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> OpenAiRequest {
        let mut openai_req = self.convert_request(persona, req, session, context);
        openai_req.stream = Some(true);
        // Without this an OpenAI-compatible server streams no usage block, so
        // every turn's prompt/completion count would be unknown.
        openai_req.stream_options = Some(OpenAiStreamOptions {
            include_usage: true,
        });
        openai_req
    }
}

impl OpenAiVendor {
    /// The Messages vendor for the same endpoint, model and key.
    ///
    /// Built per call so the delegate shares this vendor's HTTP client and
    /// never keeps a second connection pool; the endpoint normalization
    /// (a trailing `/v1` is respected) is the Messages vendor's own.
    fn messages_delegate(&self) -> super::anthropic::AnthropicVendor {
        super::anthropic::AnthropicVendor::new(
            self.client.clone(),
            self.endpoint.as_str(),
            self.model.clone(),
            self.api_key.as_str(),
        )
    }

    /// The wire path a model list is read from.
    ///
    /// The endpoint may or may not carry a trailing slash, and a doubled
    /// slash is what some gateways reject, so the suffix is stripped first.
    pub fn models_url(&self) -> String {
        let endpoint = self.endpoint.as_str();
        let endpoint = endpoint.strip_suffix('/').unwrap_or(endpoint);
        format!("{}/models", endpoint)
    }

    /// Ask the provider which model identifiers it offers.
    ///
    /// This is the wire call behind a model picker. A provider that does not
    /// implement GET /models answers with an HTTP failure, which surfaces as
    /// [LlmError::Http] rather than an empty list, so a caller never paints an
    /// empty catalog over a refusal.
    pub async fn fetch_model_ids(&self) -> Result<Vec<String>, LlmError> {
        let url = self.models_url();
        let builder = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key));
        let response = super::tap::traced_send(
            "llm.models",
            "GET",
            &url,
            None,
            self.client.as_ref(),
            builder,
        )
        .await
        .map_err(|e| LlmError::Http(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Http(format!(
                "HTTP {}: {} (endpoint: {})",
                status, body, url
            )));
        }
        let body = response
            .text()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        parse_model_ids(&body)
    }
}

/// One entry of a GET /models response.
#[derive(Deserialize)]
struct ModelEntry {
    /// The identifier the provider accepts as a model.
    id: String,
}

/// A GET /models response body.
#[derive(Deserialize)]
struct ModelListBody {
    /// The models the provider offers.
    data: Vec<ModelEntry>,
}

/// Read the model identifiers out of a GET /models body.
///
/// A malformed body is a [LlmError::Parse], never an empty list: a caller
/// painting a picker has to tell "the provider lists nothing" from "this is
/// not a model list at all".
fn parse_model_ids(body: &str) -> Result<Vec<String>, LlmError> {
    let parsed: ModelListBody =
        serde_json::from_str(body).map_err(|e| LlmError::Parse(e.to_string()))?;
    Ok(parsed.data.into_iter().map(|entry| entry.id).collect())
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for OpenAiVendor
{
    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        self.fetch_model_ids().await
    }
    async fn complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        if request.model.is_empty() {
            request.model = self.model.clone();
        }
        if request.api_mode == ApiMode::Anthropic {
            return self
                .messages_delegate()
                .complete(persona, request, session, context)
                .await;
        }
        if request.api_mode == ApiMode::Responses {
            return ResponsesTransport::new(
                self.client.clone(),
                self.api_key.clone(),
                self.endpoint.clone(),
            )
            .complete(persona, request, session, context)
            .await;
        }

        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let t0 = std::time::Instant::now();
        let openai_req = self.convert_request(persona, &request, session, context);
        info!("[TIMING] convert_request: {:?}", t0.elapsed());

        let url = format!("{}/chat/completions", endpoint);

        log_request(&openai_req);

        let t1 = std::time::Instant::now();
        let builder = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&openai_req);
        let response = super::tap::traced_send(
            "llm",
            "POST",
            &url,
            request.request_id.as_deref(),
            client.as_ref(),
            builder,
        )
        .await
        .map_err(|e| {
            let err = LlmError::Http(e.to_string());
            context.notify_error(&err);
            err
        })?;
        info!("[TIMING] HTTP send+wait: {:?}", t1.elapsed());

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let err = LlmError::Http(format!("HTTP {}: {} (endpoint: {})", status, body, url));
            context.notify_error(&err);
            return Err(err);
        }

        let t2 = std::time::Instant::now();
        let mut body: ChatCompletionResponse = response.json().await.map_err(|e| {
            let err = LlmError::Parse(e.to_string());
            context.notify_error(&err);
            err
        })?;
        info!("[TIMING] response.json(): {:?}", t2.elapsed());
        info!("[TIMING] complete total: {:?}", t0.elapsed());

        // The blocking path carries the same wire-markup risk as the stream:
        // strip it before anything records the text.
        for choice in &mut body.choices {
            if let Some(content) = &mut choice.message.content {
                *content = crate::llm::vendor::strip_special_tokens(content);
            }
            if let Some(reasoning) = &mut choice.message.reasoning_content {
                *reasoning = crate::llm::vendor::strip_special_tokens(reasoning);
            }
        }

        let resp = LlmResponse::OpenAI(body);

        if log::log_enabled!(log::Level::Info) {
            // The whole response used to be serialized into this INFO line:
            // a full transcript with base64 images in one log row, for every
            // blocking call. The body is a TRACE head like the request's.
            info!("Resp: complete");
        }
        if log::log_enabled!(log::Level::Trace) {
            let json = serde_json::to_string(&resp).unwrap_or_default();
            if crate::llm::vendor::wire_bodies_enabled() {
                log::trace!("Resp body: {json}");
            } else {
                log::trace!(
                    "Resp body head: {}",
                    crate::llm::vendor::wire_body_head(&json)
                );
            }
        }

        context.notify_response(&resp);
        Ok(resp)
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
        if request.api_mode == ApiMode::Anthropic {
            return self
                .messages_delegate()
                .stream_complete(persona, request, session, context)
                .await;
        }
        if request.api_mode == ApiMode::Responses {
            return ResponsesTransport::new(
                self.client.clone(),
                self.api_key.clone(),
                self.endpoint.clone(),
            )
            .stream_complete(persona, request, session, context)
            .await;
        }

        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let mut openai_req = self.build_stream_request(persona, &request, session, context);

        let url = format!("{}/chat/completions", endpoint);

        log_request(&openai_req);

        let builder = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&openai_req);
        let mut response = super::tap::traced_send(
            "llm",
            "POST",
            &url,
            request.request_id.as_deref(),
            client.as_ref(),
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
            let body = response.text().await.unwrap_or_default();
            // `stream_options` is a standard OpenAI field, but an older
            // compatible gateway can refuse the whole request over it. Usage
            // reporting is worth one retry, not a broken turn.
            if status == reqwest::StatusCode::BAD_REQUEST
                && body.contains("stream_options")
                && openai_req.stream_options.take().is_some()
            {
                log::warn!("provider rejected stream_options; retrying without usage reporting");
                let builder = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Content-Type", "application/json")
                    .header("Accept", "text/event-stream")
                    .json(&openai_req);
                response = super::tap::traced_send(
                    "llm",
                    "POST",
                    &url,
                    request.request_id.as_deref(),
                    client.as_ref(),
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
                    let body = response.text().await.unwrap_or_default();
                    let err =
                        LlmError::Other(format!("HTTP {}: {} (endpoint: {})", status, body, url));
                    context.notify_error(&err);
                    return Err(err);
                }
            } else {
                let err = LlmError::Other(format!("HTTP {}: {} (endpoint: {})", status, body, url));
                context.notify_error(&err);
                return Err(err);
            }
        }

        let (tx, rx) = mpsc::channel(32);
        let on_chunk = context.on_chunk_callback();
        let started = std::time::Instant::now();

        tokio::spawn(async move {
            let mut first_token: Option<std::time::Duration> = None;
            let mut last_usage: Option<Usage> = None;
            let mut byte_stream = response.bytes_stream();
            let mut extractor = OpenAIExtractor::new(JsonExtractor::default());
            let mut acc = StreamToolCallAccumulator::new();
            // Drops wire markup a gateway leaves in `content` even though it
            // parsed the structured tool calls (measured: `<｜DSML｜ calls>`
            // reached the transcript as prose).
            let mut special = crate::llm::vendor::SpecialTokenFilter::default();
            // The same markup leaks into the reasoning channel, and a model
            // that reads its own transcript would re-learn it there. A separate
            // filter keeps the two streams' held tails independent.
            let mut special_reasoning = crate::llm::vendor::SpecialTokenFilter::default();
            // A transport chunk may split a multi-byte character; keep the
            // incomplete tail and decode it with the next chunk.
            let mut utf8_tail: Vec<u8> = Vec::new();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        utf8_tail.extend_from_slice(&bytes);
                        let valid = crate::utils::complete_utf8_prefix(&utf8_tail);
                        let text = String::from_utf8_lossy(&utf8_tail[..valid]).into_owned();
                        utf8_tail.drain(..valid);

                        if let Some(chats) = extractor.push(&text) {
                            for chat in chats {
                                for choice in chat.choices {
                                    if let Some(content) = &choice.delta.reasoning_content {
                                        if !content.is_empty() {
                                            let cleaned = special_reasoning.push(content);
                                            if !cleaned.is_empty() {
                                                first_token
                                                    .get_or_insert_with(|| started.elapsed());
                                                on_chunk.as_ref().inspect(|cb| cb(&cleaned));
                                                let _ = tx
                                                    .send(Ok(StreamToken::ReasoningContent(
                                                        cleaned,
                                                    )))
                                                    .await;
                                            }
                                        }
                                    }
                                    if let Some(content) = &choice.delta.content {
                                        if !content.is_empty() {
                                            let cleaned = special.push(content);
                                            if !cleaned.is_empty() {
                                                first_token
                                                    .get_or_insert_with(|| started.elapsed());
                                                on_chunk.as_ref().inspect(|cb| cb(&cleaned));
                                                let _ =
                                                    tx.send(Ok(StreamToken::Text(cleaned))).await;
                                            }
                                        }
                                    }
                                    if let Some(calls) = &choice.delta.tool_calls {
                                        for call in calls {
                                            let index = call.index.unwrap_or(0);
                                            let id = call.id.clone().filter(|s| !s.is_empty());
                                            let name = call
                                                .function
                                                .as_ref()
                                                .and_then(|f| f.name.clone())
                                                .filter(|s| !s.is_empty());
                                            let args_delta = call
                                                .function
                                                .as_ref()
                                                .and_then(|f| f.arguments.as_deref())
                                                .unwrap_or_default();

                                            if let Some((n, args_val, id)) =
                                                acc.push_delta(index, id, name, args_delta)
                                            {
                                                first_token
                                                    .get_or_insert_with(|| started.elapsed());
                                                on_chunk.as_ref().inspect(|cb| cb(&n));
                                                let _ = tx
                                                    .send(Ok(StreamToken::ToolCall {
                                                        name: n,
                                                        args: args_val,
                                                        id,
                                                    }))
                                                    .await;
                                            }
                                        }
                                    }

                                    if choice.finish_reason.as_deref() == Some("tool_calls") {
                                        for (name, args_val, id) in acc.drain() {
                                            let _ = tx
                                                .send(Ok(StreamToken::ToolCall {
                                                    name,
                                                    args: args_val,
                                                    id,
                                                }))
                                                .await;
                                        }
                                    }

                                    if let Some(func) = &choice.delta.function_call {
                                        let name = func.name.clone().unwrap_or_default();
                                        let args_str = func.arguments.clone().unwrap_or_default();
                                        let args_val: serde_json::Value =
                                            match serde_json::from_str(&args_str) {
                                                Ok(v) => v,
                                                Err(_) => serde_json::Value::Null,
                                            };
                                        let _ = tx
                                            .send(Ok(StreamToken::ToolCall {
                                                name: name.clone(),
                                                args: args_val,
                                                id: Some(uuid::Uuid::new_v4().to_string()),
                                            }))
                                            .await;
                                    }
                                }
                                if let Some(usage) = &chat.usage {
                                    last_usage = Some(usage.clone());
                                    let _ = tx.send(Ok(StreamToken::Usage(usage.clone()))).await;
                                }
                            }
                        }

                        if sse_has_done_signal(&text) {
                            for (name, args_val, id) in acc.drain() {
                                let _ = tx
                                    .send(Ok(StreamToken::ToolCall {
                                        name,
                                        args: args_val,
                                        id,
                                    }))
                                    .await;
                            }
                            // The Done signal ends the stream, so a held
                            // partial span is flushed here too: it is the
                            // model's last words, not a marker.
                            let text_tail = special.finish();
                            if !text_tail.is_empty() {
                                let _ = tx.send(Ok(StreamToken::Text(text_tail))).await;
                            }
                            let reasoning_tail = special_reasoning.finish();
                            if !reasoning_tail.is_empty() {
                                let _ = tx
                                    .send(Ok(StreamToken::ReasoningContent(reasoning_tail)))
                                    .await;
                            }
                            log_stream_result(started, first_token, last_usage.as_ref());
                            let _ = tx.send(Ok(StreamToken::Done)).await;
                            return;
                        }
                    }
                    Err(e) => {
                        let err = crate::llm::vendor::stream_error(e);
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            }
            // Flush whatever the special-token filter still holds: a
            // trailing partial span is the model's last words, not a marker.
            let tail = special.finish();
            if !tail.is_empty() {
                let _ = tx.send(Ok(StreamToken::Text(tail))).await;
            }
            let reasoning_tail = special_reasoning.finish();
            if !reasoning_tail.is_empty() {
                let _ = tx
                    .send(Ok(StreamToken::ReasoningContent(reasoning_tail)))
                    .await;
            }
            for (name, args_val, id) in acc.drain() {
                let _ = tx
                    .send(Ok(StreamToken::ToolCall {
                        name,
                        args: args_val,
                        id,
                    }))
                    .await;
            }
            log_stream_result(started, first_token, last_usage.as_ref());
            let _ = tx.send(Ok(StreamToken::Done)).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "openai"
    }
}
/// Builder for [`OpenAiVendor`].
pub struct OpenAiVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiVendorBuilder {
    /// Create a builder with OpenAI defaults.
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".to_string(),
            model: "gpt-4".to_string(),
            api_key: None,
        }
    }

    /// Set the API endpoint.
    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Set the model id.
    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    /// Build the vendor, or fail if no key was set.
    pub fn build(self) -> Result<OpenAiVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(OpenAiVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for OpenAiVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A thin OpenAI client wrapper used by the bindings.
pub struct OpenAiClient {
    inner: OpenAiVendor,
}

impl Clone for OpenAiClient {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl OpenAiClient {
    /// Create a client with an explicit endpoint, model, and key.
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            inner: OpenAiVendor::new(endpoint, model, api_key),
        }
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for OpenAiClient
{
    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        LlmClient::<S, C>::list_models(&self.inner).await
    }
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        self.inner.complete(persona, req, session, context).await
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        self.inner
            .stream_complete(persona, req, session, context)
            .await
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::vendor::OpenAiVendor;
    use crate::llm::{Content, LlmClient, LlmContext, LlmRequest, LlmSession};

    #[test]
    fn a_stream_request_asks_the_provider_for_usage() {
        let request = super::OpenAiRequest {
            model: "m".to_string(),
            messages: Vec::new(),
            tools: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            reasoning_effort: None,
            stream: Some(true),
            stream_options: Some(super::OpenAiStreamOptions {
                include_usage: true,
            }),
        };
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn a_non_stream_request_omits_stream_options() {
        let request = super::OpenAiRequest {
            model: "m".to_string(),
            messages: Vec::new(),
            tools: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
            reasoning_effort: None,
            stream: Some(false),
            stream_options: None,
        };
        let json = serde_json::to_value(&request).expect("serialize");
        assert!(json.get("stream_options").is_none());
    }

    #[test]
    fn serialize_args_converts_null_to_empty_object() {
        assert_eq!(super::serialize_args(&serde_json::Value::Null), "{}");
    }

    #[test]
    fn serialize_args_passes_through_valid_object() {
        let args = serde_json::json!({"location": "NYC"});
        assert_eq!(super::serialize_args(&args), "{\"location\":\"NYC\"}");
    }

    #[test]
    fn serialize_args_converts_non_object_to_empty() {
        assert_eq!(
            super::serialize_args(&serde_json::Value::String("foo".into())),
            "{}"
        );
        assert_eq!(super::serialize_args(&serde_json::Value::Bool(true)), "{}");
        assert_eq!(
            super::serialize_args(&serde_json::Value::Number(42.into())),
            "{}"
        );
    }

    /// MiniMax uses the OpenAI-compatible `image_url` object, so the ORIGINAL
    /// base64 payload must reach the wire unchanged. This pins the exact shape
    /// the normal image turn and the replay path both feed to the vendor.
    #[test]
    fn image_content_serializes_as_an_openai_image_url() {
        let content = Content::binary("image/png", "AAEC");
        assert_eq!(
            super::serialize_content(&content),
            serde_json::json!([{
                "type": "image_url",
                "image_url": { "url": "data:image/png;base64,AAEC" }
            }])
        );
    }

    #[tokio::test]
    async fn test_openai_vendor() {
        let mut section = Section::default();
        let result = section.init();

        match result.await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Skipping test (no config): {}", e);
                return;
            }
        };

        #[derive(Debug, Deserialize, Clone)]
        struct LlmConfig {
            model: String,
            base_url: String,
            api_key: String,
        }

        let llm_config: LlmConfig = match section.extract("global_model") {
            Some(c) => c,
            None => {
                eprintln!("Skipping test (no global_model config)");
                return;
            }
        };

        let model_name = if llm_config.model.starts_with("openai/") {
            llm_config
                .model
                .strip_prefix("openai/")
                .unwrap()
                .to_string()
        } else if !llm_config.model.contains('/') {
            llm_config.model.clone()
        } else {
            eprintln!("Skipping test: model should not contain provider prefix for OpenAI");
            return;
        };

        let vendor = OpenAiVendor::new(
            llm_config.base_url.clone(),
            model_name,
            llm_config.api_key.clone(),
        );

        let request = LlmRequest {
            model: llm_config.model.clone(),
            input: Content::text("What is 2+2?"),
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            reasoning_effort: None,
            api_mode: crate::llm::ApiMode::Chat,
            request_id: None,
            intent: Default::default(),
        };
        let outcome = vendor
            .complete(
                None,
                request,
                &mut LlmSession::new(),
                &mut LlmContext::default(),
            )
            .await;

        if let Err(e) = outcome {
            let err_str = format!("{:?}", e);
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!(
                    "OpenAI endpoint/model not available (404), skipping test: {}",
                    err_str
                );
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("OpenAI rate limited, skipping test: {}", err_str);
                return;
            }
            panic!("OpenAI request failed: {:?}", e);
        }

        println!("{:?}", outcome);
    }

    /// A model list is either read faithfully or reported as unparsable; an
    /// empty provider answer stays an empty list instead of hiding behind the
    /// same error as a body that is not a model list.
    #[test]
    fn parse_model_ids_reads_the_openai_list() {
        let body = r#"{"object":"list","data":[{"id":"gpt-4o"},{"id":"o3-mini"}]}"#;
        assert_eq!(super::parse_model_ids(body).unwrap(), ["gpt-4o", "o3-mini"]);
        assert_eq!(
            super::parse_model_ids(r#"{"data":[]}"#).unwrap(),
            Vec::<String>::new(),
            "a provider with nothing to list is not an error"
        );
        assert!(matches!(
            super::parse_model_ids("not json"),
            Err(crate::llm::LlmError::Parse(_))
        ));
    }

    /// Both endpoint spellings reach the same path, so a trailing slash does
    /// not produce the double slash some gateways reject.
    #[test]
    fn models_url_joins_with_exactly_one_slash() {
        let bare = OpenAiVendor::new(
            "https://api.example.com/v1".to_string(),
            "m".to_string(),
            "k".to_string(),
        );
        assert_eq!(bare.models_url(), "https://api.example.com/v1/models");
        let slashed = OpenAiVendor::new(
            "https://api.example.com/v1/".to_string(),
            "m".to_string(),
            "k".to_string(),
        );
        assert_eq!(slashed.models_url(), "https://api.example.com/v1/models");
    }
    /// One captured HTTP request: path, lower-cased headers and decoded body.
    #[derive(Debug, Clone, Default)]
    struct Captured {
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    /// A local Messages endpoint: captures every request and answers with a
    /// minimal Anthropic SSE stream, so a test asserts the WIRE rather than
    /// the mere absence of an error.
    async fn spawn_messages_mock() -> (
        std::net::SocketAddr,
        std::sync::Arc<std::sync::Mutex<Vec<Captured>>>,
    ) {
        use std::sync::{Arc, Mutex};
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let sink = Arc::clone(&sink);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let head_end = loop {
                        let Ok(n) = socket.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let mut lines = head.lines();
                    let path = lines
                        .next()
                        .unwrap_or_default()
                        .split(' ')
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let mut headers = Vec::new();
                    let mut len = 0usize;
                    for line in lines {
                        if let Some((name, value)) = line.split_once(':') {
                            let name = name.trim().to_ascii_lowercase();
                            let value = value.trim().to_string();
                            if name == "content-length" {
                                len = value.parse().unwrap_or(0);
                            }
                            headers.push((name, value));
                        }
                    }
                    while buf.len() < head_end + len {
                        let Ok(n) = socket.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into_owned();
                    sink.lock().expect("lock").push(Captured {
                        path,
                        headers,
                        body,
                    });
                    let sse = concat!(
                        "event: message_start\n",
                        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
                        "event: content_block_start\n",
                        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                        "event: content_block_delta\n",
                        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello from messages\"}}\n\n",
                        "event: content_block_stop\n",
                        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                        "event: message_delta\n",
                        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
                        "event: message_stop\n",
                        "data: {\"type\":\"message_stop\"}\n\n",
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        sse.len(),
                        sse
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (addr, captured)
    }

    /// A local endpoint that answers every request with 404, capturing the
    /// path so a test can assert the error names it.
    async fn spawn_404_mock() -> (
        std::net::SocketAddr,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        use std::sync::{Arc, Mutex};
        let paths = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&paths);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let sink = Arc::clone(&sink);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let head_end = loop {
                        let Ok(n) = socket.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let mut lines = head.lines();
                    let path = lines
                        .next()
                        .unwrap_or_default()
                        .split(' ')
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let len: usize = lines
                        .find_map(|l| {
                            let (n, v) = l.split_once(':')?;
                            n.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    while buf.len() < head_end + len {
                        let Ok(n) = socket.read(&mut tmp).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    sink.lock().expect("lock").push(path);
                    let body = "{\"error\":\"no such route\"}";
                    let response = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (addr, paths)
    }

    /// A failed call names the endpoint it tried, so a gateway that lacks a
    /// protocol path is diagnosable from the error alone — on all three wires.
    #[tokio::test]
    async fn http_errors_name_the_attempted_endpoint() {
        use crate::llm::{ApiMode, LlmError};
        let (addr, paths) = spawn_404_mock().await;
        let vendor = OpenAiVendor::new(format!("http://{addr}"), "m".to_string(), "k".to_string());
        for (mode, suffix) in [
            (ApiMode::Chat, "/chat/completions"),
            (ApiMode::Responses, "/responses"),
            (ApiMode::Anthropic, "/v1/messages"),
        ] {
            let outcome = vendor
                .complete(
                    None,
                    LlmRequest {
                        model: "m".to_string(),
                        input: Content::text("hi"),
                        api_mode: mode,
                        ..Default::default()
                    },
                    &mut LlmSession::new(),
                    &mut LlmContext::default(),
                )
                .await;
            let message = match outcome {
                Err(LlmError::Http(message)) | Err(LlmError::Other(message)) => message,
                other => panic!("expected an HTTP failure, got {other:?}"),
            };
            assert!(message.contains("404"), "the status survives: {message}");
            assert!(
                message.contains(suffix),
                "the error names the attempted path {suffix}: {message}"
            );
        }
        let seen = paths.lock().expect("lock");
        assert_eq!(seen.len(), 3, "one attempt per protocol");
    }

    /// The OpenAI-compatible transport speaks all three protocols: Anthropic
    /// mode keeps the same endpoint and key but the wire is Messages —
    /// /v1/messages, x-api-key, anthropic-version, a top-level max_tokens —
    /// on both the whole-response and the streaming path.
    #[tokio::test]
    async fn anthropic_mode_speaks_messages_on_the_same_endpoint() {
        use crate::llm::{ApiMode, StreamToken};
        use futures::StreamExt;
        let (addr, captured) = spawn_messages_mock().await;
        let vendor = OpenAiVendor::new(
            format!("http://{addr}"),
            "gpt-test".to_string(),
            "test-key".to_string(),
        );
        let request = || LlmRequest {
            model: "gpt-test".to_string(),
            input: Content::text("hi"),
            api_mode: ApiMode::Anthropic,
            ..Default::default()
        };
        let outcome = vendor
            .complete(
                None,
                request(),
                &mut LlmSession::new(),
                &mut LlmContext::default(),
            )
            .await
            .expect("the messages protocol completes");
        assert_eq!(
            outcome.text(),
            Some("hello from messages"),
            "the aggregate carries the streamed text"
        );
        let mut stream = vendor
            .stream_complete(
                None,
                request(),
                &mut LlmSession::new(),
                &mut LlmContext::default(),
            )
            .await
            .expect("the messages protocol streams");
        let mut streamed = String::new();
        while let Some(token) = stream.next().await {
            if let StreamToken::Text(chunk) = token.expect("a token") {
                streamed.push_str(&chunk);
            }
        }
        assert_eq!(streamed, "hello from messages");
        let seen = captured.lock().expect("lock");
        assert_eq!(seen.len(), 2, "one request per path");
        for cap in seen.iter() {
            assert_eq!(cap.path, "/v1/messages", "the wire path is Messages");
            assert!(
                cap.headers
                    .iter()
                    .any(|(n, v)| n == "x-api-key" && v == "test-key"),
                "the key rides x-api-key, not Authorization"
            );
            assert!(
                cap.headers.iter().any(|(n, v)| n == "anthropic-version"
                    && v == crate::llm::vendor::anthropic::ANTHROPIC_VERSION),
                "the version header is present"
            );
            let body: serde_json::Value = serde_json::from_str(&cap.body).expect("json body");
            assert_eq!(body["model"], "gpt-test");
            assert!(
                body["max_tokens"].is_number(),
                "Messages requires max_tokens"
            );
            assert!(body["messages"].is_array());
        }
    }
}
