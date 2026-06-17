use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    llm::vendor::{openaicompatible::ChatCompletionResponse, OpenAIExtractor},
    utils::{JsonExtractor, StreamExtractor},
};
use async_trait::async_trait;
use futures::StreamExt;
use log::info;
use reqwest::Client;
use serde::Serialize;

use crate::llm::{
    Content, ContentPart, LlmClient, LlmError, LlmRequest, LlmResponse,
    LlmResponseResult, ReactContext, ReactSession, StreamToken, TokenStream, VendorBuilderError,
};

pub struct NvidiaVendor {
    client: Arc<Client>,
    endpoint: Arc<String>,
    model: String,
    api_key: Arc<String>,
}

#[derive(Serialize, Debug)]
struct NvidiaRequest {
    model: String,
    messages: Vec<NvidiaMessageJson>,
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
    stream: Option<bool>,
}

#[derive(Serialize, Debug, Clone)]
struct NvidiaMessageJson {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallJson>>,
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
        Content::Text(s) => {
            if let Ok(parts) = serde_json::from_str::<Vec<ContentPart>>(s) {
                serde_json::Value::Array(parts.iter().map(serialize_part).collect())
            } else if let Ok(part) = serde_json::from_str::<ContentPart>(s) {
                serde_json::Value::Array(vec![serialize_part(&part)])
                                } else {
                                    serde_json::Value::String(s.clone())
                                }
                            }
                            Content::Parts(parts) => {
                                serde_json::Value::Array(parts.iter().map(serialize_part).collect())
                            }
                        }
                    }

fn serialize_args(args: &serde_json::Value) -> String {
    if args.is_null() || !args.is_object() {
        "{}".to_string()
    } else {
        args.to_string()
    }
}

#[derive(Serialize, Debug, Clone)]
struct ToolCallJson {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: FunctionCallJson,
}

#[derive(Serialize, Debug, Clone)]
struct FunctionCallJson {
    name: String,
    arguments: String,
}

impl NvidiaVendor {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        let client = Client::builder()
            // .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client: Arc::new(client),
            endpoint: Arc::new(endpoint),
            model,
            api_key: Arc::new(api_key),
        }
    }

    pub fn builder() -> NvidiaVendorBuilder {
        NvidiaVendorBuilder::new()
    }

    fn convert_request(
        &self,
        persona: Option<String>,
        req: &LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> NvidiaRequest {
        let mut messages = Vec::new();
        if let Some(history) = session.history() {
            for message in history.iter() {
                let json_msg = match message {
                    crate::llm::LlmMessage::System { content } => NvidiaMessageJson {
                        role: "system",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::User { content } => NvidiaMessageJson {
                        role: "user",
                        content: Some(serialize_content(content)),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::Assistant { content } => NvidiaMessageJson {
                        role: "assistant",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::AssistantToolCall {
                        tool_call_id: id,
                        name,
                        args,
                    } => NvidiaMessageJson {
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
                    } => NvidiaMessageJson {
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
            messages.push(NvidiaMessageJson {
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

        // Add skills, rules, instructions as system prompt
        let mut extra_system_prompt = String::new();

        if let Some(p) = &persona {
            extra_system_prompt.push_str(&format!("Persona: {}\n", p));
        }

        if let Some(skills) = context.skills() {
            if !skills.is_empty() {
                let skill_names = skills
                    .iter()
                    .map(|s| format!("- {}: {}", s.name, s.description))
                    .collect::<Vec<_>>()
                    .join("\n");
                extra_system_prompt.push_str(&format!(
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
                    NvidiaMessageJson {
                        role: "system",
                        content: Some(serde_json::Value::String(extra_system_prompt)),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                );

        let max_tokens = req.max_tokens.unwrap_or(12800);

        NvidiaRequest {
            model: req.model.clone(),
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: Some(max_tokens),
            stream: Some(false),
            top_p: req.top_p,
            top_k: req.top_k,
        }
    }

    fn build_stream_request(
        &self,
        persona: Option<String>,
        mut req: LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> NvidiaRequest {
        if req.model.is_empty() {
            req.model = self.model.clone();
        }

        let mut nvidia_req = self.convert_request(persona, &req, session, context);
        nvidia_req.stream = Some(true);
        nvidia_req
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for NvidiaVendor
{
    async fn complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        let api_key = Arc::clone(&self.api_key);
        let client = Arc::clone(&self.client);
        let endpoint = Arc::clone(&self.endpoint);

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let t0 = std::time::Instant::now();
        let nvidia_req = self.convert_request(persona, &request, session, context);
        info!("[TIMING] convert_request: {:?}", t0.elapsed());

        let url = format!("{}/chat/completions", endpoint);

        info!(
            "Req: {}",
            serde_json::to_string(&nvidia_req)
                .unwrap_or_else(|_| "Failed to serialize request".into())
        );

        let t1 = std::time::Instant::now();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&nvidia_req)
            .send()
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
            let err = LlmError::Http(format!("HTTP {}: {}", status, body));
            context.notify_error(&err);
            return Err(err);
        }

        let t2 = std::time::Instant::now();
        let value: ChatCompletionResponse = response.json().await.map_err(|e| {
            let err = LlmError::Parse(e.to_string());
            context.notify_error(&err);
            err
        })?;
        info!("[TIMING] response.json(): {:?}", t2.elapsed());
        info!("[TIMING] complete total: {:?}", t0.elapsed());
        let resp = LlmResponse::OpenAI(value);

        info!(
            "Resp: {}",
            serde_json::to_string(&resp).unwrap_or_else(|_| "Failed to serialize response".into())
        );

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
        let api_key = Arc::clone(&self.api_key);
        let client = Arc::clone(&self.client);
        let endpoint = Arc::clone(&self.endpoint);

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let nvidia_req = self.build_stream_request(persona, request, session, context);

        info!(
            "Req: {}",
            serde_json::to_string(&nvidia_req)
                .unwrap_or_else(|_| "Failed to serialize request".into())
        );

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&nvidia_req)
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

        use tokio::sync::mpsc;
        let (tx, rx) = mpsc::channel(32);
        let on_chunk = context.on_chunk_callback();

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut extractor = OpenAIExtractor::new(JsonExtractor::default());
            // Accumulate streaming tool call deltas by index.
            // OpenAI/NVIDIA streaming splits tool call arguments across chunks;
            // we must accumulate until arguments form valid JSON.
            let mut pending_tool_calls: HashMap<u32, (Option<String>, Option<String>, String)> =
                HashMap::new();

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        if let Some(chats) = extractor.push(&text) {
                            for chat in chats {
                                for choice in chat.choices {
                                    if let Some(content) = &choice.delta.reasoning_content {
                                        if !content.is_empty() {
                                            on_chunk.as_ref().map(|cb| cb(content));
                                            let _ = tx
                                                .send(Ok(StreamToken::ReasoningContent(
                                                    content.clone(),
                                                )))
                                                .await;
                                        }
                                    }
                                    if let Some(content) = &choice.delta.content {
                                        if !content.is_empty() {
                                            on_chunk.as_ref().map(|cb| cb(content));
                                            let _ = tx
                                                .send(Ok(StreamToken::Text(content.clone())))
                                                .await;
                                        }
                                    }
                                    if let Some(calls) = &choice.delta.tool_calls {
                                        for call in calls {
                                            let index = call.index;
                                            let id = call.id.clone().filter(|s| !s.is_empty());
                                            let name = call
                                                .function
                                                .as_ref()
                                                .and_then(|f| f.name.clone())
                                                .filter(|s| !s.is_empty());
                                            let args_delta = call
                                                .function
                                                .as_ref()
                                                .and_then(|f| f.arguments.clone())
                                                .unwrap_or_default();

                                            let entry = pending_tool_calls
                                                .entry(index.unwrap_or(0))
                                                .or_insert_with(|| (None, None, String::new()));
                                            if let Some(n) = name {
                                                entry.0 = Some(n);
                                            }
                                            if let Some(i) = id {
                                                entry.1 = Some(i);
                                            }
                                            entry.2.push_str(&args_delta);

                                            // Accumulated arguments form valid JSON → emit
                                            let args_str = entry.2.clone();
                                            if let Ok(args_val) = serde_json::from_str::<
                                                serde_json::Value,
                                            >(
                                                &args_str
                                            ) {
                                                let name = entry.0.clone();
                                                let id = entry.1.clone();
                                                if let (
                                                    Some(ref n),
                                                    Some(ref i),
                                                ) = (name, id)
                                                {
                                                    on_chunk
                                                        .as_ref()
                                                        .map(|cb| cb(n));
                                                    let _ = tx
                                                        .send(Ok(StreamToken::ToolCall {
                                                            name: n.clone(),
                                                            args: args_val,
                                                            id: Some(i.clone()),
                                                        }))
                                                        .await;
                                                    pending_tool_calls.remove(&index.unwrap_or(0));
                                                }
                                            }
                                        }
                                    }

                                    // Finish reason "tool_calls" → flush any pending
                                    if choice.finish_reason.as_deref()
                                        == Some("tool_calls")
                                    {
                                        for (_, (name, id, args_str)) in
                                            pending_tool_calls.drain()
                                        {
                                            let name = name.unwrap_or_default();
                                            let id = id.unwrap_or_else(|| {
                                                uuid::Uuid::new_v4().to_string()
                                            });
                                            let args_val: serde_json::Value =
                                                serde_json::from_str(&args_str)
                                                    .unwrap_or(serde_json::json!({}));
                                            let _ = tx
                                                .send(Ok(StreamToken::ToolCall {
                                                    name,
                                                    args: args_val,
                                                    id: Some(id),
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
                                    let _ = tx
                                        .send(Ok(StreamToken::Usage(usage.clone())))
                                        .await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let err = LlmError::Other(format!("Stream error: {}", e));
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            }
            // Stream ended — flush any remaining pending tool calls
            for (_, (name, id, args_str)) in pending_tool_calls.drain() {
                let name = name.unwrap_or_default();
                let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let args_val: serde_json::Value =
                    serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
                let _ = tx
                    .send(Ok(StreamToken::ToolCall {
                        name,
                        args: args_val,
                        id: Some(id),
                    }))
                    .await;
            }
            let _ = tx.send(Ok(StreamToken::Done)).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "nvidia"
    }
}

pub struct NvidiaVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl NvidiaVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://integrate.api.nvidia.com/v1".to_string(),
            model: "mistralai/mixtral-8x7b-instruct-v0.1".to_string(),
            api_key: None,
        }
    }

    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    pub fn build(self) -> Result<NvidiaVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(NvidiaVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for NvidiaVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::{Content, LlmClient, LlmRequest, LlmSession};
    use crate::{
        llm::vendor::{NvidiaVendor, OpenAIExtractor},
        JsonExtractor, StreamExtractor,
    };

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
        assert_eq!(super::serialize_args(&serde_json::Value::String("foo".into())), "{}");
        assert_eq!(super::serialize_args(&serde_json::Value::Bool(true)), "{}");
        assert_eq!(super::serialize_args(&serde_json::Value::Number(42.into())), "{}");
    }

    fn accumulate_tool_call(
        pending: &mut std::collections::HashMap<
            u32,
            (Option<String>, Option<String>, String),
        >,
        emitted: &mut Vec<String>,
        index: u32,
        id: Option<String>,
        name: Option<String>,
        args_delta: &str,
    ) {
        let entry = pending.entry(index).or_insert_with(|| (None, None, String::new()));
        if let Some(n) = name { entry.0 = Some(n); }
        if let Some(i) = id { entry.1 = Some(i); }
        entry.2.push_str(args_delta);

        let args_str = entry.2.clone();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&args_str) {
            if let (Some(ref n), Some(ref i)) = (entry.0.clone(), entry.1.clone()) {
                emitted.push(format!("{}|{}|{}", n, i, v.to_string()));
                pending.remove(&index);
            }
        }
    }

    #[test]
    fn streaming_accumulation_multiple_chunks_produce_single_tool_call() {
        let mut pending: std::collections::HashMap<
            u32,
            (Option<String>, Option<String>, String),
        > = std::collections::HashMap::new();
        let mut emitted: Vec<String> = Vec::new();

        accumulate_tool_call(&mut pending, &mut emitted, 0,
            Some("call_abc".into()), Some("get_weather".into()), "");
        assert!(emitted.is_empty());
        assert_eq!(pending.len(), 1);

        accumulate_tool_call(&mut pending, &mut emitted, 0,
            None, None, "{\"loc");
        assert!(emitted.is_empty());
        assert_eq!(pending.len(), 1);

        accumulate_tool_call(&mut pending, &mut emitted, 0,
            None, None, "ation\": \"NYC\"}");
        assert_eq!(emitted.len(), 1);
        assert!(emitted[0].contains("get_weather"));
        assert!(emitted[0].contains("NYC"));
        assert!(pending.is_empty());
    }

    #[test]
    fn streaming_accumulation_flushes_on_finish_reason() {
        // When finish_reason is tool_calls but args never complete,
        // pending tool calls should still be flushed

        let mut pending: std::collections::HashMap<
            u32,
            (Option<String>, Option<String>, String),
        > = std::collections::HashMap::new();

        // Accumulate a partial tool call
        pending.insert(0, (Some("get_weather".into()), Some("call_abc".into()), "{\"loc".into()));

        // Simulate finish_reason flush
        let mut flushed: Vec<String> = Vec::new();
        for (_, (name, id, args_str)) in pending.drain() {
            let name = name.unwrap_or_default();
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let args_val: serde_json::Value =
                serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
            flushed.push(format!("{}|{}|{}", name, id, args_val.to_string()));
        }

        assert_eq!(flushed.len(), 1, "flush on finish_reason");
        assert!(flushed[0].contains("get_weather"), "name preserved on flush");
        assert!(flushed[0].contains("{}"), "args fallback to empty object");
        assert!(pending.is_empty(), "pending cleared after flush");
    }

    #[test]
    fn test() {
        let mut extractor = OpenAIExtractor::new(JsonExtractor::default());

        let chunk = r#"data: {"id":"chatcmpl-958091ac43bbd265","object":"chat.completion.chunk","created":1777006903,"model":"meta/llama-4-maverick-17b-128e-instruct","choices":[{"index":0,"delta":{"content":"name","reasoning_content":null},"logprobs":null,"finish_reason":null,"token_ids":null}]}"#;

        let spans = extractor.push(chunk);

        //add tool call chunk
        let chunk2 = r#"data: {"id":"chatcmpl-958091ac43bbd265","object":"chat.completion.chunk","created":1777006903,"model":"meta/llama-4-maverick-17b-128e-instruct","choices":[{"index":0,"delta":{"tool_calls":[{"id":"toolcall-123","type":"function","function":{"name":"get_current_weather","arguments":"{\"location\": \"San Francisco, CA\", \"unit\": \"celsius\"}"}}]},"logprobs":null,"finish_reason":null,"token_ids":null}]}"#;

        let spans2 = extractor.push(chunk2);

        assert!(spans.is_some());
        assert!(spans2.is_some());
        println!("Extracted spans: {:?}", spans);
        println!("Extracted spans2: {:?}", spans2);
    }

    #[tokio::test]
    async fn test_vendor() {
        let mut section = Section::default();
        let result = section.init();

        let _config = match result.await {
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

        let model_name = if llm_config.model.starts_with("nvidia/") {
            llm_config
                .model
                .strip_prefix("nvidia/")
                .unwrap()
                .to_string()
        } else {
            llm_config.model.clone()
        };

        let vendor = NvidiaVendor::new(
            llm_config.base_url.clone(),
            model_name,
            llm_config.api_key.clone(),
        );

        let request = LlmRequest {
            model: llm_config.model.clone(),
            input: Content::text("What is 2+2?, must use add tool"),
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
        };
        let result = match vendor
            .complete(None, request, &mut LlmSession::new(), &mut ())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("404") || err_str.contains("not found") {
                    eprintln!(
                        "NVIDIA endpoint/model not available (404), skipping test: {}",
                        err_str
                    );
                    return;
                }
                if err_str.contains("429") || err_str.contains("rate limit") {
                    eprintln!("NVIDIA rate limited, skipping test: {}", err_str);
                    return;
                }
                panic!("NVIDIA request failed: {:?}", e);
            }
        };

        println!("{:?}", result);
    }
}
