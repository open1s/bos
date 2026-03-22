//! Peer-to-peer RPC service discovery
//!
//! Uses pub/sub for service announcements:
//! - RpcService publishes its info to `rpc/services/{service_name}` on init
//! - RpcDiscovery subscribes to `rpc/services/*` to collect service announcements

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Codec;
use crate::error::ZenohError;
use crate::subscriber::Subscriber;
use crate::Session;

/// Discovery information published by RPC services.
#[derive(Debug, Clone, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DiscoveryInfo {
	/// Topic prefix for the service (e.g., "rpc/my-service")
	pub topic_prefix: String,
	/// Name of the discovered service
	pub service_name: String,
	/// Service version (e.g., "1.0.0")
	pub version: String,
	/// Topic for health status publishing (e.g., "rpc/health/my-service")
	pub health_topic: String,
}

impl DiscoveryInfo {
	pub fn new(service_name: &str) -> Self {
		let topic_prefix = format!("rpc/{}", service_name);
		Self {
			topic_prefix: topic_prefix.clone(),
			service_name: service_name.to_string(),
			version: "1.0.0".to_string(),
			health_topic: format!("rpc/health/{}", service_name),
		}
	}
}

/// Service discovery via pub/sub.
///
/// RpcService publishes its info on init. RpcDiscovery subscribes to
/// `rpc/services/*` and collects announcements within a timeout window.
pub struct RpcDiscovery {
    service_name: String,
}

impl RpcDiscovery {
    /// Create a discovery announcer for the given service name.
    ///
    /// After initialization with `init()`, the service info is published
    /// to `rpc/services/{service_name}`.
    pub fn announce(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    /// Initialize and publish service info.
    ///
    /// Publishes DiscoveryInfo to `rpc/services/{service_name}`.
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        let info = DiscoveryInfo::new(&self.service_name);
        let topic = format!("rpc/services/{}", self.service_name);
        let data = Codec
            .encode(&info)
            .map_err(|e| ZenohError::Serialization(e.to_string()))?;

        session
            .declare_publisher(&topic)
            .await?
            .put(data)
            .await
            .map_err(|e| ZenohError::Publisher(e.to_string()))?;

        Ok(())
    }

    /// Returns the service name.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Create a discovery query builder for finding services.
    pub fn discover(service_name: impl Into<String>) -> DiscoveryQueryBuilder {
        DiscoveryQueryBuilder::new(service_name)
    }
}

impl Clone for RpcDiscovery {
    fn clone(&self) -> Self {
        Self {
            service_name: self.service_name.clone(),
        }
    }
}

/// Builder for discovery queries.
pub struct DiscoveryQueryBuilder {
    service_name: String,
    session: Option<Arc<Session>>,
    timeout: Duration,
}

impl DiscoveryQueryBuilder {
    fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            session: None,
            timeout: Duration::from_secs(2),
        }
    }

    pub fn session(mut self, session: Arc<Session>) -> Self {
        self.session = Some(session);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Query for the service by subscribing to `rpc/services/*` and
    /// collecting announcements within the timeout window.
    pub async fn query(self) -> Result<DiscoveryInfo, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;

        let topic = format!("rpc/services/{}", self.service_name);
        let mut subscriber = Subscriber::<DiscoveryInfo>::new(&topic);
        subscriber.init(session.clone()).await?;

        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut last_seen: Option<DiscoveryInfo> = None;

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
		if let Some(info) = subscriber.recv_with_timeout(remaining).await {
			if info.service_name == self.service_name {
				return Ok(info);
			}
			last_seen = Some(info);
		} else {
			break;
		}
        }

        if let Some(info) = last_seen {
            return Ok(info);
        }

        Err(ZenohError::Query(format!(
            "No discovery response for service: {}",
            self.service_name
        )))
	}
}

/// Registry for discovering all advertised services.
/// Subscribes to `rpc/services/**` and collects announcements within a timeout.
pub struct DiscoveryRegistry {
	session: Option<Arc<Session>>,
	timeout: Duration,
}

impl DiscoveryRegistry {
	/// Create a new registry.
	pub fn new() -> Self {
		Self {
			session: None,
			timeout: Duration::from_secs(2),
		}
	}

	/// Set the session.
	pub fn session(mut self, session: Arc<Session>) -> Self {
		self.session = Some(session);
		self
	}

	/// Set the discovery timeout.
	pub fn timeout(mut self, timeout: Duration) -> Self {
		self.timeout = timeout;
		self
	}

	/// Subscribe to `rpc/services/**` and collect all discovery announcements.
	pub async fn list_services(&self) -> Result<Vec<DiscoveryInfo>, ZenohError> {
		let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;
		let topic = "rpc/services/**";
		let mut subscriber = Subscriber::<DiscoveryInfo>::new(topic);
		subscriber.init(session.clone()).await?;

		let deadline = tokio::time::Instant::now() + self.timeout;
		let mut services = Vec::new();

		while tokio::time::Instant::now() < deadline {
			let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
			if remaining.is_zero() {
				break;
			}
			if let Some(info) = subscriber.recv_with_timeout(remaining).await {
				if !services.iter().any(|s: &DiscoveryInfo| s.service_name == info.service_name) {
					services.push(info);
				}
			} else {
				break;
			}
		}

		Ok(services)
	}
}

impl Default for DiscoveryRegistry {
	fn default() -> Self {
		Self::new()
	}
}

impl Clone for DiscoveryRegistry {
	fn clone(&self) -> Self {
		Self {
			session: None,
			timeout: self.timeout,
		}
	}
}

#[cfg(test)]
mod tests {
    use super::*;

	#[test]
	fn test_discovery_info_serialization() {
		let info = DiscoveryInfo::new("my-service");
		assert_eq!(info.topic_prefix, "rpc/my-service");
		assert_eq!(info.service_name, "my-service");
		assert_eq!(info.version, "1.0.0");
		assert_eq!(info.health_topic, "rpc/health/my-service");

		let json = serde_json::to_string(&info).unwrap();
		let decoded: DiscoveryInfo = serde_json::from_str(&json).unwrap();
		assert_eq!(info.topic_prefix, decoded.topic_prefix);
		assert_eq!(info.service_name, decoded.service_name);
		assert_eq!(info.version, decoded.version);
		assert_eq!(info.health_topic, decoded.health_topic);
	}

    #[test]
    fn test_rpc_discovery_announce() {
        let discovery = RpcDiscovery::announce("test-service");
        assert_eq!(discovery.service_name(), "test-service");
    }

    #[test]
    fn test_rpc_discovery_clone() {
        let discovery = RpcDiscovery::announce("test-service");
        let cloned = discovery.clone();
        assert_eq!(cloned.service_name(), "test-service");
    }

    #[test]
    fn test_discovery_query_builder_default_timeout() {
        let builder = DiscoveryQueryBuilder::new("test-service");
        assert_eq!(builder.timeout, Duration::from_secs(2));
    }
}
