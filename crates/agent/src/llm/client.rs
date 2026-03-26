use std::pin::Pin;
use std::borrow::Cow;

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use crate::streaming::{SseDecoder, SseEvent};
use super::{LlmClient, LlmError, LlmRequest, LlmResponse, OpenAiMessage, StreamToken};

pub struct OpenAiClient {
    client: Client,
    chat_completions_url: String,
    api_key: String,
}

#[derive(Serialize, Clone)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessageJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
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

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Deserialize)]
#[serde(bound(deserialize = "'de: 'a"))]
struct BorrowedStreamChoice<'a> {
    #[serde(borrow)]
    delta: BorrowedStreamDelta<'a>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "'de: 'a"))]
struct BorrowedStreamDelta<'a> {
    content: Option<Cow<'a, str>>,
    #[serde(borrow)]
    tool_calls: Option<Vec<BorrowedStreamToolCall<'a>>>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "'de: 'a"))]
struct BorrowedStreamToolCall<'a> {
    #[serde(borrow)]
    #[serde(rename = "function")]
    function: BorrowedStreamFunctionCall<'a>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "'de: 'a"))]
struct BorrowedStreamFunctionCall<'a> {
    name: Option<Cow<'a, str>>,
    arguments: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "'de: 'a"))]
struct BorrowedOpenAiStreamResponse<'a> {
    #[serde(borrow)]
    choices: Vec<BorrowedStreamChoice<'a>>,
}

impl OpenAiClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();
        Self {
            client,
            chat_completions_url: format!("{}/chat/completions", base_url),
            api_key,
        }
    }

    fn build_request(&self, req: LlmRequest, stream: bool) -> OpenAiRequest {
        let mut messages = Vec::with_capacity(req.messages.len());
        for message in req.messages {
            let json_msg = match message {
                OpenAiMessage::System { content } => OpenAiMessageJson {
                    role: "system",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                OpenAiMessage::User { content } => OpenAiMessageJson {
                    role: "user",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                OpenAiMessage::Assistant { content } => OpenAiMessageJson {
                    role: "assistant",
                    content: Some(content),
                    tool_call_id: None,
                    tool_calls: None,
                },
                OpenAiMessage::AssistantToolCall { id, name, args } => {
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
                OpenAiMessage::ToolResult { tool_call_id, content } => OpenAiMessageJson {
                    role: "tool",
                    content: Some(content),
                    tool_call_id: Some(tool_call_id),
                    tool_calls: None,
                }
            };
            messages.push(json_msg);
        }

        OpenAiRequest {
            model: req.model,
            messages,
            tools: req.tools.map(|tools| (*tools).clone()),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream,
        }
    }

    fn parse_token(line: &str) -> Option<StreamToken> {
        let data: BorrowedOpenAiStreamResponse<'_> = serde_json::from_str(line).ok()?;
        for choice in data.choices {
            if let Some(content) = choice.delta.content {
                if !content.is_empty() {
                    return Some(StreamToken::Text(content.into_owned()));
                }
            }
            if let Some(calls) = choice.delta.tool_calls {
                for call in calls {
                    if let (Some(name), Some(args)) = (call.function.name, call.function.arguments) {
                        let args_val: serde_json::Value = serde_json::from_str(args.as_ref()).ok()?;
                        return Some(StreamToken::ToolCall {
                            name: name.into_owned(),
                            args: args_val,
                            id: None,
                        });
                    }
                }
            }
        }
        None
    }

    pub fn parse_stream_token(line: &str) -> Option<StreamToken> {
        Self::parse_token(line)
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let openai_req = self.build_request(req, false);
        let response = self
            .client
            .post(&self.chat_completions_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&openai_req)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::Http(format!("HTTP {}: {}", status, body)));
        }

        let body: OpenAiResponse = response.json().await.map_err(|e| LlmError::Parse(e.to_string()))?;

        let choice = body.choices.into_iter().next().ok_or_else(|| {
            LlmError::Parse("No choices in response".to_string())
        })?;

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
            None => {
                if choice.finish_reason.as_deref() == Some("stop") {
                    return Ok(LlmResponse::Done);
                }
                Ok(LlmResponse::Done)
            }
        }
    }

    fn stream_complete(
        &self,
        req: LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send + '_>> {
        let url = self.chat_completions_url.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let openai_req = serde_json::to_string(&self.build_request(req, true)).ok();

        let (tx, rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let body = match openai_req {
                Some(b) => b,
                None => {
                    let _ = tx.send(Err(LlmError::Parse("Failed to serialize request".to_string()))).await;
                    return;
                }
            };

            let response = match client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(LlmError::Http(e.to_string()))).await;
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let _ = tx.send(Err(LlmError::Http(format!("HTTP {}: {}", status, body)))).await;
                return;
            }

            let body_bytes = match response.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(LlmError::Http(e.to_string()))).await;
                    return;
                }
            };

            let mut decoder = SseDecoder::new();

            for event in decoder.decode_chunk(body_bytes.as_ref()) {
                let token = match event {
                    SseEvent::Data(line) => {
                        Self::parse_token(&line).map(Ok)
                    }
                    SseEvent::Done => Some(Ok(StreamToken::Done)),
                    SseEvent::Error(msg) => Some(Err(LlmError::Parse(msg))),
                };

                if let Some(t) = token {
                    if tx.send(t).await.is_err() {
                        break;
                    }
                }
            }
        });

        Box::pin(ReceiverStream::new(rx))
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
    use super::*;

    #[test]
    fn test_parse_token_text() {
        let json = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        let token = OpenAiClient::parse_token(json);

        assert!(matches!(token, Some(StreamToken::Text(ref text)) if text == "Hello"));
    }

    #[test]
    fn test_parse_token_tool_call() {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"test_tool","arguments":"{\"param\":\"value\"}"}}]}}]}"#;
        let token = OpenAiClient::parse_token(json);

        assert!(
            matches!(
                token,
                Some(StreamToken::ToolCall { ref name, ref args, .. })
                    if name == "test_tool" && args["param"] == "value"
            ),
            "unexpected token: {:?}",
            token
        );
    }
}
