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

/// Stub RateLimiter for compilation
/// TODO: Implement proper rate limiting
#[derive(Debug, Clone)]
pub struct RateLimiter {
    _tokens_per_second: f64,
}

impl RateLimiter {
    pub fn new(tokens_per_second: f64) -> Self {
        Self { _tokens_per_second: tokens_per_second }
    }

    pub fn current_rate(&self) -> f64 {
        self._tokens_per_second
    }
}

/// Stub BackpressureController for compilation
/// TODO: Implement proper backpressure control
#[derive(Debug, Clone)]
pub struct BackpressureController {
    _rate_limiter: RateLimiter,
    _max_batch_size: usize,
    _max_batch_tokens: usize,
}

impl BackpressureController {
    pub fn new(
        tokens_per_second: f64,
        max_batch_size: usize,
        max_batch_tokens: usize,
        _batch_timeout: Duration,
    ) -> Self {
        Self {
            _rate_limiter: RateLimiter::new(tokens_per_second),
            _max_batch_size: max_batch_size,
            _max_batch_tokens: max_batch_tokens,
        }
    }

    pub async fn should_publish(&self) -> bool {
        true
    }

    pub fn is_batch_ready(&self, batch: &TokenBatch) -> bool {
        batch.is_full(self._max_batch_size, self._max_batch_tokens)
    }

    pub fn report_bus_load(&mut self, _load: f64) {}

    pub fn current_rate(&self) -> f64 {
        self._rate_limiter.current_rate()
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
}
