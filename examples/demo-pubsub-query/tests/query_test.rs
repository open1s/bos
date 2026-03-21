use bus::{QueryWrapper, DEFAULT_CODEC};
use brainos_common::setup_bus;
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct QueryRequest {
    question: String,
}

#[derive(Archive, Serialize, Deserialize)]
struct QueryResponse {
    answer: String,
    timestamp: u64,
}

#[tokio::test]
async fn demo_query() {
    let session = setup_bus(None).await.unwrap();
    let topic = "test/query/answer";
    
    let wrapper = QueryWrapper::new(&topic);
    
    let req = QueryRequest {
        question: "test question".to_string(),
    };
    
    let result = wrapper.query(&session, DEFAULT_CODEC.encode(&req).unwrap(), None);
    assert!(result.is_err()); // No queryable exists
}

#[tokio::test]
async fn demo_timeout() {
    let session = setup_bus(None).await.unwrap();
    let wrapper = QueryWrapper::new("nonexistent/topic");
    
    let req = QueryRequest {
        question: "test".to_string(),
    };
    let result = wrapper.query(&session, DEFAULT_CODEC.encode(&req).unwrap(), 
        Some(tokio::time::Duration::from_millis(50)));
    assert!(result.is_err()); // Should timeout
}

#[tokio::test]
async fn demo_json_codec() {
    let msg = QueryResponse {
        answer: "test".to_string(),
        timestamp: 12345,
    };
    
    let encoded = DEFAULT_CODEC.encode(&msg).unwrap();
    let decoded: QueryResponse = DEFAULT_CODEC.decode(&encoded).unwrap();
    
    assert_eq!(decoded.answer, "test");
    assert_eq!(decoded.timestamp, 12345);
}
