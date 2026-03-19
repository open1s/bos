use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use zenoh::Session as ZenohSession;

use crate::agent::{Agent, AgentConfig};
use crate::error::AgentError;
use crate::llm::OpenAiClient;
use crate::tools::{Tool, ToolRegistry};

#[derive(Debug, Deserialize)]
pub struct TomlAgentConfig {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_system_prompt() -> String {
    "You are a helpful assistant.".to_string()
}

fn default_temperature() -> f32 {
    0.7
}

fn default_timeout() -> u64 {
    60
}

impl From<TomlAgentConfig> for AgentConfig {
    fn from(t: TomlAgentConfig) -> Self {
        Self {
            name: t.name,
            model: t.model,
            base_url: t.base_url,
            api_key: t.api_key,
            system_prompt: t.system_prompt,
            temperature: t.temperature,
            max_tokens: t.max_tokens,
            timeout_secs: t.timeout_secs,
        }
    }
}

pub struct AgentBuilder {
    config: TomlAgentConfig,
    tools: Vec<Arc<dyn Tool>>,
}

impl AgentBuilder {
    pub fn from_toml(toml_str: &str) -> Result<Self, AgentError> {
        let config: TomlAgentConfig = toml::from_str(toml_str)
            .map_err(|e| AgentError::Config(format!("TOML parse error: {}", e)))?;
        Ok(Self {
            config,
            tools: Vec::new(),
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, AgentError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AgentError::Config(e.to_string()))?;
        Self::from_toml(&content)
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub async fn build(self, session: Option<Arc<ZenohSession>>) -> Result<Agent, AgentError> {
        let llm = Arc::new(OpenAiClient::new(
            self.config.base_url.clone(),
            self.config.api_key.clone(),
        ));
        let agent = Agent::new(self.config.into(), llm);

        let registry = ToolRegistry::new();

        if let Some(_session) = session {
            // TODO: register bus tools when session provided
        }

        Ok(agent)
    }
}