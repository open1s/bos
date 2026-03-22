use bus::{rpc::{RpcHandler, RpcServiceError, RpcServiceBuilder}, DEFAULT_CODEC};
use tokio;
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: Vec<u8>,
}

#[derive(Clone)]
struct TestHandler;

#[async_trait::async_trait]
impl RpcHandler for TestHandler {
    async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        match method {
            "add" => {
                let json_payload = rkyv::from_bytes::<JsonPayload, rkyv::rancor::Error>(payload)
                    .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

                let value: serde_json::Value = serde_json::from_slice(&json_payload.json).unwrap();
                let a = value.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
                let b = value.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
                let result = a + b;
                let response = serde_json::json!({ "result": result });
                let response_bytes = serde_json::to_vec(&response).unwrap();
                let response_payload = JsonPayload { json: response_bytes };
                let encoded = DEFAULT_CODEC.encode(&response_payload).unwrap();
                Ok(encoded)
            }
            "echo" => Ok(payload.to_vec()),
            "error" => Err(RpcServiceError::Business {
                code: 500,
                message: "Test error".to_string(),
            }),
            _ => Err(RpcServiceError::Business {
                code: 404,
                message: format!("Unknown method: {}", method),
            }),
        }
    }
}

#[tokio::test]
async fn demo_rpc_add() {
    let handler = TestHandler;
    let response = serde_json::json!({ "a": 5i64, "b": 3i64 });
    let response_bytes = serde_json::to_vec(&response).unwrap();
    let json_payload = JsonPayload { json: response_bytes };
    
    let payload = DEFAULT_CODEC.encode(&json_payload).unwrap();
    
    let result = handler.handle("add", &payload).await;
    assert!(result.is_ok());
    
    let result_bytes = result.unwrap();
    let result_payload: JsonPayload = rkyv::from_bytes::<JsonPayload, rkyv::rancor::Error>(&result_bytes).unwrap();
    let result_value: serde_json::Value = serde_json::from_slice(&result_payload.json).unwrap();
    assert_eq!(result_value["result"], 8);
}

#[tokio::test]
async fn demo_rpc_echo() {
    let handler = TestHandler;
    let test_data = b"Hello, world!";
    
    let result = handler.handle("echo", test_data).await;
    assert!(result.is_ok());
    
    assert_eq!(result.unwrap(), test_data.to_vec());
}

#[tokio::test]
async fn demo_rpc_error() {
    let handler = TestHandler;
    let test_data = b"test payload";
    
    let result = handler.handle("error", test_data).await;
    assert!(result.is_err());
    
    match result.unwrap_err() {
        RpcServiceError::Business { code, message } => {
            assert_eq!(code, 500);
            assert!(message.contains("Test error"));
        }
        _ => panic!("Expected Business error"),
    }
}

#[tokio::test]
async fn demo_rpc_binary() {
    let handler = TestHandler;
    let test_data = b"binary data!";
    
    let result = handler.handle("echo", test_data).await;
    assert!(result.is_ok());
    
    assert_eq!(result.unwrap(), test_data.to_vec());
}

#[tokio::test]
async fn demo_rpc_concurrent() {
    use tokio::task::JoinSet;
    
    let handler = TestHandler;
    let mut tasks = JoinSet::new();
    
    for i in 0..10 {
        let handler_clone = handler.clone();
        tasks.spawn(async move {
            let response = serde_json::json!({ "a": i, "b": i });
            let response_bytes = serde_json::to_vec(&response).unwrap();
            let json_payload = JsonPayload { json: response_bytes };
            let payload = DEFAULT_CODEC.encode(&json_payload).unwrap();
            handler_clone.handle("add", &payload).await
        });
    }
    
    let mut results = 0;
    while let Some(Ok(result)) = tasks.join_next().await {
        assert!(result.is_ok());
        results += 1;
    }
    
    assert_eq!(results, 10);
}

#[tokio::test]
async fn demo_rpc_builder() {
    let builder = RpcServiceBuilder::new()
        .service_name("test-service")
        .topic_prefix("demo/test")
        .build();
    
    assert!(builder.is_ok());
    
    let uninit = builder.unwrap();
    assert_eq!(uninit.topic(), "demo/test/test-service");
}
