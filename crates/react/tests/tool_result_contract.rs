#![cfg(test)]
//! Every tool result the engine records is one parseable envelope.
//!
//! The engine used to stringify whatever a tool returned and prefix a failure
//! with "Error: ", so a host could only tell a failure from a success by the
//! prose. These tests pin the contract at the seam where the model reads it:
//! the ToolResult message, whatever the tool returned.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Mutex;

use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession,
    ReactSession, TokenStream,
};
use react::runtime::ReActApp;
use react::tool::ToolError;
use react::tool::registry::{AsyncTool, ToolVariant};

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

struct FixedTool {
    name: &'static str,
    output: Result<Value, ToolError>,
}

#[async_trait]
impl AsyncTool for FixedTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> String {
        format!("Answers with a fixed {}", self.name)
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        self.output.clone()
    }
}

fn tool_call(name: &str, call_id: &str) -> ToolCall {
    ToolCall {
        id: call_id.to_string(),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: Some(name.to_string()),
            arguments: Some(json!({}).to_string()),
        },
    }
}

fn response(id: &str, calls: Option<Vec<ToolCall>>, content: Option<&str>) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: id.to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: content.map(str::to_string),
                tool_calls: calls,
                function_call: None,
                reasoning_content: None,
                extra: Value::Object(serde_json::Map::new()),
            },
            finish_reason: Some(
                if content.is_some() {
                    "stop"
                } else {
                    "tool_calls"
                }
                .to_string(),
            ),
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

struct SequencedLlm {
    queued: Mutex<Vec<LlmResponse>>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for SequencedLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let mut queued = self.queued.lock().unwrap();
        if queued.is_empty() {
            Ok(response("final", None, Some("Final Answer: done")))
        } else {
            Ok(queued.remove(0))
        }
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
        "sequenced"
    }
}

/// Run one turn that calls the four tools, and return the recorded results by
/// call id.
fn run_turn() -> Vec<(String, Value)> {
    let calls = vec![
        tool_call("ok_tool", "call-ok"),
        tool_call("timeout_tool", "call-timeout"),
        tool_call("partial_tool", "call-partial"),
        tool_call("self_failed_tool", "call-self-failed"),
    ];
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(SequencedLlm {
            queued: Mutex::new(vec![response("calls", Some(calls), None)]),
        }))
        .with_tool(ToolVariant::Async(Box::new(FixedTool {
            name: "ok_tool",
            output: Ok(json!({ "content": "the file body", "lines": 3 })),
        })))
        .with_tool(ToolVariant::Async(Box::new(FixedTool {
            name: "timeout_tool",
            output: Err(ToolError::Timeout("bash exceeded its 600 s bound".into())),
        })))
        .with_tool(ToolVariant::Async(Box::new(FixedTool {
            name: "partial_tool",
            output: Ok(json!({
                "tool": "partial_tool",
                "status": "partial",
                "content": "first page",
                "next": { "offset": 40 }
            })),
        })))
        .with_tool(ToolVariant::Async(Box::new(FixedTool {
            name: "self_failed_tool",
            output: Ok(json!({
                "success": false,
                "error": { "message": "the disk is full", "retryable": true }
            })),
        })))
        .parallel_tool_calls(true)
        .max_steps(3)
        .build()
        .expect("the engine builds");

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut session = LlmSession::default();
        let mut context = LlmContext::default();
        let answer = engine
            .react(
                None,
                LlmRequest::new("test-model"),
                &mut session,
                &mut context,
            )
            .await
            .expect("the turn runs");
        assert_eq!(answer, "done");
        session
            .history()
            .expect("the session keeps its history")
            .iter()
            .filter_map(|message| match message {
                react::llm::LlmMessage::ToolResult {
                    tool_call_id,
                    content,
                } => Some((
                    tool_call_id.clone(),
                    serde_json::from_str::<Value>(content.as_str())
                        .unwrap_or_else(|_| panic!("not an envelope: {content}")),
                )),
                _ => None,
            })
            .collect()
    })
}

#[test]
fn the_engine_records_one_model_document_for_every_outcome() {
    let results = run_turn();
    assert_eq!(results.len(), 4, "one result per call");
    let by_id = |id: &str| {
        results
            .iter()
            .find(|(call, _)| call == id)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("{id} was recorded"))
    };

    // Success is the verdict and the payload, with no derivable sentence.
    let ok = by_id("call-ok");
    assert_eq!(ok["status"], "success");
    assert!(
        ok.get("summary").is_none(),
        "the model is not sent a sentence it can derive"
    );
    assert_eq!(ok["data"]["content"], "the file body");
    assert!(ok.get("error").is_none());

    // A failure is a class, not a prefix, and carries no payload.
    let timeout = by_id("call-timeout");
    assert_eq!(timeout["status"], "error");
    assert_eq!(timeout["error"]["code"], "timeout");
    assert_eq!(timeout["error"]["retryable"], false);
    assert_eq!(timeout["error"]["indeterminate"], true);
    assert!(timeout.get("data").is_none(), "a failure has no payload");

    // Partial is not failure, and the continuation is exact.
    let partial = by_id("call-partial");
    assert_eq!(partial["status"], "partial");
    assert!(partial.get("error").is_none());
    assert_eq!(partial["continuation"]["tool"], "partial_tool");
    assert_eq!(partial["continuation"]["args"]["offset"], 40);
    assert_eq!(partial["data"]["content"], "first page");

    // A tool that returns Ok but says it failed is a failure.
    let self_failed = by_id("call-self-failed");
    assert_eq!(self_failed["status"], "error");
    assert_eq!(self_failed["error"]["message"], "the disk is full");
    assert_eq!(self_failed["error"]["retryable"], true);
}

#[test]
fn the_repeat_guard_uses_the_envelope_verdict() {
    // A success envelope whose payload merely mentions a failure is still a
    // success; only the envelope's own status decides.
    let envelope = react::tool::ToolResult::from_value(
        "ok_tool",
        json!({ "note": "the earlier run said success:false" }),
    )
    .to_text(0);
    assert!(!react::tool::ToolResult::text_is_error(&envelope));

    let failed = react::tool::ToolResult::from_error(
        "ok_tool",
        &ToolError::denied("the policy refused the write"),
    )
    .to_text(0);
    assert!(react::tool::ToolResult::text_is_error(&failed));
}
