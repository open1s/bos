//! Tests for LLM error retry classification and the engine retry bound.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponseResult, LlmSession, TokenStream,
};
use react::{
    CircuitBreakerConfig, RateLimiterConfig, ReActApp, ReActEngineBuilder, ReActResilience,
    ResilienceConfig,
};

#[test]
fn retryable_errors_are_classified_by_variant_and_status() {
    assert!(LlmError::RateLimited.is_retryable());
    assert!(LlmError::Timeout.is_retryable());
    assert!(LlmError::Http("HTTP 429: slow down".to_string()).is_retryable());
    assert!(LlmError::Http("HTTP 500: server error".to_string()).is_retryable());
    assert!(LlmError::Other("HTTP 503: busy".to_string()).is_retryable());
    assert!(LlmError::Http("error sending request: connection refused".to_string()).is_retryable());
}

#[test]
fn permanent_errors_are_not_retried() {
    assert!(!LlmError::ApiKeyMissing.is_retryable());
    assert!(!LlmError::Parse("unexpected token".to_string()).is_retryable());
    assert!(!LlmError::Http("HTTP 400: invalid request".to_string()).is_retryable());
    assert!(!LlmError::Other("HTTP 401: unauthorized".to_string()).is_retryable());
}

#[test]
fn a_retryable_word_in_a_4xx_body_does_not_trigger_a_retry() {
    // The status code wins: the body mentions "timeout" but the request is bad.
    let error = LlmError::Http("HTTP 400: the timeout field is invalid".to_string());
    assert!(!error.is_retryable());
}

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

struct AlwaysRateLimited {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for AlwaysRateLimited {
    async fn complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::RateLimited)
    }

    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _req: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Err(LlmError::RateLimited)
    }
}

/// `max_retries` is the number of retries after the first attempt, so the
/// engine must call the provider `1 + max_retries` times before giving up.
#[tokio::test]
async fn call_llm_retries_exactly_max_retries_times() {
    let calls = Arc::new(AtomicUsize::new(0));
    let resilience = ReActResilience::new(ResilienceConfig {
        circuit_breaker: CircuitBreakerConfig {
            max_failures: 100,
            cooldown: Duration::from_secs(1),
        },
        rate_limiter: RateLimiterConfig {
            capacity: 100,
            window: Duration::from_secs(60),
            max_retries: 3,
            retry_backoff: Duration::from_millis(1),
            auto_wait: false,
        },
    });
    let mut engine = ReActEngineBuilder::<TestApp>::new()
        .llm(Box::new(AlwaysRateLimited {
            calls: Arc::clone(&calls),
        }))
        .resilience(resilience)
        .build()
        .expect("engine builds");

    let result = engine
        .call_llm(
            None,
            LlmRequest::new("test-model"),
            &mut LlmSession::default(),
            &mut LlmContext::default(),
        )
        .await;

    assert!(matches!(
        result,
        Err(react::ReactError::Llm(LlmError::RateLimited))
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "one initial attempt plus max_retries (3) retries"
    );
}
