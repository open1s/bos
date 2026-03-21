use std::sync::Arc;
use tokio::signal;

use bus::{
    rpc::{RpcServiceBuilder, RpcHandler, RpcServiceError, RpcService},
    DEFAULT_CODEC,
};

use brainos_common::{setup_bus, setup_logging};
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: String,
}

/// Add service handler - demonstrates basic arithmetic
#[derive(Clone)]
struct AddHandler;

#[async_trait::async_trait]
impl RpcHandler for AddHandler {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        if method != "add" {
            return Err(RpcServiceError::Business {
                code: 400,
                message: format!("Method '{}' not supported", method),
            });
        }

        // Deserialize using rkyv
        let json_payload: JsonPayload = rkyv::from_bytes::<JsonPayload, rkyv::rancor::Error>(payload)
            .map_err(|e| RpcServiceError::Internal(format!("rkyv deserialization failed: {}", e)))?;

        // Parse JSON from the string
        let value: serde_json::Value = serde_json::from_str(&json_payload.json)
            .map_err(|e| RpcServiceError::Business {
                code: 400,
                message: format!("Invalid JSON: {}", e),
            })?;

        let a = value.get("a")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| RpcServiceError::Business {
                code: 400,
                message: "Missing or invalid field 'a'".to_string(),
            })?;

        let b = value.get("b")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| RpcServiceError::Business {
                code: 400,
                message: "Missing or invalid field 'b'".to_string(),
            })?;

        let result = a + b;
        let response = serde_json::json!({ "result": result });
        let response_str = serde_json::to_string(&response).unwrap();
        let response_payload = JsonPayload { json: response_str };

        // Serialize using rkyv
        DEFAULT_CODEC.encode(&response_payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))
    }
}

/// Echo service handler - demonstrates simple echo
#[derive(Clone)]
struct EchoHandler;

#[async_trait::async_trait]
impl RpcHandler for EchoHandler {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        if method != "echo" {
            return Err(RpcServiceError::Business {
                code: 400,
                message: format!("Method '{}' not supported", method),
            });
        }

        // For echo, return the payload as-is (binary data)
        Ok(payload.to_vec())
    }
}

/// Error handler - demonstrates error handling
#[derive(Clone)]
struct ErrorHandler;

#[async_trait::async_trait]
impl RpcHandler for ErrorHandler {
    async fn handle(&self, method: &str, _payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        match method {
            "business_error" => Err(RpcServiceError::Business {
                code: 403,
                message: "Business rule violation".to_string(),
            }),
            "internal_error" => Err(RpcServiceError::Internal(
                "Internal server error".to_string(),
            )),
            _ => Err(RpcServiceError::Business {
                code: 404,
                message: format!("Method '{}' not found", method),
            }),
        }
    }
}

/// Binary handler - demonstrates binary payload processing
#[derive(Clone)]
struct BinaryHandler;

#[async_trait::async_trait]
impl RpcHandler for BinaryHandler {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        if method != "binary" {
            return Err(RpcServiceError::Business {
                code: 400,
                message: format!("Method '{}' not supported", method),
            });
        }

        // Process binary data: reverse the bytes
        let reversed: Vec<u8> = payload.iter().rev().copied().collect();
        Ok(reversed)
    }
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║  RPC Service Demo - Phase 1 Plan 01     ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;

    // Service 1: Add using RpcServiceBuilder (builder pattern)
    println!("Service 1: Add Service (Builder Pattern)");
    println!("  - Method: add(a, b) -> result\n");
    let add_service = RpcServiceBuilder::new()
        .service_name("add-service")
        .topic_prefix("demo/rpc")
        .build()?
        .init(&session, AddHandler)
        .await?;
    println!("✓ Add service registered at: {}", add_service.topic());

    // Service 2: Echo using RpcServiceBuilder
    println!("\nService 2: Echo Service (Builder Pattern)");
    println!("  - Method: echo(payload) -> payload\n");
    let echo_service = RpcServiceBuilder::new()
        .service_name("echo-service")
        .topic_prefix("demo/rpc")
        .build()?
        .init(&session, EchoHandler)
        .await?;
    println!("✓ Echo service registered at: {}", echo_service.topic());

    // Service 3: Error using RpcServiceBuilder
    println!("\nService 3: Error Service (Builder Pattern)");
    println!("  - Methods: business_error, internal_error\n");
    let error_service = RpcServiceBuilder::new()
        .service_name("error-service")
        .topic_prefix("demo/rpc")
        .build()?
        .init(&session, ErrorHandler)
        .await?;
    println!("✓ Error service registered at: {}", error_service.topic());

    // Service 4: Binary using RpcService::new() (direct constructor)
    println!("\nService 4: Binary Service (Direct Constructor)");
    println!("  - Method: binary(payload) -> reversed(payload)\n");
    let mut binary_service = RpcService::new("demo/rpc/binary-service");
    binary_service.init(session.clone()).await?;
    binary_service.announce().await?;
    println!("✓ Binary service registered at: {}", binary_service.topic());

    // Display service information
    println!("\n{}", "=".repeat(50));
    println!("Services registered:");
    println!("  1. demo/rpc/add-service - AddService");
    println!("  2. demo/rpc/echo-service - EchoHandler");
    println!("  3. demo/rpc/error-service - ErrorHandler");
    println!("  4. demo/rpc/binary-service - BinaryHandler");
    println!("{}", "=".repeat(50));

    println!("\nPress Ctrl+C to exit...");
    signal::ctrl_c().await?;
    println!("\nGoodbye!");

    Ok(())
}
