use react::{CircuitBreakerConfig, RateLimiterConfig, ResilienceConfig};
use std::time::Duration;

#[tokio::test]
async fn test_engine_with_resilience_enabled() {
    let config = ResilienceConfig {
        circuit_breaker: CircuitBreakerConfig {
            max_failures: 2,
            cooldown: Duration::from_secs(1),
        },
        rate_limiter: RateLimiterConfig {
            capacity: 10,
            window: Duration::from_secs(1),
            max_retries: 3,
            retry_backoff: Duration::from_secs(1),
            auto_wait: false,
        },
    };

    let resilience = react::ReActResilience::new(config);

    assert!(resilience.circuit_state().is_some());
    assert!(resilience.rate_limit_remaining().is_some());
}

#[tokio::test]
async fn test_engine_without_resilience() {
    let config = ResilienceConfig::default();
    assert_eq!(config.circuit_breaker.max_failures, 5);
    assert_eq!(config.rate_limiter.capacity, 40);
}
