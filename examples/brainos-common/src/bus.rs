use bus::{Session, SessionManager, ZenohConfig};
use std::sync::Arc;
use anyhow::Result;

pub async fn setup_bus(config: Option<ZenohConfig>) -> Result<Arc<Session>> {
    let zenoh_config = config.unwrap_or_else(ZenohConfig::default);
    
    let session_manager = SessionManager::new(zenoh_config);
    let session = session_manager.connect().await
        .map_err(|e| anyhow::anyhow!("Failed to connect to Zenoh bus: {}", e))?;

    tracing::info!("Connected to Zenoh bus");

    Ok(session)
}
