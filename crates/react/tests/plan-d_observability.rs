use async_trait::async_trait;
use react::calculator_tool::CalculatorTool;
use react::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, TokenStream};
use react::ReActEngine;
use std::sync::{Arc, Mutex, Once};

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
        let next = {
            let mut r = responses.lock().unwrap();
            r.remove(0)
        };
        Ok(LlmResponse::Text(next))
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
