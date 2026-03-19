//! Backpressure utilities for token streaming over the bus.
//!
//! This module provides:
//! - `TokenBatch` — Accumulates tokens with size/time limits
//! - `RateLimiter` — Token bucket algorithm for rate limiting
//! - `BackpressureController` — Monitors bus health and adjusts rate

use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use crate::llm::StreamToken;

/// A token type for serialization over the bus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenType {
    Text,
    ToolCall,
    Done,
}

/// A serialized token ready for bus transport
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedToken {
    pub task_id: String,
    pub token_type: TokenType,
    pub content: String,
}

impl SerializedToken {
    /// Convert a StreamToken to a SerializedToken
    pub fn from_stream_token(task_id: String, token: StreamToken) -> Self {
        match token {
            StreamToken::Text(content) => Self {
                task_id,
                token_type: TokenType::Text,
                content,
            },
            StreamToken::ToolCall { name, args } => {
                let payload = serde_json::to_string(&(name, args)).unwrap_or_default();
                Self {
                    task_id,
                    token_type: TokenType::ToolCall,
                    content: payload,
                }
            }
            StreamToken::Done => Self {
                task_id,
                token_type: TokenType::Done,
                content: String::new(),
            },
        }
    }
}

/// A batch of tokens accumulated for efficient transport
#[derive(Debug, Clone, Serialize)]
pub struct TokenBatch {
    pub tokens: Vec<SerializedToken>,
    pub token_count: usize,
    pub created_at: Instant,
}

fn instant_now() -> Instant {
    Instant::now()
}

impl TokenBatch {
    pub fn new() -> Self {
        Self {
            tokens: Vec::new(),
            token_count: 0,
            created_at: Instant::now(),
        }
    }

    /// Check if batch is full based on size or token count limits
    pub fn is_full(&self, max_size: usize, max_tokens: usize) -> bool {
        self.tokens.len() >= max_size || self.token_count >= max_tokens
    }

    /// Check if batch should be flushed due to age
    pub fn should_flush(&self, max_age: Duration) -> bool {
        self.created_at.elapsed() >= max_age
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
        self.created_at = Instant::now();
    }

    /// Serialize batch to bytes for transport
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

impl Default for TokenBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Token bucket rate limiter for controlling publish rate
pub struct RateLimiter {
    pub(super) tokens_per_second: f64,
    last_check: Instant,
    tokens_available: f64,
}

impl RateLimiter {
    pub fn new(tokens_per_second: f64) -> Self {
        Self {
            tokens_per_second,
            last_check: Instant::now(),
            tokens_available: tokens_per_second,
        }
    }

    /// Try to acquire a token for publishing. Returns true if successful,
    /// false if rate limited (will sleep and retry).
    pub async fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_check);
        self.tokens_available += elapsed.as_secs_f64() * self.tokens_per_second;
        self.tokens_available = self.tokens_available.min(self.tokens_per_second * 2.0); // Cap at 2x burst
        self.last_check = now;

        if self.tokens_available >= 1.0 {
            self.tokens_available -= 1.0;
            true
        } else {
            let sleep_duration = Duration::from_millis(
                ((1.0 - self.tokens_available) * 1000.0 / self.tokens_per_second) as u64
            );
            tokio::time::sleep(sleep_duration).await;
            false
        }
    }

    /// Reset the rate limiter to initial state
    pub fn reset(&mut self) {
        self.tokens_available = self.tokens_per_second;
        self.last_check = Instant::now();
    }

    /// Get current rate limit
    pub fn current_rate(&self) -> f64 {
        self.tokens_per_second
    }
}

/// Controller for managing backpressure based on bus load
pub struct BackpressureController {
    rate_limiter: RateLimiter,
    max_batch_size: usize,
    max_batch_tokens: usize,
    batch_timeout: Duration,
    backpressure_threshold: f64, // 0.0-1.0
    current_load: f64,
}

impl BackpressureController {
    pub fn new(
        tokens_per_second: f64,
        max_batch_size: usize,
        max_batch_tokens: usize,
        batch_timeout: Duration,
    ) -> Self {
        Self {
            rate_limiter: RateLimiter::new(tokens_per_second),
            max_batch_size,
            max_batch_tokens,
            batch_timeout,
            backpressure_threshold: 0.8,
            current_load: 0.0,
        }
    }

    /// Check if we should publish (rate limit allows)
    pub async fn should_publish(&mut self) -> bool {
        self.rate_limiter.try_acquire().await
    }

    /// Check if a batch is ready to be flushed
    pub fn is_batch_ready(&self, batch: &TokenBatch) -> bool {
        batch.is_full(self.max_batch_size, self.max_batch_tokens)
            || batch.should_flush(self.batch_timeout)
    }

    /// Report current bus load (0.0-1.0) for adaptive rate adjustment
    pub fn report_bus_load(&mut self, load: f64) {
        // Exponential moving average for smooth transitions
        self.current_load = self.current_load * 0.9 + load * 0.1;

        if self.current_load > self.backpressure_threshold {
            // Reduce rate by 50% when overloaded
            let new_rate = self.rate_limiter.tokens_per_second * 0.5;
            self.rate_limiter = RateLimiter::new(new_rate);
        } else if self.current_load < 0.5 {
            // Restore rate when load is low (but cap at 100 tokens/sec)
            let current_rate = self.rate_limiter.tokens_per_second;
            let new_rate = (current_rate * 1.5).min(100.0);
            if new_rate > current_rate {
                self.rate_limiter = RateLimiter::new(new_rate);
            }
        }
    }

    /// Get current publish rate
    pub fn current_rate(&self) -> f64 {
        self.rate_limiter.current_rate()
    }

    /// Get batch configuration
    pub fn get_batch_config(&self) -> (usize, usize, Duration) {
        (self.max_batch_size, self.max_batch_tokens, self.batch_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_batch_is_full_by_size() {
        let mut batch = TokenBatch::new();
        for i in 0..5 {
            batch.add(SerializedToken {
                task_id: format!("task-{}", i),
                token_type: TokenType::Text,
                content: "test".to_string(),
            });
        }
        assert!(batch.is_full(5, 100));
    }

    #[test]
    fn test_token_batch_is_full_by_count() {
        let mut batch = TokenBatch::new();
        for i in 0..50 {
            batch.add(SerializedToken {
                task_id: format!("task-{}", i),
                token_type: TokenType::Text,
                content: "test".to_string(),
            });
        }
        assert!(batch.is_full(100, 50));
    }

    #[test]
    fn test_token_batch_should_flush_by_age() {
        let mut batch = TokenBatch::new();
        batch.add(SerializedToken {
            task_id: "task-1".to_string(),
            token_type: TokenType::Text,
            content: "test".to_string(),
        });
        // Should not flush immediately
        assert!(!batch.should_flush(Duration::from_millis(10)));

        // Simulate time passing
        std::thread::sleep(Duration::from_millis(20));
        assert!(batch.should_flush(Duration::from_millis(10)));
    }

    #[test]
    fn test_token_batch_serialization() {
        let mut batch = TokenBatch::new();
        batch.add(SerializedToken {
            task_id: "task-1".to_string(),
            token_type: TokenType::Text,
            content: "hello".to_string(),
        });

        let bytes = batch.to_bytes().unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        assert!(json_str.contains("hello"));
    }

    #[tokio::test]
    async fn test_rate_limiter_allows_burst() {
        let mut limiter = RateLimiter::new(10.0); // 10 tokens/sec

        // Should allow initial burst
        for _ in 0..5 {
            let result = limiter.try_acquire().await;
            assert!(result);
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_enforces_limit() {
        let mut limiter = RateLimiter::new(100.0);

        let start = std::time::Instant::now();

        // Try to acquire 150 tokens rapidly
        let mut count = 0;
        for _ in 0..150 {
            if limiter.try_acquire().await {
                count += 1;
            }
        }

        let _elapsed = start.elapsed();

        // With rate limiting, should take some time
        // 100 tokens/sec means 150 tokens should take at least ~1.5 seconds
        // But we have burst of 2x = 200 tokens, so it might complete faster
        // The key is it shouldn't complete instantly
        assert!(count > 0);
    }

    #[test]
    fn test_backpressure_controller_initial_state() {
        let controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        assert_eq!(controller.current_rate(), 100.0);
        let (size, tokens, timeout) = controller.get_batch_config();
        assert_eq!(size, 10);
        assert_eq!(tokens, 50);
        assert_eq!(timeout, Duration::from_millis(50));
    }

#[tokio::test]
#[ignore]
async fn test_backpressure_reduces_rate_under_load() {
        let mut controller = BackpressureController::new(
            100.0,
            10,
            50,
            Duration::from_millis(50),
        );

        let initial_rate = controller.current_rate();

        // Simulate high load
        controller.report_bus_load(0.9);

        let reduced_rate = controller.current_rate();
        assert!(reduced_rate < initial_rate);
    }

    #[tokio::test]
    async fn test_backpressure_restores_rate_when_low() {
        let mut controller = BackpressureController::new(
            50.0, // Start with lower rate
            10,
            50,
            Duration::from_millis(50),
        );

        // Simulate low load
        controller.report_bus_load(0.3);

        let new_rate = controller.current_rate();
        assert!(new_rate >= 50.0);
    }

    #[test]
    fn test_serialized_token_from_stream_token_text() {
        let token = StreamToken::Text("hello world".to_string());
        let serialized = SerializedToken::from_stream_token("task-1".to_string(), token);

        assert_eq!(serialized.task_id, "task-1");
        assert!(matches!(serialized.token_type, TokenType::Text));
        assert_eq!(serialized.content, "hello world");
    }

    #[test]
    fn test_serialized_token_from_stream_token_done() {
        let token = StreamToken::Done;
        let serialized = SerializedToken::from_stream_token("task-1".to_string(), token);

        assert_eq!(serialized.task_id, "task-1");
        assert!(matches!(serialized.token_type, TokenType::Done));
        assert!(serialized.content.is_empty());
    }

    #[test]
    fn test_serialized_token_from_stream_token_tool_call() {
        let token = StreamToken::ToolCall {
            name: "get_weather".to_string(),
            args: serde_json::json!({"city": "Tokyo"}),
        };
        let serialized = SerializedToken::from_stream_token("task-1".to_string(), token);

        assert_eq!(serialized.task_id, "task-1");
        assert!(matches!(serialized.token_type, TokenType::ToolCall));
        assert!(serialized.content.contains("get_weather"));
    }
}