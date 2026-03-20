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

/// Wrapper around Zenoh publisher with batching and backpressure
pub struct TokenPublisherWrapper {
    pub_session: Arc<Session>,
    bus_publisher: BusPublisher,
    backpressure: Mutex<BackpressureController>,
    batch: Mutex<TokenBatch>,
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
        let backpressure = Mutex::new(BackpressureController::new(
            100.0, // tokens_per_second
            10, // max_batch_size
            50, // max_batch_tokens
            Duration::from_millis(50), // batch_timeout
        ));

        let bus_publisher = BusPublisher::new(format!("{}/{}/tokens/stream", topic_prefix, agent_id))
            .with_session(&session);

        Self {
            pub_session: session,
            bus_publisher,
            backpressure,
            batch: Mutex::new(TokenBatch::new()),
            topic_prefix,
            agent_id,
        }
    }

    /// Publish a single token, accumulating it into a batch
    pub async fn publish_token(
        &self,
        task_id: String,
        token: StreamToken,
    ) -> Result<(), AgentError> {
        let serialized = Self::serialize_token(task_id, token);

        // Retry loop for rate limiting
        let mut retries = 3;
        while retries > 0 {
            // Check rate limit
            {
                let mut bp = self.backpressure.lock().await;
                if !bp.should_publish().await {
                    drop(bp);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    retries -= 1;
                    continue;
                }
            }

            // Add to batch
            {
                let mut batch = self.batch.lock().await;
                batch.add(serialized);

                let bp = self.backpressure.lock().await;
                if bp.is_batch_ready(&batch) {
                    drop(bp);
                    let batch = self.batch.lock().await;
                    Self::flush_batch(self, batch).await?;
                }
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
        let batch = self.batch.lock().await;
        if !batch.tokens.is_empty() {
            Self::flush_batch(self, batch).await?;
        }
        Ok(())
    }

async fn flush_batch(
    &self,
    batch: tokio::sync::MutexGuard<'_, TokenBatch>,
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
        .map_err(|e: bus::ZenohError| AgentError::Bus(e.to_string()))?;

    drop(batch);
    let mut batch = self.batch.lock().await;
    batch.clear();

    Ok(())
}

    /// Report bus load for adaptive backpressure
    pub async fn report_bus_load(&self, load: f64) {
        let mut bp = self.backpressure.lock().await;
        bp.report_bus_load(load);
    }

    /// Get current publish rate
    pub fn get_rate(&self) -> f64 {
        let bp = self.backpressure.blocking_lock();
        bp.current_rate()
    }

    /// Inspect backpressure config with a closure
    pub async fn with_config<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&BackpressureController) -> R,
    {
        let bp = self.backpressure.lock().await;
        f(&bp)
    }
}

/// Convenience type for direct token publishing
pub struct TokenPublisher {
    wrapper: Arc<TokenPublisherWrapper>,
}

impl TokenPublisher {
    /// Create a new TokenPublisher
    pub fn new(wrapper: Arc<TokenPublisherWrapper>) -> Self {
        Self { wrapper }
    }

    /// Publish a token
    pub async fn publish(
        &self,
        task_id: String,
        token: StreamToken,
    ) -> Result<(), AgentError> {
        self.wrapper.publish_token(task_id, token).await
    }

    /// Flush pending tokens
    pub async fn flush(&self) -> Result<(), AgentError> {
        self.wrapper.flush().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_publisher_exists() {
        // Type-level verification that PublisherWrapper and TokenPublisher exist
        // Integration tests in integration_tests.rs require zenoh router
        assert!(std::mem::size_of::<TokenPublisherWrapper>() > 0);
        assert!(std::mem::size_of::<TokenPublisher>() > 0);
    }
}