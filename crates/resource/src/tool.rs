//! LLM-facing tool adapter: wraps a `ResourceClient` + URI so an agent can
//! invoke the resource by sending a JSON `ResourceAction`.

use std::sync::Arc;

use serde_json::Value;

use crate::action::{ResourceAction, ResourceOutput};
use crate::client::ResourceClient;
use crate::error::ResourceError;

/// A tool that exposes a single resource's `invoke` surface to an LLM. The agent
/// calls [`ResourceTool::call`] with a JSON `ResourceAction`; the result is the
/// `ResourceOutput` serialized back to JSON.
pub struct ResourceTool {
    uri: String,
    client: Arc<ResourceClient>,
    name: String,
    description: String,
}

impl ResourceTool {
    /// Bind a tool to `uri` on `client`.
    pub fn new(uri: impl Into<String>, client: Arc<ResourceClient>) -> Self {
        let uri = uri.into();
        let description =
            format!("Invoke actions on resource `{uri}` (send a JSON ResourceAction).");
        Self {
            name: uri.clone(),
            uri,
            client,
            description,
        }
    }

    /// Stable tool name (the resource URI).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Human-readable description for the agent.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The resource URI this tool targets.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Execute the tool: parse `action_json` as a [`ResourceAction`], invoke the
    /// resource, and return the [`ResourceOutput`] as JSON.
    pub async fn call(&self, action_json: Value) -> std::result::Result<Value, String> {
        let action: ResourceAction =
            serde_json::from_value(action_json).map_err(|e| format!("invalid action: {e}"))?;
        let out: ResourceOutput = self
            .client
            .invoke(&self.uri, action)
            .await
            .map_err(|e: ResourceError| e.to_string())?;
        serde_json::to_value(out).map_err(|e| format!("serialize output: {e}"))
    }
}
