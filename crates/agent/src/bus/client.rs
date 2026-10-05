use std::pin::Pin;
use std::sync::Arc;

use async_stream::stream;
use bus::{Caller, Session};
use futures::{Stream, StreamExt};

use react::ToolError;
use super::transport::*;
use super::wire::*;

/// Typed client for agent-to-agent RPC over bus.
///
/// This is the simplest user-facing API:
/// - `list()`
/// - `call(tool_name, args)`
/// - `llm_run(task)`
/// - `stream_run(task)`
#[derive(Clone)]
pub struct AgentRpcClient {
    endpoint: String,
    transport: Arc<dyn RpcTransport>,
}

impl AgentRpcClient {
    pub fn new(endpoint: impl Into<String>, session: Arc<Session>) -> Self {
        let endpoint = endpoint.into();
        let caller_endpoint = endpoint.clone();
        let stream_endpoint = endpoint.clone();
        let caller_session = session.clone();
        Self {
            endpoint: endpoint.clone(),
            transport: Arc::new(BusCallerTransport {
                caller: Caller::new(caller_endpoint, Some(caller_session)),
                endpoint: stream_endpoint,
                session,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_transport(endpoint: impl Into<String>, transport: Arc<dyn RpcTransport>) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport,
        }
    }

    async fn invoke_rpc(&self, req: AgentRpcRequest) -> Result<serde_json::Value, ToolError> {
        let payload = serde_json::to_string(&req)
            .map_err(|e| ToolError::Failed(format!("encode request failed: {}", e)))?;
        let response_payload = self.transport.request(&payload).await?;
        let response = decode_response(&response_payload)?;
        if response.ok {
            Ok(response.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(ToolError::Failed(
                response.error.unwrap_or_else(|| "remote error".to_string()),
            ))
        }
    }

    async fn invoke_rpc_stream(
        &self,
        req: AgentRpcRequest,
    ) -> Result<RpcResponseStream, ToolError> {
        let payload = serde_json::to_string(&req)
            .map_err(|e| ToolError::Failed(format!("encode request failed: {}", e)))?;
        self.transport.request_stream(&payload).await
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn list(&self) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "tool/list".to_string(),
            task: None,
            tool_name: None,
            args: None,
        })
        .await
    }

    pub async fn call(
        &self,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "tool/call".to_string(),
            task: None,
            tool_name: Some(tool_name.into()),
            args: Some(args),
        })
        .await
    }

    pub async fn llm_run(&self, task: impl Into<String>) -> Result<serde_json::Value, ToolError> {
        self.invoke_rpc(AgentRpcRequest {
            method: "llm/run".to_string(),
            task: Some(task.into()),
            tool_name: None,
            args: None,
        })
        .await
    }

    /// Stream remote agent tokens over bus RPC.
    pub async fn stream_run_live(
        &self,
        task: impl Into<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<crate::StreamToken, ToolError>> + Send>>, ToolError>
    {
        let response_stream = self
            .invoke_rpc_stream(AgentRpcRequest {
                method: "stream/run".to_string(),
                task: Some(task.into()),
                tool_name: None,
                args: None,
            })
            .await?;

        let stream = stream! {
            tokio::pin!(response_stream);
            while let Some(item) = response_stream.next().await {
                match item {
                    Ok(payload) => {
                        let response = match decode_response(&payload) {
                            Ok(v) => v,
                            Err(e) => {
                                yield Err(e);
                                break;
                            }
                        };

                        if !response.ok {
                            yield Err(ToolError::Failed(
                                response.error.unwrap_or_else(|| "remote error".to_string()),
                            ));
                            break;
                        }

                        let Some(result) = response.result else {
                            continue;
                        };

                        if let Some(event) = result.get("event").and_then(|v| v.as_object()) {
                            match event.get("type").and_then(|v| v.as_str()) {
                                Some("text") => {
                                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                                        yield Ok(crate::StreamToken::Text(text.to_string()));
                                    }
                                }
                                Some("tool_call") => {
                                    let name = event
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string();
                                    let args = event
                                        .get("args")
                                        .cloned()
                                        .unwrap_or_else(|| serde_json::json!({}));
                                    let id = event
                                        .get("id")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                    yield Ok(crate::StreamToken::ToolCall { name, args, id });
                                }
                                Some("done") => {
                                    yield Ok(crate::StreamToken::Done);
                                    break;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // Backward compatibility for older server responses.
                        if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
                            yield Ok(crate::StreamToken::Text(text.to_string()));
                            yield Ok(crate::StreamToken::Done);
                            break;
                        }
                        if let Some(chunks) = result.get("chunks").and_then(|v| v.as_array()) {
                            for chunk in chunks {
                                if let Some(text) = chunk.as_str() {
                                    yield Ok(crate::StreamToken::Text(text.to_string()));
                                }
                            }
                            yield Ok(crate::StreamToken::Done);
                            break;
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Run remote stream endpoint and aggregate response for compatibility.
    pub async fn stream_run(
        &self,
        task: impl Into<String>,
    ) -> Result<serde_json::Value, ToolError> {
        let mut text = String::new();
        let mut chunks = Vec::new();
        let mut _stream_placeholder_ = self.stream_run_live(task).await?;
        while let Some(item) = _stream_placeholder_.next().await {
            match item? {
                crate::StreamToken::Text(t) => {
                    text.push_str(&t);
                    chunks.push(t);
                }
                crate::StreamToken::ReasoningContent(_t) => {
                    //SKIP
                }
                crate::StreamToken::Usage(_) => {
                    //SKIP
                }
                crate::StreamToken::ToolCall { name, args, id } => chunks.push(format!(
                    "[tool_call] name={} id={} args={}",
                    name,
                    id.unwrap_or_default(),
                    args
                )),
                crate::StreamToken::Done => break,
                crate::StreamToken::Stopped => break,
            }
        }
        Ok(serde_json::json!({ "text": text, "chunks": chunks }))
    }
}
