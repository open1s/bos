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

        rx.recv()
            .map_err(|e| ToolError::Failed(format!("Channel error: {}", e)))?
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

    fn json_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn to_openai_definition(&self) -> react::tool::ToolDefinition {
        use react::tool::{ToolDefinition, ToolParameterProperty, ToolParameters};
        use std::collections::HashMap;

        let parameters = if let Ok(params) =
            serde_json::from_value::<ToolParameters>(self.input_schema.clone())
        {
            params
        } else {
            let mut tool_params = ToolParameters {
                param_type: "object".to_string(),
                properties: Some(HashMap::new()),
                required: None,
            };

            if let Some(obj) = self.input_schema.as_object() {
                if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                    tool_params.param_type = t.to_string();
                }
                if let Some(arr) = obj.get("required").and_then(|v| v.as_array()) {
                    tool_params.required = Some(
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                    );
                }
                if let Some(props) = obj.get("properties").and_then(|v| v.as_object()) {
                    let mut properties = HashMap::new();
                    for (key, val) in props {
                        let param_prop = ToolParameterProperty {
                            param_type: val
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("string")
                                .to_string(),
                            description: val
                                .get("description")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            enum_values: None,
                        };
                        properties.insert(key.clone(), param_prop);
                    }
                    tool_params.properties = Some(properties);
                }
            }
            tool_params
        };

        ToolDefinition::new(&self.registry_name, &self.description).with_parameters(parameters)
    }

    async fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.client
            .call_tool(&self.mcp_tool_name, args.clone())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))
    }
}
