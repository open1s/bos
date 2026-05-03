use std::sync::Arc;

use async_trait::async_trait;
use react::tool::registry::AsyncTool;
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
        let client = self.client.clone();
        let tool_name = self.mcp_tool_name.clone();
        let args_clone = args.clone();

        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("Failed to create runtime");
            let result = rt.block_on(client.call_tool(&tool_name, args_clone));
            let _ = tx.send(result);
        });

        rx.recv().map_err(|e| ToolError::Failed(format!("Channel error: {}", e)))?
            .map_err(|e| ToolError::Failed(e.to_string()))
    }
}

// AsyncTool implementation - proper async execution
#[async_trait]
impl AsyncTool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    async fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.client
            .call_tool(&self.mcp_tool_name, args.clone())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))
    }
}
