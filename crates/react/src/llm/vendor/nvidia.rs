use async_trait::async_trait;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
    VendorBuilderError,
};

use super::OpenAiVendor;

/// NVIDIA NIM vendor.
///
/// NVIDIA's NIM endpoints speak the OpenAI Chat Completions protocol (including
/// top_p / top_k sampling controls), so the HTTP and streaming work is delegated
/// to OpenAiVendor. This type only supplies NVIDIA's identity and default
/// endpoint/model; adding another compatible provider is a config change, not a
/// copy of the transport.
pub struct NvidiaVendor {
    inner: OpenAiVendor,
}

impl Clone for NvidiaVendor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl NvidiaVendor {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            inner: OpenAiVendor::new(endpoint, model, api_key),
        }
    }

    pub fn builder() -> NvidiaVendorBuilder {
        NvidiaVendorBuilder::new()
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for NvidiaVendor
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
        "nvidia"
    }
}

pub struct NvidiaVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl NvidiaVendorBuilder {
    pub fn new() -> Self {
        Self {
            endpoint: "https://integrate.api.nvidia.com/v1".to_string(),
            model: "mistralai/mixtral-8x7b-instruct-v0.1".to_string(),
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

    pub fn build(self) -> Result<NvidiaVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(NvidiaVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for NvidiaVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use config::Section;
    use serde::Deserialize;

    use crate::llm::{Content, LlmClient, LlmRequest, LlmSession};
    use crate::{
        llm::vendor::{NvidiaVendor, OpenAIExtractor},
        JsonExtractor, StreamExtractor,
    };

    #[test]
    fn test() {
        let mut extractor = OpenAIExtractor::new(JsonExtractor::default());

        let chunk = r#"data: {"id":"chatcmpl-958091ac43bbd265","object":"chat.completion.chunk","created":1777006903,"model":"meta/llama-4-maverick-17b-128e-instruct","choices":[{"index":0,"delta":{"content":"name","reasoning_content":null},"logprobs":null,"finish_reason":null,"token_ids":null}]}"#;

        let spans = extractor.push(chunk);

        //add tool call chunk
        let chunk2 = r#"data: {"id":"chatcmpl-958091ac43bbd265","object":"chat.completion.chunk","created":1777006903,"model":"meta/llama-4-maverick-17b-128e-instruct","choices":[{"index":0,"delta":{"tool_calls":[{"id":"toolcall-123","type":"function","function":{"name":"get_current_weather","arguments":"{\"location\": \"San Francisco, CA\", \"unit\": \"celsius\"}"}}]},"logprobs":null,"finish_reason":null,"token_ids":null}]}"#;

        let spans2 = extractor.push(chunk2);

        assert!(spans.is_some());
        assert!(spans2.is_some());
        println!("Extracted spans: {:?}", spans);
        println!("Extracted spans2: {:?}", spans2);
    }

    #[tokio::test]
    async fn test_vendor() {
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

        let model_name = if llm_config.model.starts_with("nvidia/") {
            llm_config
                .model
                .strip_prefix("nvidia/")
                .unwrap()
                .to_string()
        } else {
            llm_config.model.clone()
        };

        let vendor = NvidiaVendor::new(
            llm_config.base_url.clone(),
            model_name,
            llm_config.api_key.clone(),
        );

        let request = LlmRequest {
            model: llm_config.model.clone(),
            input: Content::text("What is 2+2?, must use add tool"),
            temperature: None,
            max_tokens: None,
            top_p: None,
            top_k: None,
            reasoning_effort: None,
            api_mode: crate::llm::ApiMode::Chat,
        };
        let result = match vendor
            .complete(None, request, &mut LlmSession::new(), &mut ())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("404") || err_str.contains("not found") {
                    eprintln!(
                        "NVIDIA endpoint/model not available (404), skipping test: {}",
                        err_str
                    );
                    return;
                }
                if err_str.contains("429") || err_str.contains("rate limit") {
                    eprintln!("NVIDIA rate limited, skipping test: {}", err_str);
                    return;
                }
                panic!("NVIDIA request failed: {:?}", e);
            }
        };

        println!("{:?}", result);
    }
}
