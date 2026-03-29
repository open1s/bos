use std::sync::{Arc};
use std::sync::atomic::{AtomicUsize, Ordering};
use futures::Future;
use std::pin::Pin;
use serde_json::Value;

use react::llm::{Llm, LlmError};
use react::engine::ReActEngine;
use react::calculator_tool::CalculatorTool;
use react::tool::Tool;

// Simple Mock LLM implementing the Llm trait
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

impl Llm for MockLlm {
  fn predict(&self, _prompt: &str) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send>> {
    let responses = self.responses.clone();
    let idx = self.index.clone();
    Box::pin(async move {
      let i = idx.load(Ordering::SeqCst);
      idx.fetch_add(1, Ordering::SeqCst);
      Ok(responses.get(i).cloned().unwrap_or_else(|| "Final Answer: 0".to_string()))
    })
  }
}

#[test]
fn react_engine_basic_flow() {
  // Run in a small tokio runtime
  let rt = tokio::runtime::Runtime::new().unwrap();
  rt.block_on(async {
    // Prepare mock responses: first an Action + Input for calculator, then a Final Answer
    let responses = vec![
      "Action: calculator\nInput: {\"expression\": \"2+3\"}".to_string(),
      "Final Answer: 5".to_string(),
    ];
    let llm = MockLlm::new(responses);
    let mut engine = ReActEngine::new(Box::new(llm), 3);
    engine.register_tool(Box::new(CalculatorTool));
    let result = engine.run("2+3").await.unwrap();
    assert_eq!(result, "5");
  });
}
