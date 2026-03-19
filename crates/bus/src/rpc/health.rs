//! Health monitoring for RPC services.
//!
//! Services publish periodic heartbeats. Clients can check liveness.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use zenoh::Session;

use crate::Codec;
use crate::error::ZenohError;
use crate::subscriber::SubscriberWrapper;

/// Health status of a service.
#[derive(Debug, Clone, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct HealthStatus {
    /// Name of the service.
    pub service_name: String,
    /// Current state of the service.
    pub state: ServiceState,
    /// Service version.
    pub version: String,
    /// Unix timestamp of the last heartbeat.
    pub timestamp: u64,
}

/// Possible service states.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ServiceState {
    /// Service is healthy and responding.
    Online,
    /// Service is running but degraded (e.g., high latency).
    Degraded,
    /// Service is not responding.
    Offline,
}

/// Publisher for service health status.
/// Publishes heartbeats at a configurable interval.
pub struct HealthPublisher {
    service_name: String,
    version: String,
    state: ServiceState,
    interval: Duration,
    topic: String,
}

impl HealthPublisher {
    /// Create a new health publisher for the given service.
    pub fn new(service_name: impl Into<String>) -> Self {
        let service_name = service_name.into();
        Self {
            service_name: service_name.clone(),
            version: "1.0.0".to_string(),
            state: ServiceState::Online,
            interval: Duration::from_secs(5),
            topic: format!("rpc/health/{}", service_name),
        }
    }

    /// Set the service version.
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Set the heartbeat interval.
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Get the health topic.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Set the current service state.
    pub fn set_state(&mut self, state: ServiceState) {
        self.state = state;
    }

    /// Start publishing heartbeats in a background task.
    /// Returns a JoinHandle that can be used to stop the publisher.
    pub fn start(self, session: Arc<Session>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(self.interval);
            // Skip the first immediate tick
            interval_timer.tick().await;

            loop {
                interval_timer.tick().await;

                let status = HealthStatus {
                    service_name: self.service_name.clone(),
                    state: self.state.clone(),
                    version: self.version.clone(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                };

                let data = match Codec.encode(&status) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                if let Ok(publisher) = session.declare_publisher(&self.topic).await {
                    let _ = publisher.put(data).await;
                }
            }
        })
    }
}

/// Checker for querying service health status.
pub struct HealthChecker {
    session: Option<Arc<Session>>,
    timeout: Duration,
}

impl HealthChecker {
    /// Create a new health checker.
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

    /// Set the timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Check the health of a specific service.
    pub async fn check(&self, service_name: &str) -> Result<HealthStatus, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;

        let topic = format!("rpc/health/{}", service_name);
        let mut subscriber = SubscriberWrapper::<HealthStatus>::new(&topic);
        subscriber.init(session).await?;

        let deadline = Instant::now() + self.timeout;

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            if let Some(status) = subscriber.recv_with_timeout(remaining).await {
                return Ok(status);
            }
        }

        Err(ZenohError::Query(format!(
            "No health response for service: {}",
            service_name
        )))
    }

    /// Check health of all services by subscribing to `rpc/health/**`.
    pub async fn check_all(&self) -> Result<Vec<HealthStatus>, ZenohError> {
        let session = self.session.as_ref().ok_or(ZenohError::NotConnected)?;

        let topic = "rpc/health/**";
        let mut subscriber = SubscriberWrapper::<HealthStatus>::new(topic);
        subscriber.init(session).await?;

        let deadline = Instant::now() + self.timeout;
        let mut statuses = Vec::new();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            if let Some(status) = subscriber.recv_with_timeout(remaining).await {
                if !statuses.iter().any(|s: &HealthStatus| s.service_name == status.service_name) {
                    statuses.push(status);
                }
            } else {
                break;
            }
        }

        Ok(statuses)
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for HealthChecker {
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
    fn test_health_status_serialization() {
        let status = HealthStatus {
            service_name: "test-service".to_string(),
            state: ServiceState::Online,
            version: "1.0.0".to_string(),
            timestamp: 1234567890,
        };

        let json = serde_json::to_string(&status).unwrap();
        let decoded: HealthStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.service_name, "test-service");
        assert_eq!(decoded.state, ServiceState::Online);
    }

    #[test]
    fn test_health_publisher_new() {
        let publisher = HealthPublisher::new("test-service");
        assert_eq!(publisher.topic(), "rpc/health/test-service");
        assert_eq!(publisher.interval, Duration::from_secs(5));
    }

    #[test]
    fn test_health_publisher_builder() {
        let publisher = HealthPublisher::new("test-service")
            .version("2.0.0")
            .interval(Duration::from_secs(10));

        assert_eq!(publisher.topic(), "rpc/health/test-service");
    }

    #[test]
    fn test_service_state_serialization() {
        let online = ServiceState::Online;
        let json = serde_json::to_string(&online).unwrap();
        assert_eq!(json, "\"Online\"");

        let degraded = ServiceState::Degraded;
        let json = serde_json::to_string(&degraded).unwrap();
        assert_eq!(json, "\"Degraded\"");

        let offline = ServiceState::Offline;
        let json = serde_json::to_string(&offline).unwrap();
        assert_eq!(json, "\"Offline\"");
    }

    #[test]
    fn test_health_checker_default() {
        let checker = HealthChecker::new();
        assert_eq!(checker.timeout, Duration::from_secs(2));
    }
}
