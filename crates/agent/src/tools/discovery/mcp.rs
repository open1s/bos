use std::sync::Arc;
use async_trait::async_trait;

use crate::mcp::McpClient;
use crate::mcp::ToolDefinition;

use super::base::{ToolDiscovery, DiscoveredTool, ToolSource};

/// Discovery for tools from MCP (Model Context Protocol) servers.
///
/// Connects to MCP servers and calls `list_tools()` to discover
/// available tools.
pub struct McpDiscovery {
    client: Arc<McpClient>,
    server_name: String,
    server_command: String,
}

impl McpDiscovery {
    /// Create a new MCP discovery from an initialized client.
    pub fn new(
        client: Arc<McpClient>,
        server_name: impl Into<String>,
        server_command: impl Into<String>,
    ) -> Self {
        Self {
            client,
            server_name: server_name.into(),
            server_command: server_command.into(),
        }
    }

    fn convert_tool_def(tool_def: &ToolDefinition, source: &ToolSource) -> DiscoveredTool {
        DiscoveredTool {
            name: tool_def.name.clone(),
            description: tool_def.description.clone(),
            input_schema: tool_def.input_schema.clone(),
            source: source.clone(),
        }
    }
}

#[async_trait]
impl ToolDiscovery for McpDiscovery {
    async fn discover_all(&self) -> Result<Vec<DiscoveredTool>, crate::error::ToolError> {
        let tool_defs = self.client.list_tools().await
            .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?;

        let source = ToolSource::Mcp {
            server_name: self.server_name.clone(),
            server_command: self.server_command.clone(),
        };

        Ok(tool_defs
            .iter()
            .map(|td| Self::convert_tool_def(td, &source))
            .collect())
    }

    fn source_name(&self) -> &'static str {
        "mcp"
    }
}

/// Builder for creating MCP discovery instances.
pub struct McpDiscoveryBuilder {
    command: String,
    args: Vec<String>,
    server_name: Option<String>,
}

impl McpDiscoveryBuilder {
    /// Create a new builder for MCP discovery.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            server_name: None,
        }
    }

    /// Add arguments for the MCP server command.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(|s| s.into()));
        self
    }

    /// Set a custom server name (defaults to command name).
    pub fn server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = Some(name.into());
        self
    }

    /// Spawn the MCP server and initialize discovery.
    pub async fn build(self) -> Result<McpDiscovery, crate::error::ToolError> {
        let server_name = self.server_name.unwrap_or_else(|| {
            self.command.split('/').last().unwrap_or(&self.command).to_string()
        });

        let args_refs: Vec<&str> = self.args.iter().map(|s| s.as_str()).collect();
        
        let client = McpClient::spawn(&self.command, &args_refs).await
            .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?;
        
        client.initialize().await
            .map_err(|e| crate::error::ToolError::ExecutionFailed(e.to_string()))?;

        Ok(McpDiscovery::new(
            Arc::new(client),
            server_name,
            self.command,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_default() {
        let builder = McpDiscoveryBuilder::new("npx");
        assert_eq!(builder.command, "npx");
        assert!(builder.args.is_empty());
        assert!(builder.server_name.is_none());
    }

    #[test]
    fn test_builder_with_args() {
        let builder = McpDiscoveryBuilder::new("npx")
            .arg("-y")
            .arg("@modelcontextprotocol/server-filesystem")
            .server_name("filesystem");
        
        assert_eq!(builder.args, vec!["-y", "@modelcontextprotocol/server-filesystem"]);
        assert_eq!(builder.server_name, Some("filesystem".to_string()));
    }

    #[test]
    fn test_server_name_extraction() {
        let builder = McpDiscoveryBuilder::new("/usr/bin/npx");
        let name = builder.server_name.unwrap_or_else(|| {
            builder.command.split('/').last().unwrap_or(&builder.command).to_string()
        });
        assert_eq!(name, "npx");
    }
}
