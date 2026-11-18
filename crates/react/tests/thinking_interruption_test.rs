//! Reasoning that arrived before an interruption is handled on every
//! recoverable stream path, and a dropped stream never reports a completed turn.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;

use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponseResult, LlmSession, ReactSession,
    StreamToken, TokenStream,
};
use react::runtime::ReActApp;
use react::thinking::ThinkingPhase;
use react::ReActEngineBuilder;

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

#[derive(Clone, Copy)]
enum Mode {
    ProviderError,
    Stopped,
    Complete,
}

struct Scripted {
    mode: Mode,
    long_reasoning: bool,
    calls: Arc<AtomicUsize>,
}

fn reasoning(long: bool) -> String {
    let claim = "The bug is in the parser file because the failure starts there. ";
    if long {
        format!("{claim}{}", "The reasoning continues. ".repeat(200))
    } else {
        claim.to_string()
    }
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for Scripted {
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
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        if index > 0 {
            return Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamToken::Text("done".to_string())),
                Ok(StreamToken::Done),
            ])));
        }
        let mut tokens: Vec<Result<StreamToken, LlmError>> = vec![Ok(
            StreamToken::ReasoningContent(reasoning(self.long_reasoning)),
        )];
        match self.mode {
            Mode::ProviderError => tokens.push(Err(LlmError::Other(
                "provider failed after reasoning".to_string(),
            ))),
            Mode::Stopped => tokens.push(Ok(StreamToken::Stopped)),
            Mode::Complete => {
                tokens.push(Ok(StreamToken::Text("done".to_string())));
                tokens.push(Ok(StreamToken::Done));
            }
        }
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

/// Drive the stream until the path under test, then drop it.
async fn run(mode: Mode, long_reasoning: bool, stop_after_first: bool) -> LlmSession {
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(Scripted {
            mode,
            long_reasoning,
            calls: Arc::new(AtomicUsize::new(0)),
        }))
        .max_steps(3)
        .build()
        .expect("the engine builds");
    let mut session = LlmSession::default();
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
            match item {
                Ok(StreamToken::ReasoningContent(_)) if stop_after_first => break,
                Ok(StreamToken::Stopped) => break,
                Err(_) => break,
                _ => {}
            }
        }
    }
    session
}

fn has_claim(session: &LlmSession) -> bool {
    session.thinking_snapshot().is_some_and(|snapshot| {
        snapshot
            .summary
            .decisions
            .iter()
            .any(|decision| decision.statement.contains("bug"))
    })
}

#[tokio::test]
async fn reasoning_before_a_provider_error_is_kept() {
    let session = run(Mode::ProviderError, false, false).await;
    assert!(
        has_claim(&session),
        "the error path folds the reasoning received before the failure"
    );
    let phase = session
        .thinking_snapshot()
        .expect("a snapshot")
        .summary
        .phase;
    assert_ne!(phase, ThinkingPhase::Complete, "an error is not completion");
}

#[tokio::test]
async fn reasoning_before_a_stop_is_kept() {
    let session = run(Mode::Stopped, false, false).await;
    assert!(has_claim(&session), "the stop path folds what arrived");
    let phase = session
        .thinking_snapshot()
        .expect("a snapshot")
        .summary
        .phase;
    assert_ne!(phase, ThinkingPhase::Complete, "a stop is not completion");
}

#[tokio::test]
async fn a_dropped_stream_keeps_the_batched_reasoning() {
    let session = run(Mode::Complete, true, true).await;
    assert!(
        has_claim(&session),
        "reasoning is persisted in batches before the stream can be dropped"
    );
    let phase = session
        .thinking_snapshot()
        .expect("a snapshot")
        .summary
        .phase;
    assert_ne!(phase, ThinkingPhase::Complete, "a drop is not completion");
}

#[tokio::test]
async fn a_drop_before_the_batch_does_not_claim_persistence_or_completion() {
    let session = run(Mode::Complete, false, true).await;
    assert!(
        !has_claim(&session),
        "a drop before the batch threshold has no summary to persist"
    );
    let phase = session
        .thinking_snapshot()
        .expect("the run opened a snapshot")
        .summary
        .phase;
    assert_ne!(phase, ThinkingPhase::Complete, "a drop is not completion");
}

#[tokio::test]
async fn normal_completion_closes_the_turn() {
    let session = run(Mode::Complete, false, false).await;
    assert!(has_claim(&session), "normal completion keeps the summary");
    let phase = session
        .thinking_snapshot()
        .expect("a snapshot")
        .summary
        .phase;
    assert_eq!(phase, ThinkingPhase::Complete);
}
