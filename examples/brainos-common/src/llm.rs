use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use agent::{
    LlmClient, LlmRequest, LlmResponse, StreamToken,
    llm::OpenAiClient,
};

/// Create LLM client from environment variables
/// 
/// Falls back to mock client if OPENAI_API_KEY is not set
pub fn create_llm_client() -> Arc<dyn LlmClient> {
    let api_key = std::env::var("OPENAI_API_KEY").ok();
    let base_url = std::env::var("OPENAI_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    
    if let Some(key) = api_key {
        tracing::info!("Using real LLM client with base_url: {}", base_url);
        Arc::new(OpenAiClient::new(base_url, key))
    } else {
        tracing::info!("No OPENAI_API_KEY set, using mock LLM client");
        Arc::new(MockLlmClient::new(vec![
            LlmResponse::Text("Hello! I'm here to help.".to_string()),
            LlmResponse::Done,
        ]))
    }
}

pub struct MockLlmClient {
    pub responses: Vec<LlmResponse>,
}

impl MockLlmClient {
    pub fn new(responses: Vec<LlmResponse>) -> Self {
        Self { responses }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _req: LlmRequest) -> Result<LlmResponse, agent::LlmError> {
        Ok(self.responses.clone().into_iter().next().unwrap_or(LlmResponse::Done))
    }

    fn stream_complete(
        &self,
        _req: LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, agent::LlmError>> + Send + '_>> {
        let responses = self.responses.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        tokio::spawn(async move {
            for response in responses {
                let token = match response {
                    LlmResponse::Text(s) => StreamToken::Text(s),
                    LlmResponse::ToolCall { name, args } => StreamToken::ToolCall { name, args },
                    LlmResponse::Done => StreamToken::Done,
                };
                let _ = tx.send(Ok(token)).await;
            }
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}
