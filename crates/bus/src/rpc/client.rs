//! RPC client wrapper

use rkyv::{Archive, Deserialize, api::high::HighDeserializer, rancor::Error};

use std::sync::Arc;
use std::time::Duration;
use zenoh::Session;

use crate::codec::Codec;
use crate::rpc::error::RpcError;
use crate::rpc::service::RpcRequest;
use crate::rpc::service::RpcResponseEnvelope;

#[derive(Debug)]
pub struct RpcClient {
    service: String,
    method: String,
    topic: String,
    session: Option<Arc<Session>>,
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
            topic: format!("rpc/{}", service),
            service,
            method,
            session: None,
            timeout: self.timeout.unwrap_or(Duration::from_secs(5)),
        })
    }
}

impl RpcClient {
    pub fn new(service: impl Into<String>, method: impl Into<String>) -> Self {
        let service = service.into();
        let method = method.into();
        Self {
            topic: format!("rpc/{}", service),
            service,
            method,
            session: None,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn builder() -> RpcClientBuilder {
        RpcClientBuilder::new()
    }

    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), RpcError> {
        self.session = Some(session);
        Ok(())
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub fn is_initialized(&self) -> bool {
        self.session.is_some()
    }

    pub async fn call<T>(
        &self,
        payload: &[u8],
    ) -> Result<T, RpcError>
    where
        T: Archive,
        T::Archived: Deserialize<T, HighDeserializer<Error>>,
    {
        let req = RpcRequest {
            method: self.method.clone(),
            payload: payload.to_vec(),
        };
        let req_bytes = Codec
            .encode(&req)
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        let responses = self.call_raw(&req_bytes).await?;
        self.extract_single(responses).await
    }

    pub async fn call_all<T>(
        &self,
        payload: &[u8],
    ) -> Result<Vec<T>, RpcError>
    where
        T: Archive,
        T::Archived: Deserialize<T, HighDeserializer<Error>>,
    {
        let req = RpcRequest {
            method: self.method.clone(),
            payload: payload.to_vec(),
        };
        let req_bytes = Codec
            .encode(&req)
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        let responses = self.call_raw(&req_bytes).await?;
        let mut results = Vec::new();
        for bytes in responses {
            let envelope: RpcResponseEnvelope = Codec
                .decode(&bytes)
                .map_err(|e| RpcError::Serialization(e.to_string()))?;
            if envelope.status == "err" {
                let err = envelope.err.unwrap();
                return Err(RpcError::Serialization(format!(
                    "RpcResponse::Err in call_all: code={}, msg={}",
                    err.code, err.message
                )));
            }
            let ok_bytes = envelope.ok.ok_or_else(|| {
                RpcError::Serialization("Response missing ok data".to_string())
            })?;
            let result: T = Codec
                .decode(&ok_bytes)
                .map_err(|e| RpcError::Serialization(e.to_string()))?;
            results.push(result);
        }
        Ok(results)
    }

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

    async fn extract_single<T>(
        &self,
        responses: Vec<Vec<u8>>,
    ) -> Result<T, RpcError>
    where
        T: Archive,
        T::Archived: Deserialize<T, HighDeserializer<Error>>,
    {
        if responses.len() > 1 {
            return Err(RpcError::Serialization(format!(
                "Expected single response but got {} - use call_all() for multiple",
                responses.len()
            )));
        }
        let bytes = responses.into_iter().next().unwrap();
        let envelope: RpcResponseEnvelope = Codec
            .decode(&bytes)
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        if envelope.status == "err" {
            let err = envelope.err.unwrap();
            return Err(RpcError::Serialization(format!(
                "RpcResponse::Err: code={}, msg={}",
                err.code, err.message
            )));
        }
        let ok_bytes = envelope.ok.ok_or_else(|| {
            RpcError::Serialization("Response missing ok data".to_string())
        })?;
        let result: T = Codec
            .decode(&ok_bytes)
            .map_err(|e| RpcError::Serialization(e.to_string()))?;
        Ok(result)
    }
}

impl Clone for RpcClient {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            method: self.method.clone(),
            topic: self.topic.clone(),
            session: None,
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

        assert_eq!(client.topic(), "rpc/calculator");
        assert_eq!(client.timeout, Duration::from_secs(10));
        assert!(!client.is_initialized());
    }

    #[test]
    fn test_rpc_client_new() {
        let client = RpcClient::new("calculator", "add");

        assert_eq!(client.topic(), "rpc/calculator");
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
