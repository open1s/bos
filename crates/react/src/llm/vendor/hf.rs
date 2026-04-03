use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, StreamToken, TokenStream,
};

pub struct HFVendor {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Arc<String>,
}

#[derive(Serialize)]
struct HFRequest {
    inputs: String,
    parameters: HFParameters,
}

#[derive(Serialize, Default)]
struct HFParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_new_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_full_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct HFResponse {
    #[serde(default)]
    generated_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HFStreamResponse {
    token: HFToken,
}

#[derive(Debug, Deserialize)]
struct HFToken {
    text: Option<String>,
    #[serde(default)]
    special: Option<bool>,
}

impl HFVendor {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            endpoint,
            model,
            api_key: Arc::new(api_key),
        }
    }

    pub fn builder() -> HFVendorBuilder {
        HFVendorBuilder::new()
    }

    fn build_input(&self, req: &LlmRequest) -> String {
        let mut input = String::new();

        if !req.context.system.is_empty() {
            input.push_str(&format!("System: {}\n", req.context.system));
        }

        for message in &req.context.history {
            match message {
                crate::llm::LlmMessage::System { content } => {
                    input.push_str(&format!("System: {}\n", content));
                }
                crate::llm::LlmMessage::User { content } => {
                    input.push_str(&format!("User: {}\n", content));
                }
                crate::llm::LlmMessage::Assistant { content } => {
                    input.push_str(&format!("Assistant: {}\n", content));
                }
                crate::llm::LlmMessage::AssistantToolCall { name, args, .. } => {
                    input.push_str(&format!(
                        "Assistant: Calling tool {} with {:?}\n",
                        name, args
                    ));
                }
                crate::llm::LlmMessage::ToolResult { content, .. } => {
                    input.push_str(&format!("Tool result: {}\n", content));
                }
            }
        }

        if !req.context.user_input.is_empty() {
            input.push_str(&format!("User: {}\n", req.context.user_input));
        }

        input
    }

    fn convert_request(&self, req: LlmRequest) -> HFRequest {
        let inputs = self.build_input(&req);

        HFRequest {
            inputs,
            parameters: HFParameters {
                temperature: if req.temperature != 0.0 {
                    Some(req.temperature)
                } else {
                    None
                },
                max_new_tokens: req.max_tokens,
                return_full_text: Some(false),
                stream: Some(false),
            },
        }
    }

    fn build_stream_request(&self, req: LlmRequest) -> HFRequest {
        let inputs = self.build_input(&req);

        HFRequest {
            inputs,
            parameters: HFParameters {
                temperature: if req.temperature != 0.0 {
                    Some(req.temperature)
                } else {
                    None
                },
                max_new_tokens: req.max_tokens,
                return_full_text: Some(false),
                stream: Some(true),
            },
        }
    }
}

#[async_trait]
impl LlmClient for HFVendor {
    async fn complete(&self, request: LlmRequest) -> LlmResponseResult {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let model = self.model.clone();

        let hf_req = self.convert_request(request);

        let url = format!("{}/models/{}/infer", endpoint, model);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&hf_req)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Http(format!("HTTP {}: {}", status, body)));
        }

        let body: HFResponse = response
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        match body.generated_text {
            Some(text) => Ok(LlmResponse::Text(text)),
            None => Ok(LlmResponse::Done),
        }
    }

    async fn stream_complete(&self, request: LlmRequest) -> Result<TokenStream, LlmError> {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let model = self.model.clone();

        let hf_req = self.build_stream_request(request);

        let url = format!("{}/models/{}/infer", endpoint, model);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&hf_req)
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

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        for line in text.lines() {
                            if line.starts_with("data: ") {
                                let data = line.strip_prefix("data: ").unwrap_or("");
                                if data.is_empty() {
                                    continue;
                                }
                                if let Ok(response) = serde_json::from_str::<HFStreamResponse>(data)
                                {
                                    if let Some(token_text) = response.token.text {
                                        if !token_text.is_empty()
                                            && response.token.special != Some(true)
                                        {
                                            let _ =
                                                tx.send(Ok(StreamToken::Text(token_text))).await;
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
        false
    }

    fn provider_name(&self) -> &'static str {
        "huggingface"
    }
}

pub struct HFVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl HFVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://api-inference.huggingface.co".to_string(),
            model: "meta-llama/Llama-2-70b-chat-hf".to_string(),
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

    pub fn build(self) -> Result<HFVendor, &'static str> {
        let api_key = self.api_key.ok_or("api_key is required")?;
        Ok(HFVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for HFVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}
