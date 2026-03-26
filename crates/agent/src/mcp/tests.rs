//! Integration tests for MCP (Model Context Protocol) client

use super::{
    protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError, ServerCapabilities, ToolDefinition},
    McpError,
};

#[tokio::test]
async fn test_json_rpc_request_serialization() {
    let request = JsonRpcRequest::new(
        "test_method",
        Some(serde_json::json!({"param": "value"})),
        1,
    );

    let serialized = serde_json::to_string(&request).unwrap();
    
    assert!(serialized.contains("test_method"));
    assert!(serialized.contains("\"id\":1"));
    assert!(serialized.contains("\"param\""));

    let deserialized: JsonRpcRequest = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.method, "test_method");
    assert_eq!(deserialized.id, serde_json::json!(1));
    assert!(deserialized.params.is_some());
}

#[tokio::test]
async fn test_json_rpc_response_success() {
    let response = JsonRpcResponse::success(
        1,
        serde_json::json!({"result": "success"}),
    );

    let serialized = serde_json::to_string(&response).unwrap();
    
    let deserialized: JsonRpcResponse = serde_json::from_str(&serialized).unwrap();
    assert!(deserialized.result.is_some());
    assert!(deserialized.error.is_none());
}

#[tokio::test]
async fn test_json_rpc_response_error() {
    let error = JsonRpcError::invalid_params("Invalid parameter type".to_string());
    let response = JsonRpcResponse::error(2, error);

    let serialized = serde_json::to_string(&response).unwrap();
    
    let deserialized: JsonRpcResponse = serde_json::from_str(&serialized).unwrap();
    assert!(deserialized.result.is_none());
    assert!(deserialized.error.is_some());
    assert_eq!(deserialized.error.as_ref().unwrap().code, -32602);
}

#[tokio::test]
async fn test_default_jsonrpc_version() {
    let request = JsonRpcRequest::new(
        "test",
        None,
        42,
    );

    assert_eq!(request.jsonrpc, "2.0");
}

#[tokio::test]
async fn test_jsonrpc_error_codes() {
    let parse_err = JsonRpcError::parse_error("Parse error details".to_string());
    assert_eq!(parse_err.code, -32700);
    assert_eq!(parse_err.message, "Parse error details");

    let not_found = JsonRpcError::method_not_found("unknownMethod");
    assert_eq!(not_found.code, -32601);

    let invalid_req = JsonRpcError::invalid_request("Invalid request".to_string());
    assert_eq!(invalid_req.code, -32600);
}

#[tokio::test]
async fn test_server_capabilities_serialization() {
    let caps = ServerCapabilities {
        tools: serde_json::Value::Bool(true),
        resources: serde_json::Value::Bool(false),
        prompts: serde_json::Value::Bool(true),
    };

    let serialized = serde_json::to_string(&caps).unwrap();
    
    let deserialized: ServerCapabilities = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.tools, serde_json::Value::Bool(true));
    assert_eq!(deserialized.resources, serde_json::Value::Bool(false));
    assert_eq!(deserialized.prompts, serde_json::Value::Bool(true));
}

#[tokio::test]
async fn test_tool_definition_serialization() {
    let tool = ToolDefinition {
        name: "search".to_string(),
        description: "Search the web".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["query"]
        }),
    };

    let serialized = serde_json::to_string(&tool).unwrap();
    
    let deserialized: ToolDefinition = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.name, "search");
    assert_eq!(deserialized.description, "Search the web");
    assert!(deserialized.input_schema["type"] == "object");
    assert!(deserialized.input_schema["properties"]["query"]["type"] == "string");
}

#[tokio::test]
async fn test_jsonrpc_request_with_params() {
    let params = serde_json::json!({
        "operation": "create",
        "data": {
            "name": "test",
            "value": 42
        }
    });

    let request = JsonRpcRequest::new("do_operation", Some(params), 3);

    assert_eq!(request.method, "do_operation");
    assert_eq!(request.id, serde_json::json!(3));
    
    let request_json = serde_json::to_string(&request).unwrap();
    assert!(request_json.contains("do_operation"));
    assert!(request_json.contains("create"));
    assert!(request_json.contains("\"name\":\"test\""));
}

#[tokio::test]
async fn test_mcp_error_conversion() {
    let transport_err = crate::mcp::transport::TransportError::Io("IO error".to_string());
    let _: McpError = transport_err.into();

    let _error = JsonRpcError::internal_error("Internal error".to_string());
}

#[tokio::test]
async fn test_tool_definition_complete_schema() {
    let tool = ToolDefinition {
        name: "calculate".to_string(),
        description: "Perform calculations".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"},
                "operator": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"]
                }
            },
            "required": ["a", "b", "operator"]
        }),
    };

    assert_eq!(tool.input_schema["required"].as_array().unwrap().len(), 3);
    assert_eq!(tool.input_schema["properties"]["operator"]["enum"].as_array().unwrap().len(), 4);
}

#[test]
fn test_jsonrpc_response_is_error() {
    let success = JsonRpcResponse::success(1, serde_json::Value::String("result".to_string()));
    assert!(!success.is_error());

    let error = JsonRpcResponse::error(1, JsonRpcError::internal_error("Error".to_string()));
    assert!(error.is_error());
}

#[test]
fn test_tool_definition_nested_schema() {
    let tool = ToolDefinition {
        name: "complex_tool".to_string(),
        description: "Complex tool with nested schema".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "enabled": {"type": "boolean"},
                        "priority": {"type": "number", "minimum": 1, "maximum": 10}
                    }
                },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["config"]
        }),
    };

    assert!(tool.input_schema["properties"]["config"]["type"] == "object");
    assert!(tool.input_schema["properties"]["items"]["type"] == "array");
    assert!(tool.input_schema["properties"]["config"]["properties"]["priority"]["type"] == "number");
}
