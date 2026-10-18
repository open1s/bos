//! Zenoh subscriber wrapper with simplified API

use rkyv::{api::high::HighDeserializer, rancor::Error, Archive, Deserialize};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::sample::Sample;

/// Receives rkyv-encoded messages from a single topic.
pub struct Subscriber<T> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>>,
    started: bool,
    handle: Option<JoinHandle<Result<(), String>>>,
    _phantom: std::marker::PhantomData<T>,
}

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

    /// Declare the underlying Zenoh subscription on `session`.
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        let subscriber = session
            .declare_subscriber(&self.topic)
            .allowed_origin(zenoh::sample::Locality::Any)
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

    /// Receive the next message.
    ///
    /// Returns None when the subscriber is not initialized, when the
    /// subscription is closed, or when a payload does not decode as T. Payload
    /// integrity is the sender's responsibility: the bytes are not
    /// archive-validated before decoding.
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

    /// Spawn a task that invokes `handler` for every decoded message.
    pub async fn run<F>(&mut self, handler: F) -> Result<(), ZenohError>
    where
        F: Fn(T) + std::marker::Send + 'static,
    {
        let subscriber = self.subscriber.take().ok_or(ZenohError::NotConnected)?;

        self.started = true;
        let handle: tokio::task::JoinHandle<Result<(), String>> = tokio::spawn(async move {
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

    /// Stop the subscriber by dropping the underlying Zenoh subscription.
    /// Any pending `recv` will return `None`.
    pub fn stop(&mut self) {
        self.subscriber = None;
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
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
        }

        if let Some(handle) = self.handle.take() {
            handle.abort();
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
