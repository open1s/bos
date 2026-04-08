//! MCP (Model Context Protocol) Bridge Implementation
//!
//! This module provides STDIO and HTTP-based MCP server communication with JSON-RPC 2.0 protocol.

pub mod adapter;
pub mod client;
pub mod http_transport;
pub mod protocol;
pub mod transport;

pub use adapter::McpToolAdapter;
pub use client::McpClient;
pub use client::McpError;
pub use http_transport::HttpTransport;
pub use protocol::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpPrompt, McpPromptArgument, McpResource,
    ReadResourceResult, ResourceContents, ServerCapabilities, ToolDefinition,
};
pub use transport::StdioTransport;
