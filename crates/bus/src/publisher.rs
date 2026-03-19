//! Zenoh publisher wrapper

use rkyv::{Archive, Serialize, ser::{allocator::ArenaHandle, sharing::Share, Serializer}, util::AlignedVec, rancor::{Error, Strategy}};

use crate::{error::ZenohError, Codec, Session};
use std::sync::Arc;

pub struct PublisherWrapper {
    topic: String,
    session: Option<Arc<Session>>,
}

impl PublisherWrapper {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            session: None,
        }
    }

    pub fn with_session(mut self, session: &Session) -> Self {
        self.session = Some(Arc::new(session.clone()));
        self
    }

    pub async fn with_connected_session(topic: impl Into<String>, session: &Session) -> Result<Self, ZenohError> {
        let mut wrapper = Self::new(topic);
        wrapper.session = Some(Arc::new(session.clone()));
        Ok(wrapper)
    }

    pub async fn publish<T>(&self, session: &Session, payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        let codec = Codec;
        let data: Vec<u8> = codec.encode(payload)
            .map_err(|e: anyhow::Error| ZenohError::Serialization(e.to_string()))?;

        let pub_: zenoh::pubsub::Publisher<'_> =
            session.declare_publisher(&self.topic).await?;
        pub_.put(data).await.map_err(ZenohError::from)
    }

    pub async fn publish_raw(&self, session: &Session, data: Vec<u8>) -> Result<(), ZenohError> {
        let pub_: zenoh::pubsub::Publisher<'_> =
            session.declare_publisher(&self.topic).await?;
        pub_.put(data).await.map_err(ZenohError::from)
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
    }
}

impl Clone for PublisherWrapper {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            session: None,
        }
    }
}
