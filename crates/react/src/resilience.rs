// Resilience layer for ReAct engine: Circuit Breaker and Rate Limiter.
// This module provides simple, production-friendly resilience patterns.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::sync::Mutex;
use thiserror::Error;

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Maximum number of failures before opening the circuit.
    pub max_failures: usize,
    /// Duration to wait before attempting to close the circuit (half-open).
    pub cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_failures: 5,
            cooldown: Duration::from_secs(30),
        }
    }
}

/// Configuration for the rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum number of requests allowed per window.
    pub capacity: u32,
    /// Time window for the rate limit.
    pub window: Duration,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            capacity: 10,
            window: Duration::from_secs(1),
        }
    }
}

/// Combined resilience configuration.
#[derive(Debug, Clone)]
pub struct ResilienceConfig {
    /// Circuit breaker settings.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Rate limiter settings.
    pub rate_limiter: RateLimiterConfig,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            circuit_breaker: CircuitBreakerConfig::default(),
            rate_limiter: RateLimiterConfig::default(),
        }
    }
}

impl ResilienceConfig {
    /// Create a new config with custom values.
    pub fn new(circuit_breaker: CircuitBreakerConfig, rate_limiter: RateLimiterConfig) -> Self {
        Self {
            circuit_breaker,
            rate_limiter,
        }
    }

    /// Builder-style method to set circuit breaker config.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker = config;
        self
    }

    /// Builder-style method to set rate limiter config.
    pub fn with_rate_limiter(mut self, config: RateLimiterConfig) -> Self {
        self.rate_limiter = config;
        self
    }
}

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation, requests are allowed.
    Closed,
    /// Too many failures, requests are blocked.
    Open,
    /// Testing if service recovered, limited requests allowed.
    HalfOpen,
}

/// Errors that can occur in the resilience layer.
#[derive(Debug, Error)]
pub enum ResilienceError<E: std::fmt::Debug> {
    /// Request was rate limited.
    #[error("Rate limit exceeded")]
    RateLimited,
    /// Circuit breaker is open.
    #[error("Circuit breaker is open")]
    CircuitOpen,
    /// Inner error from the wrapped operation.
    #[error("Inner error: {0}")]
    Inner(E),
}

impl<E: std::fmt::Debug> From<ResilienceError<E>> for String {
    fn from(e: ResilienceError<E>) -> String {
        format!("{:?}", e)
    }
}

/// Thread-safe circuit breaker implementation.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    failures: Arc<AtomicUsize>,
    last_failure_time: Arc<Mutex<Option<Instant>>>,
    state: Arc<Mutex<CircuitState>>,
    /// Counter for half-open probe attempts.
    probe_count: Arc<AtomicU64>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            failures: Arc::new(AtomicUsize::new(0)),
            last_failure_time: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(CircuitState::Closed)),
            probe_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Check if a request is allowed. Returns Ok(()) if allowed, Err(CircuitOpen) if blocked.
    pub fn check(&self) -> Result<(), ResilienceError<()>> {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();

        match *state {
            CircuitState::Closed => {
                let failures = self.failures.load(Ordering::Relaxed);
                if failures >= self.config.max_failures {
                    *state = CircuitState::Open;
                    *self.last_failure_time.lock().unwrap() = Some(now);
                    log::warn!(
                        "[CircuitBreaker] Too many failures ({}), opening circuit",
                        failures
                    );
                    return Err(ResilienceError::CircuitOpen);
                }
                Ok(())
            }
            CircuitState::Open => {
                let last_failure = self.last_failure_time.lock().unwrap();
                if let Some(last) = *last_failure {
                    if now.duration_since(last) >= self.config.cooldown {
                        *state = CircuitState::HalfOpen;
                        log::info!("[CircuitBreaker] Cooldown elapsed, entering half-open state");
                        return Ok(());
                    }
                }
                Err(ResilienceError::CircuitOpen)
            }
            CircuitState::HalfOpen => {
                let count = self.probe_count.fetch_add(1, Ordering::Relaxed);
                if count % 3 == 0 {
                    Ok(())
                } else {
                    Err(ResilienceError::CircuitOpen)
                }
            }
        }
    }

    /// Record a successful call. Resets failure count in Closed state.
    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap();
        match *state {
            CircuitState::Closed => {
                self.failures.store(0, Ordering::Relaxed);
            }
            CircuitState::HalfOpen => {
                *state = CircuitState::Closed;
                self.failures.store(0, Ordering::Relaxed);
                self.probe_count.store(0, Ordering::Relaxed);
                log::info!("[CircuitBreaker] Recovery successful, circuit closed");
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failed call. Increments failure count and may open the circuit.
    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();

        match *state {
            CircuitState::Closed => {
                let count = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
                *self.last_failure_time.lock().unwrap() = Some(now);
                if count >= self.config.max_failures {
                    *state = CircuitState::Open;
                    log::warn!(
                        "[CircuitBreaker] Failure threshold reached ({}), opening circuit",
                        count
                    );
                }
            }
            CircuitState::HalfOpen => {
                *state = CircuitState::Open;
                *self.last_failure_time.lock().unwrap() = Some(now);
                log::warn!("[CircuitBreaker] Probe failed, reopening circuit");
            }
            CircuitState::Open => {
                *self.last_failure_time.lock().unwrap() = Some(now);
            }
        }
    }

    /// Get current state (for observability).
    pub fn get_state(&self) -> CircuitState {
        *self.state.lock().unwrap()
    }
}

impl Clone for CircuitBreaker {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            failures: Arc::clone(&self.failures),
            last_failure_time: Arc::clone(&self.last_failure_time),
            state: Arc::clone(&self.state),
            probe_count: Arc::clone(&self.probe_count),
        }
    }
}

/// Thread-safe rate limiter using fixed-window algorithm.
#[derive(Debug)]
pub struct RateLimiter {
    config: RateLimiterConfig,
    used: Arc<AtomicU64>,
    window_start: Arc<Mutex<Instant>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            used: Arc::new(AtomicU64::new(0)),
            window_start: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Try to acquire a slot. Returns Ok(()) if allowed, Err(RateLimited) if exhausted.
    pub fn try_acquire(&self) -> Result<(), ResilienceError<()>> {
        let mut window_start = self.window_start.lock().unwrap();
        let now = Instant::now();
        let window_duration = self.config.window;

        if now.duration_since(*window_start) >= window_duration {
            *window_start = now;
            self.used.store(0, Ordering::Relaxed);
        }

        let current = self.used.load(Ordering::Relaxed) as u32;
        if current < self.config.capacity {
            self.used.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            log::warn!(
                "[RateLimiter] Rate limit exceeded (capacity: {}, window: {:?})",
                self.config.capacity,
                window_duration
            );
            Err(ResilienceError::RateLimited)
        }
    }

    /// Get remaining capacity (for observability).
    pub fn remaining(&self) -> u32 {
        let used = self.used.load(Ordering::Relaxed) as u32;
        self.config.capacity.saturating_sub(used)
    }

    /// Get window reset time (for observability).
    pub fn reset_at(&self) -> Option<Instant> {
        let window_start = *self.window_start.lock().unwrap();
        Some(window_start + self.config.window)
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            used: Arc::clone(&self.used),
            window_start: Arc::clone(&self.window_start),
        }
    }
}

/// Combined resilience wrapper for async operations.
#[derive(Debug, Clone)]
pub struct ReActResilience {
    circuit_breaker: Option<CircuitBreaker>,
    rate_limiter: Option<RateLimiter>,
}

impl ReActResilience {
    /// Create a new resilience wrapper with the given config.
    pub fn new(config: ResilienceConfig) -> Self {
        Self {
            circuit_breaker: Some(CircuitBreaker::new(config.circuit_breaker)),
            rate_limiter: Some(RateLimiter::new(config.rate_limiter)),
        }
    }

    /// Create a no-op resilience wrapper (no limits).
    pub fn none() -> Self {
        Self {
            circuit_breaker: None,
            rate_limiter: None,
        }
    }

    /// Execute an async function with resilience checks.
    /// Checks rate limiter first, then circuit breaker, then executes the function.
    pub async fn execute<F, Fut, T, E>(&self, op: F) -> Result<T, ResilienceError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Debug,
    {
        // 1) Rate limit check
        if let Some(limiter) = &self.rate_limiter {
            match limiter.try_acquire() {
                Ok(()) => {}
                Err(ResilienceError::RateLimited) => return Err(ResilienceError::RateLimited),
                Err(ResilienceError::CircuitOpen) => return Err(ResilienceError::CircuitOpen),
                Err(ResilienceError::Inner(_)) => unreachable!(),
            }
        }

        // 2) Circuit breaker check
        if let Some(breaker) = &self.circuit_breaker {
            match breaker.check() {
                Ok(()) => {}
                Err(ResilienceError::CircuitOpen) => return Err(ResilienceError::CircuitOpen),
                Err(ResilienceError::RateLimited) => unreachable!(),
                Err(ResilienceError::Inner(_)) => unreachable!(),
            }
        }

        // 3) Execute the operation
        let result = op().await;

        // 4) Record outcome in circuit breaker
        if let Some(breaker) = &self.circuit_breaker {
            match &result {
                Ok(_) => breaker.record_success(),
                Err(_) => breaker.record_failure(),
            }
        }

        result.map_err(ResilienceError::Inner)
    }

    /// Get current circuit state (for telemetry).
    pub fn circuit_state(&self) -> Option<CircuitState> {
        self.circuit_breaker.as_ref().map(|b| b.get_state())
    }

    /// Get remaining rate limit capacity (for telemetry).
    pub fn rate_limit_remaining(&self) -> Option<u32> {
        self.rate_limiter.as_ref().map(|l| l.remaining())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_closed_to_open() {
        let config = CircuitBreakerConfig {
            max_failures: 3,
            cooldown: Duration::from_millis(100),
        };
        let breaker = CircuitBreaker::new(config);

        assert!(breaker.check().is_ok());

        for _ in 0..3 {
            breaker.record_failure();
        }

        assert!(breaker.check().is_err());
        assert_eq!(breaker.get_state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn test_circuit_breaker_recovery() {
        let config = CircuitBreakerConfig {
            max_failures: 2,
            cooldown: Duration::from_millis(50),
        };
        let breaker = CircuitBreaker::new(config);

        breaker.record_failure();
        breaker.record_failure();
        assert!(breaker.check().is_err());

        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(breaker.check().is_ok());
        assert_eq!(breaker.get_state(), CircuitState::HalfOpen);

        breaker.record_success();

        assert_eq!(breaker.get_state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let config = RateLimiterConfig {
            capacity: 2,
            window: Duration::from_secs(1),
        };
        let limiter = RateLimiter::new(config);

        assert!(limiter.try_acquire().is_ok());
        assert!(limiter.try_acquire().is_ok());

        assert!(limiter.try_acquire().is_err());
    }

    #[tokio::test]
    async fn test_resilience_wrapper() {
        let config = ResilienceConfig::default();
        let resilience = ReActResilience::new(config);

        let result = resilience.execute(|| async { Ok::<_, ()>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_resilience_wrapper_rate_limit() {
        let config = ResilienceConfig {
            rate_limiter: RateLimiterConfig {
                capacity: 1,
                window: Duration::from_secs(1),
            },
            ..Default::default()
        };
        let resilience = ReActResilience::new(config);

        assert!(resilience.execute(|| async { Ok::<_, ()>(1) }).await.is_ok());

        assert!(resilience.execute(|| async { Ok::<_, ()>(2) }).await.is_err());
        let result = resilience.execute(|| async { Ok::<_, ()>(2) }).await;
        assert!(matches!(result, Err(ResilienceError::RateLimited)));
    }

    #[tokio::test]
    async fn test_resilience_none() {
        let resilience = ReActResilience::none();

        // Should always succeed (no checks)
        let result = resilience.execute(|| async { Ok::<_, ()>(100) }).await;
        assert_eq!(result.unwrap(), 100);
    }
}