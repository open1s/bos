//! RPC service implementation for handling incoming requests.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zenoh::Session;

use crate::queryable::QueryableWrapper;
use crate::rpc::error::RpcServiceError;
use crate::Codec;

#[async_trait]
pub trait RpcHandler: Send + Sync {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError>;
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RpcRequest {
    pub method: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponseEnvelope {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<RpcErrBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcErrBody {
    pub code: u32,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct RpcServiceBuilder {
    service_name: Option<String>,
    topic_prefix: Option<String>,
    codec: Option<Codec>,
}

impl RpcServiceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = Some(name.into());
        self
    }

    pub fn topic_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.topic_prefix = Some(prefix.into());
        self
    }

    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = Some(codec);
        self
    }

    pub fn build(self) -> Result<RpcServiceUninit, RpcServiceError> {
        let service_name = self.service_name.ok_or_else(|| {
            RpcServiceError::Internal("RpcServiceBuilder: service_name not set".to_string())
        })?;

        let topic = match self.topic_prefix {
            Some(prefix) => format!("{}/{}", prefix, service_name),
            None => format!("rpc/{}", service_name),
        };

        Ok(RpcServiceUninit {
            topic,
            codec: self.codec.unwrap_or_default(),
        })
    }
}

pub struct RpcServiceUninit {
    topic: String,
    codec: Codec,
}

impl RpcServiceUninit {
    pub fn topic(&self) -> &str {
        &self.topic
    }

    pub async fn init<H>(
        self,
        session: &Arc<Session>,
        handler: H,
    ) -> Result<RpcService, RpcServiceError>
    where
        H: RpcHandler + 'static,
    {
        let topic = self.topic.clone();
        let codec = self.codec;
        let handler = Arc::new(handler);

        let mut wrapper = QueryableWrapper::<RpcRequest, RpcResponseEnvelope>::new(&topic)
            .with_codec(codec)
            .with_handler(move |req: RpcRequest| {
                let handler = handler.clone();
                async move {
                    let result = handler.handle(&req.method, &req.payload).await;
                    match result {
                        Ok(data) => Ok(RpcResponseEnvelope {
                            status: "ok".to_string(),
                            ok: Some(data),
                            err: None,
                        }),
                        Err(e) => Ok(RpcResponseEnvelope {
                            status: "err".to_string(),
                            ok: None,
                            err: Some(RpcErrBody {
                                code: match e {
                                    RpcServiceError::Business { code, .. } => code,
                                    RpcServiceError::Internal(_) => 500,
                                },
                                message: e.to_string(),
                            }),
                        }),
                    }
                }
            });

        wrapper
            .init(session)
            .await
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let service = RpcService {
            wrapper,
            session: Some(Arc::clone(session)),
            topic,
        };

        Ok(service)
    }
}

pub struct RpcService {
    wrapper: QueryableWrapper<RpcRequest, RpcResponseEnvelope>,
    session: Option<Arc<Session>>,
    topic: String,
}

impl RpcService {
    pub fn new(topic: impl Into<String>) -> Self {
        let topic = topic.into();
        Self {
            wrapper: QueryableWrapper::new(&topic),
            session: None,
            topic,
        }
    }

    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), RpcServiceError> {
        self.session = Some(session.clone());
        self.wrapper
            .init(&session)
            .await
            .map_err(|e| RpcServiceError::Internal(e.to_string()))
    }

    /// Publish service discovery info to `rpc/services/{service_name}`.
    /// Call this after init() to announce the service for discovery.
    pub async fn announce(&self) -> Result<(), crate::error::ZenohError> {
        let session = self.session.as_ref().ok_or(crate::error::ZenohError::NotConnected)?;
        let service_name = self.topic.trim_start_matches("rpc/");
        let info = crate::rpc::discovery::DiscoveryInfo::new(service_name);
        let topic = format!("rpc/services/{}", service_name);
        let data = self
            .wrapper
            .codec()
            .encode(&info)
            .map_err(|e| crate::error::ZenohError::Serialization(e.to_string()))?;

        session
            .declare_publisher(&topic)
            .await?
            .put(data)
            .await
            .map_err(|e| crate::error::ZenohError::Publisher(e.to_string()))?;

        Ok(())
    }

    pub fn into_task(
        self,
    ) -> Result<tokio::task::JoinHandle<Result<(), crate::error::ZenohError>>, RpcServiceError> {
        self.wrapper
            .into_task()
            .map_err(|e| RpcServiceError::Internal(e.to_string()))
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl Clone for RpcService {
    fn clone(&self) -> Self {
        Self {
            wrapper: self.wrapper.clone(),
            session: None,
            topic: self.topic.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_service_builder() {
        let uninit = RpcServiceBuilder::new()
            .service_name("test")
            .topic_prefix("custom")
            .build()
            .expect("Failed to build service");

        assert_eq!(uninit.topic(), "custom/test");
    }

    #[test]
    fn test_rpc_service_builder_default_topic() {
        let uninit = RpcServiceBuilder::new()
            .service_name("test")
            .build()
            .expect("Failed to build service");

        assert_eq!(uninit.topic(), "rpc/test");
    }

    #[test]
    fn test_rpc_service_new() {
        let service = RpcService::new("rpc/test");
        assert_eq!(service.topic(), "rpc/test");
    }

    #[test]
    fn test_rpc_response_envelope_ok() {
        let envelope = RpcResponseEnvelope {
            status: "ok".to_string(),
            ok: Some(vec![1, 2, 3]),
            err: None,
        };
        assert_eq!(envelope.status, "ok");
        assert!(envelope.ok.is_some());
        assert!(envelope.err.is_none());
    }

    #[test]
    fn test_rpc_response_envelope_err() {
        let envelope = RpcResponseEnvelope {
            status: "err".to_string(),
            ok: None,
            err: Some(RpcErrBody {
                code: 404,
                message: "Not found".to_string(),
            }),
        };
        assert_eq!(envelope.status, "err");
        assert!(envelope.ok.is_none());
        assert!(envelope.err.is_some());
    }

    #[test]
    fn test_rpc_request_deserialize() {
        let json = r#"{"method":"add","payload":[1,2,3]}"#;
        let req: RpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "add");
        assert_eq!(req.payload, vec![1, 2, 3]);
    }
}
