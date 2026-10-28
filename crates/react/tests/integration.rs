use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession,
    TokenStream,
};
use react::runtime::ReActApp;
use react::tool::registry::ToolVariant;
use react::tool::{Tool, ToolError};
use serde_json::Value;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

struct TestCalculator;

impl Tool for TestCalculator {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> String {
        "Calculate math expression".to_string()
    }

    fn category(&self) -> String {
        "utility".to_string()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let expr = input
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let result: i32 = expr.parse().unwrap_or(0);
        Ok(Value::String(result.to_string()))
    }
}

struct MockLlm {
    responses: Arc<Vec<String>>,
    index: Arc<AtomicUsize>,
}

impl MockLlm {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Arc::new(responses),
            index: Arc::new(AtomicUsize::new(0)),
        }
    }
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

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for MockLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let responses = self.responses.clone();
        let idx = self.index.clone();
        let i = idx.load(Ordering::SeqCst);
        idx.fetch_add(1, Ordering::SeqCst);
        let text = responses
            .get(i)
            .cloned()
            .unwrap_or_else(|| "Final Answer: 0".to_string());
        let is_final = text.starts_with("Final Answer:");
        Ok(make_text_response(text, is_final))
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
        false
    }
    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

#[tokio::test]
async fn test_react_engine_basic() {
    let llm = MockLlm::new(vec![
        "Thought: I need to calculate 2+2\nAction: calculator\nAction Input: 2+2".to_string(),
        "Final Answer: 4".to_string(),
    ]);

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .with_tool(ToolVariant::Sync(Box::new(TestCalculator)))
        .max_steps(2)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("What is 2+2?");
    let result = engine
        .react(None, request, &mut session, &mut context)
        .await;
    if let Err(e) = &result {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());
}

/// An OpenAI-compatible final answer must land in the session once; the
/// engine used to record the content and then the final answer a second time,
/// which surfaced as two assistant rows in a subagent transcript.
#[tokio::test]
async fn a_final_answer_is_recorded_once() {
    let llm = MockLlm::new(vec!["Why did the chicken cross the road?".to_string()]);

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .max_steps(1)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("joke");
    request.input = react::llm::Content::text("Tell one joke");
    let answer = engine
        .react(None, request, &mut session, &mut context)
        .await
        .expect("the turn completes");
    assert_eq!(answer, "Why did the chicken cross the road?");
    let assistants = session
        .history
        .iter()
        .filter(|message| matches!(message, react::llm::LlmMessage::Assistant { .. }))
        .count();
    assert_eq!(assistants, 1, "the final answer is recorded once");
}

#[tokio::test]
async fn test_react_engine_no_tool() {
    let llm = MockLlm::new(vec!["Final Answer: 42".to_string()]);

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .max_steps(1)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("What is the answer?");
    let result = engine
        .react(None, request, &mut session, &mut context)
        .await;
    assert!(result.is_ok());
}

/// Drives `react_stream` with native tool-call tokens (like a real provider
/// stream) to prove the loop yields `StreamToken::ToolResult` — output plus
/// duration — right after the `ToolCall` it answers.
struct StreamingToolLlm {
    index: Arc<AtomicUsize>,
}

impl StreamingToolLlm {
    fn new() -> Self {
        Self {
            index: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for StreamingToolLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Ok(make_text_response("Final Answer: 4".to_string(), true))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        let i = self.index.fetch_add(1, Ordering::SeqCst);
        let tokens: Vec<Result<react::llm::StreamToken, LlmError>> = if i == 0 {
            vec![
                Ok(react::llm::StreamToken::Text("Need a calculator.\n".into())),
                Ok(react::llm::StreamToken::ToolCall {
                    name: "calculator".into(),
                    args: serde_json::json!({ "expression": "2+2" }),
                    id: Some("call_1".into()),
                }),
            ]
        } else {
            vec![
                Ok(react::llm::StreamToken::Text("Final Answer: 4".into())),
                Ok(react::llm::StreamToken::Done),
            ]
        };
        Ok(Box::pin(futures::stream::iter(tokens)))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "mock-stream"
    }
}

#[tokio::test]
async fn test_react_stream_yields_tool_result() {
    use futures::StreamExt;
    use react::llm::StreamToken;

    let llm = StreamingToolLlm::new();
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .with_tool(ToolVariant::Sync(Box::new(TestCalculator)))
        .max_steps(3)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("What is 2+2?");

    let stream = engine.react_stream(None, request, &mut session, &mut context);
    futures::pin_mut!(stream);

    let mut saw_call = false;
    let mut saw_result = false;
    let mut done = false;
    while let Some(item) = stream.next().await {
        match item.expect("stream item") {
            StreamToken::ToolCall { name, .. } => {
                assert_eq!(name, "calculator");
                assert!(!saw_call, "only one tool call expected");
                saw_call = true;
            }
            StreamToken::ToolResult {
                name,
                output,
                ms: _,
            } => {
                assert!(saw_call, "ToolResult must arrive after its ToolCall");
                assert_eq!(name, "calculator", "result pairs with its call by name");
                assert!(!output.is_empty(), "result carries the tool output");
                assert!(!saw_result, "only one tool result expected");
                saw_result = true;
            }
            StreamToken::Done => {
                done = true;
            }
            _ => {}
        }
    }
    assert!(done, "stream must finish with Done");
    assert!(saw_call, "tool call observed");
    assert!(saw_result, "tool result observed");
}

/// A stream client that never stops asking for a tool, so the only thing that
/// can finish the loop is the engine's own step bound.
struct LoopingStreamLlm {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for LoopingStreamLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Ok(make_text_response("Final Answer: loop".to_string(), true))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let tokens: Vec<Result<react::llm::StreamToken, LlmError>> = vec![
            Ok(react::llm::StreamToken::Text("again\n".into())),
            Ok(react::llm::StreamToken::ToolCall {
                name: "calculator".into(),
                args: serde_json::json!({ "expression": "1+1" }),
                id: Some("call_loop".into()),
            }),
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "looping-stream"
    }
}

/// The guard is a bound on WORK, not on the turn: a client that never stops
/// asking for a tool still ends with the model's own words. Before this, the
/// loop yielded `Done` at the bound and the turn simply stopped mid-task.
#[tokio::test]
async fn react_stream_closes_a_guarded_turn_with_an_answer() {
    use futures::StreamExt;
    use react::llm::StreamToken;

    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(LoopingStreamLlm {
            calls: Arc::clone(&calls),
        }))
        .with_tool(ToolVariant::Sync(Box::new(TestCalculator)))
        .max_steps(2)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("loop forever?");

    let mut done = false;
    {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            if let StreamToken::Done = item.expect("stream item") {
                done = true;
            }
        }
    }

    assert!(done, "the guarded stream still terminates with Done");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "two working steps plus one closing turn"
    );
    assert_eq!(
        engine.tool_call_count(),
        2,
        "the closing turn's tool call is not run, so the guard costs an answer, not another step"
    );
}

/// A non-streaming client that always asks for the same tool, so the only thing
/// that can finish the turn is the engine's guard.
struct LoopingToolLlm {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for LoopingToolLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LlmResponse::OpenAI(ChatCompletionResponse {
            id: "looping".to_string(),
            object: "chat.completion".to_string(),
            created: 1234567890,
            model: "looping-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some("looking again".to_string()),
                    tool_calls: Some(vec![ToolCall {
                        id: "call_loop".to_string(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: Some("calculator".to_string()),
                            arguments: Some(r#"{"expression":"1+1"}"#.to_string()),
                        },
                    }]),
                    function_call: None,
                    reasoning_content: None,
                    extra: Value::Object(serde_json::Map::new()),
                },
                finish_reason: Some("tool_calls".to_string()),
                stop_reason: None,
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
            nvext: None,
        }))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Err(LlmError::Other(
            "this client only answers `complete`".to_string(),
        ))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "looping"
    }
}

/// The same contract on the non-streaming path: the guarded turn returns the
/// closing response's text instead of an empty thought.
#[tokio::test]
async fn a_guarded_turn_ends_with_an_answer_not_a_truncation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(LoopingToolLlm {
            calls: Arc::clone(&calls),
        }))
        .with_tool(ToolVariant::Sync(Box::new(TestCalculator)))
        .max_steps(2)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("loop forever?");
    let answer = engine
        .react(None, request, &mut session, &mut context)
        .await
        .expect("the guarded turn completes");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "two working steps plus one closing turn"
    );
    assert_eq!(
        engine.tool_call_count(),
        2,
        "the closing turn's tool call is not run"
    );
    assert_eq!(
        answer, "looking again",
        "the closing response is the turn's answer, not an empty thought"
    );
}

/// A reasoning model can spend a whole step and stream neither text nor a call.
/// The turn must not close on silence: the engine asks once, and the next reply
/// is the answer.
struct BlankThenAnswerLlm {
    index: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for BlankThenAnswerLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Ok(make_text_response(String::new(), true))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        let i = self.index.fetch_add(1, Ordering::SeqCst);
        let tokens: Vec<Result<react::llm::StreamToken, LlmError>> = if i == 0 {
            vec![Ok(react::llm::StreamToken::Done)]
        } else {
            vec![
                Ok(react::llm::StreamToken::Text("4".into())),
                Ok(react::llm::StreamToken::Done),
            ]
        };
        Ok(Box::pin(futures::stream::iter(tokens)))
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "blank-then-answer"
    }
}

#[tokio::test]
async fn a_blank_stream_step_is_retried_instead_of_ending_on_silence() {
    use futures::StreamExt;
    use react::llm::LlmMessage;

    let llm = BlankThenAnswerLlm {
        index: Arc::new(AtomicUsize::new(0)),
    };
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .max_steps(3)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("what is 2+2?");

    let (answer, done) = {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        let mut answer = String::new();
        let mut done = false;
        while let Some(item) = stream.next().await {
            match item.expect("stream item") {
                react::llm::StreamToken::Text(text) => answer.push_str(&text),
                react::llm::StreamToken::Done => done = true,
                _ => {}
            }
        }
        (answer, done)
    };

    assert!(done, "the turn still closes");
    assert_eq!(answer, "4", "the retry produced the answer, not silence");
    assert!(
        session.history.iter().any(|message| matches!(
            message,
            LlmMessage::User { content }
                if content.as_text() == Some(react::engine::EMPTY_ANSWER_NOTE)
        )),
        "the retry directive is a harness note: {:?}",
        session.history
    );
}

/// The words that introduced a tool call are the model's own context. Dropping
/// them made the next step rebuild a thought it had already had.
#[tokio::test]
async fn the_preamble_before_a_tool_call_stays_in_the_session() {
    use futures::StreamExt;
    use react::llm::LlmMessage;

    let llm = StreamingToolLlm::new();
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(llm))
        .with_tool(ToolVariant::Sync(Box::new(TestCalculator)))
        .max_steps(3)
        .build()
        .unwrap();

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = react::llm::Content::text("What is 2+2?");

    {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("stream item");
        }
    }

    let messages = &session.history;
    let preamble_at = messages
        .iter()
        .position(|message| {
            matches!(message, LlmMessage::Assistant { content } if content.contains("Need a calculator"))
        })
        .expect("the preamble is recorded");
    let call_at = messages
        .iter()
        .position(|message| matches!(message, LlmMessage::AssistantToolCall { .. }))
        .expect("the call is recorded");
    assert!(
        preamble_at < call_at,
        "the model's words come before its call: {messages:?}"
    );
}
