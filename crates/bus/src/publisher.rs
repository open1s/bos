//! Zenoh publisher wrapper

use crate::{error::ZenohError, JsonCodec, Session};
use serde::Serialize;
use std::sync::Arc;

/// Cached Zenoh publisher for high-performance publishing
type CachedPublisher = zenoh::pubsub::Publisher<zenoh::handlers::DefaultHandler>;

pub struct PublisherWrapper {
    topic: String,
    codec: JsonCodec,
    session: Option<Arc<Session>>,
    // CRITICAL: Cache the declared publisher to avoid declaration overhead on every publish()
    publisher: Option<CachedPublisher>,
}

impl PublisherWrapper {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            codec: JsonCodec,
            session: None,
            publisher: None,
        }
    }

    /// Initialize the publisher with a session and declare the Zenoh publisher once.
    /// This is now efficient - the publisher is declared once and reused for all publish calls.
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        self.session = Some(session.clone());
        self.publisher = Some(session.declare_publisher(&self.topic).await?);
        Ok(())
    }

    /// Create a new PublisherWrapper with a session in one step (fluent builder pattern)
    pub fn with_session(mut self, session: Arc<Session>) -> Self {
        self.session = Some(session);
        self
    }

    /// Create a PublisherWrapper with session pre-attached (convenience alias)
    pub async fn with_connected_session(
        topic: impl Into<String>,
        session: Arc<Session>,
    ) -> Result<Self, ZenohError> {
        let mut wrapper = Self::new(topic);
        wrapper.init(session).await?;
        Ok(wrapper)
    }

    pub async fn publish<T: Serialize>(&self, payload: &T) -> Result<(), ZenohError> {
        let publisher = self.publisher.as_ref().ok_or(ZenohError::NotConnected)?;

        let data = self
            .codec
            .encode(payload)
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;

        publisher.put(data).await?;

        Ok(())
    }

    pub async fn publish_raw(&self, data: Vec<u8>) -> Result<(), ZenohError> {
        let publisher = self.publisher.as_ref().ok_or(ZenohError::NotConnected)?;
        publisher.put(data).await?;
        Ok(())
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn is_initialized(&self) -> bool {
        self.publisher.is_some()
    }

    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
    }

    /// Clone without the publisher/session (for re-initialization with a new session).
    /// To preserve the session, use `clone_with_session()` instead.
    pub fn clone_without_session(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            codec: JsonCodec,
            session: None,
            publisher: None,
        }
    }

    /// Clone with an explicit session (useful for sharing publisher config across sessions)
    pub async fn clone_with_session(&self, session: Arc<Session>) -> Result<Self, ZenohError> {
        let mut wrapper = Self::new(self.topic.clone());
        wrapper.init(session).await?;
        Ok(wrapper)
    }
}

impl Clone for PublisherWrapper {
    /// Clones the publisher without the session.
    /// This maintains backward compatibility while making the behavior explicit via clone_without_session() and clone_with_session().
    fn clone(&self) -> Self {
        self.clone_without_session()
    }
}
