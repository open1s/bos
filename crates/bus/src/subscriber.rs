//! Zenoh subscriber wrapper with simplified API

use rkyv::{Archive, Deserialize, api::high::HighDeserializer, rancor::Error};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;
use zenoh::sample::Sample;

/// A subscriber for receiving messages from a Zenoh topic.
///
/// # Example
/// ```rust,ignore
/// // Create a subscriber for a topic
/// let subscriber = TopicSubscriber::<String>::new("chat/general").with_session(&session?).await?;
///
/// // Receive messages
/// while let Some(msg) = subscriber.recv().await {
///     println!("Received: {}", msg);
/// }
/// ```
pub struct Subscriber<T> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>>,
    _phantom: std::marker::PhantomData<T>,
}

/// Convenient alias for reading code
pub type TopicSubscriber<T> = Subscriber<T>;

impl<T> Subscriber<T>
where
    T: Archive + Send + 'static,
    T::Archived: Deserialize<T, HighDeserializer<Error>>,
{
    /// Create a new subscriber for the specified topic
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            subscriber: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Initialize the subscriber with a session
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        let subscriber = session
            .declare_subscriber(&self.topic)
            .await
            .map_err(|e| ZenohError::Subscriber(e.to_string()))?;

        self.subscriber = Some(subscriber);
        Ok(())
    }

    /// Associate this subscriber with a session and initialize it
    pub async fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
        self.init(session).await?;
        Ok(self)
    }

    /// Create a subscriber directly from a session and topic
    pub async fn from_session(topic: impl Into<String>, session: Arc<Session>) -> Result<Self, ZenohError> {
        let mut sub = Self::new(topic);
        sub.init(session).await?;
        Ok(sub)
    }

    /// Receive the next message (returns None if subscriber not initialized)
    pub async fn recv(&mut self) -> Option<T> {
        let subscriber = self.subscriber.as_mut()?;

        let result: Result<Sample, zenoh::Error> = subscriber.recv_async().await;

        match result {
            Ok(sample) => {
                let bytes = sample.payload().to_bytes();
                Codec.decode(bytes.as_ref()).ok()
            }
            Err(_) => None,
        }
    }

    /// Receive with a callback that gets the raw Sample
    pub async fn recv_with_handle<F>(&mut self, handle: F) -> Result<(), ZenohError>
    where
        F: Fn(Sample),
    {
        let subscriber = self.subscriber.as_mut().ok_or(ZenohError::NotConnected)?;

        let sample = subscriber.recv_async().await?;
        handle(sample);
        Ok(())
    }

    /// Receive with a timeout
    pub async fn recv_with_timeout(&mut self, timeout: std::time::Duration) -> Option<T> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .unwrap_or_default()
    }

    /// Create a stream of messages (requires subscriber to be initialized)
    pub fn stream(&mut self) -> tokio::sync::mpsc::Receiver<T> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let subscriber = self.subscriber.take();
        if let Some(sub) = subscriber {
            tokio::spawn(async move {
                while let Ok(sample) = sub.recv_async().await {
                    let bytes = sample.payload().to_bytes();
                    if let Some(result) = Codec.decode::<T>(bytes.as_ref()).ok() {
                        // Move ownership of the decoded message
                        let msg = result;
                        if tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
            });
        }

        rx
    }

    /// Get the topic
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Check if the subscriber is initialized
    pub fn is_initialized(&self) -> bool {
        self.subscriber.is_some()
    }

    /// Get the underlying Zenoh subscriber
    pub fn subscriber(&self) -> Option<&zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>> {
        self.subscriber.as_ref()
    }
}

impl<T> Clone for Subscriber<T> {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            subscriber: None,
            _phantom: self._phantom,
        }
    }
}

impl<T> Default for Subscriber<T>
where
    T: Archive + Send + 'static,
    T::Archived: Deserialize<T, HighDeserializer<Error>>,
{
    fn default() -> Self {
        Self::new("default/subscriber")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscriber_creation() {
        let subscriber: Subscriber<String> = Subscriber::new("test/topic");
        assert_eq!(subscriber.topic(), "test/topic");
    }

    #[test]
    fn test_subscriber_clone() {
        let sub1: Subscriber<String> = Subscriber::new("test/clone");
        let sub2 = sub1.clone();
        assert_eq!(sub1.topic(), sub2.topic());
    }

    #[test]
    fn test_subscriber_default() {
        let subscriber: Subscriber<String> = Subscriber::default();
        assert_eq!(subscriber.topic(), "default/subscriber");
    }

    #[tokio::test]
    async fn test_subscriber_recv_timeout_before_init() {
        let mut subscriber: Subscriber<String> = Subscriber::new("test/timeout");
        let result = subscriber
            .recv_with_timeout(tokio::time::Duration::from_millis(100))
            .await;
        assert_eq!(result, None);
    }
}
