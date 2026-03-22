use agent::streaming::{SseDecoder, SseEvent};
use agent::streaming::TokenPublisherWrapper;
use agent::llm::StreamToken;

#[test]
fn demo_sse_decode() {
    let mut decoder = SseDecoder::new();
    let chunk = b"data: {\"x\":1}\n\ndata: [DONE]\n\n";
    let events = decoder.decode_chunk(chunk);

    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], SseEvent::Data(_)));
    assert!(matches!(events[1], SseEvent::Done));
}

#[tokio::test]
#[ignore] // Requires Zenoh router
async fn demo_token_publish() {
    use brainos_common::setup_bus;

    let session = setup_bus(None).await.expect("failed to setup bus");
    let publisher = TokenPublisherWrapper::new(
        session.clone(),
        "test-agent".to_string(),
        "test/streaming".to_string(),
    );

    let task_id = uuid::Uuid::new_v4().to_string();
    let token = StreamToken::Text("Hello".to_string());

    let result = publisher.publish_token(&task_id, token).await;
    assert!(result.is_ok(), "publish_token should succeed");

    publisher.flush().await.expect("flush should succeed");
}

#[tokio::test]
#[ignore] // Requires Zenoh router and load simulation
async fn demo_rate_limit() {
    // Implementation would publish 1000 tokens, measure throughput
    // Expect rate throttled to ~100/sec
    let _ = "rate limiting test placeholder - requires Zenoh router";
}

#[tokio::test]
#[ignore] // Requires Zenoh router and load simulation
async fn demo_backpressure() {
    // Simulate 80% bus load, verify rate decreases
    // Simulate 40% load, verify rate increases
    let _ = "backpressure test placeholder - requires Zenoh router";
}
