use std::sync::{Arc, Mutex, Once};
use async_trait::async_trait;
use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, TokenStream};
use react::calculator_tool::CalculatorTool;
use react::ReActEngine;

struct MockLlm {
  responses: Arc<Mutex<Vec<String>>>,
}

impl MockLlm {
  fn new(responses: Vec<String>) -> Self {
    Self { responses: Arc::new(Mutex::new(responses)) }
  }
}

#[async_trait]
impl LlmClient for MockLlm {
  async fn complete(&self, _request: LlmRequest) -> LlmResponseResult {
    let responses = self.responses.clone();
    let next = {
      let mut r = responses.lock().unwrap();
      r.remove(0)
    };
    Ok(LlmResponse::Text(next))
  }

  async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
    Ok(Box::pin(futures::stream::empty()))
  }

  fn supports_tools(&self) -> bool { false }
  fn provider_name(&self) -> &'static str { "mock" }
}

#[test]
fn plan_d_observability_memory_checkpoint() {
  static INIT: Once = Once::new();
  INIT.call_once(|| {
    let _ = env_logger::builder().is_test(true).try_init();
  });

  let mock_llm = MockLlm::new(vec![
    "Action: calculator\nInput: {\"expression\": \"2+3\"}".to_string(),
    "Final Answer: 5".to_string(),
  ]);

  let mut engine = ReActEngine::new(Box::new(mock_llm), 3);
  engine.register_tool(Box::new(CalculatorTool));

  let rt = tokio::runtime::Runtime::new().unwrap();
  rt.block_on(async {
    let res = engine.run("2+3").await;
    assert!(res.is_ok());
    let path = std::env::temp_dir().join("plan-d-observability-memory.json");
    engine.save_memory_checkpoint(path.to_str().unwrap()).unwrap();
    let contents = std::fs::read_to_string(path).unwrap();
    assert!(contents.trim().starts_with("["));
  });
}