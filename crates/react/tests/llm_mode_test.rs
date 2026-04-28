use async_trait::async_trait;
use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
use react::llm::{LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession, TokenStream};
use react::runtime::ReActApp;

#[derive(Default)]
struct TestApp;
impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

struct MockLlm;

#[async_trait]
impl LlmClient for MockLlm {
    type SessionType = LlmSession;
    type ContextType = LlmContext;

    async fn complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> LlmResponseResult {
        Ok(LlmResponse::OpenAI(ChatCompletionResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".to_string(),
                    content: Some("Final Answer: test".to_string()),
                    tool_calls: None,
                    function_call: None,
                    reasoning_content: None,
                    extra: serde_json::Value::Object(serde_json::Map::new()),
                },
                finish_reason: Some("stop".to_string()),
                stop_reason: Some(10u32),
                logprobs: None,
            }],
            usage: None,
            system_fingerprint: None,
            nvext: None,
        }))
    }

    async fn stream_complete(&self, _request: LlmRequest, _session: &mut LlmSession, _context: &mut LlmContext) -> Result<TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn supports_tools(&self) -> bool { false }
    fn provider_name(&self) -> &'static str { "mock" }
}

#[tokio::test]
async fn test_engine_with_llm() {
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(MockLlm))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test");
    request.input = "Hi".to_string();
    let result = engine.react(request, &mut session, &mut context).await;
    assert!(result.is_ok());
}