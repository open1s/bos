//! Token publisher for streaming over Zenoh bus.
//!
//! This module provides `TokenPublisherWrapper` and `TokenPublisher` for
//! publishing LLM tokens to subscribers over the Zenoh bus with
//! batching and backpressure support.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use zenoh::Session;

use bus::PublisherWrapper as BusPublisher;

use super::backpressure::{
    BackpressureController, TokenBatch, SerializedToken, TokenType,
};
use crate::llm::StreamToken;
use crate::error::AgentError;

/// Combined state for the publisher to reduce lock contention
struct PublisherState {
    batch: TokenBatch,
    backpressure: BackpressureController,
}

/// Wrapper around Zenoh publisher with batching and backpressure
pub struct TokenPublisherWrapper {
    pub_session: Arc<Session>,
    bus_publisher: BusPublisher,
    state: Mutex<PublisherState>,
    topic_prefix: String,
    agent_id: String,
}

impl TokenPublisherWrapper {
    /// Create a new TokenPublisherWrapper
    ///
    /// Default config: 10 tokens/batch, 50ms timeout, 100 tokens/sec rate
    pub fn new(
        session: Arc<Session>,
        agent_id: String,
        topic_prefix: String,
    ) -> Self {
        let state = Mutex::new(PublisherState {
            batch: TokenBatch::new(),
            backpressure: BackpressureController::new(
                100.0, // tokens_per_second
                10, // max_batch_size
                50, // max_batch_tokens
                Duration::from_millis(50), // batch_timeout
            ),
        });

        let bus_publisher = BusPublisher::new(format!("{}/{}/tokens/stream", topic_prefix, agent_id))
            .with_session(&session);

        Self {
            pub_session: session,
            bus_publisher,
            state,
            topic_prefix,
            agent_id,
        }
    }

    /// Publish a single token, accumulating it into a batch
    pub async fn publish_token(
        &self,
        task_id: &str,
        token: StreamToken,
    ) -> Result<(), AgentError> {
        let serialized = Self::serialize_token(task_id.to_string(), token);

        // Retry loop for rate limiting
        let mut retries = 3;
        while retries > 0 {
            let mut state = self.state.lock().await;

            // Check rate limit
            if !state.backpressure.should_publish().await {
                drop(state);
                tokio::time::sleep(Duration::from_millis(10)).await;
                retries -= 1;
                continue;
            }

            // Add to batch
            state.batch.add(serialized);

            // Check if batch is ready and flush if needed
            if state.backpressure.is_batch_ready(&state.batch) {
                let batch = std::mem::take(&mut state.batch);
                drop(state);
                Self::flush_batch_internal(self, batch).await?;
            }

            return Ok(());
        }

        Ok(())
    }

    fn serialize_token(task_id: String, token: StreamToken) -> SerializedToken {
        match token {
            StreamToken::Text(content) => SerializedToken {
                task_id,
                token_type: TokenType::Text,
                content,
            },
            StreamToken::ToolCall { name, args } => {
                let payload = serde_json::to_string(&(name, args)).unwrap_or_default();
                SerializedToken {
                    task_id,
                    token_type: TokenType::ToolCall,
                    content: payload,
                }
            }
            StreamToken::Done => SerializedToken {
                task_id,
                token_type: TokenType::Done,
                content: String::new(),
            },
        }
    }

    /// Flush any pending tokens in the batch
    pub async fn flush(&self) -> Result<(), AgentError> {
        let mut state = self.state.lock().await;
        if !state.batch.tokens.is_empty() {
            let batch = std::mem::take(&mut state.batch);
            drop(state);
            Self::flush_batch_internal(self, batch).await?;
        }
        Ok(())
    }

    async fn flush_batch_internal(
        &self,
        batch: TokenBatch,
    ) -> Result<(), AgentError> {
        if batch.tokens.is_empty() {
            return Ok(());
        }

        let bytes = batch
            .to_bytes()
            .map_err(|e| AgentError::Config(e.to_string()))?;

        self.bus_publisher
            .publish_raw(&self.pub_session, bytes)
            .await
            .map_err(|e| AgentError::Bus(e.to_string()))?;

        Ok(())
    }

    /// Report bus load for adaptive backpressure
    pub async fn report_bus_load(&self, load: f64) {
        let mut state = self.state.lock().await;
        state.backpressure.report_bus_load(load);
    }

    /// Get current publish rate
    pub fn get_rate(&self) -> f64 {
        let state = self.state.blocking_lock();
        state.backpressure.current_rate()
    }

    /// Inspect backpressure config with a closure
    pub async fn with_config<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&BackpressureController) -> R,
    {
        let state = self.state.lock().await;
        f(&state.backpressure)
    }
}

/// Convenience type for direct token publishing
pub struct TokenPublisher {
    wrapper: Arc<TokenPublisherWrapper>,
}

impl TokenPublisher {
    pub fn new(wrapper: Arc<TokenPublisherWrapper>) -> Self {
        Self { wrapper }
    }

    pub async fn publish(&self, task_id: &str, token: StreamToken) -> Result<(), AgentError> {
        self.wrapper.publish_token(task_id, token).await
    }

    pub async fn flush(&self) -> Result<(), AgentError> {
        self.wrapper.flush().await
    }
}

// Backward compatibility type alias
pub type PublisherWrapper = TokenPublisherWrapper;

