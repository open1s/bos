use std::sync::{Arc, Mutex, Once};
use std::pin::Pin;
use futures::Future;
use serde_json::Value;

use react::llm::{Llm, LlmError};
use react::calculator_tool::CalculatorTool;
use react::{ReActEngine, Memory, ToolRegistry, Action};

// Simple mock LLM for Plan D observability test
struct MockLlm {
  responses: Arc<Mutex<Vec<String>>>,
}

impl MockLlm {
  fn new(responses: Vec<String>) -> Self {
    Self { responses: Arc::new(Mutex::new(responses)) }
  }
}

impl Llm for MockLlm {
  fn predict(&self, _prompt: &str) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send>> {
    let responses = self.responses.clone();
    Box::pin(async move {
      let next = {
        let mut r = responses.lock().unwrap();
        r.remove(0)
      };
      Ok(next)
    })
  }
}

#[test]
fn plan_d_observability_memory_checkpoint() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    let _ = env_logger::builder().is_test(true).try_init();
  });

  // Prepare mock responses: first Action, then Final Answer
  let mock_llm = MockLlm::new(vec![
    "Action: calculator\nInput: {\"expression\": \"2+3\"}".to_string(),
    "Final Answer: 5".to_string(),
  ]);

  let mut registry = ToolRegistry::new();
  registry.insert(Box::new(CalculatorTool));

  let mut engine = ReActEngine::new(Box::new(mock_llm), 3);
  engine.register_tool(Box::new(CalculatorTool));

  // Run the simple plan
  let rt = tokio::runtime::Runtime::new().unwrap();
  rt.block_on(async {
    let res = engine.run("2+3").await;
    assert!(res.is_ok());
    // Persist memory to a temp file to verify Plan D observability persistence
    let path = std::env::temp_dir().join("plan-d-observability-memory.json");
    engine.save_memory_checkpoint(path.to_str().unwrap()).unwrap();
    let contents = std::fs::read_to_string(path).unwrap();
    // Expect a JSON array with memory records
    assert!(contents.trim().starts_with("["));
  });
}
