use std::sync::Arc;

use react::tool::{Tool, ToolError};

use super::client::McpClient;

pub struct McpToolAdapter {
    client: Arc<McpClient>,
    registry_name: String,
    mcp_tool_name: String,
    description: String,
}

impl McpToolAdapter {
    pub fn new(
        client: Arc<McpClient>,
        registry_name: String,
        mcp_tool_name: String,
        description: String,
        _input_schema: serde_json::Value,
    ) -> Self {
        Self {
            client,
            registry_name,
            mcp_tool_name,
            description,
        }
    }
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            self.client
                .call_tool(&self.mcp_tool_name, args.clone())
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))
        })
    }
}
