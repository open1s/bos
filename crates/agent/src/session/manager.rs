use super::{AgentState, SessionSummary, SessionConfig, SessionError};
use super::storage::SessionStorage;
use super::serializer::SessionSerializer;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::Duration;
use tokio::task::JoinHandle;
use std::path::Path;

pub struct SessionManager {
    storage: SessionStorage,
    cache: Arc<RwLock<HashMap<String, AgentState>>>,
    config: SessionConfig,
    cleanup_task: Option<JoinHandle<()>>,
}

impl SessionManager {
    pub fn new(config: SessionConfig) -> Self {
        let storage = SessionStorage::new(config.clone());
        let cache = Arc::new(RwLock::new(HashMap::new()));

        Self {
            storage,
            cache,
            config,
            cleanup_task: None,
        }
    }

    pub async fn start_cleanup(&mut self, interval: Duration) {
        let cache = self.cache.clone();
        let base_dir = self.config.base_dir.clone();

        self.cleanup_task = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                Self::cleanup_expired(&cache, &base_dir).await;
            }
        }));
    }

    async fn cleanup_expired(
        cache: &Arc<RwLock<HashMap<String, AgentState>>>,
        base_dir: &Path,
    ) {
        let mut cache_guard = cache.write().await;
        let mut to_remove = Vec::new();

        for (agent_id, state) in cache_guard.iter() {
            if SessionSerializer::is_expired(state) {
                to_remove.push(agent_id.clone());
            }
        }

        for agent_id in to_remove {
            cache_guard.remove(&agent_id);
            let _ = tokio::fs::remove_file(base_dir.join(format!("{}.json", agent_id))).await;
        }
    }

    pub async fn create(&self, agent_id: String) -> Result<AgentState, SessionError> {
        if self.exists(&agent_id).await {
            return Err(SessionError::AlreadyExists(agent_id));
        }

        let state = SessionSerializer::new_state(agent_id.clone());

        {
            let mut cache = self.cache.write().await;
            cache.insert(agent_id.clone(), state.clone());
        }

        self.storage.save(&state).await?;

        Ok(state)
    }

    pub async fn get(&self, agent_id: &str) -> Result<AgentState, SessionError> {
        {
            let cache = self.cache.read().await;
            if let Some(state) = cache.get(agent_id) {
                return Ok(state.clone());
            }
        }

        let state = self.storage.load(agent_id).await?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(agent_id.to_string(), state.clone());
        }

        Ok(state)
    }

    pub async fn update(&self, agent_id: &str, mut state: AgentState) -> Result<(), SessionError> {
        if !self.exists(agent_id).await {
            return Err(SessionError::NotFound(agent_id.to_string()));
        }

        SessionSerializer::update_metadata(&mut state);

        {
            let mut cache = self.cache.write().await;
            cache.insert(agent_id.to_string(), state.clone());
        }

        self.storage.save(&state).await?;

        Ok(())
    }

    pub async fn delete(&self, agent_id: &str) -> Result<(), SessionError> {
        self.storage.delete(agent_id).await?;

        {
            let mut cache = self.cache.write().await;
            cache.remove(agent_id);
        }

        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let files = self.storage.list_files().await?;
        let mut summaries = Vec::new();

        for file in files {
            if let Some(stem) = file.file_stem() {
                if let Some(agent_id) = stem.to_str() {
                    if let Ok(state) = self.storage.load(agent_id).await {
                        summaries.push(SessionSummary {
                            agent_id: agent_id.to_string(),
                            created_at: state.metadata.created_at,
                            updated_at: state.metadata.updated_at,
                            message_count: state.metadata.message_count,
                            labels: state.metadata.labels.clone(),
                            expires_at: state.metadata.expires_at,
                        });
                    }
                }
            }
        }

        Ok(summaries)
    }

    async fn exists(&self, agent_id: &str) -> bool {
        {
            let cache = self.cache.read().await;
            if cache.contains_key(agent_id) {
                return true;
            }
        }

        self.storage.exists(agent_id).await
    }

    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_session() {
        let temp = TempDir::new().unwrap();
        let config = SessionConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        };

        let manager = SessionManager::new(config);
        let state = manager.create("test-agent".to_string()).await.unwrap();

        assert_eq!(state.agent_id, "test-agent");
        assert_eq!(state.message_log.len(), 0);
    }

    #[tokio::test]
    async fn test_update_session() {
        let temp = TempDir::new().unwrap();
        let config = SessionConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        };

        let manager = SessionManager::new(config);
        manager.create("test-agent".to_string()).await.unwrap();

        let mut state = manager.get("test-agent").await.unwrap();
        state.metadata.labels.push("test".to_string());

        manager.update("test-agent", state).await.unwrap();
        let updated = manager.get("test-agent").await.unwrap();

        assert!(updated.metadata.labels.contains(&"test".to_string()));
    }

    #[tokio::test]
    async fn test_delete_session() {
        let temp = TempDir::new().unwrap();
        let config = SessionConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        };

        let manager = SessionManager::new(config);
        manager.create("test-agent".to_string()).await.unwrap();

        manager.delete("test-agent").await.unwrap();

        let result = manager.get("test-agent").await;
        assert!(matches!(result, Err(SessionError::NotFound(_))));
    }
}