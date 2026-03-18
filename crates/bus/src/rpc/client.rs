//! RPC client wrapper

use std::sync::Arc;
use std::time::Duration;
use zenoh::Session;

use crate::rpc::error::RpcError;
use crate::rpc::types::RpcResponse;
use crate::JsonCodec;

/// RPC client for calling remote services.
///
/// Uses topic pattern: `/rpc/{service}/{method}`
///
/// # Example
/// ```rust,ignore
/// let client = RpcClient::new("calculator".to_string(), "add".to_string())
///     .timeout(Duration::from_secs(5));
/// client.init(session).await?;
/// let result: i32 = client.call(&[1, 2]).await?;
/// ```
#[derive(Debug)]
pub struct RpcClient {
    service: String,
    method: String,
    topic: String,
    session: Option<Arc<Session>>,
    codec: JsonCodec,
    timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct RpcClientBuilder {
    service: Option<String>,
    method: Option<String>,
    timeout: Option<Duration>,
}

impl RpcClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Result<RpcClient, RpcError> {
        let service = self.service.ok_or_else(|| {
            RpcError::Serialization("RpcClientBuilder: service not set".to_string())
        })?;
        let method = self.method.ok_or_else(|| {
            RpcError::Serialization("RpcClientBuilder: method not set".to_string())
        })?;
        Ok(RpcClient {
            topic: format!("/rpc/{}/{}", service, method),
            service,
            method,
            session: None,
            codec: JsonCodec,
            timeout: self.timeout.unwrap_or(Duration::from_secs(5)),
        })
    }
}

impl RpcClient {
    /// Create a new RpcClient for a service+method.
    /// Convenience constructor equivalent to builder.
    pub fn new(service: impl Into<String>, method: impl Into<String>) -> Self {
        let service = service.into();
        let method = method.into();
        Self {
            topic: format!("/rpc/{}/{}", service, method),
            service,
            method,
            session: None,
            codec: JsonCodec,
            timeout: Duration::from_secs(5),
        }
    }

    /// Create a builder for RpcClient.
    pub fn builder() -> RpcClientBuilder {
        RpcClientBuilder::new()
    }

    /// Initialize with a Zenoh session.
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), RpcError> {
        self.session = Some(session);
        Ok(())
    }

    /// Get the full topic path.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Check if initialized.
    pub fn is_initialized(&self) -> bool {
        self.session.is_some()
    }

    /// Call the service and return a single response.
    ///
    /// Deserializes the first RpcResponse::Ok response.
    /// Returns RpcError::NotFound if no responses received.
    /// Returns error if multiple responses received.
    pub async fn call<T: serde::de::DeserializeOwned>(
        &self,
        payload: impl serde::Serialize,
    ) -> Result<T, RpcError> {
        let payload_bytes = self.codec.encode(&payload)?;
        let responses = self.call_raw(&payload_bytes).await?;
        self.extract_single(responses).await
    }

    /// Call the service and return ALL responses (including from multiple services).
    pub async fn call_all<T: serde::de::DeserializeOwned>(
        &self,
        payload: impl serde::Serialize,
    ) -> Result<Vec<T>, RpcError> {
        let payload_bytes = self.codec.encode(&payload)?;
        let responses = self.call_raw(&payload_bytes).await?;
        let mut results = Vec::new();
        for bytes in responses {
            match self.codec.decode::<RpcResponse<T>>(&bytes) {
                Ok(RpcResponse::Ok(v)) => results.push(v),
                Ok(RpcResponse::Err { code, message }) => {
                    return Err(RpcError::Serialization(format!(
                        "Unexpected RpcResponse::Err in call_all: code={}, msg={}",
                        code, message
                    )));
                }
                Err(e) => {
                    return Err(RpcError::Serialization(e.to_string()));
                }
            }
        }
        Ok(results)
    }

    /// Low-level query sending. Returns raw byte vectors.
    async fn call_raw(&self, payload: &[u8]) -> Result<Vec<Vec<u8>>, RpcError> {
        let session = self
            .session
            .as_ref()
            .ok_or(RpcError::Serialization(
                "RpcClient not initialized".to_string(),
            ))?;

        let replies = session
            .get(&self.topic)
            .payload(payload)
            .timeout(self.timeout)
            .await
            .map_err(|e| {
                if e.to_string().contains("timeout") {
                    RpcError::Timeout {
                        timeout_ms: self.timeout.as_millis() as u64,
                    }
                } else {
                    RpcError::Network(e.to_string())
                }
            })?;

        let mut results = Vec::new();
        while let Ok(reply) = replies.recv_async().await {
            if let Ok(sample) = reply.result() {
                results.push(sample.payload().to_bytes().to_vec());
            }
        }

        if results.is_empty() {
            return Err(RpcError::NotFound {
                topic: self.topic.clone(),
            });
        }

        Ok(results)
    }

    /// Extract single value from responses.
    async fn extract_single<T: serde::de::DeserializeOwned>(
        &self,
        responses: Vec<Vec<u8>>,
    ) -> Result<T, RpcError> {
        if responses.len() > 1 {
            return Err(RpcError::Serialization(format!(
                "Expected single response but got {} - use call_all() for multiple",
                responses.len()
            )));
        }
        let bytes = responses.into_iter().next().unwrap();
        let response: RpcResponse<T> = self.codec.decode(&bytes)?;
        response.into_result().map_err(|(code, msg)| {
            RpcError::Serialization(format!("RpcResponse::Err: code={}, msg={}", code, msg))
        })
    }
}

impl Clone for RpcClient {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            method: self.method.clone(),
            topic: self.topic.clone(),
            session: None, // Clones drop session (matches QueryWrapper pattern)
            codec: JsonCodec,
            timeout: self.timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_client_builder() {
        let client = RpcClient::builder()
            .service("calculator")
            .method("add")
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build client");

        assert_eq!(client.topic(), "/rpc/calculator/add");
        assert_eq!(client.timeout, Duration::from_secs(10));
        assert!(!client.is_initialized());
    }

    #[test]
    fn test_rpc_client_new() {
        let client = RpcClient::new("calculator", "add");

        assert_eq!(client.topic(), "/rpc/calculator/add");
        assert_eq!(client.timeout, Duration::from_secs(5));
        assert!(!client.is_initialized());
    }

    #[test]
    fn test_rpc_client_builder_missing_service() {
        let result = RpcClient::builder().method("add").build();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RpcError::Serialization(_)));
    }

    #[test]
    fn test_rpc_client_builder_missing_method() {
        let result = RpcClient::builder().service("calculator").build();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RpcError::Serialization(_)));
    }

    #[test]
    fn test_rpc_client_clone() {
        let client = RpcClient::new("calculator", "add");
        let cloned = client.clone();

        assert_eq!(cloned.topic(), client.topic());
        assert!(!cloned.is_initialized());
    }
}
