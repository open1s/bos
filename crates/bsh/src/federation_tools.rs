//! Model-facing tools for the federation node.
//!
//! These are deliberately coarse: the model lists peers, delegates one task,
//! queries knowledge, cancels a delegation, or cleans artifacts. It never types
//! peer ids or capability ids from memory; it reads them from federation_peers.

use std::time::{Duration, Instant};

use agent::capability::{
    unix_ms, CapabilityId, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId,
};
use agent::tools::{Tool, ToolError};
use federation::{
    ArtifactCleanupRequest, ArtifactRef, DelegationId, DelegationRequest, KnowledgePurpose,
    KnowledgeQuery,
};
use serde_json::{json, Value};

/// One federation tool, selected by name.
pub(crate) struct FederationTool {
    name: &'static str,
}

impl FederationTool {
    /// The full bundle, in the order the model sees it.
    pub(crate) fn bundle() -> Vec<Self> {
        ["status", "peers", "delegate", "cancel", "query", "cleanup"]
            .into_iter()
            .map(|name| Self {
                name: match name {
                    "status" => "federation_status",
                    "peers" => "federation_peers",
                    "delegate" => "federation_delegate",
                    "cancel" => "federation_cancel",
                    "query" => "federation_query",
                    "cleanup" => "federation_cleanup",
                    _ => unreachable!("a fixed tool list"),
                },
            })
            .collect()
    }
}

fn runtime() -> Result<std::sync::Arc<crate::federation_node::FederationRuntime>, ToolError> {
    crate::federation_node::running()
        .ok_or_else(|| ToolError::Failed("the federation node is not running".to_string()))
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new(format!("agent-fed-{}", unix_ms())),
        Principal::Local {
            session: "agent".to_string(),
            preset: "federation".to_string(),
        },
        WorkspaceId::new("ws:federation").expect("a workspace"),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(30));
    context
}

fn target(input: &Value) -> Result<NodeId, ToolError> {
    let raw = input
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidInput("target is required".to_string()))?;
    NodeId::new(raw).map_err(|error| ToolError::InvalidInput(error.to_string()))
}

fn string(input: &Value, key: &str) -> Result<String, ToolError> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidInput(format!("{key} is required")))
}

impl Tool for FederationTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> String {
        match self.name {
            "federation_status" => "Federation status: node identity, enrolled peers and outstanding delegations. The rest of the family (peers, delegate, query, cancel, cleanup) is reachable through find_tool.".to_string(),
            "federation_peers" => "List enrolled and discovered peers.".to_string(),
            "federation_delegate" => "Delegate a task to a peer.".to_string(),
            "federation_cancel" => "Cancel a delegation.".to_string(),
            "federation_query" => "Ask a peer for knowledge.".to_string(),
            "federation_cleanup" => "Delete artifacts on a peer.".to_string(),
            _ => String::new(),
        }
    }

    fn category(&self) -> String {
        "federation".to_string()
    }

    /// The family is callable without being advertised, except its signpost.
    ///
    /// Six schemas ride every request for a surface most turns never touch, and
    /// the tool array has a wire budget (see the budget test in `caps`). The
    /// browser made the same call: `find_tool` names the withheld tools and
    /// `call_tool` runs one, so a turn that needs a peer pays for the discovery
    /// step and every other turn pays nothing.
    ///
    /// `federation_status` stays in the array on purpose: it is the one cheap
    /// schema that tells the model the family EXISTS, so a prompt about peers
    /// leads to `find_tool` instead of a claim that no such capability is
    /// installed.
    fn deferred(&self) -> bool {
        self.name != "federation_status"
    }

    fn json_schema(&self) -> Value {
        match self.name {
            "federation_status" | "federation_peers" => json!({
                "type": "object",
                "properties": {}
            }),
            "federation_delegate" => json!({
                "type": "object",
                "required": ["target", "objective"],
                "properties": {
                    "target": {"type": "string"},
                    "objective": {"type": "string"},
                    "capability": {"type": "string"},
                    "input": {}
                }
            }),
            "federation_cancel" => json!({
                "type": "object",
                "required": ["target", "delegation"],
                "properties": {
                    "target": {"type": "string"},
                    "delegation": {"type": "string"}
                }
            }),
            "federation_query" => json!({
                "type": "object",
                "required": ["target", "query"],
                "properties": {
                    "target": {"type": "string"},
                    "query": {"type": "string"},
                    "max_items": {"type": "integer"}
                }
            }),
            "federation_cleanup" => json!({
                "type": "object",
                "required": ["target", "uris"],
                "properties": {
                    "target": {"type": "string"},
                    "uris": {"type": "array", "items": {"type": "string"}}
                }
            }),
            _ => json!({}),
        }
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let name = self.name;
        let input = input.clone();
        crate::extension::block_on_tokio(async move { run_tool(name, &input).await })
    }
}

impl FederationTool {
    async fn execute(&self, input: &Value) -> Result<Value, ToolError> {
        let runtime = runtime()?;
        let node = runtime.node();
        match self.name {
            "federation_status" => Ok(json!({
                "running": true,
                "node": runtime.node_id().to_string(),
                "name": runtime.node_name(),
                "peers": node.peers().read().map(|store| store.iter().count()).unwrap_or(0),
                "delegations": node.delegations().read().map(|store| store.iter().count()).unwrap_or(0),
                "leases": node.leases().read().map(|store| store.iter().count()).unwrap_or(0),
                "federation": runtime.federation().map(ToString::to_string),
            })),
            "federation_peers" => {
                let peers = node
                    .peers()
                    .read()
                    .map(|store| {
                        store
                            .iter()
                            .map(|peer| {
                                json!({
                                    "node": peer.node.to_string(),
                                    "name": peer.note,
                                    "state": format!("{:?}", peer.state).to_lowercase(),
                                    "grants": peer.grants.iter().map(|grant| grant.capability.to_string()).collect::<Vec<_>>(),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(json!({ "peers": peers }))
            }
            "federation_delegate" => {
                let target = target(input)?;
                let objective = string(input, "objective")?;
                let capability = input
                    .get("capability")
                    .and_then(Value::as_str)
                    .map(str::parse::<CapabilityId>)
                    .transpose()
                    .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
                let contract = capability.as_ref().map(|capability| {
                    json!({
                        "capability": capability.to_string(),
                        "input": input.get("input").cloned().unwrap_or(Value::Null),
                    })
                    .to_string()
                });
                let request = DelegationRequest {
                    id: DelegationId::new(format!("agent-{}", unix_ms())),
                    parent: None,
                    target: target.clone(),
                    objective,
                    capabilities: capability.into_iter().collect(),
                    deadline_ms: Some(unix_ms() + 30_000),
                    allow_sub_delegation: false,
                    result_contract: contract,
                };
                let record = node
                    .delegate(&target, request, &context())
                    .await
                    .map_err(|error| ToolError::Failed(error.to_string()))?;
                serde_json::to_value(record).map_err(|error| ToolError::Failed(error.to_string()))
            }
            "federation_cancel" => {
                let target = target(input)?;
                let delegation = DelegationId::new(string(input, "delegation")?);
                let record = node
                    .cancel_delegation(&target, &delegation, &context())
                    .await
                    .map_err(|error| ToolError::Failed(error.to_string()))?;
                serde_json::to_value(record).map_err(|error| ToolError::Failed(error.to_string()))
            }
            "federation_query" => {
                let target = target(input)?;
                let mut query =
                    KnowledgeQuery::new(format!("agent-{}", unix_ms()), KnowledgePurpose::Answer);
                query.terms = string(input, "query")?
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
                query.max_items = input
                    .get("max_items")
                    .and_then(Value::as_u64)
                    .unwrap_or(8)
                    .clamp(1, 32) as usize;
                let response = node
                    .query_knowledge(&target, &query, &context())
                    .await
                    .map_err(|error| ToolError::Failed(error.to_string()))?;
                serde_json::to_value(response).map_err(|error| ToolError::Failed(error.to_string()))
            }
            "federation_cleanup" => {
                let target = target(input)?;
                let uris = input
                    .get("uris")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ToolError::InvalidInput("uris must be an array".to_string()))?;
                let artifacts = uris
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|uri| ArtifactRef::new(uri, uri))
                    .collect::<Vec<_>>();
                let request =
                    ArtifactCleanupRequest::new(format!("agent-{}", unix_ms()), artifacts);
                let response = node
                    .cleanup_artifacts(&target, &request, &context())
                    .await
                    .map_err(|error| ToolError::Failed(error.to_string()))?;
                serde_json::to_value(response).map_err(|error| ToolError::Failed(error.to_string()))
            }
            _ => Err(ToolError::Failed("unknown federation tool".to_string())),
        }
    }
}

async fn run_tool(name: &'static str, input: &Value) -> Result<Value, ToolError> {
    let tool = FederationTool { name };
    tool.execute(input).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_federation_bundle_has_stable_model_facing_names() {
        let names: Vec<String> = FederationTool::bundle()
            .iter()
            .map(|tool| tool.name().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "federation_status",
                "federation_peers",
                "federation_delegate",
                "federation_cancel",
                "federation_query",
                "federation_cleanup",
            ]
        );
    }

    #[test]
    fn delegate_names_its_target_and_objective() {
        let tool = FederationTool {
            name: "federation_delegate",
        };
        let schema = tool.json_schema();
        assert_eq!(schema["required"][0], "target");
        assert_eq!(schema["required"][1], "objective");
    }
}
