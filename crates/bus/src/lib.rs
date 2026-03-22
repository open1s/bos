//! BrainOS Zenoh communication wrapper
//!
//! Provides simplified abstractions for Zenoh-based inter-component communication.
//!
//! # Quick Start
//!
//! ## Basic Publish/Subscribe
//!
//! ```rust,ignore
//! use brickos_bus::{SessionManager, Publisher, Subscriber};
//!
//! // Connect to Zenoh
//! let session = SessionManager::connected().await?.get_session().await?;
//!
//! // Publish messages
//! let mut publisher = Publisher::new("chat/general").with_session(session.clone()).await?;
//! publisher.publish(&session, &"Hello, world!").await?;
//!
//! // Subscribe to messages
//! let mut subscriber = Subscriber::<String>::new("chat/general")
//!     .with_session(session).await?;
//!
//! while let Some(msg) = subscriber.recv().await {
//!     println!("Received: {}", msg);
//! }
//! ```
//!
//! ## RPC Communication
//!
//! ```rust,ignore
//! use brickos_bus::{RpcServiceBuilder, RpcClient, RpcHandler};
//!
//! // Define a handler
//! struct Calculator;
//!
//! #[async_trait::async_trait]
//! impl RpcHandler for Calculator {
//!     async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
//!         match method {
//!             "add" => {
//!                 let (a, b): (i32, i32) = Codec::decode(payload)?;
//!                 let sum = a + b;
//!                 Codec::encode(&sum)
//!             }
//!             _ => Err(RpcServiceError::Business {
//!                 code: 404,
//!                 message: format!("Unknown method: {}", method),
//!             })
//!         }
//!     }
//! }
//!
//! // Create RPC service
//! let service = RpcServiceBuilder::new()
//!     .service_name("calculator")
//!     .build()?
//!     .init(session, Calculator).await?;
//!
//! let _handle = service.into_task()?;
//!
//! // Create RPC client
//! let mut client = RpcClient::new("calculator", "add");
//! client.init(session).await?;
//!
//! let payload = Codec::encode(&(5, 3))?;
//! let result: i32 = client.call(&payload).await?;
//! assert_eq!(result, 8);
//! ```
//!
//! # Key Types
//!
//! ## Core
//! - [`SessionManager`] - Connection lifecycle management
//! - [`Bus`] - Simplified session wrapper (planned helper methods)
//!
//! ## Messaging
//! - [`Publisher`]/[`TopicPublisher`] - Publish messages to topics
//! - [`Subscriber`]/[`TopicSubscriber`] - Subscribe and receive messages
//! - [`Query`]/[`Queryable`] - Query/Response pattern
//!
//! ## RPC
//! - [`RpcClient`] - Client for RPC calls
//! - [`RpcService`] - Server side RPC handler
//! - [`RpcHandler`] - Trait for implementing RPC handlers
//!
//! ## Discovery & Health
//! - [`DiscoveryRegistry`] - Service discovery
//! - [`HealthChecker`] - Health checking
//! - [`HealthPublisher`] - Publish health status

pub mod codec;
pub mod error;
pub mod publisher;
pub mod query;
pub mod queryable;
pub mod rpc;
pub mod session;
pub mod subscriber;

#[cfg(feature = "python-extension")]
pub mod python;

pub use error::ZenohError;
pub use publisher::{Publisher};
pub use query::{Query, QueryWrapper, Queryable, TopicQueryable};
pub use queryable::QueryableWrapper;
pub use rpc::cache::{CacheStats, ServiceCache};
pub use rpc::client::{RpcClient, RpcClientBuilder};
pub use rpc::discovery::{DiscoveryInfo, DiscoveryRegistry, RpcDiscovery};
pub use rpc::error::{RpcError, RpcServiceError};
pub use rpc::health::{HealthChecker, HealthPublisher, HealthStatus, ServiceState};
pub use rpc::service::{RpcHandler, RpcService, RpcServiceBuilder};
pub use rpc::types::RpcResponse;
pub use session::{Bus, BusBuilder, SessionManager, SessionManagerBuilder};
pub use subscriber::{Subscriber};

pub use zenoh::Session;

pub use codec::{Codec, RkyvCodec, DEFAULT_CODEC};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZenohConfig {
    pub mode: String,
    pub connect: Vec<String>,
    pub listen: Vec<String>,
    pub peer: Option<String>,
}

impl Default for ZenohConfig {
    fn default() -> Self {
        Self {
            mode: "peer".to_string(),
            connect: vec![],
            listen: vec![],
            peer: None,
        }
    }
}
