//! Tests for LLM error retry classification.

use react::llm::LlmError;

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
