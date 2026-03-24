//! Zenoh subscriber wrapper with simplified API

use rkyv::{Archive, Deserialize, api::high::HighDeserializer, rancor::Error};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::sample::Sample;

pub struct Subscriber<T> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>>,
    started: bool,
    handle: Option<JoinHandle<Result<(),String>>>,
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
            started: false,
            handle: None,
            _phantom: std::marker::PhantomData,
        }
    }

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

    pub async fn run<F>(mut self, handler: F) -> Result<(), ZenohError>
    where
        F: Fn(T) + std::marker::Send + 'static, {
        let subscriber = self.subscriber.take().ok_or(ZenohError::NotConnected)?;

        self.started = true;
        let handle: tokio::task::JoinHandle<Result<_, String>> = tokio::spawn(async move {
            while let Ok(sample) = subscriber.recv_async().await {
                let bytes = sample.payload().to_bytes();
                if let Ok(decoded) = Codec.decode(bytes.as_ref()) {
                    handler(decoded);
                }
            }
            Ok(())
        });

        self.handle = Some(handle);
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
}

impl<T> Drop for Subscriber<T> {
    fn drop(&mut self) {
        if self.started {
            self.started = false;
            // let handle = self.handle.take().unwrap();
            // handle.abort();
        }
    }
}

impl<T> Clone for Subscriber<T> {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            subscriber: None,
            started: false,
            handle: None,
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

pub fn subscriber_receiver<T,F>(mut subscriber: Subscriber<T>, mut handler: F) -> JoinHandle<Result<(),String>>
where
    F: FnMut(T) + Send + 'static,
    T: Archive + Send + 'static,
    T::Archived: Deserialize<T, HighDeserializer<Error>>,{
    let handle: tokio::task::JoinHandle<Result<_, String>> = tokio::spawn(async move {
        while let Some(query) = subscriber.recv().await {
            handler(query);
        }
        Ok(())
    });
    handle
}