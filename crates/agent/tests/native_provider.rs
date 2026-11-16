//! The native provider executes a capability call through a real registry.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent::capabilities::NativeToolProvider;
use agent::tools::{AsyncTool, FunctionTool, ToolError, ToolRegistry};
use async_trait::async_trait;
use capability::{
    CancelToken, CapabilityCall, CapabilityError, CapabilityId, CapabilityProvider,
    InvocationContext, InvocationId, Principal, WorkspaceId,
};
use serde_json::{json, Value};

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:test"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    )
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid capability id")
}

struct Slow;

#[async_trait]
impl AsyncTool for Slow {
    fn name(&self) -> &str {
        "slow"
    }

    fn description(&self) -> String {
        "sleeps for longer than any deadline in these tests".to_string()
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
            json!({
                "type": "object",
                "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
                "required": ["a", "b"]
            }),
            |args| {
                let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
                let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
                Ok(json!({ "sum": a + b }))
            },
        )))
        .expect("a unique tool");
    registry
        .register(Arc::new(FunctionTool::new(
            "broken",
            "Always fails",
            json!({ "type": "object" }),
            |_| Err(ToolError::Failed("the body failed".to_string())),
        )))
        .expect("a unique tool");
    registry
        .register(Arc::new(FunctionTool::new(
            "strict",
            "Requires a value",
            json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            |_| Ok(json!({ "ran": true })),
        )))
        .expect("a unique tool");
    registry
        .register_async(Arc::new(Slow))
        .expect("a unique tool");
    registry
}

fn provider() -> NativeToolProvider {
    NativeToolProvider::from_registry(&registry())
}

#[tokio::test]
async fn describe_lists_every_registered_tool() {
    let provider = provider();
    let descriptors = provider.describe().await.expect("a catalogue");

    assert_eq!(descriptors.len(), provider.registry().list().len());
    let ids: Vec<String> = descriptors
        .iter()
        .map(|descriptor| descriptor.id.to_string())
        .collect();
    assert!(ids.contains(&"builtin/echo_add".to_string()), "{ids:?}");
    assert!(ids.contains(&"builtin/slow".to_string()), "{ids:?}");
}

#[tokio::test]
async fn invokes_a_synchronous_tool() {
    let provider = provider();
    let id = capability("echo_add");
    let context = context();

    let output = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({ "a": 2, "b": 3 }),
            context: &context,
        })
        .await
        .expect("the tool runs");

    assert_eq!(output.structured, Some(json!({ "sum": 5 })));
}

#[tokio::test]
async fn an_unknown_capability_is_not_found() {
    let provider = provider();
    let id = capability("does_not_exist");
    let context = context();

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({}),
            context: &context,
        })
        .await
        .expect_err("nothing is registered under that name");

    assert!(
        matches!(error, CapabilityError::NotFound { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn invalid_arguments_are_rejected_before_the_body_runs() {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FunctionTool::new(
            "strict",
            "Requires a value",
            json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            }),
            move |_| {
                flag.store(true, Ordering::SeqCst);
                Ok(json!({ "ran": true }))
            },
        )))
        .expect("a unique tool");
    let provider = NativeToolProvider::from_registry(&registry);
    let id = capability("strict");
    let context = context();

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({}),
            context: &context,
        })
        .await
        .expect_err("the required argument is missing");

    assert!(
        matches!(error, CapabilityError::Invalid { .. }),
        "{error:?}"
    );
    assert!(!ran.load(Ordering::SeqCst), "the body must not run");
}

#[tokio::test]
async fn a_failed_tool_is_a_provider_failure_not_a_denial() {
    let provider = provider();
    let id = capability("broken");
    let context = context();

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({}),
            context: &context,
        })
        .await
        .expect_err("the body fails");

    assert!(
        matches!(error, CapabilityError::ProviderFailed { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_cancelled_context_never_reaches_the_tool() {
    let provider = provider();
    let id = capability("echo_add");
    let context = context();
    context.cancel.cancel();

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({ "a": 1, "b": 1 }),
            context: &context,
        })
        .await
        .expect_err("the context is already cancelled");

    assert!(matches!(error, CapabilityError::Cancelled), "{error:?}");
}

#[tokio::test]
async fn an_expired_deadline_is_a_timeout_before_dispatch() {
    let provider = provider();
    let id = capability("echo_add");
    let mut context = context();
    context.deadline = Some(Instant::now() - Duration::from_millis(1));

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({ "a": 1, "b": 1 }),
            context: &context,
        })
        .await
        .expect_err("the deadline already passed");

    assert!(
        matches!(error, CapabilityError::Timeout { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_async_tool_that_overruns_its_deadline_times_out() {
    let provider = provider();
    let id = capability("slow");
    let mut context = context();
    context.deadline = Some(Instant::now() + Duration::from_millis(20));

    let error = provider
        .invoke(CapabilityCall {
            capability: &id,
            operation: None,
            input: json!({}),
            context: &context,
        })
        .await
        .expect_err("the tool sleeps far longer than the deadline");

    assert!(
        matches!(error, CapabilityError::Timeout { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_cancel_during_the_call_is_reported_when_the_tool_returns() {
    let provider = Arc::new(provider());
    let token = CancelToken::new();
    let id = capability("slow");
    let task = {
        let provider = Arc::clone(&provider);
        let token = token.clone();
        tokio::spawn(async move {
            let mut context = context();
            context.cancel = token;
            provider
                .invoke(CapabilityCall {
                    capability: &id,
                    operation: None,
                    input: json!({}),
                    context: &context,
                })
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(10)).await;
    token.cancel();
    let result = task.await.expect("the task does not panic");

    assert!(
        matches!(result, Err(CapabilityError::Cancelled)),
        "{result:?}"
    );
}
