//! MCP (Model Context Protocol) Bridge Implementation
//!
//! This module provides STDIO-based MCP server communication with JSON-RPC 2.0 protocol.

pub mod protocol;
pub mod transport;
pub mod client;
pub mod adapter;

#[cfg(test)]
mod tests;

pub use protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError, ServerCapabilities, ToolDefinition,
    McpResource, McpPrompt, McpPromptArgument, ReadResourceResult, ResourceContents};
pub use transport::StdioTransport;
pub use client::McpClient;
pub use adapter::McpToolAdapter;
pub use client::McpError;