//! Reasoning is turn-local: blocks keep their own provider state inside the
//! turn, and a later turn never receives the previous turn's reasoning.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;

use react::ReActEngineBuilder;
use react::llm::{
    Content, LlmClient, LlmContext, LlmError, LlmMessage, LlmRequest, LlmResponse,
    LlmResponseResult, LlmSession, ReactSession, ReasoningState, StreamToken, TokenStream,
};
use react::runtime::ReActApp;

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

/// A custom session built from nothing but the public trait. Its call log is
/// the evidence that the Engine clears prior reasoning before it records the
/// new turn's input.
#[derive(Clone, Default)]
struct ContractSession {
    history: Vec<LlmMessage>,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl ReactSession for ContractSession {
    fn push(&mut self, msg: LlmMessage) {
        self.calls.lock().unwrap().push("push");
        self.history.push(msg);
    }

    fn history(&self) -> Option<&[LlmMessage]> {
        Some(&self.history)
    }

    fn clear_reasoning(&mut self) {
        self.calls.lock().unwrap().push("clear_reasoning");
        self.history
            .retain(|message| !matches!(message, LlmMessage::AssistantReasoning { .. }));
    }
}

#[derive(Default)]
struct ContractApp;

impl ReActApp for ContractApp {
    type Session = ContractSession;
    type Context = LlmContext;
}

struct ContractRecording {
    seen: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

#[async_trait]
impl LlmClient<ContractSession, LlmContext> for ContractRecording {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut ContractSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Err(LlmError::Other("this fixture streams".to_string()))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        req: LlmRequest,
        _session: &mut ContractSession,
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
async fn a_custom_session_clears_prior_reasoning_before_the_turn_starts() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<ContractApp>::new()
        .llm(Box::new(ContractRecording {
            seen: Arc::clone(&seen),
        }))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut session = ContractSession {
        calls: Arc::clone(&calls),
        ..ContractSession::default()
    };
    session.history.push(LlmMessage::AssistantReasoning {
        content: "earlier chain of thought".to_string(),
        provider_state: Some(signed()),
    });
    session
        .history
        .push(LlmMessage::assistant("earlier answer"));
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

    let log = calls.lock().unwrap();
    assert_eq!(
        &log[..2],
        &["clear_reasoning", "push"],
        "the Engine clears before it records the reader's input"
    );
    let requests = seen.lock().unwrap();
    let first = requests.first().expect("one request");
    assert!(
        !first
            .iter()
            .any(|message| matches!(message, LlmMessage::AssistantReasoning { .. })),
        "the first request of the new turn carries no earlier reasoning: {first:?}"
    );
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

/// The reasoning blocks a message list carries, in order.
fn reasoning_blocks(messages: &[LlmMessage]) -> Vec<(String, Option<ReasoningState>)> {
    messages
        .iter()
        .filter_map(|message| match message {
            LlmMessage::AssistantReasoning {
                content,
                provider_state,
            } => Some((content.clone(), provider_state.clone())),
            _ => None,
        })
        .collect()
}

fn block_state(signature: &str) -> ReasoningState {
    ReasoningState::new(
        "anthropic",
        serde_json::json!({ "type": "thinking", "signature": signature }),
    )
}

/// A blocking response that carries two separate reasoning blocks beside a
/// tool call, then a plain answer. This is the shape the non-streaming
/// Anthropic path produces after decoding several signed blocks.
fn non_streaming_response(with_tool: bool) -> LlmResponse {
    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if with_tool { "calling" } else { "done" },
    });
    if with_tool {
        message["_bos_reasoning_blocks"] = serde_json::json!([
            {
                "content": "alpha",
                "state": {
                    "provider": "anthropic",
                    "payload": { "type": "thinking", "signature": "sig-alpha" }
                }
            },
            {
                "content": "beta",
                "state": {
                    "provider": "anthropic",
                    "payload": { "type": "thinking", "signature": "sig-beta" }
                }
            }
        ]);
        message["tool_calls"] = serde_json::json!([{
            "id": "call-1",
            "type": "function",
            "function": { "name": "echo", "arguments": "{\"n\":1}" }
        }]);
    }
    let body = serde_json::json!({
        "id": "block-response",
        "object": "chat.completion",
        "created": 1,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if with_tool { "tool_calls" } else { "stop" },
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    LlmResponse::OpenAI(serde_json::from_value(body).expect("a valid chat response"))
}

struct NonStreamingBlocks {
    seen: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
    calls: Arc<Mutex<usize>>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for NonStreamingBlocks {
    async fn complete(
        &self,
        _persona: Option<String>,
        req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            let index = *calls;
            *calls += 1;
            index
        };
        if let Some(provider) = &req.provider_request {
            self.seen.lock().unwrap().push(provider.wire_messages());
        }
        Ok(non_streaming_response(index == 0))
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Err(LlmError::Other("this fixture completes".to_string()))
    }
}

#[tokio::test]
async fn a_non_streaming_response_keeps_each_reasoning_block() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(Mutex::new(0));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(NonStreamingBlocks {
            seen: Arc::clone(&seen),
            calls: Arc::clone(&calls),
        }))
        .max_steps(3)
        .build()
        .expect("the engine builds");
    engine.register_tool(Box::new(react::tool::FnTool {
        name: "echo".to_string(),
        description: "echo the argument".to_string(),
        f: Box::new(|input| input.clone()),
    }));
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test-model");
    request.input = Content::text("think");
    let answer = engine
        .react(None, request, &mut session, &mut context)
        .await
        .expect("the turn completes");
    assert_eq!(answer, "done");

    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 2, "the tool call costs one more request");
    assert_eq!(
        reasoning_blocks(&requests[1]),
        vec![
            ("alpha".to_string(), Some(block_state("sig-alpha"))),
            ("beta".to_string(), Some(block_state("sig-beta"))),
        ],
        "the second request replays both blocks, in order, with their own state"
    );
    assert_eq!(
        reasoning_blocks(session.history().expect("history")),
        vec![
            ("alpha".to_string(), Some(block_state("sig-alpha"))),
            ("beta".to_string(), Some(block_state("sig-beta"))),
        ],
        "the session stores one message per block rather than one merged block"
    );
}
