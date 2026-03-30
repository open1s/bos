//! Zenoh publisher wrapper with simplified API

use rkyv::{
    rancor::{Error, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive, Serialize,
};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;

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
    pub fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
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
    pub async fn publish<T>(&self, payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let codec = Codec;
        let data: Vec<u8> = codec
            .encode(payload)
            .map_err(|e: anyhow::Error| ZenohError::Serialization(e.to_string()))?;

        self.publish_raw(session, data).await
    }

    /// Publish raw bytes to the topic
    async fn publish_raw(&self, session: &Session, data: Vec<u8>) -> Result<(), ZenohError> {
        let publisher = session.declare_publisher(&self.topic).await?;
        publisher.put(data).await.map_err(ZenohError::from)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bus, BusConfig, Subscriber};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_publisher_new() {
        let config = BusConfig::default();
        let bus = Bus::from(config).await;

        let mut subscriber = Subscriber::<String>::new("test/topic")
            .with_session(bus.clone().into())
            .await
            .unwrap();
        subscriber
            .run(|mesage| {
                println!("RE: {:?}", mesage);
            })
            .await
            .expect("TODO: panic message");

        let publisher = Publisher::new("test/topic")
            .with_session(bus.clone().into())
            .unwrap();
        assert_eq!(publisher.topic(), "test/topic");

        let _a = publisher
            .publish(&String::from("This is from publisher"))
            .await;

        //sleep
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        // h.abort()
    }
}
