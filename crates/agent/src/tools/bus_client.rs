use std::sync::Arc;

use async_trait::async_trait;
use rkyv::{Archive, Deserialize, Serialize};
use zenoh::Session as ZenohSession;

use super::{Tool, ToolDescription, ToolError};
use crate::error::AgentError;
use bus::{Codec, RpcClient};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: String,
}

pub struct BusToolClient {
    session: Arc<ZenohSession>,
    service_name: String,
    tool_name: String,
}

impl BusToolClient {
    pub fn new(session: Arc<ZenohSession>, service_name: String, tool_name: String) -> Self {
        Self {
            session,
            service_name,
            tool_name,
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

#[async_trait]
impl Tool for BusToolClient {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!("Remote tool on bus: {}", self.service_name),
            parameters: "Remote tool - schema unknown".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let json_args = args.to_string();

        let mut client = RpcClient::new(&self.service_name, &self.tool_name);
        client
            .init(self.session.clone())
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let codec = Codec;
        let payload = codec.encode(&JsonPayload { json: json_args })
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let result: JsonPayload = client
            .call(&payload)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        serde_json::from_str(&result.json)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}

pub async fn register_service_tools(
    registry: &mut super::ToolRegistry,
    session: Arc<ZenohSession>,
    service_name: &str,
    tool_names: &[&str],
) -> Result<(), AgentError> {
    for tool_name in tool_names {
        let client = BusToolClient::new(
            session.clone(),
            service_name.to_string(),
            tool_name.to_string(),
        );
        registry
            .register(std::sync::Arc::new(client))
            .map_err(AgentError::Tool)?;
    }
    Ok(())
}
