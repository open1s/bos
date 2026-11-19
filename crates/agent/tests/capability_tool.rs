//! The engine-facing shim: ReAct calls a capability through the workspace.

use std::sync::Arc;
use std::time::Duration;

use agent::capabilities::{
    CapabilityCorrelation, CapabilityTool, NativeToolProvider, ProxySource, ToolFacts,
    catalogue_from_registry,
};
use agent::tools::{AsyncTool, FunctionTool, Tool, ToolError, ToolRegistry};
use capability::composition::{
    CapabilityRequirement, CompositionSpec, LocalScope, RemotePolicy, resolve_host,
};
use capability::id::WorkspaceId;
use capability::{CapabilityId, InvocationContext, InvocationId, Principal, WorkspaceRuntime};
use semver::{Version, VersionReq};
use serde_json::{Value, json};

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

fn principal() -> Principal {
    Principal::Local {
        session: "session".to_string(),
        preset: "preset".to_string(),
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
        "required": ["a", "b"]
    })
}

struct Slow;

#[async_trait::async_trait]
impl AsyncTool for Slow {
    fn name(&self) -> &str {
        "slow"
    }

    fn description(&self) -> String {
        "sleeps past any deadline".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(json!({ "slept": true }))
    }
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FunctionTool::new(
            "echo_add",
            "Add two numbers",
            schema(),
            |args| {
                let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
                let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
                Ok(json!({ "sum": a + b }))
            },
        )))
        .expect("a unique tool");
    registry
        .register_async(Arc::new(Slow))
        .expect("a unique tool");
    registry
}

fn runtime() -> Arc<WorkspaceRuntime> {
    let registry = registry();
    let catalogue = catalogue_from_registry(&registry).expect("a catalogue");
    let scope = LocalScope::admitting(workspace(), &catalogue);
    let spec = CompositionSpec::new("compose:test", Version::new(1, 0, 0), "shim test")
        .with_requirement(CapabilityRequirement::new(
            "builtin/echo_add".parse().expect("a valid id"),
            VersionReq::parse("^0.1").expect("a range"),
        ))
        .with_requirement(
            CapabilityRequirement::new(
                "builtin/slow".parse().expect("a valid id"),
                VersionReq::parse("^0.1").expect("a range"),
            )
            .with_remote(RemotePolicy::Forbidden),
        );
    let plan = resolve_host(&spec, &catalogue, &[], &scope).expect("a plan");
    let provider: Arc<dyn capability::CapabilityProvider> =
        Arc::new(NativeToolProvider::from_registry(&registry));
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

#[tokio::test]
async fn the_shim_runs_a_capability_through_the_workspace() {
    let runtime = runtime();
    let tool = CapabilityTool::new(Arc::clone(&runtime), &capability("echo_add"), principal())
        .expect("the workspace has it");

    let output = tool
        .run_with_call_id(&json!({ "a": 4, "b": 6 }), "call-1")
        .await
        .expect("the capability answers");

    assert_eq!(output, json!({ "sum": 10 }));
}

#[tokio::test]
async fn the_shim_names_the_capability_surface_and_schema() {
    let runtime = runtime();
    let tool = CapabilityTool::new(Arc::clone(&runtime), &capability("echo_add"), principal())
        .expect("the workspace has it");

    assert_eq!(tool.name(), "echo_add");
    assert_eq!(tool.description(), "Add two numbers");
    assert_eq!(tool.json_schema(), schema());
}

#[tokio::test]
async fn the_shim_reports_a_deadline_as_a_tool_timeout() {
    let runtime = runtime();
    let tool = CapabilityTool::new(Arc::clone(&runtime), &capability("slow"), principal())
        .expect("the workspace has it")
        .with_timeout(Duration::from_millis(20));

    let error = tool
        .run_with_call_id(&json!({}), "call-2")
        .await
        .expect_err("the tool sleeps past the deadline");

    assert!(matches!(error, ToolError::Timeout(_)), "{error:?}");
}

#[tokio::test]
async fn a_capability_outside_the_workspace_has_no_shim() {
    let runtime = runtime();

    assert!(CapabilityTool::new(runtime, &capability("write_file"), principal()).is_none());
}

#[tokio::test]
async fn the_shim_context_carries_the_workspace() {
    let runtime = runtime();
    let context = InvocationContext::local(InvocationId::new("call-3"), principal(), workspace());

    assert_eq!(context.workspace, *runtime.workspace());
}

struct ContextProbe {
    seen: Arc<std::sync::Mutex<Option<InvocationContext>>>,
    provider: capability::ProviderId,
    descriptor: capability::CapabilityDescriptor,
}

#[async_trait::async_trait]
impl capability::CapabilityProvider for ContextProbe {
    fn id(&self) -> &capability::ProviderId {
        &self.provider
    }

    fn kind(&self) -> capability::ProviderKind {
        capability::ProviderKind::Builtin
    }

    async fn describe(
        &self,
    ) -> Result<Vec<capability::CapabilityDescriptor>, capability::CapabilityError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn invoke(
        &self,
        call: capability::CapabilityCall<'_>,
    ) -> Result<capability::CapabilityOutput, capability::CapabilityError> {
        *self.seen.lock().expect("seen") = Some(call.context.clone());
        Ok(capability::CapabilityOutput::text("probe"))
    }

    async fn health(&self) -> capability::ProviderHealth {
        capability::ProviderHealth::Available
    }
}

fn probe_runtime() -> (
    Arc<WorkspaceRuntime>,
    Arc<std::sync::Mutex<Option<InvocationContext>>>,
) {
    use capability::composition::{Budgets, ResolvedCapability, ResolvedComposition};
    use capability::descriptor::CapabilityDescriptor;
    use capability::id::{CapabilityKind, VersionSource};

    let descriptor = CapabilityDescriptor::new(
        capability("probe"),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    );
    let provider = capability::ProviderId::builtin();
    let seen = Arc::new(std::sync::Mutex::new(None));
    let plan = ResolvedComposition {
        id: "compose:test".into(),
        version: Version::new(1, 0, 0),
        workspace: workspace(),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor.clone(),
            binding: capability::composition::Binding::Local {
                provider: provider.clone(),
            },
            decision: None,
        }],
        skipped: Vec::new(),
        budgets: Budgets::default(),
    };
    let runtime = WorkspaceRuntime::instantiate(
        plan,
        &[Arc::new(ContextProbe {
            seen: Arc::clone(&seen),
            provider,
            descriptor,
        }) as Arc<dyn capability::CapabilityProvider>],
    )
    .expect("a runtime");
    (Arc::new(runtime), seen)
}

#[tokio::test]
async fn the_shim_context_carries_turn_task_and_policy() {
    let (runtime, seen) = probe_runtime();
    let tool = CapabilityTool::new(runtime, &capability("probe"), principal())
        .expect("the capability exists")
        .with_turn("turn-7")
        .with_task("task-7")
        .with_parent_task("task-parent")
        .with_attempt(2)
        .with_policy(capability::PolicyDecisionRef::new("policy-7"));

    tool.run_with_call_id(&json!({}), "call-7")
        .await
        .expect("the probe answers");

    let context = seen
        .lock()
        .expect("seen")
        .clone()
        .expect("the provider saw a context");
    assert_eq!(
        context.turn.as_ref().map(|turn| turn.as_str()),
        Some("turn-7")
    );
    assert_eq!(
        context.task.as_ref().map(|task| task.as_str()),
        Some("task-7")
    );
    assert_eq!(
        context.parent_task.as_ref().map(|task| task.as_str()),
        Some("task-parent")
    );
    assert_eq!(context.attempt, 2);
    assert_eq!(
        context.policy.as_ref().map(|policy| policy.as_str()),
        Some("policy-7")
    );
    assert_eq!(
        context
            .composition
            .as_ref()
            .map(|composition| composition.id.as_str()),
        Some("compose:test")
    );
}

#[tokio::test]
async fn the_shim_reads_the_shared_correlation_per_call() {
    let (runtime, seen) = probe_runtime();
    let shared = Arc::new(std::sync::Mutex::new(CapabilityCorrelation::default()));
    let tool = CapabilityTool::new(runtime, &capability("probe"), principal())
        .expect("the capability exists")
        .with_correlation(Arc::clone(&shared));

    shared.lock().expect("shared").turn = Some(capability::TurnId::new("turn-1"));
    tool.run_with_call_id(&json!({}), "call-1")
        .await
        .expect("the probe answers");
    assert_eq!(
        seen.lock()
            .expect("seen")
            .as_ref()
            .and_then(|context| context.turn.as_ref())
            .map(|turn| turn.as_str()),
        Some("turn-1")
    );

    shared.lock().expect("shared").turn = Some(capability::TurnId::new("turn-2"));
    tool.run_with_call_id(&json!({}), "call-2")
        .await
        .expect("the probe answers");
    assert_eq!(
        seen.lock()
            .expect("seen")
            .as_ref()
            .and_then(|context| context.turn.as_ref())
            .map(|turn| turn.as_str()),
        Some("turn-2"),
        "the same tool reads the next turn without being rebuilt"
    );
}

fn runtime_over(registry: &ToolRegistry) -> Arc<WorkspaceRuntime> {
    let catalogue = catalogue_from_registry(registry).expect("a catalogue");
    let mut scope = LocalScope::new(workspace());
    let mut spec = CompositionSpec::new("compose:test", Version::new(1, 0, 0), "facts");
    for id in catalogue.ids() {
        scope.admit(id.clone());
        spec = spec.with_requirement(CapabilityRequirement::new(id.clone(), VersionReq::STAR));
    }
    let plan = resolve_host(&spec, &catalogue, &[], &scope).expect("a plan");
    let provider: Arc<dyn capability::CapabilityProvider> =
        Arc::new(NativeToolProvider::from_registry(registry));
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

#[tokio::test]
async fn a_shim_preserves_deferral_skill_and_cancelability() {
    let mut registry = ToolRegistry::new();
    let deferred = FunctionTool::new(
        "quiet",
        "Hidden until asked for",
        json!({ "type": "object" }),
        |_| Ok(json!({})),
    )
    .with_deferred(true);
    registry
        .register(Arc::new(deferred))
        .expect("a unique tool");
    let runtime = runtime_over(&registry);

    let shim = CapabilityTool::new(runtime, &capability("quiet"), principal())
        .expect("the capability exists")
        .with_facts(ToolFacts {
            deferred: true,
            is_skill: true,
            cancelable: true,
        });

    assert!(shim.deferred(), "the shim keeps the tool out of the advert");
    assert!(shim.is_skill(), "the shim stays a skill");
    assert!(shim.is_cancelable());
}

struct Dispatcher;

impl Tool for Dispatcher {
    fn name(&self) -> &str {
        "proxy_tool"
    }

    fn description(&self) -> String {
        "Dispatch to another tool".to_string()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        Ok(input.clone())
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        Some(("echo_add".to_string(), args.clone()))
    }
}

#[tokio::test]
async fn a_shim_preserves_a_dispatchers_proxy_target() {
    let mut registry = registry();
    let raw: Arc<dyn Tool> = Arc::new(Dispatcher);
    registry.register(Arc::clone(&raw)).expect("a unique tool");
    let runtime = runtime_over(&registry);

    let shim = CapabilityTool::new(runtime, &capability("proxy_tool"), principal())
        .expect("the dispatcher capability exists")
        .with_proxy(ProxySource::Sync(raw));

    let target = shim
        .proxy_target(&json!({ "a": 1, "b": 2 }))
        .expect("the dispatcher names its target");
    assert_eq!(target.0, "echo_add");
    assert_eq!(target.1, json!({ "a": 1, "b": 2 }));
}

/// The capability boundary keeps the class a tool reported, so a caller's
/// recovery decision does not depend on parsing the message.
#[test]
fn a_capability_failure_keeps_its_class() {
    use agent::capabilities::to_tool_error;
    use capability::{CapabilityError, DeliveryState};
    use react::tool::ToolErrorCode;

    let denied = to_tool_error(CapabilityError::Denied {
        reason: "the policy refused the write".to_string(),
        policy: None,
    });
    assert_eq!(denied.code(), ToolErrorCode::PermissionDenied);
    assert!(!denied.retryable());
    assert!(!denied.indeterminate());

    let cancelled = to_tool_error(CapabilityError::Cancelled);
    assert_eq!(cancelled.code(), ToolErrorCode::Cancelled);
    assert!(cancelled.indeterminate(), "cancellation is not rollback");

    let timed_out = to_tool_error(CapabilityError::Timeout {
        elapsed_ms: 30,
        partial: Some(json!({ "stdout": "half" })),
    });
    assert_eq!(timed_out.code(), ToolErrorCode::Timeout);
    assert!(timed_out.to_string().contains("stdout"), "{timed_out}");

    let unavailable = to_tool_error(CapabilityError::Unavailable {
        reason: "the browser engine is not running".to_string(),
    });
    assert_eq!(unavailable.code(), ToolErrorCode::Unavailable);
    assert!(unavailable.retryable());

    let not_sent = to_tool_error(CapabilityError::Transport {
        message: "dial failed".to_string(),
        delivered: DeliveryState::NotSent,
    });
    assert_eq!(not_sent.code(), ToolErrorCode::Unavailable);
    assert!(not_sent.retryable(), "nothing left this node");

    let sent = to_tool_error(CapabilityError::Transport {
        message: "no answer".to_string(),
        delivered: DeliveryState::Sent,
    });
    assert_eq!(sent.code(), ToolErrorCode::Internal);
    assert!(sent.indeterminate(), "a sent call may have run at the peer");

    let conflict = to_tool_error(CapabilityError::Duplicate {
        message: "already accepted".to_string(),
    });
    assert_eq!(conflict.code(), ToolErrorCode::Conflict);
}

/// A missing argument keeps its name across the capability boundary, because
/// the shim validates against the descriptor the model was shown.
#[tokio::test]
async fn a_missing_argument_keeps_its_field_through_the_shim() {
    let registry = registry();
    let runtime = runtime_over(&registry);
    let shim = CapabilityTool::new(runtime, &capability("echo_add"), principal())
        .expect("the capability exists");

    let error = shim
        .run(&json!({ "a": 1 }))
        .await
        .expect_err("b is required");
    match error {
        ToolError::InvalidField { field, missing, .. } => {
            assert_eq!(field, "b");
            assert!(missing);
        }
        other => panic!("expected a field error, got {other:?}"),
    }
}
