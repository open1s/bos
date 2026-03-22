//! Zenoh publisher wrapper with simplified API

use rkyv::{Archive, Serialize, ser::{allocator::ArenaHandle, sharing::Share, Serializer}, util::AlignedVec, rancor::{Error, Strategy}};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;

/// A publisher for sending messages to a Zenoh topic.
///
/// # Example
/// ```rust,ignore
/// // Create a publisher for a topic
/// let mut publisher = Publisher::new("chat/general");
/// publisher = publisher.with_session(session).await?;
///
/// // Publish a message
/// publisher.publish(&session, &"Hello, world!").await?;
///
/// // Or use with associated session
/// let publisher = Publisher::from_session("chat/general", session);
/// publisher.publish_with("Hello!").await?;
/// ```
pub struct Publisher {
    topic: String,
    session: Option<Arc<Session>>,
}

impl Publisher {
    /// Create a new publisher for the specified topic
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            session: None,
        }
    }

    /// Associate this publisher with a session and return it
    pub async fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
        self.session = Some(session);
        Ok(self)
    }

    /// Create a publisher directly from a session and topic
    pub fn from_session(topic: impl Into<String>, session: Arc<Session>) -> Self {
        Self {
            topic: topic.into(),
            session: Some(session),
        }
    }

    /// Publish a message to the topic
    pub async fn publish<T>(&self, session: &Session, payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        let codec = Codec;
        let data: Vec<u8> = codec.encode(payload)
            .map_err(|e: anyhow::Error| ZenohError::Serialization(e.to_string()))?;

        self.publish_raw(session, data).await
    }

    /// Publish raw bytes to the topic
    pub async fn publish_raw(&self, session: &Session, data: Vec<u8>) -> Result<(), ZenohError> {
        let publisher = session.declare_publisher(&self.topic).await?;
        publisher.put(data).await.map_err(ZenohError::from)
    }

    /// Publish using the associated session (must have called `with_session` or `from_session`)
    pub async fn publish_with<T>(&self, payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        self.publish(session, payload).await
    }

    /// Get the topic
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Get the associated session
    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
    }
}

impl Clone for Publisher {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            session: self.session.clone(),
        }
    }
}

/// Type alias for backward compatibility
pub type PublisherWrapper = Publisher;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publisher_new() {
        let publisher = Publisher::new("test/topic");
        assert_eq!(publisher.topic(), "test/topic");
        assert!(publisher.session().is_none());
    }

    #[test]
    fn test_publisher_clone() {
        let publisher = Publisher::new("test/topic");
        let cloned = publisher.clone();
        assert_eq!(cloned.topic(), publisher.topic());
    }
}
