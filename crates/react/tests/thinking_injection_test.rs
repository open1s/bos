//! Thinking summaries reach the next request as host guidance, not as history.
//!
//! REQ-2 and REQ-3 are end-to-end here: a reasoning step states a decision and
//! calls a tool, the tool result either corroborates or retires that decision,
//! and the following provider request carries a compact note with an explicit
//! source marker. The note never appears in the message list.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use react::llm::{
    Content, HostGuidance, LlmClient, LlmContext, LlmError, LlmMessage, LlmRequest,
    LlmResponseResult, LlmSession, ReactSession, StreamToken, TokenStream,
};
use react::runtime::ReActApp;
use react::thinking::{
    ThinkingConfidence, ThinkingDecision, ThinkingOrigin, ThinkingPhase, ThinkingSnapshot,
    THINKING_SOURCE,
};
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::ToolError;
use react::ReActEngineBuilder;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

/// What one provider call saw.
#[derive(Clone)]
struct Observed {
    guidance: Vec<HostGuidance>,
    system_prompt: Option<String>,
    messages: Vec<LlmMessage>,
}

/// A fake shell whose answer is chosen by the fixture, not by the model.
struct FakeBash {
    fail: bool,
}

#[async_trait]
impl AsyncTool for FakeBash {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "Run a fake test command".to_string()
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        if self.fail {
            Ok(serde_json::json!({
                "success": false,
                "output": "no test named parser"
            }))
        } else {
            Ok(serde_json::json!({
                "success": true,
                "output": "grep found the bug in parser.rs; test result: ok. 4 passed; 0 failed"
            }))
        }
    }
}

/// Streams one reasoning-plus-tool step, then a plain answer.
struct ReasoningThenAnswer {
    calls: Arc<Mutex<Vec<Observed>>>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for ReasoningThenAnswer {
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
        self.calls.lock().unwrap().push(Observed {
            guidance: req.host_guidance.clone(),
            system_prompt: req
                .provider_request
                .as_ref()
                .and_then(|provider| provider.wire_system_prompt()),
            messages: req
                .provider_request
                .as_ref()
                .map(|provider| provider.messages.clone())
                .unwrap_or_default(),
        });
        let first = self.calls.lock().unwrap().len() == 1;
        let tokens: Vec<Result<StreamToken, LlmError>> = if first {
            vec![
                Ok(StreamToken::ReasoningContent(
                    "The bug is in the parser file because the failure starts there. I will run the parser test."
                        .to_string(),
                )),
                Ok(StreamToken::ToolCall {
                    name: "bash".to_string(),
                    args: serde_json::json!({}),
                    id: Some("call-1".to_string()),
                }),
                Ok(StreamToken::Done),
            ]
        } else {
            vec![
                Ok(StreamToken::Text("done".to_string())),
                Ok(StreamToken::Done),
            ]
        };
        Ok(Box::pin(futures::stream::iter(tokens)))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "thinking-fixture"
    }
}

async fn run(fail: bool, thinking_context: bool) -> (LlmSession, Vec<Observed>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(ReasoningThenAnswer {
            calls: Arc::clone(&calls),
        }))
        .with_tool(ToolVariant::Async(Box::new(FakeBash { fail })))
        .max_steps(4)
        .thinking_context(thinking_context)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test-model");
    request.input = Content::text("fix the parser test");
    {
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("stream item");
        }
    }
    let seen = calls.lock().unwrap().clone();
    (session, seen)
}

fn summary_guidance(observed: &Observed) -> Option<&HostGuidance> {
    observed
        .guidance
        .iter()
        .find(|guidance| guidance.purpose == THINKING_SOURCE)
}

fn message_text(message: &LlmMessage) -> Option<&str> {
    match message {
        LlmMessage::System { content } | LlmMessage::Assistant { content } => Some(content),
        LlmMessage::User { content } => content.as_text(),
        LlmMessage::ToolResult { content, .. } => Some(content),
        LlmMessage::AssistantReasoning { .. } | LlmMessage::AssistantToolCall { .. } => None,
    }
}

#[tokio::test]
async fn a_confirming_result_verifies_the_claim_and_reaches_the_next_request() {
    let (session, seen) = run(false, true).await;
    assert_eq!(seen.len(), 2, "one reasoning step and one answer step");

    let second = &seen[1];
    let note = summary_guidance(second).expect("the next request carries a thinking note");
    assert!(
        note.content.contains("[source: thinking-summary"),
        "the note names its source: {}",
        note.content
    );
    assert!(
        note.content.contains("[verified]") && note.content.contains("bug"),
        "the claim the output confirms is verified: {}",
        note.content
    );
    assert!(
        note.content
            .contains("[inferred] I will run the parser test"),
        "the intended action is not promoted by its own success: {}",
        note.content
    );
    assert!(
        second
            .system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("[source: thinking-summary")),
        "the note reaches the system prompt, not the message list"
    );
    assert!(
        !second
            .messages
            .iter()
            .filter_map(message_text)
            .any(|text| text.contains("source: thinking-summary")),
        "the note is not a synthetic message"
    );

    let history = session.history().expect("the session keeps history");
    let reasoning = history.iter().find_map(|message| match message {
        LlmMessage::AssistantReasoning {
            content,
            provider_state,
        } => Some((content.clone(), provider_state.clone())),
        _ => None,
    });
    let (content, state) = reasoning.expect("reasoning is preserved as its own message");
    assert!(content.contains("The bug is in the parser file"));
    assert!(state.is_none(), "Chat Completions carries no opaque state");

    let snapshot = session.thinking_snapshot().expect("the summary persists");
    assert_eq!(snapshot.summary.phase, ThinkingPhase::Complete);
    assert!(snapshot.summary.decisions.iter().any(|decision| {
        decision.statement.contains("bug") && decision.confidence.as_str() == "verified"
    }));
    assert!(snapshot.summary.decisions.iter().any(|decision| {
        decision.statement.contains("I will run the parser test")
            && decision.confidence.as_str() == "inferred"
    }));
}

#[tokio::test]
async fn the_switch_disables_extraction_persistence_and_injection() {
    let (session, seen) = run(false, false).await;
    assert_eq!(
        seen.len(),
        2,
        "the turn still runs its tool and answer steps"
    );
    assert!(
        session.thinking_snapshot().is_none(),
        "a disabled switch persists no thinking snapshot"
    );
    for observed in &seen {
        assert!(
            !observed
                .guidance
                .iter()
                .any(|guidance| guidance.purpose == THINKING_SOURCE),
            "a disabled switch adds no thinking guidance"
        );
    }
    assert!(
        !seen[1]
            .system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("source: thinking-summary")),
        "no thinking note reaches the system prompt"
    );
}

#[tokio::test]
async fn a_restored_session_resumes_its_live_summary() {
    struct Resumed {
        calls: Arc<Mutex<Vec<Observed>>>,
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for Resumed {
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
            self.calls.lock().unwrap().push(Observed {
                guidance: req.host_guidance.clone(),
                system_prompt: req
                    .provider_request
                    .as_ref()
                    .and_then(|provider| provider.wire_system_prompt()),
                messages: Vec::new(),
            });
            let tokens: Vec<Result<StreamToken, LlmError>> = vec![
                Ok(StreamToken::Text("done".to_string())),
                Ok(StreamToken::Done),
            ];
            Ok(Box::pin(futures::stream::iter(tokens)))
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(Resumed {
            calls: Arc::clone(&calls),
        }))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
    let mut snapshot = ThinkingSnapshot::new(1);
    snapshot.summary.phase = ThinkingPhase::Implementing;
    snapshot.summary.decisions.push(ThinkingDecision {
        statement: "I will run the parser test".to_string(),
        rationale: Some("it reproduces the failure".to_string()),
        kind: react::thinking::ThinkingDecisionKind::Intent,
        confidence: ThinkingConfidence::Inferred,
        origin: ThinkingOrigin::Reasoning,
        evidence: Vec::new(),
        step: 1,
        active: true,
        superseded_by: None,
    });
    session.set_thinking_snapshot(Some(snapshot));

    let mut context = LlmContext::default();
    {
        let stream = engine.react_stream(
            None,
            LlmRequest::new("test-model"),
            &mut session,
            &mut context,
        );
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("stream item");
        }
    }

    let seen = calls.lock().unwrap();
    let note = summary_guidance(&seen[0]).expect("the resumed summary is injected");
    assert!(
        note.content
            .contains("[inferred] I will run the parser test"),
        "the restored decision reaches the first request: {}",
        note.content
    );
}

#[tokio::test]
async fn a_failed_result_retires_the_judgment_and_injects_the_open_question() {
    let (session, seen) = run(true, true).await;
    let second = &seen[1];
    let note = summary_guidance(second).expect("the next request carries a thinking note");
    assert!(
        !note
            .content
            .contains("[inferred] I will run the parser test"),
        "the retired judgment is not injected: {}",
        note.content
    );
    assert!(
        note.content.contains("open questions:") && note.content.contains("Why did bash fail"),
        "the failure becomes an open question: {}",
        note.content
    );
    assert!(
        note.content
            .contains("Diagnose the bash failure before retrying"),
        "the next action is diagnosis, not a retry: {}",
        note.content
    );

    let snapshot = session.thinking_snapshot().expect("the summary persists");
    assert!(snapshot
        .summary
        .decisions
        .iter()
        .any(|decision| !decision.active && decision.superseded_by.is_some()));
}
