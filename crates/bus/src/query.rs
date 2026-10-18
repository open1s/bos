//! Zenoh query wrapper with simplified API

use rkyv::{
    rancor::{Error, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive, Serialize,
};
use std::sync::Arc;
use zenoh::query::ConsolidationMode;
use zenoh::Session;

use crate::{error::ZenohError, DEFAULT_CODEC};

/// Type alias for backward compatibility
pub type QueryWrapper = Query;

/// Sends rkyv-encoded queries and decodes the first reply.
pub struct Query {
    topic: String,
    session: Option<Arc<Session>>,
}

impl Query {
    /// Create a new query client for the specified topic
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            session: None,
        }
    }

    /// Associate this query client with a session
    pub async fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
        self.session = Some(session);
        Ok(self)
    }

    /// Send a query and decode the first reply.
    pub async fn query<Q, R>(&self, payload: &Q) -> Result<R, ZenohError>
    where
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let bytes = codec
            .encode(payload)
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;

        let first = self.query_first_payload(&bytes, None).await?;
        codec
            .decode(first.as_slice())
            .map_err(|e| ZenohError::Serialization(e.to_string()))
    }

    /// Send a query with a timeout and decode the first reply.
    pub async fn query_with_timeout<Q, R>(
        &self,
        payload: &Q,
        timeout: std::time::Duration,
    ) -> Result<R, ZenohError>
    where
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let bytes = codec
            .encode(payload)
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;

        let first = self.query_first_payload(&bytes, Some(timeout)).await?;
        codec
            .decode(first.as_slice())
            .map_err(|e| ZenohError::Serialization(e.to_string()))
    }

    /// Send a query and process responses with a callback
    pub async fn stream_with_handler<Q, R>(
        &self,
        payload: &Q,
        mut handler: impl FnMut(R) -> anyhow::Result<R>,
    ) -> Result<Vec<R>, ZenohError>
    where
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let bytes = codec
            .encode(payload)
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;
        let replies = session
            .get(&self.topic)
            .payload(bytes)
            .consolidation(ConsolidationMode::None)
            .await?;

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let result: R = codec
                    .decode(sample.payload().to_bytes().as_ref())
                    .map_err(|e| ZenohError::Serialization(e.to_string()))?;
                match handler(result) {
                    Ok(result) => results.push(result),
                    Err(e) => {
                        return Err(ZenohError::Query(format!("Handler error: {e:#}")));
                    }
                }
            }
        }
        Ok(results)
    }

    /// Send a query and yield individual results as they arrive.
    /// Returns a channel receiver that yields decoded results one at a time.
    pub async fn stream_channel(
        &self,
        payload: &[u8],
    ) -> Result<tokio::sync::mpsc::Receiver<Result<Vec<u8>, ZenohError>>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let replies = session
            .get(&self.topic)
            .payload(payload)
            .consolidation(ConsolidationMode::None)
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            while let Ok(reply) = replies.recv_async().await {
                match reply.result() {
                    Ok(sample) => {
                        let _ = tx.send(Ok(sample.payload().to_bytes().to_vec())).await;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(ZenohError::Query(e.to_string()))).await;
                        break;
                    }
                }
            }
        });
        Ok(rx)
    }

    /// Send a query and return the payload of the first successful reply.
    ///
    /// The reply stream is dropped as soon as the first reply arrives, so a
    /// broadcast query does not accumulate every reply in memory.
    async fn query_first_payload(
        &self,
        payload: &[u8],
        timeout: Option<std::time::Duration>,
    ) -> Result<Vec<u8>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;

        let replies = match timeout {
            Some(dur) => {
                session
                    .get(&self.topic)
                    .payload(payload)
                    .timeout(dur)
                    .await?
            }
            None => session.get(&self.topic).payload(payload).await?,
        };

        let mut last_error = None;
        while let Ok(reply) = replies.recv_async().await {
            match reply.result() {
                Ok(sample) => return Ok(sample.payload().to_bytes().into_owned()),
                Err(e) => {
                    // The reply payload carries the callable's error message;
                    // prefer it over Zenoh's opaque "N-byte payload" summary.
                    last_error = Some(match e.payload().try_to_string() {
                        Ok(text) => text.into_owned(),
                        Err(_) => e.to_string(),
                    });
                }
            }
        }

        Err(ZenohError::Query(match last_error {
            Some(e) => format!("No successful reply received: {e}"),
            None => "No replies received — query may have timed out or no callable is listening"
                .to_string(),
        }))
    }

    /// Get the topic
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl Clone for Query {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            session: self.session.clone(),
        }
    }
}

/// Convenient alias for queryables
pub type Queryable = Query;

/// Convenient alias for typed queryables
pub type TopicQueryable = Query;
