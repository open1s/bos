//! MCP (Model Context Protocol) Bridge Implementation
//!
//! This module provides STDIO and HTTP-based MCP server communication with JSON-RPC 2.0 protocol.

/// Exposes MCP tools as local async tools.
pub mod adapter;
/// JSON-RPC client for an MCP server over stdio or HTTP.
pub mod client;
/// HTTP (streamable) transport for MCP.
pub mod http_transport;
/// JSON-RPC 2.0 and MCP message types.
pub mod protocol;
/// Stdio transport for MCP servers launched as child processes.
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
