//! RPC (Remote Procedure Call) module
//!
//! Provides typed request/response communication over Zenoh.
//!
//! # Example
//! ```rust,ignore
//! use brickos_bus::{RpcClient, RpcClientBuilder, RpcResponse};
//!
//! // Using builder pattern
//! let client = RpcClient::builder()
//!     .service("calculator")
//!     .method("add")
//!     .timeout(Duration::from_secs(5))
//!     .build()?;
//!
//! // Or using new()
//! let client = RpcClient::new("calculator".to_string(), "add".to_string());
//!
//! // Initialize with session
//! client.init(session).await?;
//!
//! // Call the service
//! let result: i32 = client.call(&[1, 2]).await?;
//! ```

pub mod cache;
pub mod client;
pub mod discovery;
pub mod error;
pub mod health;
pub mod service;
pub mod types;

pub use cache::{CacheStats, ServiceCache};
pub use client::{RpcClient, RpcClientBuilder};
pub use discovery::{DiscoveryInfo, DiscoveryQueryBuilder, RpcDiscovery, DiscoveryRegistry};
pub use error::{RpcError, RpcServiceError};
pub use health::{HealthChecker, HealthPublisher, HealthStatus, ServiceState};
pub use service::{RpcHandler, RpcService, RpcServiceBuilder};
pub use types::RpcResponse;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use crate::rpc::error::RpcServiceError;
    use crate::rpc::service::RpcHandler;
    use crate::{Codec, DiscoveryInfo, DiscoveryRegistry, RpcClient, RpcServiceBuilder, SessionManager, ZenohConfig};

    struct TestServiceHandler;

    #[async_trait]
    impl RpcHandler for TestServiceHandler {
        async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
            match method {
                "echo" => Ok(payload.to_vec()),
                "add" => {
                    let (a, b): (i32, i32) = Codec::default()
                        .decode(payload)
                        .map_err(|e| RpcServiceError::Business {
                            code: 400,
                            message: e.to_string(),
                        })?;
                    let sum = a + b;
                    Codec::default()
                        .encode(&sum)
                        .map_err(|e| RpcServiceError::Business {
                            code: 500,
                            message: e.to_string(),
                        })
                }
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rpc_discovery_query() {
        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let service = RpcServiceBuilder::new()
            .service_name("discovery-query-test")
            .build()
            .expect("Failed to build service");

        let service = service
            .init(&session, TestServiceHandler)
            .await
            .expect("Failed to init service");

        // Subscribe first so we don't miss the announcement
        let topic = "rpc/services/discovery-query-test";
        let mut sub: crate::SubscriberWrapper<crate::rpc::discovery::DiscoveryInfo> =
            crate::SubscriberWrapper::new(topic);
        sub.init(&session).await.expect("Failed to init subscriber");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Now publish the announcement
        service.announce().await.expect("Failed to announce");
        let _handle = service.into_task().expect("Failed to spawn service");

        // Verify we can receive it
        let result = sub.recv_with_timeout(tokio::time::Duration::from_secs(1)).await;
        assert!(result.is_some(), "Should have received announcement");
        assert_eq!(result.unwrap().service_name, "discovery-query-test");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_discovery_pubsub() {
        use crate::Codec;
        use crate::rpc::discovery::DiscoveryInfo;

        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let service_name = "debug-service";
        let topic = format!("rpc/services/{}", service_name);
        let info = DiscoveryInfo::new(service_name);

        let mut sub: crate::SubscriberWrapper<DiscoveryInfo> = crate::SubscriberWrapper::new(&topic);
        sub.init(&session).await.expect("Failed to init subscriber");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let data = Codec::default()
            .encode(&info)
            .expect("Encode failed");
        let pub_ = session.declare_publisher(&topic).await.unwrap();
        pub_.put(data).await.unwrap();

        let result = sub.recv_with_timeout(tokio::time::Duration::from_secs(2)).await;
        assert!(result.is_some(), "Should have received the discovery info");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_zenos_pubsub_same_session() {
        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let topic = "debug/test-pubsub";
        
        let mut sub: crate::SubscriberWrapper<String> = crate::SubscriberWrapper::new(topic);
        sub.init(&session).await.expect("Failed to init subscriber");
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let pub_: crate::PublisherWrapper = crate::PublisherWrapper::new(topic)
            .with_session(&session);
        pub_.publish(&session, &"hello".to_string()).await.expect("Publish failed");
        let result = sub.recv_with_timeout(tokio::time::Duration::from_secs(1)).await;
        assert!(result.is_some(), "Should have received the published message");
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_rpc_full_cycle() {
        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let handler = TestServiceHandler;

        let service = RpcServiceBuilder::new()
            .service_name("test-service")
            .build()
            .expect("Failed to build service");

        let service = service
            .init(&session, handler)
            .await
            .expect("Failed to init service");

        let mut discovery_sub: crate::SubscriberWrapper<DiscoveryInfo> =
            crate::SubscriberWrapper::new("rpc/services/test-service");
        discovery_sub.init(&session).await.expect("Failed to init discovery subscriber");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        service.announce().await.expect("Failed to announce service");
        let _handle = service.into_task().expect("Failed to spawn service");
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let info = discovery_sub
            .recv_with_timeout(tokio::time::Duration::from_secs(2))
            .await
            .expect("Should have received discovery info");
        assert_eq!(info.service_name, "test-service");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.health_topic, "rpc/health/test-service");

        let mut echo_client = RpcClient::new("test-service", "echo");
        echo_client.init(session.clone()).await.expect("Failed to init echo client");

        let echo_payload: String = "hello".to_string();
        let echo_result: String = echo_client.call(&echo_payload).await.expect("Echo call failed");
        assert_eq!(echo_result, "hello");

        let mut add_client = RpcClient::new("test-service", "add");
        add_client.init(session.clone()).await.expect("Failed to init add client");

        let add_payload = (5, 3);
        let add_result: i32 = add_client.call(&add_payload).await.expect("Add call failed");
        assert_eq!(add_result, 8);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_discovery_registry_list_services() {
        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let registry = DiscoveryRegistry::new()
            .session(session.clone())
            .timeout(std::time::Duration::from_secs(5));

        let topic1 = "rpc/services/reg-test-1";
        let topic2 = "rpc/services/reg-test-2";

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let info1 = DiscoveryInfo::new("reg-test-1");
        let pub1 = session.declare_publisher(topic1).await.unwrap();
        pub1.put(Codec::default().encode(&info1).unwrap()).await.unwrap();

        let info2 = DiscoveryInfo::new("reg-test-2");
        let pub2 = session.declare_publisher(topic2).await.unwrap();
        pub2.put(Codec::default().encode(&info2).unwrap()).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let services = registry.list_services().await.expect("list_services failed");
        assert!(!services.is_empty(), "Should have received at least one service");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_health_publisher_checker() {
        use crate::rpc::health::{HealthChecker, HealthPublisher, ServiceState};

        let config = ZenohConfig::default();
        let manager = SessionManager::new(config);
        let session = manager.connect().await.expect("Failed to connect session");

        let publisher = HealthPublisher::new("health-test-svc")
            .interval(std::time::Duration::from_millis(100));
        let handle = publisher.start(session.clone());

        let checker = HealthChecker::new()
            .session(session.clone())
            .timeout(std::time::Duration::from_secs(2));

        let status = checker.check("health-test-svc").await.expect("check failed");
        assert_eq!(status.service_name, "health-test-svc");
        assert_eq!(status.state, ServiceState::Online);
        assert_eq!(status.version, "1.0.0");

        handle.abort();
    }
}
