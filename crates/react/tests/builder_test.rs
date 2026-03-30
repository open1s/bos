use std::sync::{Arc, Mutex};
use std::pin::Pin;
use futures::Future;
use serde_json::{json, Value};

use react::llm::{Llm, LlmError};
use react::engine::{ReActEngineBuilder, BuilderError};
use react::tool::{Tool, ToolError};
use react::tool::FnTool;

#[test]
fn test_builder_pattern() {
    // Simple mock LLM that returns a fixed action then final answer
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
    impl Llm for MockLlm {
        fn predict(&self, _prompt: &str) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send>> {
            let responses = self.responses.clone();
            Box::pin(async move {
                let mut lock = responses.lock().unwrap();
                if lock.is_empty() {
                    Ok("Final Answer: 0".to_string())
                } else {
                    let resp = lock.remove(0);
                    Ok(resp)
                }
            })
        }
    }

    let mock_llm = MockLlm::new(vec![
        "Action: calculator\nInput: {\"expression\": \"2+3\"}".to_string(),
        "Final Answer: 5".to_string(),
    ]);

    // Build engine using builder
    let mut engine = ReActEngineBuilder::new()
        .llm(Box::new(mock_llm))
        .with_tool(Box::new(FnTool {
            name: "calculator".to_string(),
            f: Box::new(|input: &Value| {
                let expr = input.get("expression").and_then(|v| v.as_str()).unwrap_or("0");
                // Simple eval for 2+3 only for demo
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

    // Use a tokio runtime to run the async method
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