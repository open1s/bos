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

    /// Create a query client directly from a session and topic
    pub async fn from_session(
        topic: impl Into<String>,
        session: Arc<Session>,
    ) -> Result<Self, ZenohError> {
        let mut query = Self::new(topic);
        query.session = Some(session);
        Ok(query)
    }

    /// Send a query with string payload
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

        let results = self.query_internal_bytes(&bytes, None).await?;

        if results.is_empty() {
            return Err(ZenohError::Query(
                "No replies received — query may have timed out or no callable is listening"
                    .to_string(),
            ));
        }

        let result: R = codec
            .decode(results[0].as_slice())
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;
        Ok(result)
    }

    /// Send a query with string payload and timeout
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

        let results = self.query_internal_bytes(&bytes, Some(timeout)).await?;
        let result: R = codec
            .decode(results[0].as_slice())
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;
        Ok(result)
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
                    Err(e) => return Err(ZenohError::Serialization(e.to_string())),
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

    /// Send a query and process responses with a callback
    pub async fn stream<Q, R>(&self, payload: &Q) -> Result<Vec<R>, ZenohError>
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
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let replies = session
            .get(&self.topic)
            .payload(bytes)
            .consolidation(ConsolidationMode::None)
            .await?;

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let decoded = codec
                    .decode(sample.payload().to_bytes().as_ref())
                    .map_err(|e| ZenohError::Serialization(e.to_string()))?;
                results.push(decoded);
            }
        }
        Ok(results)
    }

    async fn query_internal_bytes(
        &self,
        payload: &[u8],
        timeout: Option<std::time::Duration>,
    ) -> Result<Vec<Vec<u8>>, ZenohError> {
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

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                results.push(sample.payload().to_bytes().to_vec());
            }
        }

        Ok(results)
    }

    /// Compat shim for old naming
    pub async fn query_with_timeoutquery<Q, R>(
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
        self.query_with_timeout(payload, timeout).await
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
