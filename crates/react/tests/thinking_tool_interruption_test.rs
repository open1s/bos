//! A tool call interrupted while it is executing is recovered as unknown, not
//! as a success; a different task after the interruption clears it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;

use react::llm::{
    Content, ContextBlock, LlmClient, LlmContext, LlmError, LlmRequest, LlmResponseResult,
    LlmSession, ReactSession, StreamToken, TokenStream,
};
use react::runtime::ReActApp;
use react::thinking::{ThinkingConfidence, ThinkingPhase, THINKING_SOURCE};
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::ToolError;
use react::ReActEngineBuilder;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

/// A tool that signals it started and then never returns on its own.
struct BlockingTool {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl AsyncTool for BlockingTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> String {
        "Blocks until the stream is dropped".to_string()
    }

    async fn run(&self, _input: &Value) -> Result<Value, ToolError> {
        self.started.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(serde_json::json!({
            "success": true,
            "output": "this result must never be observed"
        }))
    }
}

/// One reasoning step plus a tool call.
struct AskForTool;

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for AskForTool {
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
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

struct Observed {
    context_blocks: Vec<ContextBlock>,
    system_prompt: Option<String>,
    pending_interrupted: Vec<bool>,
}

/// Captures the recovery request, then answers.
struct RecoveryLlm {
    calls: Arc<Mutex<Vec<Observed>>>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for RecoveryLlm {
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
        session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        self.calls.lock().unwrap().push(Observed {
            context_blocks: req.context_blocks.clone(),
            system_prompt: req
                .provider_request
                .as_ref()
                .and_then(|provider| provider.wire_system_prompt()),
            pending_interrupted: session
                .thinking_snapshot()
                .map(|snapshot| {
                    snapshot
                        .pending
                        .iter()
                        .map(|pending| pending.interrupted)
                        .collect()
                })
                .unwrap_or_default(),
        });
        let tokens: Vec<Result<StreamToken, LlmError>> = vec![
            Ok(StreamToken::Text("done".to_string())),
            Ok(StreamToken::Done),
        ];
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

/// Start a turn, drive it until the tool is executing, then drop the stream.
async fn interrupt_during_tool() -> LlmSession {
    let started = Arc::new(AtomicBool::new(false));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(AskForTool))
        .with_tool(ToolVariant::Async(Box::new(BlockingTool {
            started: Arc::clone(&started),
        })))
        .max_steps(3)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    {
        let mut request = LlmRequest::new("test-model");
        request.input = Content::text("fix the parser failure");
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        loop {
            match stream.next().await {
                Some(Ok(StreamToken::ToolCall { .. })) => break,
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("the turn failed before the tool: {error}"),
                None => panic!("the stream ended before the tool call"),
            }
        }

        // Let the generator pass the ToolCall yield, record the pending call,
        // and enter the tool. The next poll is left pending; the select ends
        // when the tool reports that it started.
        let next = stream.next();
        futures::pin_mut!(next);
        let wait_started = async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::select! {
            _ = &mut next => panic!("the tool call resolved before it could be interrupted"),
            _ = wait_started => {}
        }
        // Dropping the stream drops the in-flight tool future; the pending
        // call was already persisted before the tool began.
    }
    session
}

fn note_for(observed: &Observed) -> &ContextBlock {
    observed
        .context_blocks
        .iter()
        .find(|block| block.source == THINKING_SOURCE)
        .expect("the recovery request carries a thinking note")
}

#[tokio::test]
async fn a_tool_interrupted_mid_execution_is_recovered_as_unknown() {
    let mut session = interrupt_during_tool().await;
    let interrupted = session.thinking_snapshot().expect("a snapshot");
    assert_eq!(interrupted.pending.len(), 1, "the call was recorded");
    assert!(!interrupted.pending[0].interrupted);
    assert_eq!(interrupted.summary.phase, ThinkingPhase::Implementing);
    assert!(
        !interrupted
            .summary
            .decisions
            .iter()
            .any(|decision| decision.confidence == ThinkingConfidence::Verified),
        "an unfinished call cannot verify a decision"
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(RecoveryLlm {
            calls: Arc::clone(&calls),
        }))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut context = LlmContext::default();
    {
        let mut request = LlmRequest::new("test-model");
        request.input = Content::text("fix the parser failure");
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("the recovery turn runs");
        }
    }

    let seen = calls.lock().unwrap();
    let note = note_for(&seen[0]);
    assert!(
        note.content.contains("interrupted") && note.content.contains("outcome is unknown"),
        "the recovery note says the outcome is unknown: {}",
        note.content
    );
    assert!(
        note.content
            .contains("Determine whether the interrupted bash call took effect before retrying"),
        "the next action resolves the interruption first: {}",
        note.content
    );
    assert!(
        !note.content.contains("[verified]"),
        "nothing is verified from an unfinished call: {}",
        note.content
    );
    assert!(
        !seen[0]
            .system_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("outcome is unknown")),
        "the note must not reach the system prompt"
    );
    assert_eq!(
        seen[0].pending_interrupted,
        vec![true],
        "the restored call is marked interrupted before the recovery request"
    );
}

#[tokio::test]
async fn a_different_task_after_a_tool_interruption_clears_the_pending_call() {
    let mut session = interrupt_during_tool().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(RecoveryLlm {
            calls: Arc::clone(&calls),
        }))
        .max_steps(2)
        .build()
        .expect("the engine builds");
    let mut context = LlmContext::default();
    {
        let mut request = LlmRequest::new("test-model");
        request.input = Content::text("deploy the release notes");
        let stream = engine.react_stream(None, request, &mut session, &mut context);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.expect("the new task runs");
        }
    }
    let snapshot = session.thinking_snapshot().expect("a snapshot");
    assert!(
        snapshot.pending.is_empty(),
        "a new task does not inherit the interrupted call"
    );
    assert!(
        !snapshot
            .summary
            .open_questions
            .iter()
            .any(|question| question.question.contains("interrupted")),
        "a new task does not inherit the unresolved question"
    );
}
