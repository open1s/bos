use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
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
    request.input = "What is 2+2?".to_string();
    let result = engine.react(request, &mut session, &mut context).await;
    if let Err(e) = &result {
        eprintln!("Error: {:?}", e);
    }
    assert!(result.is_ok());
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
    request.input = "What is the answer?".to_string();
    let result = engine.react(request, &mut session, &mut context).await;
    assert!(result.is_ok());
}
