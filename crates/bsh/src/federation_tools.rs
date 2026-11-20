//! Model-facing tools for the federation node.
//!
//! These are deliberately coarse: the model lists peers, delegates one task,
//! queries knowledge, cancels a delegation, or cleans artifacts. It never types
//! peer ids or capability ids from memory; it reads them from federation_peers.

use std::time::{Duration, Instant};

use agent::capability::{
    CapabilityId, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId, unix_ms,
};
use agent::tools::{Tool, ToolError};
use federation::{
    ArtifactCleanupRequest, ArtifactRef, DelegationId, DelegationRequest, KnowledgePurpose,
    KnowledgeQuery,
};
use serde_json::{Value, json};

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
            "await",
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
                "await" => "federation_await",
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
        InvocationId::new(agent_request_id("agent-fed")),
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
    delegate_record_until(target, objective, capability, input, unix_ms() + 30_000).await
}

/// The same submission with an explicit execution bound.
///
/// The tool's own wait sets this bound: a delegation whose deadline expires
/// while the caller is still waiting would settle as failed for a reason the
/// caller never chose.
pub(crate) async fn delegate_record_until(
    target: &str,
    objective: &str,
    capability: Option<&str>,
    input: Value,
    deadline_ms: u64,
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
        // A millisecond stamp is not unique: two parallel tool calls in the
        // same millisecond collided on one id and the peer refused the second
        // as "already exists".
        id: DelegationId::new(agent_request_id("agent")),
        parent: None,
        target: target.clone(),
        objective: objective.to_string(),
        capabilities: capability.into_iter().collect(),
        operations: Vec::new(),
        resources: Vec::new(),
        deadline_ms: Some(deadline_ms),
        allow_sub_delegation: false,
        result_contract: contract,
        trace_id: None,
        span_id: None,
    };
    node.delegate(&target, request, &context())
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))
}

/// A request id no two calls can share, whatever the clock says.
fn agent_request_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
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

/// How long a tool call waits, clamped to the bound it may honour.
fn wait_ms(input: &Value, default_ms: u64, cap_ms: u64) -> u64 {
    input
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(default_ms)
        .min(cap_ms)
}

/// Wait for one delegation to settle, asking less often as the wait grows.
///
/// A fixed 200 ms poll spent about a hundred round trips on one 20 s wait. The
/// backoff starts fast enough to catch a quick capability invocation and grows
/// to a two-second steady cadence, so a long wait costs a handful of RPCs. It
/// never polls past the requested wait, and a status failure names the id and
/// target so the caller can resume with federation_await instead of
/// resubmitting the work.
async fn await_delegation(
    node: &federation::FederationNode,
    mut record: federation::DelegationRecord,
    wait_ms: u64,
) -> Result<federation::DelegationRecord, ToolError> {
    let target = record.request.target.clone();
    let id = record.request.id.clone();
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let mut interval = Duration::from_millis(50);
    while !record.state.is_terminal() {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let nap = interval.min(deadline.saturating_duration_since(now));
        tokio::time::sleep(nap).await;
        match node.delegation_status(&target, &id, &context()).await {
            Ok(next) => record = next,
            Err(error) => {
                return Err(ToolError::Failed(format!(
                    "delegation {} on {target} is still open, but its status could not be read: {error}; call federation_await with this target and delegation id to keep waiting",
                    id.as_str()
                )));
            }
        }
        interval = interval.mul_f32(1.5).min(Duration::from_secs(2));
    }
    Ok(record)
}

/// Whether the request names an executable capability.
fn is_executable(record: &federation::DelegationRecord) -> bool {
    record.request.result_contract.is_some() || !record.request.capabilities.is_empty()
}

/// The tool-facing shape of one delegation.
///
/// The note is the model's next move: keep waiting by id, or learn that a
/// prose objective is queued rather than run. Terminal stays false for the
/// prose case, so the tool never claims a settled answer it does not have.
fn delegation_result(record: &federation::DelegationRecord) -> Value {
    let executable = is_executable(record);
    let terminal = record.state.is_terminal();
    let note = if terminal {
        String::new()
    } else if !executable {
        "recorded on the peer but NOT executed: a delegation runs one capability, so pass capability and input to run work; a prose objective is queued, not run".to_string()
    } else {
        "still running: call federation_await with this target and delegation id to keep waiting; do not submit it again".to_string()
    };
    json!({
        "target": record.request.target.to_string(),
        "delegation": record.request.id.as_str(),
        "state": format!("{:?}", record.state).to_lowercase(),
        "terminal": terminal,
        "executable": executable,
        "result": record.result,
        "error": record.error,
        "note": note,
    })
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
            "federation_delegate" => "Run work on another node by naming a capability and its input. Use this whenever the user asks a peer/node/other machine to do something executable. target may be omitted when exactly one trusted peer exists. A prose objective is only queued, not run, on a peer with no language-model executor; when the answer is not ready within wait_ms, call federation_await with the returned id instead of submitting again.".to_string(),
            "federation_await" => "Keep waiting for a delegation you already submitted, by id. It reads the owner's record and never resubmits the work; use it when federation_delegate returned terminal:false.".to_string(),
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
            "federation_await" => json!({
                "type": "object",
                "required": ["target", "delegation"],
                "properties": {
                    "target": {"type": "string", "description": "peer id or name the delegation was sent to"},
                    "delegation": {"type": "string", "description": "the id federation_delegate returned"},
                    "wait_ms": {"type": "integer", "description": "how long to wait for a terminal state; the delegation's own deadline still wins"}
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
                let wait = wait_ms(input, 20_000, 60_000);
                // The delegation's own bound must outlast the wait it is
                // asked for, or the peer expires work the caller is still
                // waiting on.
                let deadline = unix_ms() + wait.saturating_add(5_000).max(30_000);
                let record = delegate_record_until(
                    &target.to_string(),
                    &objective,
                    capability,
                    payload,
                    deadline,
                )
                .await?;
                // A prose objective is recorded and left accepted: bsh has no
                // language-model executor on the peer. Waiting on it would
                // burn the whole request for a state that cannot move, so the
                // note says so instead.
                let record = if is_executable(&record) {
                    await_delegation(node.as_ref(), record, wait).await?
                } else {
                    record
                };
                Ok(delegation_result(&record))
            }
            "federation_await" => {
                let target = target(input)?;
                let id = DelegationId::new(string(input, "delegation")?);
                let wait = wait_ms(input, 20_000, 300_000);
                let record = node
                    .delegation_status(&target, &id, &context())
                    .await
                    .map_err(|error| {
                        ToolError::Failed(format!(
                            "delegation {} on {target}: {error}",
                            id.as_str()
                        ))
                    })?;
                // A prose record cannot move; waiting on it would burn the
                // request for a state nobody can settle.
                let record = if is_executable(&record) {
                    await_delegation(node.as_ref(), record, wait).await?
                } else {
                    record
                };
                Ok(delegation_result(&record))
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
                    KnowledgeQuery::new(agent_request_id("agent"), KnowledgePurpose::Answer);
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
                let request = ArtifactCleanupRequest::new(agent_request_id("agent"), artifacts);
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
                "federation_await",
                "federation_cancel",
                "federation_query",
                "federation_cleanup",
            ]
        );
    }

    #[test]
    fn a_request_id_is_unique_within_a_millisecond() {
        let ids: std::collections::BTreeSet<String> =
            (0..1_000).map(|_| agent_request_id("agent")).collect();
        assert_eq!(ids.len(), 1_000, "no two calls share a request id");
    }

    #[test]
    fn wait_is_clamped_to_the_bound_a_call_may_honour() {
        assert_eq!(wait_ms(&json!({}), 20_000, 60_000), 20_000);
        assert_eq!(wait_ms(&json!({"wait_ms": 5}), 20_000, 60_000), 5);
        assert_eq!(
            wait_ms(&json!({"wait_ms": 999_999}), 20_000, 60_000),
            60_000
        );
    }

    fn a_delegation(
        state: federation::DelegationState,
        executable: bool,
    ) -> federation::DelegationRecord {
        let node = NodeIdentity::from_seed([9u8; 32]).node_id();
        federation::DelegationRecord {
            origin: None,
            request: federation::DelegationRequest {
                id: DelegationId::new("agent-test"),
                parent: None,
                target: node,
                objective: "look around".to_string(),
                capabilities: if executable {
                    vec!["builtin/read_file".parse().expect("capability")]
                } else {
                    Vec::new()
                },
                operations: Vec::new(),
                resources: Vec::new(),
                deadline_ms: None,
                allow_sub_delegation: false,
                result_contract: executable.then(|| "{}".to_string()),
                trace_id: None,
                span_id: None,
            },
            state,
            updated_at_ms: 1,
            result: None,
            error: None,
        }
    }

    #[test]
    fn a_prose_delegation_is_reported_as_queued_not_run() {
        let payload =
            delegation_result(&a_delegation(federation::DelegationState::Accepted, false));
        assert_eq!(payload["terminal"], false);
        assert_eq!(payload["executable"], false);
        assert!(
            payload["note"]
                .as_str()
                .expect("note")
                .contains("NOT executed"),
            "{payload}"
        );
    }

    #[test]
    fn a_running_delegation_points_at_await_instead_of_resubmitting() {
        let payload = delegation_result(&a_delegation(federation::DelegationState::Running, true));
        assert_eq!(payload["terminal"], false);
        assert!(
            payload["note"]
                .as_str()
                .expect("note")
                .contains("federation_await"),
            "{payload}"
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
    fn await_names_its_target_and_delegation() {
        let tool = FederationTool {
            name: "federation_await",
        };
        let schema = tool.json_schema();
        assert_eq!(schema["required"][0], "target");
        assert_eq!(schema["required"][1], "delegation");
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
