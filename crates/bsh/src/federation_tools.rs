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
        [
            "status",
            "peers",
            "capabilities",
            "delegate",
            "cancel",
            "query",
            "cleanup",
        ]
        .into_iter()
        .map(|name| Self {
            name: match name {
                "status" => "federation_status",
                "peers" => "federation_peers",
                "capabilities" => "federation_capabilities",
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

/// Send a delegation and return its record.
///
/// The model-facing tool and the task executor share this, so the request and
/// its contract shape cannot drift between the two callers.
pub(crate) async fn delegate_record(
    target: &str,
    objective: &str,
    capability: Option<&str>,
    input: Value,
) -> Result<federation::DelegationRecord, ToolError> {
    let runtime = runtime()?;
    let node = runtime.node();
    let target = resolve_peer(target, &enrolled_peers()?)?;
    let capability = capability
        .map(str::parse::<CapabilityId>)
        .transpose()
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let contract = capability.as_ref().map(|capability| {
        json!({
            "capability": capability.to_string(),
            "input": input,
        })
        .to_string()
    });
    let request = DelegationRequest {
        id: DelegationId::new(format!("agent-{}", unix_ms())),
        parent: None,
        target: target.clone(),
        objective: objective.to_string(),
        capabilities: capability.into_iter().collect(),
        operations: Vec::new(),
        resources: Vec::new(),
        deadline_ms: Some(unix_ms() + 30_000),
        allow_sub_delegation: false,
        result_contract: contract,
        trace_id: None,
        span_id: None,
    };
    node.delegate(&target, request, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))
}

/// Ask a peer to own a task.
pub(crate) async fn create_peer_task(
    target: &str,
    request: federation::TaskCreateRequest,
) -> Result<federation::FederatedTask, ToolError> {
    let runtime = runtime()?;
    let node = runtime.node();
    let target = resolve_peer(target, &enrolled_peers()?)?;
    node.create_task(&target, request, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))
}

/// Read a peer-owned task.
pub(crate) async fn peer_task_status(
    target: &str,
    id: &str,
) -> Result<federation::FederatedTask, ToolError> {
    let runtime = runtime()?;
    let node = runtime.node();
    let target = resolve_peer(target, &enrolled_peers()?)?;
    node.task_status(&target, id, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))
}

/// Cancel a peer-owned task.
pub(crate) async fn cancel_peer_task(
    target: &str,
    id: &str,
) -> Result<federation::FederatedTask, ToolError> {
    let runtime = runtime()?;
    let node = runtime.node();
    let target = resolve_peer(target, &enrolled_peers()?)?;
    node.cancel_task(&target, id, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))
}

/// Cancel a delegation on its peer. The task executor uses this; the tool has
/// its own arm because it returns the record.
pub(crate) async fn cancel_delegation(target: &str, id: &str) -> Result<(), ToolError> {
    let runtime = runtime()?;
    let node = runtime.node();
    let target = resolve_peer(target, &enrolled_peers()?)?;
    let delegation = DelegationId::new(id.to_string());
    node.cancel_delegation(&target, &delegation, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))?;
    Ok(())
}

/// The delegation's snapshot for the task seam, when this node knows it.
pub(crate) fn delegation_snapshot(id: &str) -> Option<crate::extension::runs::RunSnapshot> {
    use crate::extension::runs::{RunSnapshot, RunState};
    use federation::DelegationState;
    let runtime = crate::federation_node::running()?;
    let node = runtime.node();
    let delegations = node.delegations();
    let store = delegations.read().ok()?;
    let record = store.record(&DelegationId::new(id.to_string()))?;
    let state = match record.state {
        DelegationState::Created | DelegationState::Accepted | DelegationState::Running => {
            RunState::Running
        }
        DelegationState::Succeeded => RunState::Succeeded,
        DelegationState::Failed | DelegationState::Rejected => RunState::Failed,
        DelegationState::Cancelled => RunState::Cancelled,
    };
    let result = record.result.as_ref().map(|value| match value {
        Value::String(text) => text.clone(),
        other => other
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| other.to_string()),
    });
    Some(RunSnapshot {
        id: id.to_string(),
        state,
        result,
        tokens: 0,
        steps: 0,
        error: record.error.clone(),
    })
}

/// Resolve a model-supplied target by id, name, unique id prefix, or "auto".
///
/// Models often know a peer by its announced name, not its base64 id. The tool
/// resolves the same spelling a reader sees in the pane; an unknown target
/// returns the enrolled names instead of an opaque "not enrolled".
fn resolve_peer(raw: &str, peers: &[(NodeId, Option<String>)]) -> Result<NodeId, ToolError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("auto") {
        return match peers {
            [only] => Ok(only.0.clone()),
            [] => Err(ToolError::InvalidInput(
                "no enrolled peer: ask the reader to trust a peer first".to_string(),
            )),
            many => Err(ToolError::InvalidInput(format!(
                "target is required when {} peers are enrolled; choose one of: {}",
                many.len(),
                peer_labels(peers)
            ))),
        };
    }
    if let Some((node, _)) = peers.iter().find(|(node, _)| node.as_str() == raw) {
        return Ok(node.clone());
    }
    if let Some((node, _)) = peers.iter().find(|(_, name)| {
        name.as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(raw))
    }) {
        return Ok(node.clone());
    }
    let matches: Vec<&(NodeId, Option<String>)> = peers
        .iter()
        .filter(|(node, _)| raw.len() >= 8 && node.as_str().starts_with(raw))
        .collect();
    match matches.as_slice() {
        [only] => Ok(only.0.clone()),
        [] => Err(ToolError::InvalidInput(format!(
            "unknown peer {raw:?}; enrolled peers: {}",
            peer_labels(peers)
        ))),
        _ => Err(ToolError::InvalidInput(format!(
            "peer prefix {raw:?} is ambiguous; enrolled peers: {}",
            peer_labels(peers)
        ))),
    }
}

fn peer_labels(peers: &[(NodeId, Option<String>)]) -> String {
    if peers.is_empty() {
        return "none".to_string();
    }
    peers
        .iter()
        .map(|(node, name)| {
            let id = node.as_str();
            let short = &id[..id.len().min(10)];
            match name {
                Some(name) => format!("{name} ({short})"),
                None => short.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn enrolled_peers() -> Result<Vec<(NodeId, Option<String>)>, ToolError> {
    let runtime = runtime()?;
    let peers = runtime.node().peers();
    let store = peers
        .read()
        .map_err(|_| ToolError::Failed("the peer store is poisoned".to_string()))?;
    let own = runtime.node_id();
    Ok(store
        .iter()
        .filter(|peer| matches!(peer.state, federation::TrustState::Trusted))
        .filter(|peer| &peer.node != own)
        .map(|peer| (peer.node.clone(), peer.note.clone()))
        .collect())
}

fn target(input: &Value) -> Result<NodeId, ToolError> {
    let raw = input.get("target").and_then(Value::as_str).unwrap_or("");
    let peers = enrolled_peers()?;
    if raw.trim().is_empty() || raw.eq_ignore_ascii_case("auto") {
        let runtime = runtime()?;
        let live: std::collections::BTreeSet<NodeId> = runtime
            .announced_peers()
            .into_iter()
            .map(|row| row.node)
            .collect();
        let reachable: Vec<(NodeId, Option<String>)> = peers
            .iter()
            .filter(|(node, _)| live.contains(node))
            .cloned()
            .collect();
        if !reachable.is_empty() {
            return resolve_peer("auto", &reachable);
        }
    }
    resolve_peer(raw, &peers)
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
            "federation_delegate" => "Run work on another node. Use this whenever the user asks a peer/node/other machine to do something. target may be omitted when exactly one trusted peer exists; the tool waits for the terminal result.".to_string(),
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
    /// the reported tool runs directly, so a turn that needs a peer pays for
    /// the discovery step and every other turn pays nothing.
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
                "required": ["objective"],
                "properties": {
                    "target": {"type": "string", "description": "peer id or name; omit to use the only trusted peer"},
                    "objective": {"type": "string"},
                    "capability": {"type": "string"},
                    "input": {},
                    "wait_ms": {"type": "integer"}
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
                let live: std::collections::BTreeSet<NodeId> = runtime
                    .announced_peers()
                    .into_iter()
                    .map(|row| row.node)
                    .collect();
                let peers = node
                    .peers()
                    .read()
                    .map(|store| {
                        store
                            .iter()
                            .filter(|peer| matches!(peer.state, federation::TrustState::Trusted))
                            .filter(|peer| peer.node != runtime.node_id().clone())
                            .map(|peer| {
                                json!({
                                    "node": peer.node.to_string(),
                                    "name": peer.note,
                                    "state": "trusted",
                                    "online": live.contains(&peer.node),
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
                let capability = input.get("capability").and_then(Value::as_str);
                let payload = input.get("input").cloned().unwrap_or(Value::Null);
                let record =
                    delegate_record(&target.to_string(), &objective, capability, payload).await?;
                let target = record.request.target.clone();
                let id = record.request.id.clone();
                let wait_ms = input
                    .get("wait_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(20_000)
                    .min(60_000);
                let mut record = record;
                let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms);
                while !record.state.is_terminal() && std::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    match node.delegation_status(&target, &id, &context()).await {
                        Ok(next) => record = next,
                        Err(error) => {
                            return Err(ToolError::Failed(format!(
                                "delegation {} submitted, but status failed: {error}",
                                id.as_str()
                            )))
                        }
                    }
                }
                Ok(json!({
                    "target": target.to_string(),
                    "delegation": id.as_str(),
                    "state": format!("{:?}", record.state).to_lowercase(),
                    "terminal": record.state.is_terminal(),
                    "result": record.result,
                    "error": record.error,
                }))
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
    use federation::NodeIdentity;

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
                "federation_capabilities",
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
        assert_eq!(schema["required"][0], "objective");
        assert!(schema["properties"]["target"].is_object());
    }

    #[test]
    fn a_peer_name_resolves_to_its_node_id() {
        let node = NodeIdentity::from_seed([7u8; 32]).node_id();
        let peers = vec![(node.clone(), Some("provider".to_string()))];
        assert_eq!(resolve_peer("provider", &peers).expect("name"), node);
        assert_eq!(resolve_peer("PROVIDER", &peers).expect("case"), node);
        assert_eq!(resolve_peer("auto", &peers).expect("auto"), node);
        assert_eq!(
            resolve_peer(&node.as_str()[..10], &peers).expect("prefix"),
            node
        );
        let error = resolve_peer("node-b", &peers).expect_err("unknown");
        assert!(error.to_string().contains("provider"), "{error}");
    }
}
