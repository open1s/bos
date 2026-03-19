//! BrainOS Zenoh communication wrapper
//!
//! Provides common abstractions for Zenoh-based inter-component communication.

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
pub use publisher::PublisherWrapper;
pub use query::QueryWrapper;
pub use queryable::QueryableWrapper;
pub use rpc::cache::{CacheStats, ServiceCache};
pub use rpc::client::{RpcClient, RpcClientBuilder};
pub use rpc::discovery::{DiscoveryInfo, DiscoveryRegistry, RpcDiscovery};
pub use rpc::error::{RpcError, RpcServiceError};
pub use rpc::health::{HealthChecker, HealthPublisher, HealthStatus, ServiceState};
pub use rpc::service::{RpcHandler, RpcService, RpcServiceBuilder};
pub use rpc::types::RpcResponse;
pub use session::{SessionManager, SessionManagerBuilder};
pub use subscriber::SubscriberWrapper;

pub use zenoh::Session;

pub use codec::{Codec, BincodeCodec, JsonCodec, DEFAULT_CODEC};

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
