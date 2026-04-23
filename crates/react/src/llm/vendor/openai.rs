use std::sync::Arc;

use crate::{extractor::{JsonExtractor, StreamExtractor}, llm::vendor::openaicompatible::{ChatCompletionResponse, OpenAIExtractor}};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, StreamToken, Stringfy,
    TokenStream,
};

pub struct OpenAiVendor {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Arc<String>,
}

impl Clone for OpenAiVendor {
    fn clone(&self) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
            endpoint: self.endpoint.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
        }
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
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize, Clone)]
struct OpenAiMessageJson {
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

impl OpenAiVendor {
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

    pub fn builder() -> OpenAiVendorBuilder {
        OpenAiVendorBuilder::new()
    }

    fn convert_request(&self, req: LlmRequest) -> OpenAiRequest {
        let mut messages = Vec::new();

        for message in req.context.conversations {
            let json_msg = match message {
                crate::llm::LlmMessage::System { content } => OpenAiMessageJson {
                    role: "system",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::User { content } => OpenAiMessageJson {
                    role: "user",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::Assistant { content } => OpenAiMessageJson {
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
                    OpenAiMessageJson {
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
                } => OpenAiMessageJson {
                    role: "tool",
                    content: Some(content),
                    tool_call_id: Some(tool_call_id),
                    tool_calls: None,
                },
            };
            messages.push(json_msg);
        }

        //Availale Skill as system
        let available_skills = if !req.context.skills.is_empty() {
            let available_skills = req
                .context
                .skills
                .into_iter()
                .map(|s| s.json())
                .collect::<Vec<_>>()
                .join("\n");
            let skill_schema = format!(
                "You have access to the following skills(call load_skill to read it when needed):\n{}\n",
                available_skills
            );
            Some(skill_schema)
        } else {
            None
        };

        let available_rules = if !req.context.rules.is_empty() {
            let available_rules = req
                .context
                .rules
                .into_iter()
                .map(|r| r.json())
                .collect::<Vec<_>>()
                .join("\n");
            let rules = format!("You should follow below rules:\n{}\n", available_rules);
            Some(rules)
        } else {
            None
        };

        let available_instructions = if !req.context.instructions.is_empty() {
            let available_insts = req
                .context
                .instructions
                .into_iter()
                .map(|i| i.yaml())
                .collect::<Vec<_>>()
                .join("\n");
            let insts = format!("You should follow below rules:\n{}\n", available_insts);
            Some(insts)
        } else {
            None
        };

        let mut extra_system_prompt = available_skills.unwrap_or_default();

        extra_system_prompt.push('\n');
        if let Some(rules) = available_rules {
            extra_system_prompt.push_str(&rules);
        }

        if let Some(instructions) = available_instructions {
            extra_system_prompt.push_str(&instructions);
        }

        if !extra_system_prompt.is_empty() {
            let meta = OpenAiMessageJson {
                role: "system",
                content: Some(extra_system_prompt),
                tool_call_id: None,
                tool_calls: None,
            };
            messages.insert(0, meta);
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

        OpenAiRequest {
            model: req.model,
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream: Some(false),
        }
    }

    fn build_stream_request(&self, req: LlmRequest) -> OpenAiRequest {
        let mut req = req;

        if req.model.is_empty() {
            req.model = self.model.clone();
        }

        let mut openai_req = self.convert_request(req);
        openai_req.stream = Some(true);
        openai_req
    }
}

#[async_trait]
impl LlmClient for OpenAiVendor {
    async fn complete(&self, mut request: LlmRequest) -> LlmResponseResult {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        let openai_req = self.convert_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&openai_req)
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

        let openai_req = self.build_stream_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&openai_req)
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
        "openai"
    }
}

pub struct OpenAiVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".to_string(),
            model: "gpt-4".to_string(),
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

    pub fn build(self) -> Result<OpenAiVendor, &'static str> {
        let api_key = self.api_key.ok_or("api_key is required")?;
        Ok(OpenAiVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for OpenAiVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            inner: OpenAiVendor::new(endpoint, model, api_key),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(&self, req: LlmRequest) -> LlmResponseResult {
        self.inner.complete(req).await
    }

    async fn stream_complete(&self, req: LlmRequest) -> Result<TokenStream, LlmError> {
        let (tx, rx) = mpsc::channel(32);
        let inner = self.inner.clone();

        tokio::spawn(async move {
            match inner.stream_complete(req).await {
                Ok(stream) => {
                    let mut stream = Box::pin(stream);
                    while let Some(item) = stream.next().await {
                        if tx.send(item).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                }
            }
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

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::vendor::OpenAiVendor;
    use crate::{LlmClient, LlmRequest};

    #[tokio::test]
    async fn test_openai_vendor() {
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

        let model_name = if llm_config.model.starts_with("openai/") {
            llm_config.model.strip_prefix("openai/").unwrap().to_string()
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

        let request = LlmRequest::with_user(&llm_config.model, "What is 2+2?");
        let outcome = vendor.complete(request).await;
        
        if let Err(e) = outcome {
            let err_str = format!("{:?}", e);
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!("OpenAI endpoint/model not available (404), skipping test: {}", err_str);
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
}
