use std::sync::Arc;

use crate::{extractor::{JsonExtractor, StreamExtractor}, llm::vendor::openaicompatible::{ChatCompletionResponse, OpenAIExtractor}};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Serialize};

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, StreamToken, Stringfy,
    TokenStream,
};

pub struct OpenRouterVendor {
    client: Client,
    endpoint: String,
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
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallJson>>,
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
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            endpoint,
            model,
            api_key: Arc::new(api_key),
        }
    }

    pub fn builder() -> OpenRouterVendorBuilder {
        OpenRouterVendorBuilder::new()
    }

    fn convert_request(&self, req: LlmRequest) -> OpenRouterRequest {
        let mut messages = Vec::new();

        for message in req.context.conversations {
            let json_msg = match message {
                crate::llm::LlmMessage::System { content } => OpenRouterMessageJson {
                    role: "system",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::User { content } => OpenRouterMessageJson {
                    role: "user",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::Assistant { content } => OpenRouterMessageJson {
                    role: "assistant",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::AssistantToolCall {
                    tool_call_id: id,
                    name,
                    args,
                } => {
                    let args_str = serde_json::to_string(&args).unwrap_or_default();
                    OpenRouterMessageJson {
                        role: "assistant",
                        content: None,
                        tool_call_id: None,
                        tool_calls: Some(vec![ToolCallJson {
                            id,
                            call_type: "function",
                            function: FunctionCallJson {
                                name,
                                arguments: args_str,
                            },
                        }]),
                    }
                }
                crate::llm::LlmMessage::ToolResult {
                    tool_call_id,
                    content,
                } => OpenRouterMessageJson {
                    role: "tool",
                    content: Some(content),
                    tool_call_id: Some(tool_call_id),
                    tool_calls: None,
                },
            };
            messages.push(json_msg);
        }

        let tools = if !req.context.tools.is_empty() {
            let tools = req
                .context
                .tools
                .into_iter()
                .map(|t| t.to_value().unwrap())
                .collect::<Vec<_>>();
            Some(tools)
        } else {
            None
        };

        OpenRouterRequest {
            model: req.model,
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: false,
        }
    }

    fn build_stream_request(&self, mut req: LlmRequest) -> OpenRouterRequest {
        if req.model.is_empty() {
            req.model = self.model.clone();
        }

        let mut openrouter_req = self.convert_request(req);
        openrouter_req.stream = true;
        openrouter_req
    }
}

#[async_trait]
impl LlmClient for OpenRouterVendor {
    async fn complete(&self, mut request: LlmRequest) -> LlmResponseResult {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        let openrouter_req = self.convert_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&openrouter_req)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Http(format!("HTTP {}: {}", status, body)));
        }

        let body: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        Ok(LlmResponse::OpenAI(body))
    }

    async fn stream_complete(&self, mut request: LlmRequest) -> Result<TokenStream, LlmError> {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        let openrouter_req = self.build_stream_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&openrouter_req)
            .send()
            .await
            .map_err(|e| LlmError::Other(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Other(format!("HTTP {}: {}", status, body)));
        }

        use tokio::sync::mpsc;
        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut extractor = OpenAIExtractor::new(JsonExtractor::default());

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        if let Some(chats) = extractor.push(&text) {
                            for chat in chats {
                                for choice in chat.choices {
                                    if let Some(content) = &choice.delta.reasoning_content {
                                        if !content.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamToken::ReasoningContent(content.clone())))
                                                .await;
                                        }
                                    }
                                    if let Some(content) = &choice.delta.content {
                                        if !content.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamToken::Text(content.clone())))
                                                .await;
                                        }
                                    }
                                    if let Some(calls) = &choice.delta.tool_calls {
                                        for call in calls {
                                            let name = call.function.as_ref().and_then(|f| f.name.clone()).unwrap_or_default();
                                            let args_str = call.function.as_ref().and_then(|f| f.arguments.clone()).unwrap_or_default();
                                            let args_val: serde_json::Value = match serde_json::from_str(&args_str) {
                                                Ok(v) => v,
                                                Err(_) => serde_json::Value::Null,
                                            };
                                            let id = call.id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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

                                    if let Some(reason) = &choice.finish_reason {
                                        if !reason.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamToken::Done))
                                                .await;
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(LlmError::Other(format!("Stream error: {}", e))))
                            .await;
                        return;
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

    pub fn build(self) -> Result<OpenRouterVendor, &'static str> {
        let api_key = self.api_key.ok_or("api_key is required")?;
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
    use crate::{LlmClient, LlmRequest};

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
            llm_config.model.strip_prefix("openrouter/").unwrap().to_string()
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

        let request = LlmRequest::with_user(&llm_config.model, "What is 2+2?");
        let outcome = vendor.complete(request).await;
        
        if let Err(e) = outcome {
            let err_str = format!("{:?}", e);
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!("OpenRouter endpoint/model not available (404), skipping test: {}", err_str);
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
