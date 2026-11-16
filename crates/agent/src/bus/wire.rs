use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use react::ToolError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentRpcRequest {
    pub(crate) method: String,
    pub(crate) task: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentRpcResponse {
    pub(crate) ok: bool,
    pub(crate) result: Option<serde_json::Value>,
    pub(crate) error: Option<String>,
}

pub(crate) fn parse_request(payload: &str) -> Result<AgentRpcRequest, String> {
    serde_json::from_str(payload).map_err(|e| format!("invalid request JSON: {}", e))
}

pub(crate) fn encode_response(resp: AgentRpcResponse) -> Result<String, String> {
    serde_json::to_string(&resp).map_err(|e| format!("encode response failed: {}", e))
}

pub(crate) fn decode_response(payload: &str) -> Result<AgentRpcResponse, ToolError> {
    serde_json::from_str(payload)
        .map_err(|e| ToolError::Failed(format!("invalid response JSON: {}", e)))
}

pub(crate) type RpcResponseStream = Pin<Box<dyn Stream<Item = Result<String, ToolError>> + Send>>;
