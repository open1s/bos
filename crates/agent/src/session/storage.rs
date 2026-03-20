use super::{AgentState, SessionError, SessionConfig};
use std::path::PathBuf;
use tokio::fs;

pub struct SessionStorage {
    config: SessionConfig,
}

impl SessionStorage {
    pub fn new(config: SessionConfig) -> Self {
        Self { config }
    }

    pub async fn ensure_base_dir(&self) -> Result<(), SessionError> {
        fs::create_dir_all(&self.config.base_dir).await?;
        Ok(())
    }

    fn session_path(&self, agent_id: &str) -> PathBuf {
        self.config.base_dir.join(format!("{}.json", agent_id))
    }

    pub async fn exists(&self, agent_id: &str) -> bool {
        self.session_path(agent_id).exists()
    }

    pub async fn save(&self, state: &AgentState) -> Result<(), SessionError> {
        self.ensure_base_dir().await?;

        let bytes = crate::session::serializer::SessionSerializer::serialize(state)?;

        if self.config.compression_enabled {
            let compressed = crate::session::serializer::SessionSerializer::compress(&bytes)?;
            fs::write(self.session_path(&state.agent_id), compressed).await?;
        } else {
            fs::write(self.session_path(&state.agent_id), bytes).await?;
        }

        Ok(())
    }

    pub async fn load(&self, agent_id: &str) -> Result<AgentState, SessionError> {
        let path = self.session_path(agent_id);

        if !path.exists() {
            return Err(SessionError::NotFound(agent_id.to_string()));
        }

        let bytes = fs::read(&path).await?;

        let data = if self.config.compression_enabled {
            crate::session::serializer::SessionSerializer::decompress(&bytes)?
        } else {
            bytes
        };

        let state = crate::session::serializer::SessionSerializer::deserialize(&data)?;

        if crate::session::serializer::SessionSerializer::is_expired(&state) {
            return Err(SessionError::Expired(agent_id.to_string()));
        }

        Ok(state)
    }

    pub async fn delete(&self, agent_id: &str) -> Result<(), SessionError> {
        let path = self.session_path(agent_id);

        if !path.exists() {
            return Err(SessionError::NotFound(agent_id.to_string()));
        }

        fs::remove_file(path).await?;
        Ok(())
    }

    pub async fn list_files(&self) -> Result<Vec<PathBuf>, SessionError> {
        let mut entries = fs::read_dir(&self.config.base_dir)
            .await
            .map_err(SessionError::Io)?;

        let mut files = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(SessionError::Io)? {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                files.push(path);
            }
        }

        Ok(files)
    }
}