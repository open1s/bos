#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "requires zenoh router"]
    async fn test_stream_tokens_over_bus() {
        use zenoh::Config;
        use crate::streaming::{PublisherWrapper, TokenPublisher};
        use crate::llm::StreamToken;

        let config = Config::from_key_value("mode/client", "true").unwrap();
        let session = zenoh::open(config).await.unwrap();

        let wrapper = Arc::new(PublisherWrapper::new(
            Arc::new(session),
            "test-agent".to_string(),
            "agent/test".to_string(),
        ));

        let publisher = TokenPublisher::new(wrapper.clone());

        publisher.publish("task-01".to_string(), StreamToken::Text("Hello".to_string())).await.unwrap();
        publisher.publish("task-01".to_string(), StreamToken::Text(" world".to_string())).await.unwrap();
        publisher.publish("task-01".to_string(), StreamToken::Done).await.unwrap();

        wrapper.flush().await.unwrap();

        assert!(wrapper.get_rate() > 0.0);
    }

    #[tokio::test]
    #[ignore = "requires zenoh router"]
    async fn test_batch_timeout_triggers_flush() {
        use zenoh::Config;
        use crate::streaming::PublisherWrapper;

        let config = Config::from_key_value("mode/client", "true").unwrap();
        let session = zenoh::open(config).await.unwrap();

        let wrapper = Arc::new(PublisherWrapper::new(
            Arc::new(session),
            "test-agent".to_string(),
            "agent/test".to_string(),
        ));

        wrapper.with_config(|bp| {
            let (max_size, max_tokens, timeout) = bp.get_batch_config();
            assert_eq!(timeout, Duration::from_millis(50));
        }).await;

        wrapper.publish_token("task-02".to_string(), StreamToken::Text("one".to_string())).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        wrapper.flush().await.unwrap();

        assert!(wrapper.get_rate() > 0.0);
    }

    #[tokio::test]
    #[ignore = "requires zenoh router"]
    async fn test_backpressure_reduces_rate() {
        use zenoh::Config;
        use crate::streaming::PublisherWrapper;

        let config = Config::from_key_value("mode/client", "true").unwrap();
        let session = zenoh::open(config).await.unwrap();

        let wrapper = Arc::new(PublisherWrapper::new(
            Arc::new(session),
            "test-agent".to_string(),
            "agent/test".to_string(),
        ));

        let initial_rate = wrapper.get_rate();

        wrapper.report_bus_load(0.9).await;

        tokio::time::sleep(Duration::from_millis(10)).await;

        let reduced_rate = wrapper.get_rate();

        assert!(reduced_rate < initial_rate);
        assert!(reduced_rate > 0.0);
    }

    #[tokio::test]
    #[ignore = "requires zenoh router - tests rate limiting"]
    async fn test_rate_limiter_limits_publishes() {
        use zenoh::Config;
        use crate::streaming::PublisherWrapper;
        use crate::llm::StreamToken;

        let config = Config::from_key_value("mode/client", "true").unwrap();
        let session = zenoh::open(config).await.unwrap();

        let wrapper = Arc::new(PublisherWrapper::new(
            Arc::new(session),
            "test-agent".to_string(),
            "agent/test".to_string(),
        ));

        let start = std::time::Instant::now();

        for i in 0..50 {
            wrapper.publish_token(format!("task-{:03}", i), StreamToken::Text(format!("token {}", i))).await.unwrap();
        }

        wrapper.flush().await.unwrap();

        let elapsed = start.elapsed();

        // With rate limiting, 50 tokens at 100/sec should take some time
        // Accounting for batching overhead, should be at least 300ms
        assert!(elapsed > Duration::from_millis(300));
    }
}