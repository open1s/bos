//! Zenoh query wrapper with simplified API

use std::sync::Arc;
use zenoh::Session;

use crate::error::ZenohError;

/// Type alias for backward compatibility
pub type QueryWrapper = Query;

/// A query client for sending queries and receiving responses from queryables.
///
/// # Example
/// ```rust,ignore
/// // Create a query client
/// let query = Query::new("calculator/add").with_session(session).await?;
///
/// // Send a query
/// let payload = Codec::encode(&(5, 3))?;
/// let responses = query.query_bytes(&payload).await?;
///
/// // Or use a callback for processing responses
/// let results = query.stream_reply(&payload, |bytes| {
///     let response: i32 = Codec::decode(&bytes)?;
///     Ok(response)
/// }).await?;
/// ```
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

    /// Initialize the query client with a session
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        self.session = Some(session);
        Ok(())
    }

    /// Associate this query client with a session
    pub async fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
        self.init(session).await?;
        Ok(self)
    }

    /// Create a query client directly from a session and topic
    pub async fn from_session(topic: impl Into<String>, session: Arc<Session>) -> Result<Self, ZenohError> {
        let mut query = Self::new(topic);
        query.init(session).await?;
        Ok(query)
    }

    /// Send a query with string payload
    pub async fn query(&self, payload: &str) -> Result<Vec<Vec<u8>>, ZenohError> {
        self.query_bytes(payload.as_bytes()).await
    }

    /// Send a query with bytes payload
    pub async fn query_bytes(&self, payload: &[u8]) -> Result<Vec<Vec<u8>>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        self.query_internal_bytes(session, payload, None).await
    }

    /// Send a query with string payload and timeout
    pub async fn query_with_timeout(
        &self,
        payload: &str,
        timeout: std::time::Duration,
    ) -> Result<Vec<Vec<u8>>, ZenohError> {
        self.query_bytes_with_timeout(payload.as_bytes(), timeout).await
    }

    /// Send a query with bytes payload and timeout
    pub async fn query_bytes_with_timeout(
        &self,
        payload: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Vec<Vec<u8>>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        self.query_internal_bytes(session, payload, Some(timeout)).await
    }

    /// Send a query and process responses with a callback
    pub async fn stream_reply<T>(
        &self,
        payload: &[u8],
        mut handler: impl FnMut(Vec<u8>) -> anyhow::Result<T>,
    ) -> Result<Vec<T>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        let replies = session.get(&self.topic).payload(payload).await?;

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                let data = sample.payload().to_bytes().to_vec();
                match handler(data) {
                    Ok(result) => results.push(result),
                    Err(e) => return Err(ZenohError::Serialization(e.to_string())),
                }
            }
        }

        Ok(results)
    }

    /// Send a query and decode single response
    pub async fn query_decode<T>(&self, payload: &[u8]) -> Result<T, ZenohError>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let responses = self.query_bytes(payload).await?;
        let bytes = responses.get(0).ok_or_else(|| ZenohError::Query("No response received".to_string()))?;
        serde_json::from_slice(bytes).map_err(|e| ZenohError::Serialization(e.to_string()))
    }

    async fn query_internal_bytes(
        &self,
        session: &Arc<Session>,
        payload: &[u8],
        timeout: Option<std::time::Duration>,
    ) -> Result<Vec<Vec<u8>>, ZenohError> {
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

    /// Check if the query client is initialized
    pub fn is_initialized(&self) -> bool {
        self.session.is_some()
    }

    /// Get the associated session
    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    const TEST_TOPIC: &str = "bos/test/query";

    async fn setup_session_or_skip() -> Option<Arc<zenoh::Session>> {
        let config = crate::ZenohConfig::default();
        let manager = crate::SessionManager::new(config);
        match manager.connect().await {
            Ok(session) => Some(session),
            Err(err) => {
                eprintln!("skipping Zenoh integration test: {err}");
                None
            }
        }
    }

    async fn setup_query_wrapper() -> Option<Query> {
        let session = setup_session_or_skip().await?;
        let mut wrapper = Query::new(TEST_TOPIC);
        wrapper.init(session).await.expect("Failed to init wrapper");
        Some(wrapper)
    }

    #[test]
    fn test_query_wrapper_new() {
        let wrapper = Query::new("test/topic");
        assert_eq!(wrapper.topic(), "test/topic");
        assert!(!wrapper.is_initialized());
    }

    #[test]
    fn test_query_wrapper_clone() {
        let wrapper = Query::new("test/topic");
        let cloned = wrapper.clone();
        assert_eq!(cloned.topic(), "test/topic");
        assert!(!cloned.is_initialized());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_init() {
        let Some(session) = setup_session_or_skip().await else {
            return;
        };

        let mut wrapper = Query::new(TEST_TOPIC);
        assert!(!wrapper.is_initialized());

        wrapper.init(session).await.expect("Failed to init wrapper");
        assert!(wrapper.is_initialized());
        assert!(wrapper.session().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_query() {
        let Some(wrapper) = setup_query_wrapper().await else {
            return;
        };

        let payload = r#"{"test": "data"}"#;
        let result = wrapper.query(payload).await;

        // Query may return empty results if no queryable is registered
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_query_with_timeout() {
        let Some(wrapper) = setup_query_wrapper().await else {
            return;
        };

        let payload = r#"{"test": "data"}"#;
        let timeout = Duration::from_secs(1);
        let result = wrapper.query_with_timeout(payload, timeout).await;

        // Query may return empty results if no queryable is registered
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_empty_payload() {
        let Some(wrapper) = setup_query_wrapper().await else {
            return;
        };

        let payload = "";
        let result = wrapper.query(payload).await;

        // Empty payload should still work
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_large_payload() {
        let Some(wrapper) = setup_query_wrapper().await else {
            return;
        };

        let large_payload = "x".repeat(10000);
        let result = wrapper.query(&large_payload).await;

        // Large payload should still work
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_not_connected_error() {
        let wrapper = Query::new(TEST_TOPIC);

        let payload = r#"{"test": "error"}"#;
        let result = wrapper.query(payload).await;

        // Should return NotConnected error without init
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ZenohError::NotConnected));
    }
}
