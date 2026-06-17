use std::sync::Arc;

use crate::{
    llm::vendor::openaicompatible::{ChatCompletionResponse, OpenAIExtractor},
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

pub struct OpenRouterVendor {
    client: Arc<Client>,
    endpoint: Arc<String>,
    model: String,
    api_key: Arc<String>,
}

#[derive(Serialize)]
struct OpenRouterRequest {
    model: String,
    messages: Vec<OpenRouterMessageJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Serialize, Clone)]
struct OpenRouterMessageJson {
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

impl OpenRouterVendor {
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

    pub fn builder() -> OpenRouterVendorBuilder {
        OpenRouterVendorBuilder::new()
    }

    fn convert_request(
        &self,
        persona: Option<String>,
        req: &LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> OpenRouterRequest {
        let mut messages = Vec::new();
        if let Some(history) = session.history() {
            for message in history {
                let json_msg = match message {
                    crate::llm::LlmMessage::System { content } => OpenRouterMessageJson {
                        role: "system",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::User { content } => OpenRouterMessageJson {
                        role: "user",
                        content: Some(serialize_content(content)),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::Assistant { content } => OpenRouterMessageJson {
                        role: "assistant",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    crate::llm::LlmMessage::AssistantToolCall {
                        tool_call_id: id,
                        name,
                        args,
                    } => OpenRouterMessageJson {
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
                    } => OpenRouterMessageJson {
                        role: "tool",
                        content: Some(serde_json::Value::String(content.clone())),
                        tool_call_id: Some(tool_call_id.clone()),
                        tool_calls: None,
                    },
                };
                messages.push(json_msg);
            }
        };

        if messages.is_empty() {
            messages.push(OpenRouterMessageJson {
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
            OpenRouterMessageJson {
                role: "system",
                content: Some(serde_json::Value::String(extra_system_prompt)),
                tool_call_id: None,
                tool_calls: None,
            },
        );

        let max_tokens = req.max_tokens.unwrap_or(12800);

        OpenRouterRequest {
            model: req.model.clone(),
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: Some(max_tokens),
            stream: false,
        }
    }

    fn build_stream_request(
        &self,
        persona: Option<String>,
        mut req: LlmRequest,
        session: &impl ReactSession,
        context: &impl crate::llm::types::ReactContext,
    ) -> OpenRouterRequest {
        if req.model.is_empty() {
            req.model = self.model.clone();
        }

        let mut openrouter_req = self.convert_request(persona,&req, session, context);
        openrouter_req.stream = true;
        openrouter_req
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for OpenRouterVendor
{
    async fn complete(
        &self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        let api_key = self.api_key.clone();
        let client = Arc::clone(&self.client);
        let endpoint = Arc::clone(&self.endpoint);

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let t0 = std::time::Instant::now();
        let openrouter_req = self.convert_request(persona,&request, session, context);
        info!("[TIMING] convert_request: {:?}", t0.elapsed());

        let url = format!("{}/chat/completions", endpoint);

        info!(
            "Req: {}",
            serde_json::to_string(&openrouter_req)
                .unwrap_or_else(|_| "Failed to serialize request".into())
        );

        let t1 = std::time::Instant::now();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&openrouter_req)
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
        let body: ChatCompletionResponse = response.json().await.map_err(|e| {
            let err = LlmError::Parse(e.to_string());
            context.notify_error(&err);
            err
        })?;
        info!("[TIMING] response.json(): {:?}", t2.elapsed());
        info!("[TIMING] complete total: {:?}", t0.elapsed());
        let resp = LlmResponse::OpenAI(body);

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
        let api_key = self.api_key.clone();
        let client = Arc::clone(&self.client);
        let endpoint = Arc::clone(&self.endpoint);

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        context.notify_request(&request);

        let openrouter_req = self.build_stream_request(persona,request, session, context);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&openrouter_req)
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
            let mut pending_tool_calls: std::collections::HashMap<
                u32,
                (Option<String>, Option<String>, String),
            > = std::collections::HashMap::new();

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
                                            let index = call.index.unwrap_or(0);
                                            let id =
                                                call.id.clone().filter(|s| !s.is_empty());
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
                                                .entry(index)
                                                .or_insert_with(|| (None, None, String::new()));
                                            if let Some(n) = name {
                                                entry.0 = Some(n);
                                            }
                                            if let Some(i) = id {
                                                entry.1 = Some(i);
                                            }
                                            entry.2.push_str(&args_delta);

                                            let args_str = entry.2.clone();
                                            if let Ok(args_val) =
                                                serde_json::from_str::<serde_json::Value>(
                                                    &args_str,
                                                )
                                            {
                                                let name = entry.0.clone();
                                                let id = entry.1.clone();
                                                if let (Some(ref n), Some(ref i)) = (name, id) {
                                                    on_chunk.as_ref().map(|cb| cb(n));
                                                    let _ = tx
                                                        .send(Ok(StreamToken::ToolCall {
                                                            name: n.clone(),
                                                            args: args_val,
                                                            id: Some(i.clone()),
                                                        }))
                                                        .await;
                                                    pending_tool_calls.remove(&index);
                                                }
                                            }
                                        }
                                    }

                                    if choice.finish_reason.as_deref() == Some("tool_calls") {
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
        "openrouter"
    }
}

pub struct OpenRouterVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl OpenRouterVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/v1".to_string(),
            model: "anthropic/claude-3.5-sonnet".to_string(),
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

    pub fn build(self) -> Result<OpenRouterVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(OpenRouterVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for OpenRouterVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::vendor::OpenRouterVendor;
    use crate::llm::{Content, LlmClient, LlmContext, LlmRequest, LlmSession};

    #[tokio::test]
    async fn test_openrouter_vendor() {
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

        let model_name = if llm_config.model.starts_with("openrouter/") {
            llm_config
                .model
                .strip_prefix("openrouter/")
                .unwrap()
                .to_string()
        } else if llm_config.model.contains('/') {
            llm_config.model.clone()
        } else {
            eprintln!("Skipping test: model should contain provider/model for OpenRouter");
            return;
        };

        let vendor = OpenRouterVendor::new(
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
        };
        let outcome = vendor
            .complete(None, request, &mut LlmSession::new(), &mut LlmContext::default())
            .await;

        if let Err(e) = outcome {
            let err_str = format!("{:?}", e);
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!(
                    "OpenRouter endpoint/model not available (404), skipping test: {}",
                    err_str
                );
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("OpenRouter rate limited, skipping test: {}", err_str);
                return;
            }
            panic!("OpenRouter request failed: {:?}", e);
        }

        println!("{:?}", outcome);
    }
}
