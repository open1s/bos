use super::serializer::SessionSerializer;
use super::storage::SessionStorage;
use super::{AgentState, SessionConfig, SessionError, SessionSummary};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

pub struct SessionManager {
    pub(crate) storage: SessionStorage,
    cache: Arc<RwLock<HashMap<String, AgentState>>>,
    pub config: SessionConfig,
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

    async fn cleanup_expired(cache: &Arc<RwLock<HashMap<String, AgentState>>>, base_dir: &Path) {
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

    pub async fn create(&self, agent_id: String, workspace: Option<String>) -> Result<AgentState, SessionError> {
        if self.exists(&agent_id).await {
            return Err(SessionError::AlreadyExists(agent_id));
        }

        let state = SessionSerializer::new_state(agent_id.clone(), workspace);

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
                workspace: state.metadata.workspace.clone(),
                alias: state.metadata.alias.clone(),
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

    pub async fn set_alias(&self, agent_id: &str, alias: String) -> Result<AgentState, SessionError> {
        let mut state = self.get(agent_id).await?;
        state.metadata.alias = Some(alias);
        self.update(agent_id, state.clone()).await?;
        Ok(state)
    }

    pub async fn get_by_alias(&self, alias: &str) -> Result<AgentState, SessionError> {
        let summaries = self.list().await?;
        for summary in summaries {
            if summary.alias.as_deref() == Some(alias) {
                return self.get(&summary.agent_id).await;
            }
        }
        Err(SessionError::NotFound(format!("No session with alias: {}", alias)))
    }

    pub async fn delete_by_alias(&self, alias: &str) -> Result<(), SessionError> {
        let summaries = self.list().await?;
        for summary in summaries {
            if summary.alias.as_deref() == Some(alias) {
                return self.delete(&summary.agent_id).await;
            }
        }
        Err(SessionError::NotFound(format!("No session with alias: {}", alias)))
    }

    /// List sessions filtered by workspace directory
    pub async fn list_by_workspace(&self, workspace: &str) -> Result<Vec<SessionSummary>, SessionError> {
        let summaries = self.list().await?;
        Ok(summaries
            .into_iter()
            .filter(|s| s.workspace.as_deref() == Some(workspace))
            .collect())
    }

    /// Get the most recent session for a given workspace
    pub async fn get_latest_for_workspace(&self, workspace: &str) -> Result<SessionSummary, SessionError> {
        let summaries = self.list_by_workspace(workspace).await?;
        summaries
            .into_iter()
            .max_by_key(|s| s.updated_at)
            .ok_or_else(|| SessionError::NotFound(format!("No sessions found for workspace: {}", workspace)))
    }

    /// Branch a new session from an existing one (experimental workflow support)
    pub async fn branch(&self, source_agent_id: &str, new_agent_id: String, new_alias: Option<String>) -> Result<AgentState, SessionError> {
        let source_state = self.get(source_agent_id).await?;
        
        if self.exists(&new_agent_id).await {
            return Err(SessionError::AlreadyExists(new_agent_id));
        }

        // Create new state based on source
        let mut new_state = AgentState {
            agent_id: new_agent_id.clone(),
            message_log: source_state.message_log.clone(),
            context: source_state.context.clone(),
            metadata: source_state.metadata.clone(),
        };
        
        // Update metadata for the branch
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        new_state.metadata.created_at = now;
        new_state.metadata.updated_at = now;
        new_state.metadata.labels.push("branched".to_string());
        if let Some(ref parent) = source_state.metadata.labels.iter().find(|l| l.starts_with("parent:")) {
            new_state.metadata.labels.push((*parent).clone());
        }
        new_state.metadata.labels.push(format!("parent:{}", source_agent_id));
        new_state.metadata.alias = new_alias;

        // Save the new branch
        {
            let mut cache = self.cache.write().await;
            cache.insert(new_agent_id.clone(), new_state.clone());
        }
        self.storage.save(&new_state).await?;

        Ok(new_state)
    }

    /// Get parent session ID if this is a branch
    pub async fn get_parent_id(&self, agent_id: &str) -> Result<Option<String>, SessionError> {
        let state = self.get(agent_id).await?;
        let parent_label = state.metadata.labels.iter()
            .find(|l| l.starts_with("parent:"));
        Ok(parent_label.map(|l| l.strip_prefix("parent:").unwrap().to_string()))
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
        let state = manager.create("test-agent".to_string(), None).await.unwrap();

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
manager.create("test-agent".to_string(), None).await.unwrap();

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
    manager.create("test-agent".to_string(), None).await.unwrap();

    manager.delete("test-agent").await.unwrap();

        let result = manager.get("test-agent").await;
        assert!(matches!(result, Err(SessionError::NotFound(_))));
    }
}
