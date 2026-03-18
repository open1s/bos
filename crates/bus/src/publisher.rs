//! Zenoh publisher wrapper

use crate::{error::ZenohError, Codec, Session};
use serde::Serialize;
use std::sync::Arc;

pub struct PublisherWrapper {
    topic: String,
    codec: Codec,
    session: Option<Arc<Session>>,
}

impl PublisherWrapper {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            codec: Codec::default(),
            session: None,
        }
    }

    pub fn with_codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
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

    pub async fn publish<T: Serialize>(&self, session: &Session, payload: &T) -> Result<(), ZenohError> {
        let data: Vec<u8> = self
            .codec
            .encode(payload)
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

    pub fn codec(&self) -> Codec {
        self.codec
    }
}

impl Clone for PublisherWrapper {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            codec: self.codec,
            session: None,
        }
    }
}
