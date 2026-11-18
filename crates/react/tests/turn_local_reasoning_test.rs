//! Reasoning is turn-local: blocks keep their own provider state inside the
//! turn, and a later turn never receives the previous turn's reasoning.

use std::sync::{Arc, Mutex};

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

fn redacted() -> ReasoningState {
    ReasoningState::new(
        "anthropic",
        serde_json::json!({ "type": "redacted_thinking", "data": "opaque" }),
    )
}

fn signed() -> ReasoningState {
    ReasoningState::new(
        "anthropic",
        serde_json::json!({ "type": "thinking", "signature": "sig-2" }),
    )
}

/// Emits a redacted block (state only) followed by a signed block (text then
/// state), the order Anthropic uses for interleaved thinking.
struct TwoBlocks;

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for TwoBlocks {
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
        let tokens: Vec<Result<StreamToken, LlmError>> = vec![
            Ok(StreamToken::ReasoningState(redacted())),
            Ok(StreamToken::ReasoningContent("signed text".to_string())),
            Ok(StreamToken::ReasoningState(signed())),
            Ok(StreamToken::Text("done".to_string())),
            Ok(StreamToken::Done),
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

#[tokio::test]
async fn each_reasoning_block_keeps_its_own_state() {
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(TwoBlocks))
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

    let blocks: Vec<(String, Option<ReasoningState>)> = session
        .history()
        .expect("history")
        .iter()
        .filter_map(|message| match message {
            LlmMessage::AssistantReasoning {
                content,
                provider_state,
            } => Some((content.clone(), provider_state.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(
        blocks.len(),
        2,
        "one message per provider block: {blocks:?}"
    );
    assert_eq!(blocks[0].0, "", "redacted thinking has no text");
    assert_eq!(
        blocks[0]
            .1
            .as_ref()
            .map(|state| state.payload["type"].clone()),
        Some(serde_json::json!("redacted_thinking")),
        "the redacted state is preserved on its own block"
    );
    assert_eq!(blocks[1].0, "signed text");
    assert_eq!(
        blocks[1]
            .1
            .as_ref()
            .map(|state| state.payload["signature"].clone()),
        Some(serde_json::json!("sig-2")),
        "the signed block keeps its own signature"
    );
}

/// Records the messages one request serialized.
struct Recording {
    seen: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for Recording {
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
        req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        if let Some(provider) = &req.provider_request {
            self.seen.lock().unwrap().push(provider.wire_messages());
        }
        let tokens: Vec<Result<StreamToken, LlmError>> = vec![
            Ok(StreamToken::Text("answer".to_string())),
            Ok(StreamToken::Done),
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

#[tokio::test]
async fn a_new_turn_does_not_send_the_previous_turns_reasoning() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(Recording {
            seen: Arc::clone(&seen),
        }))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
    session.push(LlmMessage::AssistantReasoning {
        content: "previous turn chain of thought".to_string(),
        provider_state: Some(signed()),
    });
    session.push(LlmMessage::assistant("previous answer"));
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test-model");
    request.input = Content::text("next");
    {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("stream item");
        }
    }

    let requests = seen.lock().unwrap();
    let first = requests.first().expect("one request");
    assert!(
        !first
            .iter()
            .any(|message| matches!(message, LlmMessage::AssistantReasoning { .. })),
        "a new turn must not serialize earlier reasoning: {first:?}"
    );
    assert!(
        !session
            .history()
            .unwrap()
            .iter()
            .any(|message| matches!(message, LlmMessage::AssistantReasoning { .. })),
        "the session drops the earlier reasoning before the turn"
    );
}
