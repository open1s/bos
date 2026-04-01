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
                Ok(LlmResponse::Text("Final Answer: 0".to_string()))
            } else {
                Ok(LlmResponse::Text(lock.remove(0)))
            }
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
    let result = rt.block_on(async { engine.run("2+3").await });
    assert_eq!(result.unwrap(), "5");
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
