//! Zenoh subscriber wrapper

use std::sync::Arc;

use crate::{error::ZenohError, Session};
use serde::de::DeserializeOwned;
use zenoh::sample::Sample;

pub struct SubscriberWrapper<T: DeserializeOwned + Send + Sized + 'static> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: DeserializeOwned + Send + Sized + 'static> SubscriberWrapper<T> {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            subscriber: None,
            _phantom: std::marker::PhantomData,
        }
    }

    pub async fn init(&mut self, session: &Arc<Session>) -> Result<(), ZenohError> {
        let subscriber = session
            .declare_subscriber(&self.topic)
            .await
            .map_err(|e| ZenohError::Subscriber(e.to_string()))?;

        self.subscriber = Some(subscriber);
        Ok(())
    }

    pub async fn recv(&mut self) -> Option<T> {
        let subscriber = self.subscriber.as_mut()?;

        let result: Result<Sample, zenoh::Error> = subscriber.recv_async().await;

        match result {
            Ok(sample) => {
                let bytes = sample.payload().to_bytes();
                serde_json::from_slice::<T>(bytes.as_ref()).ok()
            }
            Err(_) => None,
        }
    }

    pub async fn recv_with_handle<F>(&mut self, handle: F) -> Result<(), ZenohError>
    where
        F: Fn(Sample),
    {
        let subscriber = self.subscriber.as_mut().ok_or(ZenohError::NotConnected)?;

        let sample = subscriber.recv_async().await?;
        handle(sample);
        Ok(())
    }

    pub async fn recv_with_timeout(&mut self, timeout: std::time::Duration) -> Option<T> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .unwrap_or_default()
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn is_initialized(&self) -> bool {
        self.subscriber.is_some()
    }

    pub fn subscriber(
        &self,
    ) -> Option<&zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>> {
        self.subscriber.as_ref()
    }
}

impl<T: DeserializeOwned + Send + Sized + 'static> Clone for SubscriberWrapper<T> {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            subscriber: None,
            _phantom: self._phantom,
        }
    }
}

impl<T: DeserializeOwned + Send + Sized + 'static> Drop for SubscriberWrapper<T> {
    fn drop(&mut self) {
        self.subscriber = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscriber_creation() {
        let subscriber: SubscriberWrapper<String> = SubscriberWrapper::new("test/topic");
        assert_eq!(subscriber.topic(), "test/topic");
    }

    #[test]
    fn test_subscriber_clone() {
        let sub1: SubscriberWrapper<String> = SubscriberWrapper::new("test/clone");
        let sub2 = sub1.clone();
        assert_eq!(sub1.topic(), sub2.topic());
    }

    #[tokio::test]
    async fn test_subscriber_recv_timeout_before_init() {
        let mut subscriber: SubscriberWrapper<String> = SubscriberWrapper::new("test/timeout");
        // Subscriber not initialized, should return None
        let result = subscriber
            .recv_with_timeout(tokio::time::Duration::from_millis(100))
            .await;
        assert_eq!(result, None);
    }
}
