//! Zenoh query wrapper with simplified API

use std::sync::Arc;
use rkyv::{
    Archive, Serialize, rancor::{Error, Strategy}, ser::{Serializer, allocator::ArenaHandle, sharing::Share}, util::AlignedVec
};
use zenoh::Session;

use crate::{DEFAULT_CODEC, error::ZenohError};

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
    pub async fn from_session(topic: impl Into<String>, session: Arc<Session>) -> Result<Self, ZenohError> {
        let mut query = Self::new(topic);
        query.session = Some(session);
        Ok(query)
    }

    /// Send a query with string payload
    pub async fn query<Q, R>(&self, payload: &Q) -> Result<R, ZenohError> where 
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let bytes = codec.encode(payload).unwrap();

        let results = self.query_internal_bytes(&bytes, None).await?;

        let result: R = codec.decode(results[0].as_slice()).unwrap();
        Ok(result)
    }

    /// Send a query with string payload and timeout
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
        let codec = DEFAULT_CODEC;
        let bytes = codec.encode(payload).unwrap();

        let results = self.query_internal_bytes(&bytes, Some(timeout)).await?;
        let result: R = codec.decode(results[0].as_slice()).unwrap();
        Ok(result)
    }

    /// Send a query and process responses with a callback
    pub async fn stream_with_handler<Q, R>(
        &self,
        payload: &Q,
        mut handler: impl FnMut(R) -> anyhow::Result<R>,
    ) -> Result<Vec<R>, ZenohError> where 
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,  
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let bytes = codec.encode(payload).unwrap();
        let replies = session.get(&self.topic).payload(bytes).await?;

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let result: R = codec.decode(&sample.payload().to_bytes().to_vec()).unwrap();
                match handler(result) {
                    Ok(result) => results.push(result),
                    Err(e) => return Err(ZenohError::Serialization(e.to_string())),
                }
            }
        }
        Ok(results)
    }

    /// Send a query and process responses with a callback
    pub async fn stream<Q, R>(
        &self,
        payload: &Q
    ) -> Result<Vec<R>, ZenohError> where
        Q: Archive,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
        R: Archive,
        R::Archived: rkyv::Deserialize<R, rkyv::api::high::HighDeserializer<Error>>,
    {
        let codec = DEFAULT_CODEC;
        let bytes = codec.encode(payload).unwrap();
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let replies = session.get(&self.topic).payload(bytes).await?;
        
        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let decoded= codec.decode(&sample.payload().to_bytes().to_vec());
                match decoded {
                    Ok(result) => results.push(result),
                    Err(e) => return Err(ZenohError::Serialization(e.to_string())),
                }
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

    /// Get the topic
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl Clone for Query {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            session: None,
        }
    }
}

/// Convenient alias for queryables
pub type Queryable = Query;

/// Convenient alias for typed queryables
pub type TopicQueryable = Query;