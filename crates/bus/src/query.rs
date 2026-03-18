//! Zenoh query wrapper

use std::sync::Arc;
use zenoh::Session;

use crate::error::ZenohError;

pub struct QueryWrapper {
    topic: String,
    session: Option<Arc<Session>>,
}

impl QueryWrapper {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            session: None,
        }
    }

    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        self.session = Some(session);
        Ok(())
    }

    pub async fn query(&self, payload: &str) -> Result<Vec<Vec<u8>>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        self.query_internal(session, payload, None).await
    }

    pub async fn query_with_timeout(
        &self,
        payload: &str,
        timeout: std::time::Duration,
    ) -> Result<Vec<Vec<u8>>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
        self.query_internal(session, payload, Some(timeout)).await
    }

    async fn query_internal(
        &self,
        session: &Arc<Session>,
        payload: &str,
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

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn is_initialized(&self) -> bool {
        self.session.is_some()
    }

    pub fn session(&self) -> Option<&Arc<Session>> {
        self.session.as_ref()
    }

    pub async fn stream_reply<T>(
        &self,
        payload: &str,
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
}

impl Clone for QueryWrapper {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            session: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    const TEST_TOPIC: &str = "bos/test/query";

    async fn setup_query_wrapper() -> QueryWrapper {
        let config = crate::ZenohConfig::default();
        let manager = crate::SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let mut wrapper = QueryWrapper::new(TEST_TOPIC);
        wrapper.init(session).await.expect("Failed to init wrapper");
        wrapper
    }

    #[test]
    fn test_query_wrapper_new() {
        let wrapper = QueryWrapper::new("test/topic");
        assert_eq!(wrapper.topic(), "test/topic");
        assert!(!wrapper.is_initialized());
    }

    #[test]
    fn test_query_wrapper_clone() {
        let wrapper = QueryWrapper::new("test/topic");
        let cloned = wrapper.clone();
        assert_eq!(cloned.topic(), "test/topic");
        // Cloned wrapper should not be initialized
        assert!(!cloned.is_initialized());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_init() {
        let config = crate::ZenohConfig::default();
        let manager = crate::SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let mut wrapper = QueryWrapper::new(TEST_TOPIC);
        assert!(!wrapper.is_initialized());

        wrapper.init(session).await.expect("Failed to init wrapper");
        assert!(wrapper.is_initialized());
        assert!(wrapper.session().is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_query() {
        let wrapper = setup_query_wrapper().await;

        let payload = r#"{"test": "data"}"#;
        let result = wrapper.query(payload).await;

        // Query may return empty results if no queryable is registered
        // This test verifies the method doesn't panic
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_query_with_timeout() {
        let wrapper = setup_query_wrapper().await;

        let payload = r#"{"test": "data"}"#;
        let timeout = Duration::from_secs(1);
        let result = wrapper.query_with_timeout(payload, timeout).await;

        // Query may return empty results if no queryable is registered
        // This test verifies the method doesn't panic
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_integration() {
        use crate::QueryableWrapper;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Serialize, Deserialize)]
        struct TestQuery {
            query: String,
        }

        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestResponse {
            result: String,
        }

        let config = crate::ZenohConfig::default();
        let manager = crate::SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let mut queryable = QueryableWrapper::<TestQuery, TestResponse>::new(TEST_TOPIC)
            .with_handler(|q| {
                Ok(TestResponse {
                    result: q.query.to_uppercase(),
                })
            });

        queryable
            .init(&session)
            .await
            .expect("Failed to init queryable");

        let _handle = queryable
            .into_task()
            .expect("Failed to spawn queryable task");

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut wrapper = QueryWrapper::new(TEST_TOPIC);
        wrapper
            .init(session.clone())
            .await
            .expect("Failed to init querier");

        let payload = r#"{"query": "test"}"#;
        let results = wrapper.query(payload).await.expect("Query failed");

        assert_eq!(results.len(), 1, "Expected exactly one response");

        let response: TestResponse =
            serde_json::from_slice(&results[0]).expect("Failed to deserialize response");

        assert_eq!(response.result, "TEST");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_empty_payload() {
        let wrapper = setup_query_wrapper().await;

        let payload = "";
        let result = wrapper.query(payload).await;

        // Empty payload should still work
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_large_payload() {
        let wrapper = setup_query_wrapper().await;

        let large_payload = "x".repeat(10000);
        let result = wrapper.query(&large_payload).await;

        // Large payload should still work
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_timeout_behavior() {
        let wrapper = setup_query_wrapper().await;

        let payload = r#"{"test": "timeout"}"#;
        let short_timeout = Duration::from_millis(100);
        let result = wrapper.query_with_timeout(payload, short_timeout).await;

        // Should complete within timeout even with no queryable
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_not_connected_error() {
        let wrapper = QueryWrapper::new(TEST_TOPIC);

        let payload = r#"{"test": "error"}"#;
        let result = wrapper.query(payload).await;

        // Should return NotConnected error without init
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ZenohError::NotConnected));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_query_wrapper_clone_behavior() {
        let wrapper1 = setup_query_wrapper().await;
        let wrapper2 = wrapper1.clone();

        // Original should be initialized, clone should not
        assert!(wrapper1.is_initialized());
        assert!(!wrapper2.is_initialized());

        // Original should work, clone should fail with NotConnected
        let payload = r#"{"test": "clone"}"#;
        assert!(wrapper1.query(payload).await.is_ok());
        assert!(wrapper2.query(payload).await.is_err());
    }
}
