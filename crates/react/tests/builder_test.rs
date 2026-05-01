use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};

use react::engine::{BuilderError, ReActEngineBuilder};
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice, FunctionCall, ToolCall};
use react::llm::{LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession, TokenStream};
use react::tool::FnTool;
use react::tool::registry::ToolVariant;
use react::runtime::ReActApp;

#[derive(Default)]
struct TestApp;
impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
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

fn make_tool_call_response(name: &str, args: Value, call_id: &str) -> LlmResponse {
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
                        name: Some(name.to_string()),
                        arguments: Some(args.to_string()),
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

#[test]
fn test_builder_pattern() {
    struct MockLlm {
        responses: Arc<Mutex<Vec<String>>>,
    }
    impl MockLlm {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses: Arc::new(Mutex::new(responses)),
            }
        }
    }

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for MockLlm {
    async fn complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> LlmResponseResult {
            let responses = self.responses.clone();
            let mut lock = responses.lock().unwrap();
            if lock.is_empty() {
                Ok(make_text_response("Final Answer: 5".to_string(), true))
            } else {
                let resp = lock.remove(0);
                if resp.contains("Action:") {
                    Ok(make_tool_call_response(
                        "calculator",
                        serde_json::json!({"expression": "2+3"}),
                        "1",
                    ))
                } else {
                    let is_final = resp.starts_with("Final Answer:");
                    Ok(make_text_response(resp, is_final))
                }
            }
        }

        async fn stream_complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> Result<TokenStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn supports_tools(&self) -> bool {
            true
        }
        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    let mock_llm = MockLlm::new(vec![
        "Action: calculator\nInput: {\"expression\": \"2+3\"}".to_string(),
        "Final Answer: 5".to_string(),
    ]);

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(mock_llm))
        .with_tool(ToolVariant::Sync(Box::new(FnTool {
            name: "calculator".to_string(),
            description: "Calculates expressions".to_string(),
            f: Box::new(|input: &Value| {
                let expr = input
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                if expr == "2+3" {
                    Value::String("5".to_string())
                } else {
                    Value::String("0".to_string())
                }
            }),
        })))
        .max_steps(5)
        .build()
        .expect("Failed to build engine");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let result = rt.block_on(async { engine.react(LlmRequest::new("test-model"), &mut session, &mut context).await });
    assert_eq!(result.unwrap(), "5");
}

#[test]
fn test_builder_missing_llm() {
    let err = ReActEngineBuilder::<TestApp>::new()
        .with_tool(ToolVariant::Sync(Box::new(FnTool {
            name: "dummy".to_string(),
            description: "Dummy tool".to_string(),
            f: Box::new(|_| Value::String("0".to_string())),
        })))
        .build();
    assert!(err.is_err());
    match err {
        Err(BuilderError::MissingLlm) => {}
        _ => panic!("Expected MissingLlm error"),
    }
}

#[test]
fn test_message_log_input() {
    use std::sync::{Arc, Mutex};

    struct MockLlmWithHistory {
        received_inputs: Arc<Mutex<Vec<String>>>,
    }
    impl MockLlmWithHistory {
        fn new() -> Self {
            Self {
                received_inputs: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for MockLlmWithHistory {

        async fn complete(&self, request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> LlmResponseResult {
            self.received_inputs
                .lock()
                .unwrap()
                .push(request.input.clone());
            Ok(make_text_response("Hello back!".to_string(), true))
        }

        async fn stream_complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> Result<TokenStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    let mock = MockLlmWithHistory::new();
    let received = mock.received_inputs.clone();

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(mock))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = "New message".to_string();
    let _result = rt
        .block_on(async { engine.react(request, &mut session, &mut context).await })
        .unwrap();

    let inputs = received.lock().unwrap();
    assert!(!inputs.is_empty());
    assert_eq!(inputs[0], "New message");
}

#[test]
fn test_react_with_request() {
    use react::llm::{LlmContext, LlmMessage};

    struct MockLlmFullRequest {
        received_model: Arc<Mutex<Option<String>>>,
    }
    impl MockLlmFullRequest {
        fn new() -> Self {
            Self {
                received_model: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for MockLlmFullRequest {

        async fn complete(&self, request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> LlmResponseResult {
            *self.received_model.lock().unwrap() = Some(request.model.clone());
            Ok(make_text_response(
                "Answer from custom request".to_string(),
                true,
            ))
        }

        async fn stream_complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> Result<TokenStream, LlmError> {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn supports_tools(&self) -> bool {
            false
        }

        fn provider_name(&self) -> &'static str {
            "mock"
        }
    }

    let mock = MockLlmFullRequest::new();
    let received_model = mock.received_model.clone();

    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(mock))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let mut context = LlmContext::default();
    context.conversations.push(LlmMessage::user("Hello"));
    let mut session = LlmSession::default();
    let request = LlmRequest {
        model: "custom-model".to_string(),
        temperature: Some(0.9),
        input: String::new(),
        top_p: None,
        top_k: None,
        max_tokens: None,
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { engine.react(request, &mut session, &mut context).await })
        .unwrap();

    let model = received_model.lock().unwrap();
    assert_eq!(model.as_ref().unwrap(), "custom-model");
}
