use std::pin::Pin;
use std::sync::Arc;

use bus::Session;
use futures::Stream;

use react::tool::Tool;
use react::ToolError;
use super::client::AgentRpcClient;

#[cfg(test)]
use super::transport::RpcTransport;

/// A local Tool that invokes another agent over the bus RPC path.
pub struct AgentCallerTool {
    tool_name: String,
    client: AgentRpcClient,
}

impl AgentCallerTool {
    pub fn new(
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        session: Arc<Session>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            client: AgentRpcClient::new(endpoint, session),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_transport(
        tool_name: impl Into<String>,
        endpoint: impl Into<String>,
        transport: Arc<dyn RpcTransport>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            client: AgentRpcClient::with_transport(endpoint, transport),
        }
    }

    /// List remote tools from the callee agent endpoint.
    pub async fn list(&self) -> Result<serde_json::Value, ToolError> {
        self.client.list().await
    }

    /// Call a specific remote tool on the callee agent endpoint.
    pub async fn call(
        &self,
        tool_name: impl Into<String>,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.client.call(tool_name, args).await
    }

    /// Run the remote agent using explicit llm/run method.
    pub async fn llm_run(&self, task: impl Into<String>) -> Result<serde_json::Value, ToolError> {
        self.client.llm_run(task).await
    }

    /// Run the remote agent using stream/run and return aggregated result payload.
    pub async fn stream_run(
        &self,
        task: impl Into<String>,
    ) -> Result<serde_json::Value, ToolError> {
        self.client.stream_run(task).await
    }

    /// Stream the remote agent response token-by-token using stream/run.
    pub async fn stream_run_live(
        &self,
        task: impl Into<String>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<crate::StreamToken, ToolError>> + Send>>, ToolError>
    {
        self.client.stream_run_live(task).await
    }
}

impl Tool for AgentCallerTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> String {
        format!(
            "Call remote agent via bus endpoint '{}' using method=llm/run/stream/run/tool/list/tool/call",
            self.client.endpoint()
        )
    }

    fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                if args.get("tool_name").is_some() {
                    Some("tool/call".to_string())
                } else if args.get("task").is_some() {
                    Some("llm/run".to_string())
                } else {
                    Some("tool/list".to_string())
                }
            })
            .unwrap_or_else(|| "tool/list".to_string());

        let rt = tokio::runtime::Handle::current();
        match method.as_str() {
            "llm/run" | "llm_run" => {
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::Failed("missing 'task'".to_string()))?;
                rt.block_on(self.llm_run(task.to_string()))
            }
            "stream/run" | "stream_run" => {
                let task = args
                    .get("task")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::Failed("missing 'task'".to_string()))?;
                rt.block_on(self.stream_run(task.to_string()))
            }
            "tool/list" => rt.block_on(self.list()),
            "tool/call" => {
                let tool_name = args
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::Failed("missing 'tool_name'".to_string()))?;
                let call_args = args
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                rt.block_on(self.call(tool_name.to_string(), call_args))
            }
            _ => Err(ToolError::Failed(
                "method must be one of: llm/run, stream/run, tool/list, tool/call".to_string(),
            )),
        }
    }
}
