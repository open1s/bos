use async_trait::async_trait;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
    VendorBuilderError,
};

use super::OpenAiVendor;

/// DeepSeek vendor. DeepSeek's API is OpenAI-compatible on both protocols:
/// `/chat/completions` for `ApiMode::Chat` and `/responses` for
/// `ApiMode::Responses`. The HTTP work is delegated to [`OpenAiVendor`] (which
/// already routes `Responses` mode through [`ResponsesTransport`]); this type
/// exists to give DeepSeek its own identity and correct defaults.
pub struct DeepSeekVendor {
    inner: OpenAiVendor,
}

impl Clone for DeepSeekVendor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl DeepSeekVendor {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            inner: OpenAiVendor::new(endpoint, model, api_key),
        }
    }

    pub fn builder() -> DeepSeekVendorBuilder {
        DeepSeekVendorBuilder::new()
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for DeepSeekVendor
{
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        self.inner.complete(persona, req, session, context).await
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        self.inner
            .stream_complete(persona, req, session, context)
            .await
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "deepseek"
    }
}

pub struct DeepSeekVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl DeepSeekVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.deepseek.com".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
        }
    }

    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    pub fn build(self) -> Result<DeepSeekVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(DeepSeekVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for DeepSeekVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}
