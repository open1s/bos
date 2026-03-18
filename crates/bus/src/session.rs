//! Zenoh session management

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{error::ZenohError, Session, ZenohConfig};

pub struct SessionManager {
    session: Arc<RwLock<Option<Arc<Session>>>>,
    config: ZenohConfig,
}

impl SessionManager {
    /// Convenience: create SessionManager with defaults and auto-connect
    pub async fn connected() -> Result<Arc<Self>, ZenohError> {
        let sm = Self::new(ZenohConfig::default());
        sm.connect().await?;
        Ok(Arc::new(sm))
    }

    /// Create a builder for SessionManager configuration
    pub fn builder() -> SessionManagerBuilder {
        SessionManagerBuilder::default()
    }

    /// Connect and wrap self in Arc
    pub async fn connect_and_wrap(self) -> Result<Arc<Self>, ZenohError> {
        self.connect().await?;
        Ok(Arc::new(self))
    }

    pub fn new(config: ZenohConfig) -> Self {
        Self {
            session: Arc::new(RwLock::new(None)),
            config,
        }
    }

    pub async fn connect(&self) -> Result<Arc<Session>, ZenohError> {
        let mut guard = self.session.write().await;
        if guard.is_some() {
            return Err(ZenohError::AlreadyConnected);
        }

        // Build bus config from ZenohConfig with fast accept timeout for quick operations
        let mut zenoh_config_json = serde_json::json!({
            "mode": self.config.mode,
            "scouting": {
                "multicast": {
                    "enabled": true
                }
            },
            "transport": {
                "unicast": {
                    "accept_timeout": 100  // ms - Fast accept timeout
                }
            }
        });

        if !self.config.connect.is_empty() {
            zenoh_config_json["connect"]["endpoints"] = serde_json::json!(self.config.connect);
        }

        if !self.config.listen.is_empty() {
            zenoh_config_json["listen"]["endpoints"] = serde_json::json!(self.config.listen);
        }

        let zenoh_config: zenoh::Config = serde_json::from_value(zenoh_config_json)
            .map_err(|e| ZenohError::Session(e.to_string()))?;

        let session = zenoh::open(zenoh_config).await?;
        let session = Arc::new(session);
        *guard = Some(session.clone());

        Ok(session)
    }

    pub async fn disconnect(&self) -> Result<(), ZenohError> {
        let mut guard = self.session.write().await;
        if let Some(session) = guard.take() {
            session.close().await?;
        }
        Ok(())
    }

    /// Disconnect with timeout - prevents blocking indefinitely on slow peer disconnection
    /// Returns Timeout error if session close doesn't complete within timeout
    pub async fn disconnect_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<(), ZenohError> {
        let mut guard = self.session.write().await;
        if let Some(session) = guard.take() {
            tokio::time::timeout(timeout, session.close())
                .await
                .map_err(|_| ZenohError::Timeout)?
                .map_err(ZenohError::from)?;
        }
        Ok(())
    }

    /// Disconnect asynchronously without waiting for completion
    /// Returns a JoinHandle that can be awaited if needed, or ignored for fire-and-forget
    pub fn disconnect_async(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut guard = self.session.write().await;
            if let Some(session) = guard.take() {
                let _ = session.close().await;
            }
        })
    }

    pub async fn get_session(&self) -> Result<Arc<Session>, ZenohError> {
        let guard = self.session.read().await;
        guard.clone().ok_or(ZenohError::NotConnected)
    }

    pub async fn is_connected(&self) -> bool {
        let guard = self.session.read().await;
        guard.is_some()
    }

    pub async fn ensure_connected(&self) -> Result<Arc<Session>, ZenohError> {
        if self.is_connected().await {
            return self.get_session().await;
        }
        self.connect().await
    }
}

impl Clone for SessionManager {
    fn clone(&self) -> Self {
        Self {
            session: self.session.clone(),
            config: self.config.clone(),
        }
    }
}

/// Builder for SessionManager with fluent configuration
#[derive(Debug, Clone, Default)]
pub struct SessionManagerBuilder {
    mode: Option<String>,
    connect: Vec<String>,
    listen: Vec<String>,
    peer: Option<String>,
}

impl SessionManagerBuilder {
    /// Set the connection mode ("peer" or "client")
    pub fn mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Add a connection endpoint
    pub fn connect(mut self, endpoint: impl Into<String>) -> Self {
        self.connect.push(endpoint.into());
        self
    }

    /// Add multiple connection endpoints
    pub fn connect_many(mut self, endpoints: Vec<String>) -> Self {
        self.connect.extend(endpoints);
        self
    }

    /// Add a listen endpoint
    pub fn listen(mut self, endpoint: impl Into<String>) -> Self {
        self.listen.push(endpoint.into());
        self
    }

    /// Add multiple listen endpoints
    pub fn listen_many(mut self, endpoints: Vec<String>) -> Self {
        self.listen.extend(endpoints);
        self
    }

    /// Set the peer ID
    pub fn peer(mut self, peer: impl Into<String>) -> Self {
        self.peer = Some(peer.into());
        self
    }

    /// Build the ZenohConfig
    pub fn build_config(self) -> ZenohConfig {
        ZenohConfig {
            mode: self.mode.unwrap_or_else(|| "peer".to_string()),
            connect: self.connect,
            listen: self.listen,
            peer: self.peer,
        }
    }

    /// Create SessionManager from builder
    pub fn build(self) -> SessionManager {
        SessionManager::new(self.build_config())
    }

    /// Build and connect the SessionManager
    pub async fn build_and_connect(self) -> Result<Arc<SessionManager>, ZenohError> {
        let sm = self.build();
        sm.connect_and_wrap().await
    }
}
