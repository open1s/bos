use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use surfing::JSONParser;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, StreamToken,
    Stringfy, TokenStream, StreamResponseAccumulator,
};

pub struct NvidiaVendor {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Arc<String>,
}

#[derive(Serialize)]
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

#[derive(Serialize, Clone)]
struct NvidiaMessageJson {
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

#[derive(Debug, Deserialize)]
struct NvidiaResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Choice {
    message: MessageContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageContent {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    id: Option<String>,
    function: FunctionCall,
}

#[derive(Debug, Deserialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamChoice {
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    #[allow(dead_code)]
    content: Option<String>,
    tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct StreamToolCall {
    id: Option<String>,
    #[allow(dead_code)]
    index: Option<usize>,
    #[serde(rename = "type")]
    _call_type: Option<String>,
    function: StreamFunctionCall,
}

#[derive(Debug, Deserialize)]
struct StreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NvidiaStreamResponse {
    choices: Vec<StreamChoice>,
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

        NvidiaRequest {
            model: req.model,
            messages,
            tools,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
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

        let body: NvidiaResponse = response
            .json()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        let choice = body
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Parse("No choices in response".to_string()))?;

        if let Some(tool_calls) = choice.message.tool_calls {
            if let Some(tc) = tool_calls.into_iter().next() {
                let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or_else(|_| serde_json::json!({}));
                return Ok(LlmResponse::ToolCall {
                    name: tc.function.name,
                    args,
                    id: tc.id,
                });
            }
        }

        match choice.message.content {
            Some(content) => Ok(LlmResponse::Text(content)),
            None => Ok(LlmResponse::Done),
        }
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
            // Use surfing for JSON extraction
            let mut parser = JSONParser::new();
            let mut accumulator = StreamResponseAccumulator::new(move |response, start_idx| {
                let remaining = if start_idx < response.len() {
                    &response[start_idx..]
                } else {
                    return (start_idx, None);
                };

                let mut all_tokens = Vec::new();
                let mut bytes_written = Vec::new();
                
                let write_result = {
                    let mut writer = std::io::BufWriter::new(&mut bytes_written);
                    parser.extract_json_from_stream(&mut writer, remaining)
                };
                
                if write_result.is_ok() {
                    let json_str = String::from_utf8_lossy(&bytes_written);
                    if let Ok(resp) = serde_json::from_str::<NvidiaStreamResponse>(&json_str) {
                        for choice in &resp.choices {
                            if let Some(content) = &choice.delta.content {
                                if !content.is_empty() {
                                    all_tokens.push(StreamToken::Text(content.clone()));
                                }
                            }
                            if let Some(calls) = &choice.delta.tool_calls {
                                for call in calls {
                                    let name = call.function.name.as_ref();
                                    let args = call.function.arguments.as_ref();
                                    if let (Some(n), Some(a)) = (name, args) {
                                        let args_val: serde_json::Value =
                                            match serde_json::from_str(a) {
                                                Ok(v) => v,
                                                Err(_) => serde_json::Value::Null,
                                            };
                                        all_tokens.push(StreamToken::ToolCall {
                                            name: n.clone(),
                                            args: args_val,
                                            id: call.id.clone(),
                                        });
                                    }
                                }
                            }
                        }
                        let new_idx = start_idx + bytes_written.len();
                        return (new_idx, Some(all_tokens));
                    }
                }
                
                (start_idx, None)
            });

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).to_string();
                        if let Some(tokens) = accumulator.push(&text) {
                            for tk in tokens {
                                match &tk {
                                    StreamToken::Done => {
                                        let _ = tx.send(Ok(StreamToken::Done)).await;
                                        return;
                                    }
                                    _ => {
                                        let _ = tx.send(Ok(tk)).await;
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