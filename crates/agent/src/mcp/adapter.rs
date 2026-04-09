use async_trait::async_trait;
use std::sync::Arc;

use super::client::McpClient;
use crate::error::ToolError;
use crate::tools::{Tool, ToolDescription};

pub struct McpToolAdapter {
    client: Arc<McpClient>,
    registry_name: String,
    mcp_tool_name: String,
    description: String,
    input_schema: serde_json::Value,
}

impl McpToolAdapter {
    pub fn new(
        client: Arc<McpClient>,
        registry_name: String,
        mcp_tool_name: String,
        description: String,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            client,
            registry_name,
            mcp_tool_name,
            description,
            input_schema,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: self.description.clone(),
            parameters: self.input_schema.to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.client
            .call_tool(&self.mcp_tool_name, args.clone())
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}
