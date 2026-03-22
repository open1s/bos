use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::Session;

use super::backpressure::{SerializedToken, TokenBatch, TokenType};

#[derive(Debug, thiserror::Error)]
pub enum SubscriberError {
    #[error("Failed to initialize subscriber: {0}")]
    Init(String),
    #[error("Failed to deserialize tokens: {0}")]
    Deserialization(String),
}

pub struct TokenSubscriber {
    session: Arc<Session>,
    agent_id: String,
    topic_prefix: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>>,
    receive_task: Option<JoinHandle<()>>,
}

impl TokenSubscriber {
    pub fn new(
        session: Arc<Session>,
        agent_id: String,
        topic_prefix: String,
    ) -> Self {
        Self {
            session,
            agent_id,
            topic_prefix,
            subscriber: None,
            receive_task: None,
        }
    }

    fn topic(&self) -> String {
        format!("{}/{}/tokens/stream", self.topic_prefix, self.agent_id)
    }

    pub async fn subscribe_tokens<F>(&mut self, callback: F) -> Result<(), SubscriberError>
    where
        F: Fn(SerializedToken) + Send + Sync + 'static,
    {
        let subscriber = self
            .session
            .declare_subscriber(self.topic())
            .await
            .map_err(|e| SubscriberError::Init(e.to_string()))?;

        self.subscriber = Some(subscriber);

        let callback = Arc::new(callback);
        let subscriber = self
            .subscriber
            .take()
            .ok_or_else(|| SubscriberError::Init("Subscriber not initialized".to_string()))?;

        let handle = tokio::spawn(async move {
            let mut last_sequence: Option<u64> = None;

            loop {
                match subscriber.recv_async().await {
                    Ok(sample) => {
                        let bytes = sample.payload().to_bytes();

                        match Self::deserialize_batch(bytes.as_ref()) {
                            Ok(batch) => {
                                for token in batch.tokens {
                                    if let Err(e) = verify_token_order(&token, &mut last_sequence) {
                                        tracing::warn!("Token order verification failed: {}", e);
                                    }
                                    callback(token);
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to deserialize token batch: {}", e);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Subscriber receive error: {}", e);
                        break;
                    }
                }
            }
        });

        self.receive_task = Some(handle);

        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(task) = self.receive_task.take() {
            task.abort();
        }
        self.subscriber = None;
    }

    pub fn is_active(&self) -> bool {
        self.receive_task
            .as_ref()
            .map(|t| !t.is_finished())
            .unwrap_or(false)
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn topic_prefix(&self) -> &str {
        &self.topic_prefix
    }

    fn deserialize_batch(bytes: &[u8]) -> Result<TokenBatch, SubscriberError> {
        TokenBatch::from_bytes_rkyv(bytes)
            .or_else(|_| TokenBatch::from_bytes(bytes))
            .map_err(|e| SubscriberError::Deserialization(e.to_string()))
    }
}

impl Drop for TokenSubscriber {
    fn drop(&mut self) {
        if let Some(task) = self.receive_task.take() {
            task.abort();
        }
    }
}

pub fn verify_token_order(
    token: &SerializedToken,
    last_sequence: &mut Option<u64>,
) -> Result<(), String> {
    if matches!(token.token_type, TokenType::Done) {
        *last_sequence = None;
        return Ok(());
    }

    let current_seq = hash_task_id(&token.task_id);

    match *last_sequence {
        Some(last) => {
            if current_seq < last {
                return Err(format!(
                    "Token out of order: task_id={} (seq {} < last {})",
                    token.task_id, current_seq, last
                ));
            }
        }
        None => {}
    }

    *last_sequence = Some(current_seq);
    Ok(())
}

fn hash_task_id(task_id: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    task_id.hash(&mut hasher);
    hasher.finish()
}

pub type TokenCallback = Arc<dyn Fn(SerializedToken) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_token_order_first_token() {
        let mut last_seq: Option<u64> = None;
        let token = SerializedToken {
            task_id: "task-1".to_string(),
            token_type: TokenType::Text,
            tool_name: None,
            tool_args: None,
            content: "hello".to_string(),
        };

        assert!(verify_token_order(&token, &mut last_seq).is_ok());
        assert!(last_seq.is_some());
    }

    #[test]
    fn test_verify_token_order_done_resets() {
        let mut last_seq: Option<u64> = Some(12345);
        let token = SerializedToken {
            task_id: "task-1".to_string(),
            token_type: TokenType::Done,
            tool_name: None,
            tool_args: None,
            content: String::new(),
        };

        assert!(verify_token_order(&token, &mut last_seq).is_ok());
        assert!(last_seq.is_none());
    }

    #[test]
    fn test_hash_task_id_consistency() {
        let hash1 = hash_task_id("task-123");
        let hash2 = hash_task_id("task-123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_task_id_different_inputs() {
        let hash1 = hash_task_id("task-1");
        let hash2 = hash_task_id("task-2");
        assert_ne!(hash1, hash2);
    }
}
