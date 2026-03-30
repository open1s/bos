use crate::llm::{LlmClient, LlmError, LlmRequest, LlmResponse, LlmResponseResult, TokenStream};
use async_trait::async_trait;

pub struct MockVendor {
    pub name: String,
    pub reply: String,
}

impl MockVendor {
    pub fn new(name: String, reply: String) -> Self {
        Self { name, reply }
    }
}

#[async_trait]
impl LlmClient for MockVendor {
    async fn complete(&self, _request: LlmRequest) -> LlmResponseResult {
        Ok(LlmResponse::Text(self.reply.clone()))
    }

    async fn stream_complete(&self, _request: LlmRequest) -> Result<TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn provider_name(&self) -> &'static str {
        "mock_vendor"
    }

    fn supports_tools(&self) -> bool {
        false
    }
}
