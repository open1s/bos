use serde::{Deserialize, Serialize};

/// A JSON-RPC request identifier, numeric or string.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Numeric identifier.
    Number(u64),
    /// String identifier.
    String(String),
}

impl Default for JsonRpcId {
    fn default() -> Self {
        JsonRpcId::Number(0)
    }
}

/// A JSON-RPC 2.0 request.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version, always `2.0`.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// Request identifier echoed in the response.
    #[serde(default)]
    pub id: serde_json::Value,
    /// Method name to invoke.
    pub method: String,
    /// Optional method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version, always `2.0`.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// Identifier of the request being answered.
    #[serde(default)]
    pub id: serde_json::Value,
    /// Successful result, if any.
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// Error result, if any.
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional additional error data.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

/// Capabilities advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Tool capability description, or `false` when unsupported.
    #[serde(default)]
    pub tools: serde_json::Value,
    /// Resource capability description, or `false` when unsupported.
    #[serde(default)]
    pub resources: serde_json::Value,
    /// Prompt capability description, or `false` when unsupported.
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

/// An MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Tool description.
    #[serde(default)]
    pub description: String,
    /// JSON schema for the tool arguments.
    #[serde(default, alias = "inputSchema")]
    pub input_schema: serde_json::Value,
}

impl JsonRpcRequest {
    /// Build a request for `method` with an optional params payload.
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
    /// Build a success response.
    pub fn success(id: u64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(id),
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    pub fn error(id: u64, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(id),
            result: None,
            error: Some(error),
        }
    }

    /// Whether this response carries an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

impl JsonRpcError {
    /// A `-32700` parse error.
    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: msg.into(),
            data: None,
        }
    }

    /// A `-32600` invalid-request error.
    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: msg.into(),
            data: None,
        }
    }

    /// A `-32601` method-not-found error.
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: method.into(),
            data: None,
        }
    }

    /// A `-32602` invalid-params error.
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }

    /// A `-32603` internal error.
    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            data: None,
        }
    }
}

/// An MCP resource exposed by a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Resource URI.
    pub uri: String,
    /// Human-readable resource name.
    pub name: String,
    /// Resource description.
    #[serde(default)]
    pub description: String,
    /// Optional MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// An MCP prompt exposed by a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Prompt name.
    pub name: String,
    /// Prompt description.
    #[serde(default)]
    pub description: String,
    /// Optional prompt arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<McpPromptArgument>>,
}

/// A single argument accepted by an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    /// Argument name.
    pub name: String,
    /// Argument description.
    pub description: String,
    /// Whether the argument is required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// Result of a `resources/read` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    /// Resource contents returned by the server.
    pub contents: Vec<ResourceContents>,
}

/// Contents of one resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContents {
    /// Resource URI.
    pub uri: String,
    /// Optional MIME type.
    pub mime_type: Option<String>,
    /// Optional text body.
    pub text: Option<String>,
}
