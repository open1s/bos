//! User settings for the chat GUI, persisted to `~/.bos/gui/settings.toml`.
//!
//! Defaults are discovered from the BOS config (`~/.bos/conf/config.toml` via
//! [`config::ConfigLoader`]) and may be overridden with `BOS_*` environment
//! variables. The GUI's own TOML file wins over both when present.

use std::fs;
use std::path::{Path, PathBuf};

use agent::AgentConfig;
use serde::{Deserialize, Serialize};

/// Default skills directory (`~` expands at agent build time).
pub(crate) const DEFAULT_SKILLS_DIR: &str = "~/.bos/skills";

/// Editable connection and prompt settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// Model identifier, or the name of an `[llm.<name>]` profile.
    pub(crate) model: String,
    /// OpenAI-compatible API base URL.
    pub(crate) base_url: String,
    /// Optional API key override; empty means "use discovered config".
    pub(crate) api_key: String,
    /// System prompt prepended to every conversation.
    pub(crate) system_prompt: String,
    /// Sampling temperature.
    pub(crate) temperature: f32,
    /// Optional reasoning effort for reasoning models (`low`/`medium`/`high`).
    pub(crate) reasoning_effort: Option<String>,
    /// Whether the bash tool is registered with the chat agent.
    pub(crate) bash_enabled: bool,
    /// Whether the filesystem tools (`read_file`/`write_file`/`list_dir`)
    /// are registered with the chat agent.
    pub(crate) file_tools_enabled: bool,
    /// Workspace root for the bash and file tools; empty means the process
    /// working directory (bash unrestricted, file tools relative to cwd).
    pub(crate) bash_workspace: String,
    /// Directory scanned for skills (`SKILL.md` folders); empty disables skills.
    pub(crate) skills_dir: String,
    /// Require one-click user approval before `bash` and `write_file` run.
    pub(crate) require_approval: bool,
    /// User-configured MCP servers ("plugins") whose tools join the agent.
    pub(crate) mcp_servers: Vec<McpServerEntry>,

    /// API key discovered from the BOS config at load time (never serialized).
    #[serde(skip)]
    pub(crate) config_api_key: Option<String>,
    /// `[llm.<name>]` profiles discovered at load time (never serialized).
    #[serde(skip)]
    pub(crate) profiles: Vec<Profile>,
}

/// One `[llm.<name>]` endpoint profile from the BOS config.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Profile {
    /// Profile name as used in `[llm.<name>]`.
    pub(crate) name: String,
    /// Model identifier of the profile.
    pub(crate) model: String,
    /// Base URL of the profile.
    pub(crate) base_url: String,
    /// API key of the profile.
    pub(crate) api_key: String,
}

/// One configured MCP server — a pluggable external tool provider.
///
/// Persisted as `[[mcp_servers]]` entries in `settings.toml`. `transport` is
/// `"stdio"` (spawn `command` with `args`) or `"http"` (talk to `url`); tools
/// register in the agent as `{name}_{tool}`, so `name` doubles as the
/// namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct McpServerEntry {
    /// Unique name; also the tool namespace (`[A-Za-z0-9._-]`).
    pub(crate) name: String,
    /// `"stdio"` or `"http"`.
    pub(crate) transport: String,
    /// stdio transport: executable to spawn (ignored for http).
    pub(crate) command: String,
    /// stdio transport: arguments passed to `command`.
    pub(crate) args: Vec<String>,
    /// http transport: base URL (`http://` or `https://`).
    pub(crate) url: String,
    /// Disabled entries persist but are never connected.
    pub(crate) enabled: bool,
}

impl Default for McpServerEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: "stdio".to_string(),
            command: String::new(),
            args: Vec::new(),
            url: String::new(),
            enabled: true,
        }
    }
}

impl McpServerEntry {
    /// Validate this entry on its own: name charset and transport fields.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("MCP server name must not be empty".to_string());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!(
                "MCP server name '{name}': allowed chars are [A-Za-z0-9._-]"
            ));
        }
        match self.transport.as_str() {
            "stdio" => {
                if self.command.trim().is_empty() {
                    return Err(format!(
                        "MCP server '{name}': a command is required for stdio"
                    ));
                }
            }
            "http" => {
                let url = self.url.trim();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(format!(
                        "MCP server '{name}': url must start with http:// or https://"
                    ));
                }
            }
            other => {
                return Err(format!(
                    "MCP server '{name}': unknown transport '{other}' (stdio or http)"
                ));
            }
        }
        Ok(())
    }
}

/// Validate a whole server list: every entry valid and names unique.
///
/// Names must be unique because each one namespaces its tools in the shared
/// registry.
pub(crate) fn validate_mcp_servers(entries: &[McpServerEntry]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        entry.validate()?;
        if !seen.insert(entry.name.trim().to_string()) {
            return Err(format!("duplicate MCP server name '{}'", entry.name.trim()));
        }
    }
    Ok(())
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_config_value(&serde_json::Value::Null).with_env_overrides()
    }
}

impl Settings {
    /// Build settings from a discovered BOS config JSON value.
    ///
    /// Reads `[global_model]` for defaults and resolves `model` against the
    /// `[llm.<name>]` profile table when it names a profile. Missing pieces
    /// fall back to [`AgentConfig::default`].
    pub(crate) fn from_config_value(config: &serde_json::Value) -> Self {
        let fallback = AgentConfig::default();
        let global = config.get("global_model");

        let profiles: Vec<Profile> = config
            .get("llm")
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(name, value)| {
                        Some(Profile {
                            name: name.clone(),
                            model: value.get("model")?.as_str()?.to_string(),
                            base_url: value
                                .get("base_url")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            api_key: value
                                .get("api_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut settings = Self {
            model: global
                .and_then(|v| v.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or(&fallback.model)
                .to_string(),
            base_url: global
                .and_then(|v| v.get("base_url"))
                .and_then(|v| v.as_str())
                .unwrap_or(&fallback.base_url)
                .to_string(),
            api_key: String::new(),
            system_prompt: fallback.system_prompt.clone(),
            temperature: fallback.temperature,
            reasoning_effort: global
                .and_then(|v| v.get("reasoning_effort"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            config_api_key: global
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            profiles,
            bash_enabled: true,
            file_tools_enabled: true,
            bash_workspace: String::new(),
            skills_dir: DEFAULT_SKILLS_DIR.to_string(),
            require_approval: true,
            mcp_servers: Vec::new(),
        };

        settings.resolve_profile();
        settings
    }

    /// If `model` names an `[llm.<name>]` profile, pull its endpoint data.
    ///
    /// Runs after building settings from a config value and after merging
    /// profiles into file-loaded settings, so profile names resolve on every
    /// load path (and whenever the user picks a profile in the UI).
    pub(crate) fn resolve_profile(&mut self) {
        if let Some(profile) = self.profiles.iter().find(|p| p.name == self.model) {
            self.model = profile.model.clone();
            self.base_url = profile.base_url.clone();
            self.config_api_key = Some(profile.api_key.clone());
        }
    }

    /// Apply `BOS_*` environment overrides on top of the current values.
    pub(crate) fn with_env_overrides(mut self) -> Self {
        if let Ok(v) = std::env::var("BOS_MODEL") {
            if !v.is_empty() {
                self.model = v;
            }
        }
        if let Ok(v) = std::env::var("BOS_BASE_URL") {
            if !v.is_empty() {
                self.base_url = v;
            }
        }
        if let Ok(v) = std::env::var("BOS_API_KEY") {
            if !v.is_empty() {
                self.api_key = v;
            }
        }
        if let Ok(v) = std::env::var("BOS_SYSTEM_PROMPT") {
            if !v.is_empty() {
                self.system_prompt = v;
            }
        }
        self
    }

    /// Conventional settings file: `~/.bos/gui/settings.toml`.
    pub(crate) fn default_path() -> PathBuf {
        PathBuf::from(shellexpand::tilde("~/.bos/gui/settings.toml").into_owned())
    }

    /// Load settings: file first, else discovered config; then env overrides.
    pub(crate) fn load() -> Self {
        Self::load_from(&Self::default_path())
    }

    /// Load from an explicit path (used by tests); fall back to discovered config.
    pub(crate) fn load_from(path: &Path) -> Self {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(mut settings) = toml::from_str::<Settings>(&text) {
                if settings.config_api_key.is_none() {
                    settings.merge_discovered_config();
                }
                return settings.with_env_overrides();
            }
        }
        Self::from_config_value(&discover_config()).with_env_overrides()
    }

    /// Save to the conventional path.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::default_path())
    }

    /// Save to an explicit path (used by tests).
    pub(crate) fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, text)
    }

    /// The effective API key: explicit override, else discovered config key.
    pub(crate) fn effective_api_key(&self) -> String {
        if self.api_key.is_empty() {
            self.config_api_key.clone().unwrap_or_default()
        } else {
            self.api_key.clone()
        }
    }

    /// Build the [`AgentConfig`] used to create agents for chat sessions.
    pub(crate) fn agent_config(&self) -> AgentConfig {
        let config = AgentConfig::default()
            .model(self.model.clone())
            .base_url(self.base_url.clone())
            .api_key(self.effective_api_key())
            .system_prompt(self.system_prompt.clone())
            .temperature(self.temperature);
        if let Some(effort) = &self.reasoning_effort {
            config.reasoning_effort(effort.clone())
        } else {
            config
        }
    }

    fn merge_discovered_config(&mut self) {
        let discovered = Self::from_config_value(&discover_config());
        self.config_api_key = discovered.config_api_key;
        self.profiles = discovered.profiles;
        self.resolve_profile();
    }
}

/// Discover the BOS config via [`config::ConfigLoader`] (`~/.bos/conf/config.toml`, env).
fn discover_config() -> serde_json::Value {
    config::ConfigLoader::new()
        .discover()
        .load_sync()
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bos-gui-settings-{name}-{}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn from_config_reads_global_model() {
        let value = serde_json::json!({
            "global_model": {
                "model": "deepseek-chat",
                "base_url": "https://api.deepseek.com/v1",
                "api_key": "sk-cfg",
                "reasoning_effort": "high"
            },
            "agent": { "max_iterations": 5 }
        });
        let settings = Settings::from_config_value(&value);
        assert_eq!(settings.model, "deepseek-chat");
        assert_eq!(settings.base_url, "https://api.deepseek.com/v1");
        assert_eq!(settings.api_key, "");
        assert_eq!(settings.effective_api_key(), "sk-cfg");
        assert_eq!(settings.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(settings.system_prompt, "You are a helpful assistant.");
    }

    #[test]
    fn profile_names_resolve_to_endpoint() {
        let value = serde_json::json!({
            "llm": {
                "kimi-k3": {
                    "model": "kimi-k3-20260101",
                    "base_url": "https://api.moonshot.cn/v1",
                    "api_key": "sk-moon"
                }
            },
            "global_model": { "model": "gpt-4o" }
        });
        let mut settings = Settings::from_config_value(&value);
        assert_eq!(settings.model, "gpt-4o");

        // User picks a profile by name; resolution runs the way load() does.
        settings.model = "kimi-k3".into();
        settings.resolve_profile();
        assert_eq!(settings.model, "kimi-k3-20260101");
        assert_eq!(settings.base_url, "https://api.moonshot.cn/v1");

        // Profile selection via a config whose global_model names the profile.
        let value2 = serde_json::json!({
            "llm": {
                "kimi-k3": {
                    "model": "kimi-k3-20260101",
                    "base_url": "https://api.moonshot.cn/v1",
                    "api_key": "sk-moon"
                }
            },
            "global_model": { "model": "kimi-k3" }
        });
        let resolved2 = Settings::from_config_value(&value2);
        assert_eq!(resolved2.model, "kimi-k3-20260101");
        assert_eq!(resolved2.base_url, "https://api.moonshot.cn/v1");
        assert_eq!(resolved2.effective_api_key(), "sk-moon");
    }

    #[test]
    fn empty_config_falls_back_to_agent_defaults() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        assert_eq!(settings.model, "gpt-4");
        assert_eq!(settings.base_url, "https://api.openai.com/v1");
        assert!(settings.effective_api_key().is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = tmp_path("roundtrip");
        let _ = fs::remove_file(&path);

        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.model = "my-model".into();
        settings.base_url = "https://example.com/v1".into();
        settings.api_key = "sk-user".into();
        settings.system_prompt = "be brief".into();
        settings.temperature = 0.2;
        settings.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.model, "my-model");
        assert_eq!(loaded.base_url, "https://example.com/v1");
        assert_eq!(loaded.api_key, "sk-user");
        assert_eq!(loaded.system_prompt, "be brief");
        assert!((loaded.temperature - 0.2).abs() < f32::EPSILON);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn agent_config_reflects_settings() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.model = "m".into();
        settings.base_url = "https://u.example".into();
        settings.api_key = "sk".into();
        settings.system_prompt = "sys".into();
        settings.temperature = 0.1;
        settings.reasoning_effort = Some("low".into());

        let agent = settings.agent_config();
        assert_eq!(agent.model, "m");
        assert_eq!(agent.base_url, "https://u.example");
        assert_eq!(agent.api_key, "sk");
        assert_eq!(agent.system_prompt, "sys");
        assert!((agent.temperature - 0.1).abs() < f32::EPSILON);
        assert_eq!(agent.reasoning_effort.as_deref(), Some("low"));
    }

    /* ---- MCP server entries ---- */

    #[test]
    fn mcp_servers_default_empty_and_serde_roundtrips() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        assert!(settings.mcp_servers.is_empty());

        let entry = McpServerEntry {
            name: "files".into(),
            transport: "stdio".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-fs".into()],
            url: String::new(),
            enabled: true,
        };
        let toml = toml::to_string(&Settings {
            mcp_servers: vec![entry.clone()],
            ..Settings::from_config_value(&serde_json::Value::Null)
        })
        .expect("serialize");
        let parsed: Settings = toml::from_str(&toml).expect("deserialize");
        assert_eq!(parsed.mcp_servers, vec![entry]);
    }

    #[test]
    fn legacy_settings_toml_without_mcp_servers_loads() {
        let settings: Settings = toml::from_str(
            r#"
            model = "m1"
            temperature = 0.5
            "#,
        )
        .expect("missing mcp_servers must default");
        assert!(settings.mcp_servers.is_empty());
        assert_eq!(settings.model, "m1");
    }

    #[test]
    fn mcp_entry_validation_covers_transports() {
        let mut entry = McpServerEntry::default();
        assert!(entry.validate().is_err(), "blank name must fail");

        entry.name = "srv".into();
        assert!(
            entry.validate().is_err(),
            "stdio without a command must fail"
        );

        entry.command = "npx".into();
        entry.validate().expect("stdio with command passes");

        entry.transport = "http".into();
        entry.url = "ftp://nope".into();
        assert!(entry.validate().is_err(), "non-http scheme must fail");
        entry.url = "http://127.0.0.1:8931/mcp".into();
        entry.validate().expect("http url passes");

        entry.transport = "carrier-pigeon".into();
        assert!(entry.validate().is_err(), "unknown transport must fail");

        entry.name = "bad/name".into();
        entry.transport = "http".into();
        assert!(entry.validate().is_err(), "name charset must be enforced");
    }

    #[test]
    fn mcp_server_list_requires_unique_names() {
        let mk = |name: &str| McpServerEntry {
            name: name.into(),
            transport: "stdio".into(),
            command: "cmd".into(),
            ..McpServerEntry::default()
        };
        validate_mcp_servers(&[mk("a"), mk("b")]).expect("distinct names pass");
        let dup = validate_mcp_servers(&[mk("a"), mk(" a ")]);
        assert!(dup.is_err(), "trimmed-duplicate names must fail");
    }
}
