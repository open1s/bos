//! Resilience layer for ReAct engine: Circuit Breaker and Rate Limiter.
//! This module provides simple, production-friendly resilience patterns.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
#[qserde::Archive]
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
#[qserde::Archive]
pub struct RateLimiterConfig {
    /// Maximum number of requests allowed per window.
    pub capacity: u32,
    /// Time window for the rate limit.
    pub window: Duration,
    /// Number of retry attempts on 429 errors.
    pub max_retries: u32,
    /// Initial backoff duration for retries.
    pub retry_backoff: Duration,
    /// Auto-wait when rate limited (instead of failing immediately).
    pub auto_wait: bool,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            capacity: 40,
            window: Duration::from_secs(60),
            max_retries: 3,
            retry_backoff: Duration::from_secs(1),
            auto_wait: true,
        }
    }
}

/// Combined resilience configuration.
#[derive(Debug, Clone, Default)]
pub struct ResilienceConfig {
    /// Circuit breaker settings.
    pub circuit_breaker: CircuitBreakerConfig,
    /// Rate limiter settings.
    pub rate_limiter: RateLimiterConfig,
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

/// Receives resilience events so callers can instrument retries and trips.
///
/// Every method defaults to doing nothing, so an implementation only
/// overrides the events it cares about. Attach one with
/// [`ReActResilience::with_observer`].
pub trait ResilienceObserver: Send + Sync + std::fmt::Debug {
    /// A rate-limit wait of `wait` occurred.
    fn on_rate_limit_wait(&self, wait: Duration) {
        let _ = wait;
    }

    /// A retry backoff of `wait` is about to be taken.
    fn on_retry_wait(&self, wait: Duration) {
        let _ = wait;
    }

    /// The circuit breaker opened.
    fn on_circuit_trip(&self) {}
}

/// Thread-safe circuit breaker implementation.
#[derive(Debug)]
#[qserde::Archive]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    failures: Arc<AtomicUsize>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    last_failure_time: Arc<Mutex<Option<Instant>>>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    state: Arc<Mutex<CircuitState>>,
    /// Counter for half-open probe attempts.
    #[rkyv(with = qserde::rkyv::with::Skip)]
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
    fn lock_state(&self) -> std::sync::MutexGuard<'_, CircuitState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_last_failure_time(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.last_failure_time
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Allow a call through, or return the current failure mode.
    pub fn check(&self) -> Result<(), ResilienceError<()>> {
        let mut state = self.lock_state();
        let now = Instant::now();

        match *state {
            CircuitState::Closed => {
                let failures = self.failures.load(Ordering::Relaxed);
                if failures >= self.config.max_failures {
                    *state = CircuitState::Open;
                    *self.lock_last_failure_time() = Some(now);
                    log::warn!(
                        "[CircuitBreaker] Too many failures ({}), opening circuit",
                        failures
                    );
                    return Err(ResilienceError::CircuitOpen);
                }
                Ok(())
            }
            CircuitState::Open => {
                let last_failure = self.lock_last_failure_time();
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
                if count.is_multiple_of(3) {
                    Ok(())
                } else {
                    Err(ResilienceError::CircuitOpen)
                }
            }
        }
    }

    /// Record a successful call. Resets failure count in Closed state.
    pub fn record_success(&self) {
        let mut state = self.lock_state();
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
        let mut state = self.lock_state();
        let now = Instant::now();

        match *state {
            CircuitState::Closed => {
                let count = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
                *self.lock_last_failure_time() = Some(now);
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
                *self.lock_last_failure_time() = Some(now);
                log::warn!("[CircuitBreaker] Probe failed, reopening circuit");
            }
            CircuitState::Open => {
                *self.lock_last_failure_time() = Some(now);
            }
        }
    }

    /// Get current state (for observability).
    pub fn get_state(&self) -> CircuitState {
        *self.lock_state()
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

/// Thread-safe rate limiter using sliding window algorithm.
/// Tracks individual request timestamps for more accurate rate limiting.
#[derive(Debug)]
#[qserde::Archive]
pub struct RateLimiter {
    config: RateLimiterConfig,
    /// Sorted timestamps of recent requests (oldest first).
    #[rkyv(with = qserde::rkyv::with::Skip)]
    timestamps: Arc<Mutex<VecDeque<Instant>>>,
}

impl RateLimiter {
    fn lock_timestamps(&self) -> std::sync::MutexGuard<'_, VecDeque<Instant>> {
        self.timestamps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Create a new rate limiter with the given configuration.
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            timestamps: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Try to acquire a slot.
    ///
    /// Returns `Err(RateLimited)` when the limiter is exhausted and either
    /// `auto_wait` is disabled or `max_retries` attempts are used up. When
    /// `auto_wait` is enabled each attempt sleeps until the window is due to
    /// slide, using `retry_backoff` as the fallback wait.
    pub async fn acquire(&self) -> Result<(), ResilienceError<()>> {
        let attempts = self.config.max_retries.max(1);

        for attempt in 0..attempts {
            if self.try_acquire().is_ok() {
                return Ok(());
            }

            if !self.config.auto_wait || attempt + 1 == attempts {
                break;
            }

            let backoff = self.config.retry_backoff * (1u32 << attempt).min(6);
            let wait_time = self
                .reset_at()
                .map(|reset| reset.saturating_duration_since(Instant::now()))
                .filter(|wait| !wait.is_zero())
                .unwrap_or(backoff);
            log::info!(
                "[RateLimiter] Waiting {:?} for rate limit reset (attempt {}/{})",
                wait_time,
                attempt + 1,
                attempts
            );
            tokio::time::sleep(wait_time).await;
        }

        Err(ResilienceError::RateLimited)
    }

    fn try_acquire(&self) -> Result<(), ResilienceError<()>> {
        let mut timestamps = self.lock_timestamps();
        let now = Instant::now();
        let window = self.config.window;

        while let Some(oldest) = timestamps.front() {
            if now.duration_since(*oldest) >= window {
                timestamps.pop_front();
            } else {
                break;
            }
        }

        let current_count = timestamps.len() as u32;
        if current_count >= self.config.capacity {
            log::warn!(
                "[RateLimiter] Rate limit exceeded (capacity: {}, window: {:?})",
                self.config.capacity,
                window
            );
            return Err(ResilienceError::RateLimited);
        }

        timestamps.push_back(now);
        Ok(())
    }

    /// Number of calls still allowed in the current window.
    pub fn remaining(&self) -> u32 {
        let timestamps = self.lock_timestamps();
        let now = Instant::now();
        let window = self.config.window;

        let valid_count = timestamps
            .iter()
            .filter(|t| now.duration_since(**t) < window)
            .count() as u32;
        self.config.capacity.saturating_sub(valid_count)
    }

    /// Get window reset time (for observability).
    pub fn reset_at(&self) -> Option<Instant> {
        let timestamps = self.lock_timestamps();
        timestamps
            .front()
            .map(|oldest| *oldest + self.config.window)
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            timestamps: Arc::clone(&self.timestamps),
        }
    }
}

/// Combined resilience wrapper for async operations.
#[derive(Debug)]
#[qserde::Archive]
pub struct ReActResilience {
    #[rkyv(with = qserde::rkyv::with::Skip)]
    circuit_breaker: Option<CircuitBreaker>,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    rate_limiter: Option<RateLimiter>,
    rate_limit_config: RateLimiterConfig,
    #[rkyv(with = qserde::rkyv::with::Skip)]
    observer: Option<Arc<dyn ResilienceObserver>>,
}

/// Classify an error as transient by matching its debug text.
///
/// This is the default classifier for [`ReActResilience::execute`]. It
/// only looks for a status code or a known phrase in the formatted error,
/// so it can miss or misclassify errors; prefer
/// [`ReActResilience::execute_with`] with a classifier over your own
/// error type.
#[must_use]
pub fn is_transient_debug<E: std::fmt::Debug>(error: &E) -> bool {
    let text = format!("{error:?}").to_ascii_lowercase();
    [
        "429",
        "too many requests",
        "rate limit",
        "timeout",
        "timed out",
        "connection refused",
        "connection reset",
        "service unavailable",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|fragment| text.contains(fragment))
}

impl ReActResilience {
    /// Create a new resilience wrapper with the given config.
    pub fn new(config: ResilienceConfig) -> Self {
        Self {
            circuit_breaker: Some(CircuitBreaker::new(config.circuit_breaker)),
            rate_limiter: Some(RateLimiter::new(config.rate_limiter.clone())),
            rate_limit_config: config.rate_limiter,
            observer: None,
        }
    }

    /// Create a no-op resilience wrapper (no limits).
    pub fn none() -> Self {
        Self {
            circuit_breaker: None,
            rate_limiter: None,
            rate_limit_config: RateLimiterConfig::default(),
            observer: None,
        }
    }

    /// Execute an async function with resilience checks.
    ///
    /// Checks the rate limiter first (with auto-wait), then the circuit
    /// breaker, then runs the operation. Transient errors are retried up
    /// to `max_retries` times, classified by [`is_transient_debug`]; prefer
    /// [`ReActResilience::execute_with`] to classify precisely.
    pub async fn execute<F, Fut, T, E>(&self, op: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Debug,
    {
        self.execute_with(is_transient_debug::<E>, op).await
    }

    /// Execute an async function, classifying retryable errors with `classify`.
    ///
    /// `classify` returns `true` when an error is transient and the
    /// operation should be retried. Passing an explicit classifier avoids
    /// the text matching in [`is_transient_debug`], which can retry a
    /// permanent failure whose message merely mentioned a transient word.
    pub async fn execute_with<F, Fut, T, E, C>(
        &self,
        classify: C,
        mut op: F,
    ) -> Result<T, ResilienceError<E>>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Debug,
        C: Fn(&E) -> bool,
    {
        let max_retries = self.rate_limit_config.max_retries;
        let base_backoff = self.rate_limit_config.retry_backoff;

        for attempt in 0..=max_retries {
            // Rate limit check - use acquire() to wait when about to exceed
            if let Some(limiter) = &self.rate_limiter {
                log::debug!(
                    "[Resilience] Attempt {}/{}: rate_limit check",
                    attempt + 1,
                    max_retries + 1
                );
                let started = Instant::now();
                let acquired = limiter.acquire().await;
                let waited = started.elapsed();
                if acquired.is_err() {
                    self.notify_rate_limit_wait(Duration::ZERO);
                    log::warn!(
                        "[Resilience] Rate limited, attempt {}/{}",
                        attempt + 1,
                        max_retries + 1
                    );
                    if attempt < max_retries {
                        let duration = base_backoff * (1 << attempt).min(6);
                        self.notify_retry_wait(duration);
                        log::info!("[Resilience] Retrying in {:?}", duration);
                        tokio::time::sleep(duration).await;
                        continue;
                    }
                    return Err(ResilienceError::RateLimited);
                }
                if waited > Duration::from_millis(1) {
                    // acquire() waited internally for the window to slide.
                    self.notify_rate_limit_wait(waited);
                }
            }

            // 2) Circuit breaker check
            if let Some(breaker) = &self.circuit_breaker {
                log::debug!(
                    "[Resilience] Circuit breaker state: {:?}",
                    breaker.get_state()
                );
                match breaker.check() {
                    Ok(()) => {}
                    Err(ResilienceError::CircuitOpen) => {
                        log::warn!("[Resilience] Circuit breaker is OPEN");
                        return Err(ResilienceError::CircuitOpen);
                    }
                    Err(ResilienceError::RateLimited) => {
                        let duration = base_backoff * (1 << attempt).min(6);
                        self.notify_retry_wait(duration);
                        tokio::time::sleep(duration).await;
                        continue;
                    }
                    Err(ResilienceError::Inner(())) => {}
                }
            }

            // 3) Execute the operation
            let result = op().await;

            // 4) Check for transient error and retry
            let is_transient = match &result {
                Err(error) => classify(error),
                Ok(_) => false,
            };

            if is_transient && attempt < max_retries {
                let duration = base_backoff * (1 << attempt).min(6);
                self.notify_retry_wait(duration);
                tokio::time::sleep(duration).await;
                continue;
            }

            // 5) Record outcome in circuit breaker
            if self.circuit_breaker.is_some() {
                match &result {
                    Ok(_) => {
                        log::debug!("[Resilience] Success, closing circuit if open");
                        self.record_success();
                    }
                    Err(e) => {
                        log::warn!("[Resilience] Failure recorded: {:?}", e);
                        self.record_failure();
                    }
                }
            }

            return result.map_err(ResilienceError::Inner);
        }

        Err(ResilienceError::RateLimited)
    }

    /// Get current circuit state (for telemetry).
    pub fn circuit_state(&self) -> Option<CircuitState> {
        self.circuit_breaker.as_ref().map(|b| b.get_state())
    }

    /// Check circuit breaker and return error if open.
    pub fn check_circuit(&self) -> Result<(), ResilienceError<()>> {
        if let Some(breaker) = &self.circuit_breaker {
            breaker.check()
        } else {
            Ok(())
        }
    }

    /// Record success with circuit breaker.
    pub fn record_success(&self) {
        if let Some(breaker) = &self.circuit_breaker {
            breaker.record_success();
        }
    }

    /// Record failure with circuit breaker.
    pub fn record_failure(&self) {
        if let Some(breaker) = &self.circuit_breaker {
            let was_open = breaker.get_state() == CircuitState::Open;
            breaker.record_failure();
            if !was_open && breaker.get_state() == CircuitState::Open {
                self.notify_circuit_trip();
            }
        }
    }

    /// Get remaining rate limit capacity (for telemetry).
    pub fn rate_limit_remaining(&self) -> Option<u32> {
        self.rate_limiter.as_ref().map(|l| l.remaining())
    }

    /// Get the rate limiter configuration.
    pub fn rate_limit_config(&self) -> &RateLimiterConfig {
        &self.rate_limit_config
    }

    /// Attach an observer notified of retries, waits and circuit trips.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ResilienceObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    fn notify_rate_limit_wait(&self, wait: Duration) {
        if let Some(observer) = &self.observer {
            observer.on_rate_limit_wait(wait);
        }
    }

    fn notify_retry_wait(&self, wait: Duration) {
        if let Some(observer) = &self.observer {
            observer.on_retry_wait(wait);
        }
    }

    fn notify_circuit_trip(&self) {
        if let Some(observer) = &self.observer {
            observer.on_circuit_trip();
        }
    }

    /// Try to acquire a rate limit slot without executing anything.
    /// Returns Ok(()) if allowed, Err(RateLimited) if exhausted.
    pub fn try_acquire(&self) -> Result<(), ResilienceError<()>> {
        if let Some(limiter) = &self.rate_limiter {
            limiter.try_acquire()
        } else {
            Ok(())
        }
    }

    /// Acquire a rate limit slot with automatic waiting.
    pub async fn acquire(&self) -> Result<(), ResilienceError<()>> {
        if let Some(limiter) = &self.rate_limiter {
            limiter.acquire().await
        } else {
            Ok(())
        }
    }
}

impl Clone for ReActResilience {
    fn clone(&self) -> Self {
        Self {
            circuit_breaker: self.circuit_breaker.clone(),
            rate_limiter: self.rate_limiter.clone(),
            rate_limit_config: self.rate_limit_config.clone(),
            observer: self.observer.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct RecordingObserver {
        rate_limit_waits: AtomicUsize,
        retry_waits: AtomicUsize,
        circuit_trips: AtomicUsize,
    }

    impl ResilienceObserver for RecordingObserver {
        fn on_rate_limit_wait(&self, _wait: Duration) {
            self.rate_limit_waits.fetch_add(1, Ordering::Relaxed);
        }

        fn on_retry_wait(&self, _wait: Duration) {
            self.retry_waits.fetch_add(1, Ordering::Relaxed);
        }

        fn on_circuit_trip(&self) {
            self.circuit_trips.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn observer_sees_a_rate_limit_wait_and_retry() {
        let observer = Arc::new(RecordingObserver::default());
        let resilience = ReActResilience::new(ResilienceConfig {
            circuit_breaker: CircuitBreakerConfig {
                max_failures: 10,
                cooldown: Duration::from_secs(30),
            },
            rate_limiter: RateLimiterConfig {
                capacity: 1,
                window: Duration::from_millis(5),
                max_retries: 2,
                retry_backoff: Duration::from_millis(10),
                auto_wait: false,
            },
        })
        .with_observer(observer.clone());

        let run = || async { Ok::<_, &str>(()) };
        assert!(resilience.execute_with(|_| false, run).await.is_ok());
        assert!(resilience.execute_with(|_| false, run).await.is_ok());

        assert!(observer.rate_limit_waits.load(Ordering::Relaxed) >= 1);
        assert!(observer.retry_waits.load(Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn observer_sees_a_circuit_trip() {
        let observer = Arc::new(RecordingObserver::default());
        let resilience = ReActResilience::new(ResilienceConfig {
            circuit_breaker: CircuitBreakerConfig {
                max_failures: 1,
                cooldown: Duration::from_secs(30),
            },
            rate_limiter: RateLimiterConfig::default(),
        })
        .with_observer(observer.clone());

        let result = resilience
            .execute_with(|_| false, || async { Err::<(), &str>("boom") })
            .await;

        assert!(result.is_err());
        assert_eq!(observer.circuit_trips.load(Ordering::Relaxed), 1);
    }

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
            max_retries: 3,
            retry_backoff: Duration::from_secs(1),
            auto_wait: false,
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
                window: Duration::from_millis(5),
                max_retries: 3,
                retry_backoff: Duration::from_millis(10),
                auto_wait: false,
            },
            ..Default::default()
        };
        let resilience = ReActResilience::new(config);

        assert!(resilience
            .execute(|| async { Ok::<_, ()>(1) })
            .await
            .is_ok());

        assert!(resilience
            .execute(|| async { Ok::<_, ()>(2) })
            .await
            .is_ok());
        let result = resilience.execute(|| async { Ok::<_, ()>(2) }).await;
        assert!(matches!(result, Ok(2)));
    }

    #[tokio::test]
    async fn test_resilience_none() {
        let resilience = ReActResilience::none();

        // Should always succeed (no checks)
        let result = resilience.execute(|| async { Ok::<_, ()>(100) }).await;
        assert_eq!(result.unwrap(), 100);
    }

    #[tokio::test]
    async fn acquire_fails_fast_without_auto_wait() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            capacity: 1,
            window: Duration::from_secs(60),
            max_retries: 3,
            retry_backoff: Duration::from_millis(10),
            auto_wait: false,
        });

        assert!(limiter.acquire().await.is_ok());
        let start = Instant::now();
        assert!(limiter.acquire().await.is_err());
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn acquire_waits_when_auto_wait_is_enabled() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            capacity: 1,
            window: Duration::from_millis(30),
            max_retries: 3,
            retry_backoff: Duration::from_millis(5),
            auto_wait: true,
        });

        assert!(limiter.acquire().await.is_ok());
        assert!(limiter.acquire().await.is_ok());
    }

    #[tokio::test]
    async fn execute_with_honours_a_custom_classifier() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let resilience = ReActResilience::none();
        let attempts = AtomicUsize::new(0);
        let result: Result<u32, ResilienceError<&'static str>> = resilience
            .execute_with(
                |error: &&'static str| *error == "transient",
                || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if attempt == 0 {
                            Err("transient")
                        } else {
                            Ok(9)
                        }
                    }
                },
            )
            .await;

        assert_eq!(result.unwrap(), 9);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn execute_with_does_not_retry_when_the_classifier_says_no() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let resilience = ReActResilience::none();
        let attempts = AtomicUsize::new(0);
        // "timeout" and "503" would trip the default heuristic; the
        // explicit classifier overrides it and stops after one attempt.
        let result: Result<u32, ResilienceError<&'static str>> = resilience
            .execute_with(
                |_error: &&'static str| false,
                || {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    async move { Err("timeout 503") }
                },
            )
            .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
