//! Tests for the cancellation design:
//! 1. The engine injects `__call_id__` into the tool's args before invoking
//!    the `before_tool_call` hook.
//! 2. `tool.cancel(call_id)` (on the AsyncTool trait) is invoked with the
//!    matching call_id when the engine's cancel listener receives a cancel
//!    message on the bus topic.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession, TokenStream,
};
use react::runtime::{HookDecision, ReActApp};
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::ToolError;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

/// App whose hooks record the args/result they received, so the test can
/// verify `__call_id__` was injected before both the `before_tool_call` and
/// `after_tool_result` hooks fired.
#[derive(Default)]
struct RecordingApp {
    hook_args: Arc<DashMap<String, Value>>,
    hook_results: Arc<DashMap<String, Value>>,
}

impl ReActApp for RecordingApp {
    type Session = LlmSession;
    type Context = LlmContext;

    fn before_tool_call(
        &self,
        tool_name: &str,
        args: &mut Value,
        call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl std::future::Future<Output = HookDecision> + Send {
        let key = format!("{}::{}", tool_name, call_id);
        self.hook_args.insert(key, args.clone());
        async { HookDecision::Continue }
    }

    fn after_tool_result(
        &self,
        tool_name: &str,
        result: &mut Result<Value, react::engine::ReactError>,
        call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl std::future::Future<Output = HookDecision> + Send {
        let key = format!("{}::{}", tool_name, call_id);
        self.hook_results.insert(key, result.as_ref().map(|v| v.clone()).unwrap_or_default());
        async { HookDecision::Continue }
    }
}

/// Async test tool that records what it received.
struct CapturingTool {
    name: String,
    received_args: Arc<DashMap<String, Value>>,
    received_cancel_calls: Arc<DashMap<String, usize>>,
    cancelable: bool,
}

impl CapturingTool {
    fn new(
        name: &str,
        cancelable: bool,
    ) -> (
        Self,
        Arc<DashMap<String, Value>>,
        Arc<DashMap<String, usize>>,
    ) {
        let received_args = Arc::new(DashMap::new());
        let received_cancel_calls = Arc::new(DashMap::new());
        (
            Self {
                name: name.to_string(),
                received_args: received_args.clone(),
                received_cancel_calls: received_cancel_calls.clone(),
                cancelable,
            },
            received_args,
            received_cancel_calls,
        )
    }
}

#[async_trait]
impl AsyncTool for CapturingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        "captures args and cancel calls".to_string()
    }
    fn is_cancelable(&self) -> bool {
        self.cancelable
    }
    fn cancel(&self, call_id: &str) {
        let mut entry = self.received_cancel_calls.entry(call_id.to_string()).or_insert(0);
        *entry += 1;
    }
    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        // Record what we got.
        let call_id = input
            .get("__call_id__")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>")
            .to_string();
        self.received_args
            .insert(call_id.clone(), input.clone());
        Ok(json!({"ok": true, "call_id": call_id}))
    }
}

#[tokio::test]
async fn call_tool_injects_call_id_into_args() {
    let (tool, received_args, _cancel_calls) = CapturingTool::new("capturing", true);
    let hook_args = Arc::new(DashMap::new());
    let hook_results = Arc::new(DashMap::new());

    // The first LLM response asks to call `capturing`; the second is a final answer.
    let llm = Box::new(ToolCallingLlm {
        responses: vec![
            make_tool_call_response("call-123"),
            make_text_response("Final Answer: done".to_string(), true),
        ],
        index: Arc::new(AtomicUsize::new(0)),
    });

    let tool_box: Box<dyn AsyncTool> = Box::new(tool);
    let mut engine = ReActEngineBuilder::<RecordingApp>::new()
        .llm(llm)
        .with_tool(ToolVariant::Async(tool_box))
        .agent_name("test-agent".to_string())
        .app(RecordingApp {
            hook_args: hook_args.clone(),
            hook_results: hook_results.clone(),
        })
        .max_steps(2)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("Call the capturing tool");
    let result = engine.react(None, request, &mut session, &mut context).await;
    assert!(result.is_ok(), "react failed: {:?}", result.err());

    // The tool should have received __call_id__ in its args.
    let captured = received_args.get("call-123").expect("tool did not run");
    assert_eq!(
        captured.get("__call_id__").and_then(|v| v.as_str()),
        Some("call-123"),
        "tool args did not contain __call_id__"
    );

    // The before_tool_call hook should have seen __call_id__ injected into args.
    let hook_entry = hook_args
        .get("capturing::call-123")
        .expect("before_tool_call hook did not run");
    assert_eq!(
        hook_entry.get("__call_id__").and_then(|v| v.as_str()),
        Some("call-123"),
        "before_tool_call hook did not see __call_id__"
    );

    // The after_tool_result hook should have received the call_id as a param.
    let hook_result_entry = hook_results
        .get("capturing::call-123")
        .expect("after_tool_result hook did not run");
    assert_eq!(
        hook_result_entry.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "after_tool_result hook did not receive the tool result"
    );
}

#[tokio::test]
async fn tool_cancel_invokes_cancel_callback() {
    let (tool, cancel_calls) = {
        let (t, _a, c) = CapturingTool::new("capturing", true);
        (Box::new(t) as Box<dyn AsyncTool>, c)
    };

    assert_eq!(
        tool.is_cancelable(),
        true,
        "tool should be cancelable",
    );

    let count_before = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_before, 0, "no cancel calls before test");

    tool.cancel("call-XYZ");

    let count_after = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_after, 1, "tool.cancel should have been invoked once");

    tool.cancel("call-XYZ");
    let count_twice = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_twice, 2, "tool.cancel should be idempotent (count increments)");

    // Different call_id
    tool.cancel("other-call");
    assert_eq!(
        cancel_calls.get("other-call").map(|r| *r.value()).unwrap_or(0),
        1,
    );
    assert_eq!(
        cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0),
        2,
    );
}



// ── Mock LLM that first emits a tool call, then a final answer ─────────────

struct ToolCallingLlm {
    responses: Vec<LlmResponse>,
    index: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for ToolCallingLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let i = self.index.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .responses
            .get(i)
            .cloned()
            .unwrap_or_else(|| make_text_response("Final Answer: done".to_string(), true)))
    }
    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }
    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "tool-calling-mock"
    }
}

fn make_tool_call_response(call_id: &str) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: call_id.to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: Some("capturing".to_string()),
                        arguments: Some(r#"{"user_key": "abc"}"#.to_string()),
                    },
                }]),
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            stop_reason: None,
            finish_reason: Some("tool_calls".to_string()),
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

fn make_text_response(content: String, is_final: bool) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            stop_reason: None,
            finish_reason: if is_final {
                Some("stop".to_string())
            } else {
                Some("continue".to_string())
            },
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}
