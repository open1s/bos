use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, Mutex};

use react::engine::{BuilderError, ReActEngineBuilder};
use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, TokenStream};
use react::tool::FnTool;

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
    impl LlmClient for MockLlm {
        async fn complete(&self, _request: LlmRequest) -> LlmResponseResult {
            let responses = self.responses.clone();
            let mut lock = responses.lock().unwrap();
            if lock.is_empty() {
                Ok(LlmResponse::Text("Final Answer: 5".to_string()))
            } else {
                let resp = lock.remove(0);
                if resp.contains("Action:") {
                    Ok(LlmResponse::ToolCall {
                        name: "calculator".to_string(),
                        args: serde_json::json!({"expression": "2+3"}),
                        id: Some("1".to_string()),
                    })
                } else {
                    Ok(LlmResponse::Text(resp))
                }
            }
        }

        async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
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

    let mut engine = ReActEngineBuilder::new()
        .llm(Box::new(mock_llm))
        .with_tool(Box::new(FnTool {
            name: "calculator".to_string(),
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
        }))
        .max_steps(5)
        .build()
        .expect("Failed to build engine");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { engine.react("2+3").await });
    assert_eq!(result.unwrap().0, "5");
}

#[test]
fn test_builder_missing_llm() {
    let err = ReActEngineBuilder::new()
        .with_tool(Box::new(FnTool {
            name: "dummy".to_string(),
            f: Box::new(|_| Value::String("0".to_string())),
        }))
        .build();
    assert!(err.is_err());
    match err {
        Err(BuilderError::MissingLlm) => {}
        _ => panic!("Expected MissingLlm error"),
    }
}

#[test]
fn test_message_log_input() {
    use react::llm::LlmMessage;
    use std::sync::{Arc, Mutex};

    struct MockLlmWithHistory {
        received_conversations: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    }
    impl MockLlmWithHistory {
        fn new() -> Self {
            Self {
                received_conversations: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlmWithHistory {
        async fn complete(&self, request: LlmRequest) -> LlmResponseResult {
            self.received_conversations
                .lock()
                .unwrap()
                .push(request.context.conversations.clone());
            Ok(LlmResponse::Text("Hello back!".to_string()))
        }

        async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
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
    let received = mock.received_conversations.clone();

    let mut engine = ReActEngineBuilder::new()
        .llm(Box::new(mock))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    engine.set_input_messages(vec![
        LlmMessage::user("Previous conversation"),
        LlmMessage::assistant("I remember that"),
    ]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_result, _context) = rt
        .block_on(async { engine.react("New message").await })
        .unwrap();

    let convos = received.lock().unwrap();
    assert!(!convos.is_empty());
    let first_convo = &convos[0];
    assert!(
        first_convo.len() >= 3,
        "Should have system + history + new user"
    );
    assert!(
        matches!(first_convo[1], LlmMessage::User { content: ref c } if c == "Previous conversation")
    );
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
    impl LlmClient for MockLlmFullRequest {
        async fn complete(&self, request: LlmRequest) -> LlmResponseResult {
            *self.received_model.lock().unwrap() = Some(request.model.clone());
            Ok(LlmResponse::Text("Answer from custom request".to_string()))
        }

        async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
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

    let mut engine = ReActEngineBuilder::new()
        .llm(Box::new(mock))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let mut context = LlmContext::default();
    context.conversations.push(LlmMessage::user("Hello"));
    let request = LlmRequest {
        model: "custom-model".to_string(),
        context,
        temperature: Some(0.9),
        ..Default::default()
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async { engine.react_with_request(request).await })
        .unwrap();

    let model = received_model.lock().unwrap();
    assert_eq!(model.as_ref().unwrap(), "custom-model");
}
