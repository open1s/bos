#![cfg(test)]
//! One assistant response can carry several tool calls at once.
//!
//! The engine runs such a batch through react, and AgentConfig::parallel_tool_calls
//! decides whether the calls of that one response run at the same time or one
//! after another. The two tests below are the discrimination pair: the same
//! batch, the same two 400 ms tools, and the flag deciding the wall clock.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession,
    TokenStream,
};
use react::runtime::ReActApp;
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::ToolError;

/// How long each tool in the batch waits.
const TOOL_MILLIS: u64 = 400;
/// At or above this, the batch waited for its tools in turn.
const BATCH_BUDGET_MS: u128 = 650;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

/// An async tool that answers its own name after a fixed wait.
struct SlowTool {
    name: String,
}

#[async_trait]
impl AsyncTool for SlowTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!("Waits {} ms, then answers its own name", TOOL_MILLIS)
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        tokio::time::sleep(Duration::from_millis(TOOL_MILLIS)).await;
        Ok(Value::String(self.name.clone()))
    }
}

fn tool_call(name: &str, call_id: &str) -> ToolCall {
    ToolCall {
        id: call_id.to_string(),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: Some(name.to_string()),
            arguments: Some(serde_json::json!({}).to_string()),
        },
    }
}

/// A response that asks for both tools in one message.
fn two_call_response() -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-parallel".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(vec![
                    tool_call("slow_one", "call-1"),
                    tool_call("slow_two", "call-2"),
                ]),
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

/// The answer the loop is expected to end on.
fn final_response() -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-final".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("Final Answer: done".to_string()),
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

/// Hands out the two-call response once, then the final answer.
struct SequencedLlm {
    queued: Mutex<Vec<LlmResponse>>,
}

impl SequencedLlm {
    fn new() -> Self {
        Self {
            queued: Mutex::new(vec![two_call_response()]),
        }
    }
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
            Ok(final_response())
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

/// Run the same two-call batch, with parallel dispatch on or off.
async fn run_batch(parallel: bool) -> (String, Duration) {
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(SequencedLlm::new()))
        .with_tool(ToolVariant::Async(Box::new(SlowTool {
            name: "slow_one".to_string(),
        })))
        .with_tool(ToolVariant::Async(Box::new(SlowTool {
            name: "slow_two".to_string(),
        })))
        .parallel_tool_calls(parallel)
        .max_steps(4)
        .build()
        .expect("the engine builds");

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let started = Instant::now();
    let answer = engine
        .react(
            None,
            LlmRequest::new("test-model"),
            &mut session,
            &mut context,
        )
        .await
        .expect("the batch runs");
    (answer, started.elapsed())
}

#[test]
fn a_parallel_batch_runs_its_tools_at_the_same_time() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (answer, elapsed) = rt.block_on(run_batch(true));
    assert_eq!(answer, "done");
    assert!(
        elapsed.as_millis() < BATCH_BUDGET_MS,
        "two {TOOL_MILLIS} ms tools in one batch must overlap when the switch is on, but the batch took {elapsed:?}"
    );
}

#[test]
fn a_sequential_batch_waits_for_each_tool() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (answer, elapsed) = rt.block_on(run_batch(false));
    assert_eq!(answer, "done");
    assert!(
        elapsed.as_millis() >= BATCH_BUDGET_MS,
        "the default dispatch runs one tool at a time, so the batch must pay both waits, but it took {elapsed:?}"
    );
}

/// A tool that records how many of its siblings run at the same time.
struct CountingTool {
    name: String,
    current: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl AsyncTool for CountingTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        "Waits 150 ms and records the batch's peak concurrency".to_string()
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        use std::sync::atomic::Ordering;
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(Value::String(self.name.clone()))
    }
}

/// A response that asks for `count` tools in one message.
fn many_call_response(count: usize) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-many".to_string(),
        object: "chat.completion".to_string(),
        created: 0,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: None,
                tool_calls: Some(
                    (0..count)
                        .map(|i| tool_call(&format!("slow_{i}"), &format!("call-{i}")))
                        .collect(),
                ),
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

/// A twelve-call batch overlaps, stays inside the concurrency window, and
/// records its results in the model's order - not completion order.
#[test]
fn a_large_batch_is_bounded_and_keeps_the_models_order() {
    use react::engine::TOOL_BATCH_CONCURRENCY;
    use react::llm::ReactSession;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut builder = ReActEngineBuilder::<TestApp>::new()
            .llm(Box::new(SequencedLlm {
                queued: Mutex::new(vec![many_call_response(12)]),
            }))
            .parallel_tool_calls(true)
            .max_steps(4);
        for i in 0..12 {
            builder = builder.with_tool(ToolVariant::Async(Box::new(CountingTool {
                name: format!("slow_{i}"),
                current: current.clone(),
                peak: peak.clone(),
            })));
        }
        let mut engine = builder.build().expect("the engine builds");
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
            .expect("the batch runs");
        assert_eq!(answer, "done");
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak > 1, "the batch must overlap its waits");
        assert!(
            peak <= TOOL_BATCH_CONCURRENCY,
            "the batch must stay inside the window, peaked at {peak}"
        );
        let ids: Vec<String> = session
            .history()
            .expect("the session keeps its history")
            .iter()
            .filter_map(|message| match message {
                react::llm::LlmMessage::ToolResult { tool_call_id, .. } => {
                    Some(tool_call_id.clone())
                }
                _ => None,
            })
            .collect();
        let expected: Vec<String> = (0..12).map(|i| format!("call-{i}")).collect();
        assert_eq!(ids, expected, "results keep the model's order");
    });
}
