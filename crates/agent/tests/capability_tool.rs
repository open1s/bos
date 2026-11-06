//! The engine-facing shim: ReAct calls a capability through the workspace.

use std::sync::Arc;
use std::time::Duration;

use agent::capabilities::{
    catalogue_from_registry, CapabilityTool, NativeToolProvider, ProxySource, ToolFacts,
};
use agent::tools::{AsyncTool, FunctionTool, Tool, ToolError, ToolRegistry};
use capability::composition::{
    resolve, CapabilityRequirement, CompositionSpec, LocalScope, RemotePolicy,
};
use capability::id::WorkspaceId;
use capability::{CapabilityId, InvocationContext, InvocationId, Principal, WorkspaceRuntime};
use semver::{Version, VersionReq};
use serde_json::{json, Value};

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
    let plan = resolve(&spec, &catalogue, &[], &scope).expect("a plan");
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

fn runtime_over(registry: &ToolRegistry) -> Arc<WorkspaceRuntime> {
    let catalogue = catalogue_from_registry(registry).expect("a catalogue");
    let mut scope = LocalScope::new(workspace());
    let mut spec = CompositionSpec::new("compose:test", Version::new(1, 0, 0), "facts");
    for id in catalogue.ids() {
        scope.admit(id.clone());
        spec = spec.with_requirement(CapabilityRequirement::new(id.clone(), VersionReq::STAR));
    }
    let plan = resolve(&spec, &catalogue, &[], &scope).expect("a plan");
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
        "call_tool"
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

    let shim = CapabilityTool::new(runtime, &capability("call_tool"), principal())
        .expect("the dispatcher capability exists")
        .with_proxy(ProxySource::Sync(raw));

    let target = shim
        .proxy_target(&json!({ "a": 1, "b": 2 }))
        .expect("the dispatcher names its target");
    assert_eq!(target.0, "echo_add");
    assert_eq!(target.1, json!({ "a": 1, "b": 2 }));
}
