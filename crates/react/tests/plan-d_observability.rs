use async_trait::async_trait;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession,
    TokenStream,
};
use std::sync::{Arc, Mutex};

fn make_text_response(content: String, is_final: bool) -> LlmResponse {
    LlmResponse::OpenAI(ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            finish_reason: if is_final {
                Some("stop".to_string())
            } else {
                Some("continue".to_string())
            },
            stop_reason: None,
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    })
}

#[allow(dead_code)]
struct MockLlm {
    responses: Arc<Mutex<Vec<String>>>,
}

#[allow(dead_code)]
impl MockLlm {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for MockLlm {
    async fn complete(
        &self,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let responses = self.responses.clone();
        let next = {
            let mut r = responses.lock().unwrap();
            r.remove(0)
        };
        let is_final = next.starts_with("Final Answer:");
        Ok(make_text_response(next, is_final))
    }

    async fn stream_complete(
        &self,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool {
        false
    }
    fn provider_name(&self) -> &'static str {
        "mock"
    }
}
