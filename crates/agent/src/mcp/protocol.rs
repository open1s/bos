use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(u64),
    String(String),
}

impl Default for JsonRpcId {
    fn default() -> Self {
        JsonRpcId::Number(0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: serde_json::Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: serde_json::Value,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: serde_json::Value,
    #[serde(default)]
    pub resources: serde_json::Value,
    #[serde(default)]
    pub prompts: serde_json::Value,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            tools: serde_json::Value::Bool(false),
            resources: serde_json::Value::Bool(false),
            prompts: serde_json::Value::Bool(false),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, alias = "inputSchema")]
    pub input_schema: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>, id: u64) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(id),
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    pub fn success(id: u64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(id),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(id),
            result: None,
            error: Some(error),
        }
    }

    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

impl JsonRpcError {
    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: msg.into(),
            data: None,
        }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: msg.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: method.into(),
            data: None,
        }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<McpPromptArgument>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContents>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContents {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
}
