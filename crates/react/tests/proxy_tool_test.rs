//! A dispatcher reports the tool it forwarded to.
//!
//! `call_tool` is one advertised tool that runs many: the provider still sees
//! one name (the transcript has to pair the call it emitted), but hooks, bus
//! events and the repeated-failure guard must see the tool that actually ran.
//! The engine resolves the proxy before dispatch; this test pins that.

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession,
    ReactSession, TokenStream,
};
use react::runtime::{HookDecision, ReActApp};
use react::tool::registry::ToolVariant;
use react::tool::{Tool, ToolError};
use serde_json::{json, Value};

/// Records the name each tool hook was given.
#[derive(Default)]
struct RecordingApp {
    before: Arc<Mutex<Vec<String>>>,
    after: Arc<Mutex<Vec<String>>>,
}

impl ReActApp for RecordingApp {
    type Session = LlmSession;
    type Context = LlmContext;

    async fn before_tool_call(
        &self,
        tool_name: &str,
        _args: &mut Value,
        _call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> HookDecision {
        self.before.lock().unwrap().push(tool_name.to_string());
        HookDecision::Continue
    }

    async fn after_tool_result(
        &self,
        tool_name: &str,
        _result: &mut Result<Value, react::engine::ReactError>,
        _call_id: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> HookDecision {
        self.after.lock().unwrap().push(tool_name.to_string());
        HookDecision::Continue
    }
}

/// A tool that forwards to another: `{name, args}` in, one call out.
struct Dispatcher;

impl Tool for Dispatcher {
    fn name(&self) -> &str {
        "call_tool"
    }

    fn description(&self) -> String {
        "run a tool by name".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({"type": "object", "properties": {"name": {"type": "string"}, "args": {"type": "object"}}})
    }

    fn run(&self, args: &Value) -> Result<Value, ToolError> {
        Ok(json!({"dispatched": args["name"], "args": args["args"]}))
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        let name = args.get("name")?.as_str()?.to_string();
        let inner = args.get("args").cloned().unwrap_or_else(|| json!({}));
        Some((name, inner))
    }
}

/// The tool the dispatcher forwards to, registered so the name is real.
struct Bash;

impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "run a command".to_string()
    }

    fn json_schema(&self) -> Value {
        json!({"type": "object", "properties": {"command": {"type": "string"}}})
    }

    fn run(&self, _args: &Value) -> Result<Value, ToolError> {
        Ok(json!({"exit_code": 0}))
    }
}

fn tool_call_response(name: &str, arguments: &str) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "proxy-test".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "mock-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: Some(name.to_string()),
                        arguments: Some(arguments.to_string()),
                    },
                }]),
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some("tool_calls".to_string()),
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

fn text_response(content: &str) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "proxy-test-2".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567891,
        model: "mock-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(content.to_string()),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some("stop".to_string()),
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

/// Asks the dispatcher for `bash` once, then answers.
struct TwoStepLlm {
    step: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for TwoStepLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let step = self.step.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(if step == 0 {
            tool_call_response("call_tool", r#"{"name":"bash","args":{"command":"ls"}}"#)
        } else {
            text_response("Final Answer: dispatched")
        })
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
        "two_step"
    }
}

#[test]
fn a_proxy_call_reaches_the_hooks_as_the_tool_it_forwarded_to() {
    let app = RecordingApp::default();
    let before = Arc::clone(&app.before);
    let after = Arc::clone(&app.after);

    let mut engine = ReActEngineBuilder::<RecordingApp>::new()
        .llm(Box::new(TwoStepLlm {
            step: std::sync::atomic::AtomicUsize::new(0),
        }))
        .app(app)
        .with_tool(ToolVariant::Sync(Box::new(Dispatcher)))
        .with_tool(ToolVariant::Sync(Box::new(Bash)))
        .max_steps(4)
        .build()
        .expect("engine builds");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();

    runtime
        .block_on(async {
            engine
                .react(
                    None,
                    LlmRequest::new("list the files"),
                    &mut session,
                    &mut context,
                )
                .await
        })
        .expect("the turn completes");

    assert_eq!(
        *before.lock().unwrap(),
        vec!["bash".to_string()],
        "the before hook names the tool that ran, not the dispatcher"
    );
    assert_eq!(
        *after.lock().unwrap(),
        vec!["bash".to_string()],
        "the result hook names the tool that ran"
    );

    // The transcript keeps the name the model emitted: a provider pairs a tool
    // result with the call it produced, and that call said `call_tool`.
    let recorded = session
        .history()
        .expect("history")
        .iter()
        .find_map(|message| match message {
            react::llm::LlmMessage::AssistantToolCall { name, .. } => Some(name.clone()),
            _ => None,
        })
        .expect("a tool call was recorded");
    assert_eq!(recorded, "call_tool");
}
