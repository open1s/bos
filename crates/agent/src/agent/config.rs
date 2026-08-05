use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use zenoh::Session as ZenohSession;

use crate::agent::agentic::LlmProvider;
use crate::agent::{Agent, AgentConfig};
use crate::error::AgentError;
use crate::tools::{FunctionTool, Tool};

#[derive(Debug, Deserialize, Clone)]
pub struct TomlToolRef {
    pub name: String,
    pub description: Option<String>,
    pub schema: Option<serde_json::Value>,
}

impl TomlToolRef {
    pub fn to_openai_tool(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description.clone().unwrap_or_default(),
                "parameters": self.schema.clone().unwrap_or(serde_json::json!({"type": "object", "properties": {}}))
            }
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
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
    pub max_steps: Option<usize>,
    #[serde(default)]
    pub api_mode: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<TomlToolRef>>,
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
            max_steps: t.max_steps.unwrap_or(10),
            api_mode: t.api_mode.unwrap_or_default(),
            reasoning_effort: t.reasoning_effort,
            circuit_breaker: None,
            rate_limit: None,
        }
    }
}

pub struct TomlAgentBuilder {
    config: TomlAgentConfig,
    tools: Vec<Arc<dyn Tool>>,
}

impl TomlAgentBuilder {
    pub fn from_toml(toml_str: &str) -> Result<Self, AgentError> {
        let config: TomlAgentConfig = toml::from_str(toml_str)
            .map_err(|e| AgentError::Config(format!("TOML parse error: {}", e)))?;
        Ok(Self {
            config,
            tools: Vec::new(),
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, AgentError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| AgentError::Config(e.to_string()))?;
        Self::from_toml(&content)
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn config_tools(&self) -> Option<Vec<serde_json::Value>> {
        self.config
            .tools
            .as_ref()
            .map(|tools| tools.iter().map(|t| t.to_openai_tool()).collect())
    }

    /// DeepSeek natively supports the Responses API; default to it unless the
    /// config explicitly selects chat.
    fn deepseek_api_mode_default(model: &str, explicit: Option<&str>) -> Option<&'static str> {
        if explicit.is_none() && model.split('/').next() == Some("deepseek") {
            Some("responses")
        } else {
            None
        }
    }

    pub async fn build(self, _session: Option<Arc<ZenohSession>>) -> Result<Agent, AgentError> {
        let mut config: AgentConfig = self.config.clone().into();
        apply_model_defaults(&mut config);

        // DeepSeek natively supports the Responses API; default to it unless
        // the config explicitly selects chat.
        if let Some(mode) =
            Self::deepseek_api_mode_default(&config.model, self.config.api_mode.as_deref())
        {
            config.api_mode = mode.to_string();
        }

        let mut llm = LlmProvider::new();
        let (vendor_name, vendor) = crate::agent::agentic::build_vendor(&config);
        llm.register_vendor(vendor_name, vendor);
        let llm = Arc::new(llm);

        let mut agent = Agent::new(config, llm);

        for tool in self.tools {
            agent.try_add_tool(tool)?;
        }

        if let Some(toml_tools) = self.config.tools {
            for toml_tool in toml_tools {
                if let Some(schema) = toml_tool.schema {
                    let tool = Arc::new(FunctionTool::new(
                        &toml_tool.name,
                        toml_tool.description.as_deref().unwrap_or("Tool"),
                        schema,
                        |_args| Ok(serde_json::json!("tool not yet implemented")),
                    ));
                    agent.try_add_tool(tool)?;
                }
            }
        }

        Ok(agent)
    }
}

/// Applies per-model defaults from a home config's `[llm.*]` sections to the
/// given agent config. A section is matched when its `model` field equals
/// `config.model` exactly. The section's `api_mode`/`reasoning_effort` values
/// override any agent-level setting (model always wins).
pub fn apply_model_defaults_from(config: &mut AgentConfig, home: &serde_json::Value) {
    let Some(llm) = home.get("llm") else {
        return;
    };
    let Some(sections) = llm.as_object() else {
        return;
    };
    for section in sections.values() {
        let Some(model) = section.get("model").and_then(|v| v.as_str()) else {
            continue;
        };
        if model != config.model {
            continue;
        }
        if let Some(api_mode) = section.get("api_mode").and_then(|v| v.as_str()) {
            config.api_mode = api_mode.to_string();
        }
        if let Some(effort) = section.get("reasoning_effort").and_then(|v| v.as_str()) {
            config.reasoning_effort = Some(effort.to_string());
        }
    }
}

/// Loads the home config and applies per-model `[llm.*]` defaults to the given
/// agent config. No-op when no config is discoverable.
pub fn apply_model_defaults(config: &mut AgentConfig) {
    let mut loader = config::loader::ConfigLoader::new().discover();
    if let Ok(home) = loader.load_sync() {
        apply_model_defaults_from(config, &home);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConfig;

    #[test]
    fn applies_model_defaults_by_exact_model_match() {
        let home = serde_json::json!({
            "llm": {
                "deepseek": {
                    "model": "nvidia/deepseek-ai/deepseek-v4-flash",
                    "base_url": "http://127.0.0.1:11436/v1",
                    "api_key": "1234560",
                    "api_mode": "responses",
                    "reasoning_effort": "high"
                },
                "other": {
                    "model": "nvidia/other/model",
                    "api_mode": "chat"
                }
            }
        });

        let mut config = AgentConfig {
            model: "nvidia/deepseek-ai/deepseek-v4-flash".to_string(),
            api_mode: "chat".to_string(),
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };

        apply_model_defaults_from(&mut config, &home);

        assert_eq!(config.api_mode, "responses");
        assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn unmatched_model_keeps_agent_defaults() {
        let home = serde_json::json!({
            "llm": {
                "deepseek": {
                    "model": "nvidia/deepseek-ai/deepseek-v4-flash",
                    "api_mode": "responses",
                    "reasoning_effort": "high"
                }
            }
        });

        let mut config = AgentConfig {
            model: "openai/gpt-4o".to_string(),
            api_mode: "chat".to_string(),
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };

        apply_model_defaults_from(&mut config, &home);

        assert_eq!(config.api_mode, "chat");
        assert_eq!(config.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn missing_home_sections_is_noop() {
        let home = serde_json::json!({ "global_model": {} });

        let mut config = AgentConfig {
            model: "nvidia/deepseek-ai/deepseek-v4-flash".to_string(),
            api_mode: "chat".to_string(),
            reasoning_effort: None,
            ..Default::default()
        };

        apply_model_defaults_from(&mut config, &home);

        assert_eq!(config.api_mode, "chat");
        assert_eq!(config.reasoning_effort, None);
    }

    #[test]
    fn deepseek_defaults_to_responses_unless_explicit() {
        assert_eq!(
            TomlAgentBuilder::deepseek_api_mode_default("deepseek/deepseek-v4-flash", None),
            Some("responses")
        );
        assert_eq!(
            TomlAgentBuilder::deepseek_api_mode_default("openai/gpt-4o", None),
            None
        );
        assert_eq!(
            TomlAgentBuilder::deepseek_api_mode_default("deepseek/deepseek-v4-flash", Some("chat")),
            None
        );
    }
}
