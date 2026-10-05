use std::sync::Arc;

use async_trait::async_trait;
use bus::{Caller, Session, DEFAULT_CODEC};
use tokio_stream::wrappers::ReceiverStream;
use zenoh::query::ConsolidationMode;

use super::wire::*;
use react::ToolError;

#[async_trait]
pub(crate) trait RpcTransport: Send + Sync {
    async fn request(&self, payload: &str) -> Result<String, ToolError>;
    async fn request_stream(&self, payload: &str) -> Result<RpcResponseStream, ToolError>;
}

pub(crate) struct BusCallerTransport {
    pub(crate) caller: Caller,
    pub(crate) endpoint: String,
    pub(crate) session: Arc<Session>,
}

#[async_trait]
impl RpcTransport for BusCallerTransport {
    async fn request(&self, payload: &str) -> Result<String, ToolError> {
        self.caller
            .call::<String, String>(&payload.to_string())
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))
    }

    async fn request_stream(&self, payload: &str) -> Result<RpcResponseStream, ToolError> {
        let bytes = DEFAULT_CODEC
            .encode(&payload.to_string())
            .map_err(|e| ToolError::Failed(format!("encode request failed: {}", e)))?;

        let replies = self
            .session
            .get(&self.endpoint)
            .payload(bytes)
            .consolidation(ConsolidationMode::None)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            while let Ok(reply) = replies.recv_async().await {
                match reply.result() {
                    Ok(sample) => {
                        let payload = sample.payload().to_bytes();
                        match DEFAULT_CODEC.decode::<String>(payload.as_ref()) {
                            Ok(decoded) => {
                                if tx.send(Ok(decoded)).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(Err(ToolError::Failed(format!(
                                        "decode response failed: {}",
                                        e
                                    ))))
                                    .await;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(ToolError::Failed(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}
