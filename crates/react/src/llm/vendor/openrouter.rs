use async_trait::async_trait;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
    VendorBuilderError,
};

use super::OpenAiVendor;

/// OpenRouter vendor.
///
/// OpenRouter exposes an OpenAI-compatible Chat Completions endpoint, so the HTTP
/// and streaming work is delegated to OpenAiVendor. This type only supplies
/// OpenRouter's identity and default endpoint/model.
pub struct OpenRouterVendor {
    inner: OpenAiVendor,
}

impl Clone for OpenRouterVendor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl OpenRouterVendor {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            inner: OpenAiVendor::new(endpoint, model, api_key),
        }
    }

    pub fn builder() -> OpenRouterVendorBuilder {
        OpenRouterVendorBuilder::new()
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for OpenRouterVendor
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
        "openrouter"
    }
}

pub struct OpenRouterVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl OpenRouterVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/v1".to_string(),
            model: "anthropic/claude-3.5-sonnet".to_string(),
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

    pub fn build(self) -> Result<OpenRouterVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(OpenRouterVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for OpenRouterVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::vendor::OpenRouterVendor;
    use crate::llm::{Content, LlmClient, LlmContext, LlmRequest, LlmSession};

    #[tokio::test]
    async fn test_openrouter_vendor() {
        let mut section = Section::default();
        let result = section.init();

        let _config = match result.await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Skipping test (no config): {}", e);
                return;
            }
        };

        #[derive(Debug, Deserialize, Clone)]
        struct LlmConfig {
            model: String,
            base_url: String,
            api_key: String,
        }

        let llm_config: LlmConfig = match section.extract("global_model") {
            Some(c) => c,
            None => {
                eprintln!("Skipping test (no global_model config)");
                return;
            }
        };

        let model_name = if llm_config.model.starts_with("openrouter/") {
            llm_config
                .model
                .strip_prefix("openrouter/")
                .unwrap()
                .to_string()
        } else if llm_config.model.contains('/') {
            llm_config.model.clone()
        } else {
            eprintln!("Skipping test: model should contain provider/model for OpenRouter");
            return;
        };

        let vendor = OpenRouterVendor::new(
            llm_config.base_url.clone(),
            model_name,
            llm_config.api_key.clone(),
        );

        let request = LlmRequest {
            model: llm_config.model.clone(),
            input: Content::text("What is 2+2?"),
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            reasoning_effort: None,
            api_mode: crate::llm::ApiMode::Chat,
        };
        let outcome = vendor
            .complete(
                None,
                request,
                &mut LlmSession::new(),
                &mut LlmContext::default(),
            )
            .await;

        if let Err(e) = outcome {
            let err_str = format!("{:?}", e);
            if err_str.contains("404") || err_str.contains("not found") {
                eprintln!(
                    "OpenRouter endpoint/model not available (404), skipping test: {}",
                    err_str
                );
                return;
            }
            if err_str.contains("429") || err_str.contains("rate limit") {
                eprintln!("OpenRouter rate limited, skipping test: {}", err_str);
                return;
            }
            panic!("OpenRouter request failed: {:?}", e);
        }

        println!("{:?}", outcome);
    }
}
