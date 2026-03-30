use std::sync::{Arc};
use std::sync::atomic::{AtomicUsize, Ordering};
use async_trait::async_trait;

use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, TokenStream};
use react::engine::ReActEngineBuilder;
use react::calculator_tool::CalculatorTool;

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

#[async_trait]
impl LlmClient for MockLlm {
  async fn complete(&self, _request: LlmRequest) -> LlmResponseResult {
    let responses = self.responses.clone();
    let idx = self.index.clone();
    let i = idx.load(Ordering::SeqCst);
    idx.fetch_add(1, Ordering::SeqCst);
    Ok(LlmResponse::Text(responses.get(i).cloned().unwrap_or_else(|| "Final Answer: 0".to_string())))
  }

  async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
    Ok(Box::pin(futures::stream::empty()))
  }

  fn supports_tools(&self) -> bool { false }
  fn provider_name(&self) -> &'static str { "mock" }
}

#[tokio::test]
async fn test_react_engine_basic() {
  let llm = MockLlm::new(vec![
    "Thought: I need to calculate 2+2\nAction: calculator\nAction Input: 2+2".to_string(),
    "Final Answer: 4".to_string(),
  ]);

  let mut engine = ReActEngineBuilder::new()
    .llm(Box::new(llm))
    .with_tool(Box::new(CalculatorTool))
    .max_steps(2)
    .build()
    .unwrap();

  let result = engine.run("What is 2+2?").await;
  if let Err(e) = &result {
    eprintln!("Error: {:?}", e);
  }
  assert!(result.is_ok());
}

#[tokio::test]
async fn test_react_engine_no_tool() {
  let llm = MockLlm::new(vec!["Final Answer: 42".to_string()]);

  let mut engine = ReActEngineBuilder::new()
    .llm(Box::new(llm))
    .max_steps(1)
    .build()
    .unwrap();

  let result = engine.run("What is the answer?").await;
  assert!(result.is_ok());
}