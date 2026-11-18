//! A provider's opaque reasoning state is stored beside the raw reasoning, not
//! merged into assistant text.

use async_trait::async_trait;
use futures::StreamExt;

use react::llm::{
    Content, LlmClient, LlmContext, LlmError, LlmMessage, LlmRequest, LlmResponseResult,
    LlmSession, ReactSession, ReasoningState, StreamToken, TokenStream,
};
use react::runtime::ReActApp;
use react::ReActEngineBuilder;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

struct StatefulReasoning;

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for StatefulReasoning {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Err(LlmError::Other("this fixture streams".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        let state = ReasoningState::new(
            "anthropic",
            serde_json::json!({ "type": "thinking", "signature": "sig-1" }),
        );
        let tokens: Vec<Result<StreamToken, LlmError>> = vec![
            Ok(StreamToken::ReasoningContent("raw thought".to_string())),
            Ok(StreamToken::ReasoningState(state)),
            Ok(StreamToken::Text("done".to_string())),
            Ok(StreamToken::Done),
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

#[tokio::test]
async fn reasoning_state_is_stored_with_the_reasoning_message() {
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(StatefulReasoning))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test-model");
    request.input = Content::text("think");
    {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("stream item");
        }
    }

    let messages = session.history().expect("history");
    let reasoning = messages.iter().find_map(|message| match message {
        LlmMessage::AssistantReasoning {
            content,
            provider_state,
        } => Some((content, provider_state)),
        _ => None,
    });
    let (content, state) = reasoning.expect("the reasoning message exists");
    assert_eq!(content, "raw thought");
    let state = state.as_ref().expect("the opaque state is preserved");
    assert_eq!(state.provider, "anthropic");
    assert_eq!(state.payload["signature"], "sig-1");
    assert!(
        messages.iter().any(
            |message| matches!(message, LlmMessage::Assistant { content } if content == "done")
        ),
        "the visible answer stays a separate assistant message"
    );
}
