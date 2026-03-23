use std::sync::Arc;
use zenoh::Session;
use async_trait::async_trait;
use bus::{RpcHandler, RpcServiceError, RpcServiceBuilder};
use serde_json::{json, Value};

pub async fn run_provider(session: Arc<Session>) -> anyhow::Result<()> {
    println!("[Provider] Starting...");

    let handler = ToolHandler;

    let service = RpcServiceBuilder::new()
        .service_name("demo-tools")
        .topic_prefix("agent/demo")
        .build()?
        .init(&session, handler)
        .await?;

    println!("[Provider] RPC service started at topic: {}", service.topic());
    println!("[Provider] Tools exposed: add, multiply");

    println!("[Provider] Ready to serve tools!");

    std::future::pending::<()>().await;

    Ok(())
}

struct ToolHandler;

#[async_trait]
impl RpcHandler for ToolHandler {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        println!("[Provider] Received RPC call: {}", method);

        let args: Value = serde_json::from_slice(payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let result = match method {
            "add" => {
                let a = args["a"].as_f64()
                    .ok_or_else(|| RpcServiceError::Internal("Missing 'a'".to_string()))?;
                let b = args["b"].as_f64()
                    .ok_or_else(|| RpcServiceError::Internal("Missing 'b'".to_string()))?;
                json!(a + b)
            }
            "multiply" => {
                let a = args["a"].as_f64()
                    .ok_or_else(|| RpcServiceError::Internal("Missing 'a'".to_string()))?;
                let b = args["b"].as_f64()
                    .ok_or_else(|| RpcServiceError::Internal("Missing 'b'".to_string()))?;
                json!(a * b)
            }
        _ => {
            return Err(RpcServiceError::Business {
                code: 404,
                message: format!("Unknown method: {}", method),
            });
        }
        };

        println!("[Provider] Returning result: {}", result);

        serde_json::to_vec(&result)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))
    }
}