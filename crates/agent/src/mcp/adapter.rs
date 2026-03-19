use async_trait::async_trait;
use std::sync::Arc;

use super::client::McpClient;
use crate::error::ToolError;
use crate::tools::{Tool, ToolDescription};

pub struct McpToolAdapter {
    client: Arc<McpClient>,
    tool_name: String,
    description: String,
    input_schema: serde_json::Value,
}

impl McpToolAdapter {
    pub fn new(
        client: Arc<McpClient>,
        name: String,
        description: String,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            client,
            tool_name: name,
            description,
            input_schema,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.tool_name
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

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.client
            .call_tool(&self.tool_name, args)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_tool_adapter_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        });

        let client = Arc::new(McpClient::spawn("echo", &["hello"]).await.unwrap());
        let adapter = McpToolAdapter::new(
            client,
            "test_tool".to_string(),
            "A test tool".to_string(),
            schema.clone(),
        );

        assert_eq!(adapter.name(), "test_tool");
        assert_eq!(adapter.description().short, "A test tool");
        assert_eq!(adapter.json_schema(), schema);
    }
}