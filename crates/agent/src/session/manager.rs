use super::{SessionConfig, SessionSummary};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SessionManager {
    cache: Arc<RwLock<HashMap<String, SessionState>>>,
}

#[derive(Debug, Clone)]
struct SessionState {
    agent_id: String,
    message_count: usize,
    created_at: u64,
    updated_at: u64,
}

impl SessionManager {
    pub fn new(_config: SessionConfig) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create(&self, agent_id: String) -> Result<SessionSummary, SessionError> {
        if self.exists(&agent_id).await {
            return Err(SessionError::AlreadyExists(agent_id));
        }

        let now = current_timestamp();
        let state = SessionState {
            agent_id: agent_id.clone(),
            message_count: 0,
            created_at: now,
            updated_at: now,
        };

        let summary = SessionSummary {
            agent_id: agent_id.clone(),
            created_at: now,
            updated_at: now,
            message_count: 0,
        };

        let mut cache = self.cache.write().await;
        cache.insert(agent_id, state);

        Ok(summary)
    }

    pub async fn get(&self, agent_id: &str) -> Option<SessionSummary> {
        let cache = self.cache.read().await;
        cache.get(agent_id).map(|s| SessionSummary {
            agent_id: s.agent_id.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            message_count: s.message_count,
        })
    }

    pub async fn update_message_count(
        &self,
        agent_id: &str,
        count: usize,
    ) -> Result<(), SessionError> {
        let mut cache = self.cache.write().await;
        if let Some(state) = cache.get_mut(agent_id) {
            state.message_count = count;
            state.updated_at = current_timestamp();
            Ok(())
        } else {
            Err(SessionError::NotFound(agent_id.to_string()))
        }
    }

    pub async fn delete(&self, agent_id: &str) -> Result<(), SessionError> {
        let mut cache = self.cache.write().await;
        if cache.remove(agent_id).is_some() {
            Ok(())
        } else {
            Err(SessionError::NotFound(agent_id.to_string()))
        }
    }

    pub async fn list(&self) -> Vec<SessionSummary> {
        let cache = self.cache.read().await;
        cache
            .values()
            .map(|s| SessionSummary {
                agent_id: s.agent_id.clone(),
                created_at: s.created_at,
                updated_at: s.updated_at,
                message_count: s.message_count,
            })
            .collect()
    }

    async fn exists(&self, agent_id: &str) -> bool {
        let cache = self.cache.read().await;
        cache.contains_key(agent_id)
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(String),

    #[error("Session already exists: {0}")]
    AlreadyExists(String),
}
