use std::sync::Arc;

use crate::{extractor::{JsonExtractor, StreamExtractor}, llm::vendor::{OpenAIExtractor, openaicompatible::ChatCompletionResponse}};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Serialize};

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, StreamToken, Stringfy,
    TokenStream,
};

pub struct NvidiaVendor {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Arc<String>,
}

#[derive(Serialize,Debug)]
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

#[derive(Serialize,Debug, Clone)]
struct NvidiaMessageJson {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallJson>>,
}

#[derive(Serialize,Debug, Clone)]
struct ToolCallJson {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: FunctionCallJson,
}

#[derive(Serialize,Debug, Clone)]
struct FunctionCallJson {
    name: String,
    arguments: String,
}

impl NvidiaVendor {
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

    pub fn builder() -> NvidiaVendorBuilder {
        NvidiaVendorBuilder::new()
    }

    fn convert_request(&self, req: LlmRequest) -> NvidiaRequest {
        let mut messages = Vec::new();
        for message in req.context.conversations {
            let json_msg = match message {
                crate::llm::LlmMessage::System { content } => NvidiaMessageJson {
                    role: "system",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::User { content } => NvidiaMessageJson {
                    role: "user",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                crate::llm::LlmMessage::Assistant { content } => NvidiaMessageJson {
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
                    NvidiaMessageJson {
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
                } => NvidiaMessageJson {
                    role: "tool",
                    content: Some(content),
                    tool_call_id: Some(tool_call_id),
                    tool_calls: None,
                },
            };
            messages.push(json_msg);
        }

        // Availale Skill as system
        let available_skills = if !req.context.skills.is_empty() {
            let available_skills = req
                .context
                .skills
                .into_iter()
                .map(|s| s.json())
                .collect::<Vec<_>>()
                .join("\n");
            let skill_schema = format!(
                "Available Skills(call load_skill to read it when needed):\n{}\n",
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
            let meta = NvidiaMessageJson {
                role: "system",
                content: Some(extra_system_prompt),
                tool_call_id: None,
                tool_calls: None,
            };
            messages.insert(0, meta);
        }

        let tools = if !req.context.tools.is_empty() {
            let tools: Vec<serde_json::Value> = req
                .context
                .tools
                .into_iter()
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
                .collect();
            Some(tools)
        } else {
            None
        };

        let max_tokens = req.max_tokens.unwrap_or(1280000);

        NvidiaRequest {
            model: req.model,
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: Some(max_tokens),
            stream: Some(false),
            top_p: req.top_p,
            top_k: req.top_k,
        }
    }

    fn build_stream_request(&self, mut req: LlmRequest) -> NvidiaRequest {
        if req.model.is_empty() {
            req.model = self.model.clone();
        }

        let mut nvidia_req = self.convert_request(req);
        nvidia_req.stream = Some(true);
        nvidia_req
    }
}

#[async_trait]
impl LlmClient for NvidiaVendor {
    async fn complete(&self, mut request: LlmRequest) -> LlmResponseResult {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        let nvidia_req = self.convert_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&nvidia_req)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Http(format!("HTTP {}: {}", status, body)));
        }

        let value: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        Ok(LlmResponse::OpenAI(value))        
    }

    async fn stream_complete(&self, mut request: LlmRequest) -> Result<TokenStream, LlmError> {
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        if request.model.is_empty() {
            request.model = self.model.clone();
        }

        let nvidia_req = self.build_stream_request(request);

        let url = format!("{}/chat/completions", endpoint);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&nvidia_req)
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

    pub fn build(self) -> Result<NvidiaVendor, &'static str> {
        let api_key = self.api_key.ok_or("api_key is required")?;
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

    use crate::{JsonExtractor, LlmClient, LlmRequest, StreamExtractor, llm::vendor::{NvidiaVendor, OpenAIExtractor}};

    
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
    async fn test_vendor(){
        let mut section = Section::default();
        let result = section.init();


        let _config = match result.await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Skipping test (no config): {}", e);
                return;
            }
        };

        #[derive(Debug,Deserialize, Clone)]
        struct LlmConfig  {
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
            llm_config.model.strip_prefix("nvidia/").unwrap().to_string()
        } else {
            llm_config.model.clone()
        };

        let vendor = NvidiaVendor::new(
            llm_config.base_url.clone(),
            model_name,
            llm_config.api_key.clone(),
        );

        let request = LlmRequest::with_user(&llm_config.model, "What is 2+2?, must use add tool");
        let result = match vendor.complete(request).await {
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

        println!("{:?}",result);
    }
}