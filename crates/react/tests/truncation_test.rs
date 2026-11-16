//! A reply the output cap cut short is continued, not reported as the answer.
//!
//! Measured: a reasoning model spent 12,800 tokens thinking, the provider
//! stopped it at the cap (`finish_reason: "length"`), and the turn closed on a
//! fragment. Eight such turns cost 48 minutes and produced one sentence each.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use react::llm::{
    Content, LlmClient, LlmContext, LlmError, LlmRequest, LlmResponseResult, LlmSession,
    StreamToken, TokenStream, Usage,
};
use react::{ReActApp, ReActEngineBuilder};

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

/// Streams a reply that stops at the cap, then answers when continued.
struct CappedThenAnswered {
    calls: Arc<AtomicUsize>,
    /// How many leading steps stop at the cap.
    capped_steps: usize,
    cap: u32,
}

impl CappedThenAnswered {
    fn new(calls: Arc<AtomicUsize>, capped_steps: usize) -> Self {
        Self {
            calls,
            capped_steps,
            cap: react::llm::DEFAULT_COMPLETION_CAP,
        }
    }
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for CappedThenAnswered {
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
        let tokens: Vec<Result<StreamToken, LlmError>> = if index < self.capped_steps {
            vec![
                Ok(StreamToken::Text("drafting ".into())),
                Ok(StreamToken::Usage(Usage {
                    prompt_tokens: 100,
                    completion_tokens: self.cap,
                    total_tokens: 100 + self.cap,
                    prompt_tokens_details: None,
                })),
                Ok(StreamToken::Done),
            ]
        } else {
            vec![
                Ok(StreamToken::Text("answer".into())),
                Ok(StreamToken::Done),
            ]
        };
        Ok(Box::pin(futures::stream::iter(tokens)))
    }
}

async fn run(capped_steps: usize, max_steps: usize) -> (String, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(CappedThenAnswered::new(
            Arc::clone(&calls),
            capped_steps,
        )))
        .max_steps(max_steps)
        .build()
        .expect("engine builds");

    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
    let mut request = LlmRequest::new("test-model");
    request.input = Content::text("write the whole thing");

    let stream = engine.react_stream(None, request, &mut session, &mut context);
    futures::pin_mut!(stream);
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let StreamToken::Text(chunk) = item.expect("stream item") {
            text.push_str(&chunk);
        }
    }
    (text, calls.load(Ordering::SeqCst))
}

/// The cut-off step is continued: the reader gets the rest of the reply, and
/// the turn does not close on the fragment.
#[tokio::test]
async fn a_reply_cut_short_by_the_cap_is_continued() {
    let (text, calls) = run(1, 6).await;
    assert_eq!(calls, 2, "one capped step plus the continuation");
    assert_eq!(
        text, "drafting answer",
        "the continued half reaches the reader"
    );
}

/// A model that only ever thinks cannot loop: the continuation is bounded, so
/// the turn ends after the limit instead of paying for it forever.
#[tokio::test]
async fn a_reply_that_always_stops_at_the_cap_ends_after_the_limit() {
    let (_, calls) = run(99, 12).await;
    assert_eq!(
        calls, 3,
        "the initial step plus two bounded continuations, then done"
    );
}

/// A step that reported no usage keeps the old behaviour: the engine cannot
/// tell a capped reply from a finished one, so it does not guess.
#[tokio::test]
async fn a_step_without_usage_is_not_continued() {
    struct NoUsage {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient<LlmSession, LlmContext> for NoUsage {
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
            self.calls.fetch_add(1, Ordering::SeqCst);
            let tokens: Vec<Result<StreamToken, LlmError>> =
                vec![Ok(StreamToken::Text("done".into())), Ok(StreamToken::Done)];
            Ok(Box::pin(futures::stream::iter(tokens)))
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(NoUsage {
            calls: Arc::clone(&calls),
        }))
        .build()
        .expect("engine builds");
    let mut session = LlmSession::default();
    let mut context = LlmContext::default();
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
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one step, no retry");
}
