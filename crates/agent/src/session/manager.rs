use super::{SessionConfig, SessionSummary};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tracks live sessions in an in-memory cache.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two callers racing to create the same session must not both win.
    ///
    /// The existence check runs under the same write lock as the insert, so
    /// the loser sees the winner's entry instead of silently replacing it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_creates_yield_exactly_one_session() {
        let manager = SessionManager::new(SessionConfig::default());
        let (first, second) = tokio::join!(
            manager.create("shared".to_string()),
            manager.create("shared".to_string())
        );
        assert!(
            first.is_ok() ^ second.is_ok(),
            "exactly one create may succeed"
        );
        assert_eq!(manager.list().await.len(), 1);
        assert!(manager.get("shared").await.is_some());
    }

    #[tokio::test]
    async fn a_duplicate_create_is_refused_and_keeps_the_original() {
        let manager = SessionManager::new(SessionConfig::default());
        let created = manager.create("a".to_string()).await.expect("first");
        let error = manager
            .create("a".to_string())
            .await
            .expect_err("duplicate");
        assert!(matches!(error, SessionError::AlreadyExists(id) if id == "a"));
        let current = manager.get("a").await.expect("still registered");
        assert_eq!(current.created_at, created.created_at);
    }
}

impl SessionManager {
    /// Create an empty manager.
    pub fn new(_config: SessionConfig) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new session, failing if it already exists.
    ///
    /// The existence check and the insert happen under one write lock, so two
    /// concurrent creates cannot both succeed and lose one caller's session.
    pub async fn create(&self, agent_id: String) -> Result<SessionSummary, SessionError> {
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
        if cache.contains_key(&agent_id) {
            return Err(SessionError::AlreadyExists(agent_id));
        }
        cache.insert(agent_id, state);

        Ok(summary)
    }

    /// Summary for one session, if it exists.
    pub async fn get(&self, agent_id: &str) -> Option<SessionSummary> {
        let cache = self.cache.read().await;
        cache.get(agent_id).map(|s| SessionSummary {
            agent_id: s.agent_id.clone(),
            created_at: s.created_at,
            updated_at: s.updated_at,
            message_count: s.message_count,
        })
    }

    /// Remove a session, failing if it does not exist.
    pub async fn delete(&self, agent_id: &str) -> Result<(), SessionError> {
        let mut cache = self.cache.write().await;
        if cache.remove(agent_id).is_some() {
            Ok(())
        } else {
            Err(SessionError::NotFound(agent_id.to_string()))
        }
    }

    /// Summaries for every live session.
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
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Errors returned by [`SessionManager`].
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// No session is registered for the agent.
    #[error("Session not found: {0}")]
    NotFound(String),

    /// A session is already registered for the agent.
    #[error("Session already exists: {0}")]
    AlreadyExists(String),
}
