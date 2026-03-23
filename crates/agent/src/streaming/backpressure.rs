//! Backpressure utilities for token streaming over the bus.
//!
//! This module provides:
//! - `TokenBatch` — Accumulates tokens with size/time limits
//! - `RateLimiter` — Token bucket algorithm for rate limiting
//! - `BackpressureController` — Monitors bus health and adjusts rate

use crate::llm::StreamToken;
use rkyv::{Archive, Deserialize, Serialize, rancor::Error};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum BackpressureError {
    #[error("Deserialization error: {0}")]
    Deserialization(String),
}

/// A token type for serialization over the bus
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize)]
pub enum TokenType {
    Text,
    ToolCall,
    Done,
}

impl TokenType {
    pub fn to_bytes_rkyv(&self) -> Result<Vec<u8>, BackpressureError> {
        rkyv::to_bytes::<Error>(self)
            .map(|bytes| bytes.into_vec())
            .map_err(|e| BackpressureError::Deserialization(e.to_string()))
    }

    pub fn from_bytes_rkyv(bytes: &[u8]) -> Result<Self, BackpressureError> {
        unsafe {
            rkyv::from_bytes_unchecked::<TokenType, Error>(bytes)
                .map_err(|e| BackpressureError::Deserialization(e.to_string()))
        }
    }
}

/// A serialized token ready for bus transport
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize)]
pub struct SerializedToken {
    pub task_id: String,
    pub token_type: TokenType,
    pub tool_name: Option<String>,
    pub tool_args: Option<Vec<u8>>,
    pub content: String,
}

impl SerializedToken {
    pub fn from_stream_token(task_id: String, token: StreamToken) -> Self {
        match token {
            StreamToken::Text(content) => Self {
                task_id,
                token_type: TokenType::Text,
                tool_name: None,
                tool_args: None,
                content,
            },
            StreamToken::ToolCall { name, args } => {
                Self {
                    task_id,
                    token_type: TokenType::ToolCall,
                    tool_name: Some(name),
                    tool_args: Some(args.to_string().into_bytes()),
                    content: String::new(),
                }
            }
            StreamToken::Done => Self {
                task_id,
                token_type: TokenType::Done,
                tool_name: None,
                tool_args: None,
                content: String::new(),
            },
        }
    }

    pub fn to_bytes_rkyv(&self) -> Result<Vec<u8>, BackpressureError> {
        rkyv::to_bytes::<Error>(self)
            .map(|bytes| bytes.into_vec())
            .map_err(|e| BackpressureError::Deserialization(e.to_string()))
    }

    pub fn from_bytes_rkyv(bytes: &[u8]) -> Result<Self, BackpressureError> {
        unsafe {
            rkyv::from_bytes_unchecked::<SerializedToken, Error>(bytes)
                .map_err(|e| BackpressureError::Deserialization(e.to_string()))
        }
    }
}

/// A batch of tokens accumulated for efficient transport
#[derive(Debug, Clone, Archive, Serialize, Deserialize, SerdeSerialize)]
pub struct TokenBatch {
    pub tokens: Vec<SerializedToken>,
    pub token_count: usize,
}

impl TokenBatch {
    pub fn new() -> Self {
        Self {
            tokens: Vec::new(),
            token_count: 0,
        }
    }

    pub fn created_at(&self) -> Instant {
        Instant::now()
    }

    /// Check if batch is full based on size or token count limits
    pub fn is_full(&self, max_size: usize, max_tokens: usize) -> bool {
        self.tokens.len() >= max_size || self.token_count >= max_tokens
    }

    /// Add a token to the batch
    pub fn add(&mut self, token: SerializedToken) {
        self.token_count += 1;
        self.tokens.push(token);
    }

    /// Clear the batch for reuse
    pub fn clear(&mut self) {
        self.tokens.clear();
        self.token_count = 0;
    }

    /// Serialize using rkyv (zero-copy optimization)
    pub fn to_bytes_rkyv(&self) -> Result<Vec<u8>, BackpressureError> {
        rkyv::to_bytes::<Error>(self)
            .map(|bytes| bytes.into_vec())
            .map_err(|e| BackpressureError::Deserialization(e.to_string()))
    }

    /// Deserialize using rkyv (zero-copy if possible)
    pub fn from_bytes_rkyv(bytes: &[u8]) -> Result<Self, BackpressureError> {
        unsafe {
            rkyv::from_bytes_unchecked::<TokenBatch, Error>(bytes)
                .map_err(|e| BackpressureError::Deserialization(e.to_string()))
        }
    }

    /// Serialize using JSON (fallback for compatibility)
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize using JSON (fallback for compatibility)
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BackpressureError> {
        serde_json::from_slice(bytes).map_err(|e| BackpressureError::Deserialization(e.to_string()))
    }
}

impl Default for TokenBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl<'de> SerdeDeserialize<'de> for TokenBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(SerdeDeserialize)]
        struct TokenBatchHelper {
            tokens: Vec<SerializedToken>,
            token_count: usize,
        }

        let helper = TokenBatchHelper::deserialize(deserializer)?;
        Ok(Self {
            tokens: helper.tokens,
            token_count: helper.token_count,
        })
    }
}

/// Token bucket rate limiter for token streaming.
///
/// Uses the token bucket algorithm to enforce rate limits while allowing burst traffic.
///
/// # Thread Safety
///
/// This implementation uses atomic operations for thread-safe access without locks,
/// making it suitable for high-frequency concurrent usage.
///
/// # Example
///
/// ```rust
/// let limiter = RateLimiter::new(100.0, 50.0); // 100 tokens/sec, 50 burst capacity
///
/// // Allow publishing if tokens available
/// if limiter.should_publish() {
///     publish_token();
/// }
/// ```
#[derive(Debug)]
pub struct RateLimiter {
    /// Rate at which tokens are added (tokens per second)
    tokens_per_second: f64,
    /// Maximum burst capacity (tokens)
    capacity: f64,
    /// Current token count (scaled to avoid floating point precision issues)
    tokens_scaled: std::sync::atomic::AtomicU64,
    /// Scaling factor to convert tokens to integer representation
    scale_factor: f64,
    /// Last refill timestamp (nanoseconds since Unix epoch)
    last_refill: std::sync::atomic::AtomicU64,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    ///
    /// * `tokens_per_second` - Rate at which tokens are added (tokens/second)
    /// * `burst_capacity` - Maximum burst capacity (tokens). Defaults to `tokens_per_second` if 0.0.
    ///
    /// # Example
    ///
    /// ```rust
    /// // 100 tokens/second, 50 burst capacity
    /// let limiter = RateLimiter::new(100.0, 50.0);
    /// ```
    pub fn new(tokens_per_second: f64, burst_capacity: f64) -> Self {
        assert!(tokens_per_second > 0.0, "tokens_per_second must be positive");

        let capacity = if burst_capacity > 0.0 {
            burst_capacity
        } else {
            tokens_per_second
        };

        // Scale factor to maintain precision when converting to integer
        // 10000 gives us 4 decimal places of precision
        let scale_factor = 10000.0;

        Self {
            tokens_per_second,
            capacity,
            tokens_scaled: std::sync::atomic::AtomicU64::new((capacity * scale_factor) as u64),
            scale_factor,
            last_refill: std::sync::atomic::AtomicU64::new(Self::now_nanos()),
        }
    }

    /// Check if a token can be consumed (i.e., should_publish).
    ///
    /// This automatically refills the token bucket based on elapsed time.
    ///
    /// # Returns
    ///
    /// `true` if a token is available (should publish), `false` otherwise.
    ///
    /// # Thread Safety
    ///
    /// This method is lock-free and can be called concurrently from multiple threads.
    pub fn should_publish(&self) -> bool {
        self.refill();

        let tokens_scaled = self.tokens_scaled.load(std::sync::atomic::Ordering::SeqCst);

        if tokens_scaled >= self.scale_factor as u64 {
            let new_tokens = tokens_scaled - self.scale_factor as u64;
            self.tokens_scaled.store(new_tokens, std::sync::atomic::Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Get the current rate (tokens per second).
    ///
    /// This is the configured rate, not the current consumption rate.
    pub fn current_rate(&self) -> f64 {
        self.tokens_per_second
    }

    /// Get the current available tokens (burst capacity remaining).
    ///
    /// # Note
    ///
    /// This is primarily useful for debugging and monitoring.
    pub fn available_tokens(&self) -> f64 {
        let tokens_scaled = self.tokens_scaled.load(std::sync::atomic::Ordering::SeqCst);
        tokens_scaled as f64 / self.scale_factor
    }

    /// Refill tokens based on elapsed time since last refill.
    fn refill(&self) {
        let now = Self::now_nanos();
        let last = self.last_refill.load(std::sync::atomic::Ordering::SeqCst);

        if now <= last {
            return;
        }

        let elapsed_nanos = now - last;
        let elapsed_seconds = elapsed_nanos as f64 / 1_000_000_000.0;

        // Calculate tokens to add
        let tokens_to_add = (self.tokens_per_second * elapsed_seconds).min(self.capacity);

        if tokens_to_add > 0.0 {
            let tokens_add_scaled = (tokens_to_add * self.scale_factor) as u64;

            loop {
                let current = self.tokens_scaled.load(std::sync::atomic::Ordering::SeqCst);
                let new = current.saturating_add(tokens_add_scaled);

                // Ensure we don't exceed capacity
                let capacity_scaled = (self.capacity * self.scale_factor) as u64;
                let capped = new.min(capacity_scaled);

                if self.tokens_scaled.compare_exchange_weak(
                    current,
                    capped,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                ).is_ok() {
                    break;
                }
            }
        }

        self.last_refill.store(now, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get current time in nanoseconds since Unix epoch.
    fn now_nanos() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

impl Clone for RateLimiter {
    fn clone(&self) -> Self {
        // Create a new limiter with same config but independent state
        Self::new(self.tokens_per_second, self.capacity)
    }
}

/// Backpressure controller with adaptive rate adjustment.
///
/// Monitors bus health and adjusts token publishing rate to prevent overload.
/// Uses exponential moving average of bus load to smooth rate adjustments.
///
/// # Thread Safety
///
/// This implementation is thread-safe and can be used from multiple concurrent contexts.
///
/// # Adaptive Rate Adjustment
///
/// - Load < 0.5 (normal): maintain or increase rate
/// - Load 0.5-0.8 (elevated): reduce rate by 10-20%
/// - Load > 0.8 (critical): reduce rate by 50%
pub struct BackpressureController {
    /// Token bucket rate limiter
    rate_limiter: RateLimiter,
    /// Maximum batch size (tokens)
    max_batch_size: usize,
    /// Maximum batch token count
    max_batch_tokens: usize,
    /// Batch timeout (unused in async context, kept for API compatibility)
    _batch_timeout: Duration,
    /// Current load factor (scaled by 10000 to avoid floating point)
    load_factor_scaled: std::sync::atomic::AtomicU64,
    /// Base tokens per second (for rate adjustment)
    base_rate: f64,
    /// Minimum rate (20% of base)
    min_rate: f64,
}

impl BackpressureController {
    /// Create a new backpressure controller.
    ///
    /// # Arguments
    ///
    /// * `tokens_per_second` - Initial rate (tokens/second)
    /// * `max_batch_size` - Maximum number of tokens in a batch
    /// * `max_batch_tokens` - Maximum token count before flush
    /// * `batch_timeout` - Maximum time to wait before flushing (not used in async context)
    pub fn new(
        tokens_per_second: f64,
        max_batch_size: usize,
        max_batch_tokens: usize,
        _batch_timeout: Duration,
    ) -> Self {
        let min_rate = tokens_per_second * 0.2;

        Self {
            rate_limiter: RateLimiter::new(tokens_per_second, 0.0),
            max_batch_size,
            max_batch_tokens,
            _batch_timeout,
            load_factor_scaled: std::sync::atomic::AtomicU64::new(0),
            base_rate: tokens_per_second,
            min_rate,
        }
    }

    /// Check if publishing should be allowed based on rate limit.
    ///
    /// # Returns
    ///
    /// `true` if rate limit allows publishing, `false` otherwise.
    pub async fn should_publish(&self) -> bool {
        self.rate_limiter.should_publish()
    }

    /// Check if a batch is ready to be published.
    ///
    /// # Arguments
    ///
    /// * `batch` - The token batch to check
    ///
    /// # Returns
    ///
    /// `true` if batch is full or should be flushed, `false` otherwise.
    pub fn is_batch_ready(&self, batch: &TokenBatch) -> bool {
        batch.is_full(self.max_batch_size, self.max_batch_tokens)
    }

    /// Report bus load for adaptive rate adjustment.
    ///
    /// Updates internal load metrics and adjusts rate based on bus health.
    ///
    /// # Arguments
    ///
    /// * `load` - Bus load factor (0.0-1.0, where 1.0 is maximum load)
    pub fn report_bus_load(&mut self, load: f64) {
        let clamped_load = load.clamp(0.0, 1.0);
        const LOAD_SCALE: f64 = 10000.0;

        let current_scaled = self.load_factor_scaled.load(std::sync::atomic::Ordering::SeqCst);
        let current = current_scaled as f64 / LOAD_SCALE;

        let smoothed = current * 0.7 + clamped_load * 0.3;
        let smoothed_scaled = (smoothed * LOAD_SCALE) as u64;
        self.load_factor_scaled.store(smoothed_scaled, std::sync::atomic::Ordering::SeqCst);

        let new_rate = self.calculate_adaptive_rate(smoothed);
        self.update_rate(new_rate);
    }

    /// Get the current publishing rate.
    ///
    /// # Returns
    ///
    /// Current tokens per second.
    pub fn current_rate(&self) -> f64 {
        self.rate_limiter.current_rate()
    }

    /// Get the current load factor.
    ///
    /// # Returns
    ///
    /// Current load factor (0.0-1.0).
    pub fn current_load(&self) -> f64 {
        const LOAD_SCALE: f64 = 10000.0;
        let scaled = self.load_factor_scaled.load(std::sync::atomic::Ordering::SeqCst);
        scaled as f64 / LOAD_SCALE
    }

    /// Calculate adaptive rate based on load.
    fn calculate_adaptive_rate(&self, load: f64) -> f64 {
        let rate_fraction = if load < 0.5 {
            1.0
        } else if load < 0.8 {
            0.8
        } else {
            0.5
        };

        (self.base_rate * rate_fraction).max(self.min_rate)
    }

    /// Update the rate limiter with new rate.
    fn update_rate(&mut self, new_rate: f64) {
        if (new_rate - self.current_rate()).abs() > 0.1 {
            self.rate_limiter = RateLimiter::new(new_rate, 0.0);
        }
    }
}

impl Clone for BackpressureController {
    fn clone(&self) -> Self {
        Self {
            rate_limiter: RateLimiter::new(self.rate_limiter.current_rate(), 0.0),
            max_batch_size: self.max_batch_size,
            max_batch_tokens: self.max_batch_tokens,
            _batch_timeout: self._batch_timeout,
            load_factor_scaled: std::sync::atomic::AtomicU64::new(self.load_factor_scaled.load(std::sync::atomic::Ordering::SeqCst)),
            base_rate: self.base_rate,
            min_rate: self.min_rate,
        }
    }
}

impl std::fmt::Debug for BackpressureController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackpressureController")
            .field("rate", &self.current_rate())
            .field("load", &self.current_load())
            .field("batch_size", &self.max_batch_size)
            .field("batch_tokens", &self.max_batch_tokens)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_type_serialize_deserialize() {
        let token_type = TokenType::Text;

        let bytes = token_type.to_bytes_rkyv().unwrap();
        let deserialized = TokenType::from_bytes_rkyv(&bytes).unwrap();

        assert_eq!(token_type, deserialized);
    }

    #[test]
    fn test_serialized_token_rkyv() {
        let token = SerializedToken {
            task_id: "test_task".to_string(),
            token_type: TokenType::ToolCall,
            tool_name: Some("test_tool".to_string()),
            tool_args: Some(br#"{"value":42}"#.to_vec()),
            content: String::new(),
        };

        let bytes = token.to_bytes_rkyv().unwrap();
        let deserialized = SerializedToken::from_bytes_rkyv(&bytes).unwrap();

        assert_eq!(token.task_id, deserialized.task_id);
        assert_eq!(token.token_type, deserialized.token_type);
        assert_eq!(token.tool_name, deserialized.tool_name);
        assert_eq!(token.tool_args, deserialized.tool_args);
        assert_eq!(token.content, deserialized.content);
    }

    #[test]
    fn test_token_batch_rkyv() {
        let mut batch = TokenBatch::new();

        let token1 = SerializedToken {
            task_id: "task1".to_string(),
            token_type: TokenType::Text,
            tool_name: None,
            tool_args: None,
            content: "Hello".to_string(),
        };

        let token2 = SerializedToken {
            task_id: "task2".to_string(),
            token_type: TokenType::Done,
            tool_name: None,
            tool_args: None,
            content: String::new(),
        };

        batch.add(token1);
        batch.add(token2);

        let bytes = batch.to_bytes_rkyv().unwrap();
        let deserialized = TokenBatch::from_bytes_rkyv(&bytes).unwrap();

        assert_eq!(batch.tokens.len(), deserialized.tokens.len());
        assert_eq!(batch.token_count, deserialized.token_count);
    }

    #[test]
    fn test_rkyv_vs_json_produce_same_result() {
        let batch = TokenBatch {
            tokens: vec![
                SerializedToken {
                    task_id: "task1".to_string(),
                    token_type: TokenType::Text,
                    tool_name: None,
                    tool_args: None,
                    content: "Test content".to_string(),
                }
            ],
            token_count: 1,
        };

        let rkyv_bytes = batch.to_bytes_rkyv().unwrap();
        let json_bytes = batch.to_bytes().unwrap();

        let rkyv_batch = TokenBatch::from_bytes_rkyv(&rkyv_bytes).unwrap();
        let json_batch = TokenBatch::from_bytes(&json_bytes).unwrap();

        assert_eq!(rkyv_batch.tokens.len(), json_batch.tokens.len());
        assert_eq!(rkyv_batch.token_count, json_batch.token_count);
        assert_eq!(rkyv_batch.tokens[0].task_id, json_batch.tokens[0].task_id);
        assert_eq!(rkyv_batch.tokens[0].token_type, json_batch.tokens[0].token_type);
        assert_eq!(rkyv_batch.tokens[0].tool_name, json_batch.tokens[0].tool_name);
        assert_eq!(rkyv_batch.tokens[0].tool_args, json_batch.tokens[0].tool_args);
        assert_eq!(rkyv_batch.tokens[0].content, json_batch.tokens[0].content);
    }

    #[test]
    fn test_rkyv_size_advantage() {
        let batch = TokenBatch {
            tokens: vec![
                SerializedToken {
                    task_id: "task1".to_string(),
                    token_type: TokenType::Text,
                    tool_name: None,
                    tool_args: None,
                    content: "This is a longer piece of content to demonstrate size difference".to_string(),
                },
                SerializedToken {
                    task_id: "task2".to_string(),
                    token_type: TokenType::Done,
                    tool_name: None,
                    tool_args: None,
                    content: String::new(),
                }
            ],
            token_count: 2,
        };

        let rkyv_bytes = batch.to_bytes_rkyv().unwrap();
        let json_bytes = batch.to_bytes().unwrap();

        println!("rkyv size: {} bytes", rkyv_bytes.len());
        println!("json size: {} bytes", json_bytes.len());
        println!("size reduction: {:.1}%", 100.0 * (1.0 - rkyv_bytes.len() as f64 / json_bytes.len() as f64));

        // rkyv should generally produce smaller output
        assert!(rkyv_bytes.len() <= json_bytes.len());
    }

    #[test]
    fn test_tool_call_token_stores_name_separately() {
        let token = SerializedToken::from_stream_token(
            "task-1".to_string(),
            StreamToken::ToolCall {
                name: "get_weather".to_string(),
                args: serde_json::json!({"city": "Shanghai"}),
            },
        );

        assert!(matches!(token.token_type, TokenType::ToolCall));
        assert_eq!(token.tool_name.as_deref(), Some("get_weather"));
        assert_eq!(token.tool_args, Some(br#"{"city":"Shanghai"}"#.to_vec()));
        assert!(token.content.is_empty());
    }

    #[test]
    fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new(10.0, 5.0);

        assert_eq!(limiter.current_rate(), 10.0);
        assert!(limiter.available_tokens() >= 4.9);
    }

    #[test]
    fn test_rate_limiter_consume_tokens() {
        let limiter = RateLimiter::new(100.0, 10.0);

        for _ in 0..10 {
            assert!(limiter.should_publish());
        }

        assert!(!limiter.should_publish());
        assert!(limiter.available_tokens() < 0.1);
    }

    #[test]
    fn test_rate_limiter_burst_capacity() {
        let limiter = RateLimiter::new(10.0, 100.0);

        let mut consumed = 0;
        for _ in 0..100 {
            if limiter.should_publish() {
                consumed += 1;
            }
        }

        assert_eq!(consumed, 100);
    }

    #[test]
    fn test_rate_limiter_clone_independent() {
        let limiter1 = RateLimiter::new(10.0, 5.0);
        let limiter2 = limiter1.clone();

        limiter1.should_publish();
        limiter1.should_publish();

        assert!(limiter2.available_tokens() >= 4.9);
    }

    #[test]
    fn test_backpressure_controller_basic() {
        let controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        assert_eq!(controller.current_rate(), 100.0);
        assert_eq!(controller.current_load(), 0.0);
    }

    #[test]
    fn test_backpressure_controller_batch_ready() {
        let controller = BackpressureController::new(100.0, 10, 50, Duration::from_millis(50));
        let mut batch = TokenBatch::new();

        for i in 0..10 {
            batch.add(SerializedToken {
                task_id: i.to_string(),
                token_type: TokenType::Text,
                tool_name: None,
                tool_args: None,
                content: "test".to_string(),
            });
        }

        assert!(controller.is_batch_ready(&batch));
    }

    #[test]
    fn test_backpressure_controller_adaptive_rate() {
        let mut controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        controller.report_bus_load(0.3);
        assert_eq!(controller.current_rate(), 100.0);

        controller.report_bus_load(0.6);
        assert_eq!(controller.current_rate(), 80.0);

        controller.report_bus_load(0.9);
        assert_eq!(controller.current_rate(), 50.0);
    }

    #[test]
    fn test_backpressure_controller_load_clamping() {
        let mut controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        controller.report_bus_load(1.5);
        assert!(controller.current_load() <= 1.0);

        controller.report_bus_load(-0.5);
        assert!(controller.current_load() >= 0.0);
    }

    #[test]
    fn test_backpressure_controller_min_rate() {
        let mut controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        controller.report_bus_load(0.9);
        assert!(controller.current_rate() >= 20.0);
    }

    #[test]
    fn test_backpressure_controller_debug() {
        let controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        let debug_str = format!("{:?}", controller);
        assert!(debug_str.contains("rate"));
        assert!(debug_str.contains("load"));
    }
}
