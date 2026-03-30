use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, McpPrompt, McpResource, ReadResourceResult,
    ServerCapabilities, ToolDefinition,
};
use super::transport::StdioTransport;

#[derive(Error, Debug)]
pub enum McpError {
    #[error("Transport error: {0}")]
    Transport(#[from] super::transport::TransportError),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Server error: {code} {message}")]
    Server { code: i32, message: String },

    #[error("Not initialized")]
    NotInitialized,

    #[error("Initialization failed: {0}")]
    InitFailed(String),
}

impl From<McpError> for crate::error::ToolError {
    fn from(e: McpError) -> Self {
        crate::error::ToolError::ExecutionFailed(e.to_string())
    }
}

pub struct McpClient {
    transport: Arc<Mutex<StdioTransport>>,
    request_id: std::sync::atomic::AtomicU64,
    capabilities: std::sync::Mutex<Option<ServerCapabilities>>,
    recv_buffer: std::sync::Mutex<String>,
}

impl McpClient {
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, McpError> {
        let transport = StdioTransport::spawn(command, args).await?;
        Ok(Self {
            transport: Arc::new(Mutex::new(transport)),
            request_id: std::sync::atomic::AtomicU64::new(1),
            capabilities: std::sync::Mutex::new(None),
            recv_buffer: std::sync::Mutex::new(String::with_capacity(8192)),
        })
    }

    pub async fn initialize(&self) -> Result<ServerCapabilities, McpError> {
        let resp = self
            .call(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {
                        "roots": { "listChanged": true },
                        "sampling": {}
                    },
                    "clientInfo": {
                        "name": "brainos-agent",
                        "version": "1.0.0"
                    }
                })),
            )
            .await?;

        if let Some(error) = resp.error {
            return Err(McpError::InitFailed(error.message));
        }

        let capabilities = resp
            .result
            .as_ref()
            .and_then(|v| v.get("capabilities"))
            .cloned()
            .unwrap_or(serde_json::json!({"tools": false, "resources": false, "prompts": false}));

        let caps: ServerCapabilities =
            serde_json::from_value(capabilities).map_err(|e| McpError::Protocol(e.to_string()))?;

        *self.capabilities.lock().unwrap() = Some(caps.clone());

        self.notify("notifications/initialized", None).await?;

        Ok(caps)
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>, McpError> {
        let resp = self.call("tools/list", None).await?;

        if let Some(error) = resp.error {
            return Err(McpError::Server {
                code: error.code,
                message: error.message,
            });
        }

        let tools = resp
            .result
            .as_ref()
            .and_then(|v| v.get("tools"))
            .cloned()
            .unwrap_or(serde_json::json!([]));

        let defs: Vec<ToolDefinition> =
            serde_json::from_value(tools).map_err(|e| McpError::Protocol(e.to_string()))?;

        Ok(defs)
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let resp = self
            .call(
                "tools/call",
                Some(serde_json::json!({
                    "name": name,
                    "arguments": args
                })),
            )
            .await?;

        if let Some(error) = resp.error {
            return Err(McpError::Server {
                code: error.code,
                message: error.message,
            });
        }

        let result = resp
            .result
            .ok_or_else(|| McpError::Protocol("No result in response".to_string()))?;

        Self::parse_tool_call_result(result)
    }

    pub async fn list_resources(&self) -> Result<Vec<McpResource>, McpError> {
        let resp = self.call("resources/list", None).await?;

        if let Some(error) = resp.error {
            return Err(McpError::Server {
                code: error.code,
                message: error.message,
            });
        }

        match resp.result {
            Some(result) => {
                let resources: Vec<McpResource> = serde_json::from_value(result)
                    .map_err(|e| McpError::Protocol(e.to_string()))?;
                Ok(resources)
            }
            None => Ok(vec![]),
        }
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, McpError> {
        let resp = self
            .call(
                "resources/read",
                Some(serde_json::json!({
                    "uri": uri
                })),
            )
            .await?;

        if let Some(error) = resp.error {
            return Err(McpError::Server {
                code: error.code,
                message: error.message,
            });
        }

        match resp.result {
            Some(result) => {
                let content: ReadResourceResult = serde_json::from_value(result)
                    .map_err(|e| McpError::Protocol(e.to_string()))?;
                Ok(content)
            }
            None => Err(McpError::Protocol("No result in response".to_string())),
        }
    }

    pub async fn list_prompts(&self) -> Vec<McpPrompt> {
        match self.call("prompts/list", None).await {
            Ok(resp) => {
                if resp.error.is_some() {
                    return vec![];
                }

                match resp.result {
                    Some(result) => serde_json::from_value(result).unwrap_or_default(),
                    None => vec![],
                }
            }
            Err(_) => vec![],
        }
    }

    pub async fn get_capabilities(&self) -> Option<ServerCapabilities> {
        self.capabilities.lock().unwrap().clone()
    }

    async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, McpError> {
        let id = self
            .request_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let req = JsonRpcRequest::new(method, params, id);
        let json = serde_json::to_vec(&req)?;
        let mut transport = self.transport.lock().await;
        transport.send(&json).await?;

        let mut buffer = {
            let lock = self.recv_buffer.lock().unwrap();
            lock.clone()
        };
        transport.recv_line(&mut buffer).await?;
        let resp: JsonRpcResponse = serde_json::from_str(&buffer)?;
        Ok(resp)
    }

    async fn notify(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), McpError> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let json = serde_json::to_vec(&req)?;
        let mut transport = self.transport.lock().await;
        transport.send(&json).await?;
        Ok(())
    }

    fn parse_tool_call_result(result: serde_json::Value) -> Result<serde_json::Value, McpError> {
        if result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let message = result
                .get("content")
                .and_then(|v| v.as_array())
                .and_then(|items| {
                    items
                        .iter()
                        .find_map(|item| item.get("text").and_then(|v| v.as_str()))
                })
                .map(|s| s.to_string())
                .or_else(|| {
                    result
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "MCP tool call returned isError=true".to_string());

            return Err(McpError::Protocol(message));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::McpClient;

    #[test]
    fn parse_tool_call_result_accepts_success_payload() {
        let payload = serde_json::json!({
            "content": [{"type": "text", "text": "ok"}]
        });

        let parsed = McpClient::parse_tool_call_result(payload.clone()).unwrap();
        assert_eq!(parsed, payload);
    }

    #[test]
    fn parse_tool_call_result_rejects_is_error_payload() {
        let payload = serde_json::json!({
            "isError": true,
            "content": [{"type": "text", "text": "tool failed"}]
        });

        let err = McpClient::parse_tool_call_result(payload).unwrap_err();
        assert!(err.to_string().contains("tool failed"));
    }
}
