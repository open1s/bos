// Patch D: Skeleton integration test for per-tool policy integration points
// This test builds a tiny local Tool and a DenyPolicy to verify policy gate is exercised.

use agent::tools::policy::{BoxedPolicy, PolicyContext, ToolPolicy};
use agent::tools::registry::ToolRegistry;
use agent::tools::{Tool, ToolDescription};
use agent::ToolError;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
// Use crate-local ToolError type via the Tool trait re-export

// Minimal local tool for testing
struct MinimalTool;
#[async_trait]
impl Tool for MinimalTool {
    fn name(&self) -> &str {
        "dummy"
    }
    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "A dummy tool".to_string(),
            parameters: "none".to_string(),
        }
    }
    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        Ok(serde_json::json!("executed"))
    }
}

// Deny policy for testing
struct DenyPolicy;
impl ToolPolicy for DenyPolicy {
    fn is_allowed(&self, _tool_name: &str, _ctx: &PolicyContext) -> bool {
        false
    }
}

#[tokio::test]
async fn patch_d_tool_policy_integration_skeleton() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(MinimalTool)).unwrap();
    let policy: BoxedPolicy = Arc::new(DenyPolicy);
    registry.configure_policy("dummy", policy);

    let args = json!({});
    let res = registry.execute_with_policy("dummy", &args).await;
    assert!(res.is_err());
}
