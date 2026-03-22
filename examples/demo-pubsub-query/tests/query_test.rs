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

async fn setup_bus_or_skip() -> Option<std::sync::Arc<bus::Session>> {
    match setup_bus(None).await {
        Ok(session) => Some(session),
        Err(err) => {
            eprintln!("skipping Zenoh integration assertion: {err}");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_query() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let topic = "test/query/answer";

    let mut wrapper = QueryWrapper::new(topic);
    wrapper.init(session).await.unwrap();

    let req = QueryRequest {
        question: "test question".to_string(),
    };

    let result = wrapper
        .query_bytes(&DEFAULT_CODEC.encode(&req).unwrap())
        .await;
    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn demo_timeout() {
    let Some(session) = setup_bus_or_skip().await else {
        return;
    };
    let mut wrapper = QueryWrapper::new("nonexistent/topic");
    wrapper.init(session).await.unwrap();

    let req = QueryRequest {
        question: "test".to_string(),
    };
    let result = wrapper
        .query_bytes_with_timeout(
            &DEFAULT_CODEC.encode(&req).unwrap(),
            tokio::time::Duration::from_millis(50),
        )
        .await;
    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
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
