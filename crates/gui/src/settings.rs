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

/// Default persistent-memory file (`~` expands at agent build time).
pub(crate) const DEFAULT_MEMORY_PATH: &str = "~/.bos/gui/memory.jsonl";

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
    /// Fold the workspace's `AGENTS.md`-style instruction files into the
    /// system prompt of every agent built for [`Self::bash_workspace`]
    /// (codex/harness parity; empty workspace stays a no-op).
    pub(crate) project_instructions: bool,
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
    /// Tools the user allowed permanently ("always allow"), so an approval is
    /// asked once rather than every turn. Ordered and de-duplicated; the
    /// per-session allowances live in the broker and are deliberately not here,
    /// because they are not meant to outlive the session.
    #[serde(default)]
    pub(crate) allowed_tools: Vec<String>,
    /// Send-side context budget in tokens: transcripts estimated over this
    /// are compacted into a summary before each turn; 0 disables it.
    pub(crate) context_budget: usize,
    /// Attach a persistent memory store so the agent learns across sessions.
    pub(crate) memory_enabled: bool,
    /// Memory JSON-lines file; empty means `~/.bos/gui/memory.jsonl`.
    pub(crate) memory_path: String,
    /// User-configured MCP servers ("plugins") whose tools join the agent.
    pub(crate) mcp_servers: Vec<McpServerEntry>,
    /// User-configured fallback LLM providers, tried in order after the
    /// primary endpoint when a request fails.
    pub(crate) providers: Vec<ProviderEntry>,

    /// Workspaces: each owns the things that only make sense next to a root
    /// folder — its sessions, the LLM binding, tools, skills, MCP servers and
    /// memory. See §24 of the GUI requirements.
    pub(crate) workspaces: Vec<Workspace>,
    /// Id of the workspace the GUI is showing; empty means "the first one".
    pub(crate) active_workspace: String,
    /// Built-in tools switched off, carried here so that
    /// [`Settings::effective`] can hand the agent one flat instruction set.
    /// The globals leave it empty; the workspace fills it.
    pub(crate) disabled_tools: Vec<String>,
    /// Skills switched off, same carrier as [`Self::disabled_tools`].
    pub(crate) disabled_skills: Vec<String>,
    /// Reusable workspace configurations, offered when creating a workspace.
    pub(crate) presets: Vec<Preset>,

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

/// One discovered `[llm.<name>]` profile, as the webview may see it.
///
/// A profile is selected by typing its **name** into the model field, so the UI
/// needs the names; it never needs the keys. `has_key` exists so the UI can say
/// whether a profile is usable without ever receiving the secret.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct ProfileInfo {
    /// Profile name, the value that selects it in the model field.
    pub(crate) name: String,
    /// Model identifier the profile points at.
    pub(crate) model: String,
    /// Base URL the profile points at.
    pub(crate) base_url: String,
    /// Whether the profile carries a key (the key itself is never included).
    pub(crate) has_key: bool,
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

/// One fallback LLM endpoint tried after the primary provider errors.
///
/// Persisted as `[[providers]]` entries in `settings.toml`. Entries are
/// tried in list order, each at most once per request, so a dead cloud
/// endpoint can fall through to a local one without losing the session.
/// Disabled entries persist but are never tried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProviderEntry {
    /// Unique display name (e.g. `local`, `backup`).
    pub(crate) name: String,
    /// OpenAI-compatible base URL of the fallback endpoint.
    pub(crate) base_url: String,
    /// API key of the fallback endpoint; empty inherits the primary key.
    pub(crate) api_key: String,
    /// Model identifier in `vendor/model` form.
    pub(crate) model: String,
    /// Disabled entries persist but are never tried.
    pub(crate) enabled: bool,
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            enabled: true,
        }
    }
}

impl ProviderEntry {
    /// Validate this entry on its own: name plus required endpoint fields.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("provider name must not be empty".to_string());
        }
        if self.base_url.trim().is_empty() {
            return Err(format!("provider '{name}': base URL must not be empty"));
        }
        if self.model.trim().is_empty() {
            return Err(format!("provider '{name}': model must not be empty"));
        }
        Ok(())
    }
}

/// Validate a whole provider list: every entry valid and names unique.
///
/// Names must be unique so the settings UI and logs can point at one
/// endpoint unambiguously.
pub(crate) fn validate_providers(entries: &[ProviderEntry]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        entry.validate()?;
        if !seen.insert(entry.name.trim().to_string()) {
            return Err(format!("duplicate provider name '{}'", entry.name.trim()));
        }
    }
    Ok(())
}

/// One workspace: the aggregate that owns everything which only makes sense next
/// to a root folder (§24 of the GUI requirements).
/// The runtime fields a workspace may override.
///
/// Grouped into one argument so adding a field does not lengthen a
/// nine-argument call and push every call site through a rename: the last two
/// rounds each paid that tax, and this is the last time they have to.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceRuntime {
    /// Model id, blank to inherit.
    pub(crate) model: String,
    /// Base URL, blank to inherit.
    pub(crate) base_url: String,
    /// API key, blank to inherit.
    pub(crate) api_key: String,
    /// System prompt, empty to inherit.
    pub(crate) system_prompt: String,
    /// Sampling temperature, `None` to inherit.
    pub(crate) temperature: Option<f32>,
    /// Reasoning effort, blank to inherit.
    pub(crate) reasoning_effort: String,
    /// Memory file, blank to inherit the shared one.
    pub(crate) memory_path: String,
    /// Skills directory, blank to inherit.
    pub(crate) skills_dir: String,
    /// Whether memory is attached, `None` to leave the workspace's own answer.
    pub(crate) memory_enabled: Option<bool>,
    /// Bash tool, `None` to leave the workspace's own answer.
    pub(crate) bash_enabled: Option<bool>,
    /// File tools, `None` to leave the workspace's own answer.
    pub(crate) file_tools_enabled: Option<bool>,
    /// Whether tool calls need approval, `None` to leave the workspace's answer.
    pub(crate) require_approval: Option<bool>,
    /// Context budget in tokens, `None` to leave the workspace's own answer.
    pub(crate) context_budget: Option<usize>,
    /// Whether `AGENTS.md` is read, `None` to leave the workspace's answer.
    pub(crate) project_instructions: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Workspace {
    /// Stable identifier. Migration seeds the copied-over globals as `default`,
    /// so adopting old session files stays deterministic.
    pub(crate) id: String,
    /// Human name shown in the left panel.
    pub(crate) name: String,
    /// Root folder (absolute). Empty means the process working directory.
    pub(crate) root: String,
    /// Model identifier or `[llm.<name>]` profile name for this workspace.
    pub(crate) model: String,
    /// API base URL; empty inherits the global one.
    pub(crate) base_url: String,
    /// API key override; empty inherits the global one.
    pub(crate) api_key: String,
    /// System prompt prepended to this workspace's conversations; empty
    /// inherits the global one.
    pub(crate) system_prompt: String,
    /// Sampling temperature. `None` inherits — 0.0 is a legitimate value, so
    /// this one cannot use the "empty means inherit" rule the strings use.
    pub(crate) temperature: Option<f32>,
    /// Reasoning effort (`low`/`medium`/`high`); `None` inherits.
    pub(crate) reasoning_effort: Option<String>,
    /// Fallback providers tried after the primary endpoint.
    pub(crate) providers: Vec<ProviderEntry>,
    /// Whether the bash tool is registered here.
    pub(crate) bash_enabled: bool,
    /// Whether the filesystem tools are registered here.
    pub(crate) file_tools_enabled: bool,
    /// Built-in tools switched off in this workspace (the §24.8 deny-list).
    pub(crate) disabled_tools: Vec<String>,
    /// Skills switched off in this workspace (the §24.8 deny-list).
    pub(crate) disabled_skills: Vec<String>,
    /// MCP servers, each carrying its own opt-in switch.
    pub(crate) mcp_servers: Vec<McpServerEntry>,
    /// Require one-click approval before `bash`/`write_file`.
    pub(crate) require_approval: bool,
    /// Send-side context budget in tokens; 0 disables compaction.
    pub(crate) context_budget: usize,
    /// Skills directory scanned for `SKILL.md` folders.
    pub(crate) skills_dir: String,
    /// Fold `AGENTS.md`-style instruction files into this workspace's prompt.
    pub(crate) project_instructions: bool,
    /// Persistent memory store for this workspace.
    pub(crate) memory_enabled: bool,
    /// Memory JSON-lines file; empty means the shared default.
    pub(crate) memory_path: String,
    /// Unix seconds when the workspace was created.
    pub(crate) created_at: u64,
    /// Unix seconds of the last edit.
    pub(crate) updated_at: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            root: String::new(),
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            system_prompt: String::new(),
            temperature: None,
            reasoning_effort: None,
            providers: Vec::new(),
            bash_enabled: true,
            file_tools_enabled: true,
            disabled_tools: Vec::new(),
            disabled_skills: Vec::new(),
            mcp_servers: Vec::new(),
            require_approval: true,
            context_budget: 32_768,
            skills_dir: DEFAULT_SKILLS_DIR.to_string(),
            project_instructions: true,
            memory_enabled: true,
            memory_path: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Workspace {
    /// Copy the pasted-over global settings into the first workspace, so that
    /// migration adds a workspace rather than moving anything.
    pub(crate) fn adopted_from(settings: &Settings, id: &str, now: u64) -> Self {
        Self {
            id: id.to_string(),
            name: if settings.bash_workspace.trim().is_empty() {
                "Default".to_string()
            } else {
                settings.bash_workspace.clone()
            },
            root: settings.bash_workspace.clone(),
            model: settings.model.clone(),
            base_url: settings.base_url.clone(),
            api_key: settings.api_key.clone(),
            system_prompt: settings.system_prompt.clone(),
            temperature: Some(settings.temperature),
            reasoning_effort: settings.reasoning_effort.clone(),
            providers: settings.providers.clone(),
            bash_enabled: settings.bash_enabled,
            file_tools_enabled: settings.file_tools_enabled,
            disabled_tools: Vec::new(),
            disabled_skills: Vec::new(),
            mcp_servers: settings.mcp_servers.clone(),
            require_approval: settings.require_approval,
            context_budget: settings.context_budget,
            skills_dir: settings.skills_dir.clone(),
            project_instructions: settings.project_instructions,
            memory_enabled: settings.memory_enabled,
            memory_path: settings.memory_path.clone(),
            created_at: now,
            updated_at: now,
        }
    }
}

impl Settings {
    /// Configure how one workspace runs: the model and endpoint it talks to and
    /// how hard it should think. Empty strings inherit from the globals (the same
    /// rule `effective` applies), while `temperature` is an `Option` because 0.0
    /// is a legitimate setting that "empty means inherit" cannot express.
    // Seven knobs is what a workspace's runtime is; grouping them into a struct
    // would only move the same fields a level out of sight.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_workspace_runtime(
        &mut self,
        id: &str,
        runtime: &WorkspaceRuntime,
    ) -> Result<(), String> {
        let Some(w) = self.workspaces.iter_mut().find(|w| w.id == id) else {
            return Err(format!("unknown workspace '{id}'"));
        };
        w.model = runtime.model.trim().to_string();
        w.base_url = runtime.base_url.trim().to_string();
        w.api_key = runtime.api_key.trim().to_string();
        w.system_prompt = runtime.system_prompt.to_string();
        w.temperature = runtime.temperature;
        let effort = runtime.reasoning_effort.trim();
        w.reasoning_effort = if effort.is_empty() {
            None
        } else {
            Some(effort.to_string())
        };
        // Memory is per workspace, like the model: blank inherits the shared file.
        w.memory_path = runtime.memory_path.trim().to_string();
        // Skills follow the same rule: blank inherits the shared directory.
        w.skills_dir = runtime.skills_dir.trim().to_string();
        // The one field here that can be left alone: a payload that does not
        // mention the switch keeps the workspace's own answer, because "memory
        // off" and "no opinion" are different states and only one is a bool.
        if let Some(on) = runtime.memory_enabled {
            w.memory_enabled = on;
        }
        // The same rule for the rest of the policy the workspace owns: an absent
        // field is not an answer, so these five are left as they are.
        if let Some(on) = runtime.bash_enabled {
            w.bash_enabled = on;
        }
        if let Some(on) = runtime.file_tools_enabled {
            w.file_tools_enabled = on;
        }
        if let Some(on) = runtime.require_approval {
            w.require_approval = on;
        }
        if let Some(on) = runtime.context_budget {
            w.context_budget = on;
        }
        if let Some(on) = runtime.project_instructions {
            w.project_instructions = on;
        }

        w.updated_at = crate::session::unix_now();
        Ok(())
    }

    /// Save a workspace as a reusable preset. The preset keeps a **copy** of the
    /// workspace's configuration and gets its own id, so later edits on either
    /// side do not travel (§24.9). Names are how a preset is picked, so a
    /// duplicate is refused rather than silently shadowing one.
    pub(crate) fn save_preset(&mut self, workspace_id: &str, name: &str) -> Result<String, String> {
        let Some(w) = self
            .workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .cloned()
        else {
            return Err(format!("unknown workspace '{workspace_id}'"));
        };
        let preset_name = if name.trim().is_empty() {
            w.name.clone()
        } else {
            name.trim().to_string()
        };
        if preset_name.trim().is_empty() {
            return Err("a preset needs a name".to_string());
        }
        if self.presets.iter().any(|p| p.name == preset_name) {
            return Err(format!("a preset named '{preset_name}' already exists"));
        }
        let mut copy = w;
        // A preset is not a workspace: it has no sessions of its own, so the id is
        // a placeholder until `apply_preset` gives the new workspace a real one.
        copy.id = String::new();
        let id = uuid::Uuid::new_v4().to_string();
        self.presets.push(Preset {
            id: id.clone(),
            name: preset_name,
            workspace: copy,
        });
        Ok(id)
    }

    /// Apply a preset by **creating** a workspace from it and switching to that
    /// workspace. The copy is the point (§24.9): were a workspace to follow its
    /// preset, one edit would silently rewrite every workspace made from it.
    pub(crate) fn apply_preset(&mut self, id: &str, name: &str) -> Result<String, String> {
        let Some(preset) = self.presets.iter().find(|p| p.id == id).cloned() else {
            return Err(format!("unknown preset '{id}'"));
        };
        let now = crate::session::unix_now();
        let new_id = uuid::Uuid::new_v4().to_string();
        let mut w = preset.workspace;
        w.id = new_id.clone();
        w.created_at = now;
        w.updated_at = now;
        let wanted = name.trim().to_string();
        w.name = if !wanted.is_empty() {
            wanted
        } else if !preset.name.trim().is_empty() {
            preset.name
        } else {
            w.root.clone()
        };
        self.workspaces.push(w);
        self.active_workspace = new_id.clone();
        Ok(new_id)
    }

    /// The settings an agent is actually built from: the global defaults with the
    /// active workspace's own choices applied on top (§24). Owned values replace
    /// the globals; an empty string inherits the global one, which is what keeps a
    /// half-configured workspace usable.
    pub(crate) fn effective(&self) -> Settings {
        let mut s = self.clone();
        let Some(w) = self
            .workspaces
            .iter()
            .find(|w| w.id == self.active_workspace)
            .or_else(|| self.workspaces.first())
        else {
            return s;
        };
        fn pick(own: &str, global: &str) -> String {
            if own.trim().is_empty() {
                global.to_string()
            } else {
                own.to_string()
            }
        }
        s.model = pick(&w.model, &self.model);
        s.base_url = pick(&w.base_url, &self.base_url);
        s.api_key = pick(&w.api_key, &self.api_key);
        s.bash_workspace = pick(&w.root, &self.bash_workspace);
        s.skills_dir = pick(&w.skills_dir, &self.skills_dir);
        s.memory_path = pick(&w.memory_path, &self.memory_path);
        if !w.system_prompt.is_empty() {
            s.system_prompt = w.system_prompt.clone();
        }
        if let Some(t) = w.temperature {
            s.temperature = t;
        }
        if w.reasoning_effort.is_some() {
            s.reasoning_effort = w.reasoning_effort.clone();
        }
        if !w.providers.is_empty() {
            s.providers = w.providers.clone();
        }
        // Booleans, budget and the selection sets are the workspace's own
        // (migration copies the globals into the first workspace, so the default
        // installation keeps behaving exactly as before).
        s.bash_enabled = w.bash_enabled;
        s.file_tools_enabled = w.file_tools_enabled;
        s.require_approval = w.require_approval;
        s.context_budget = w.context_budget;
        s.project_instructions = w.project_instructions;
        s.memory_enabled = w.memory_enabled;
        s.disabled_tools = w.disabled_tools.clone();
        s.disabled_skills = w.disabled_skills.clone();
        s.mcp_servers = w.mcp_servers.clone();
        // A workspace may name an `[llm.<name>]` profile instead of a model.
        s.resolve_profile();
        s
    }

    /// Give every installation at least one workspace exactly once.
    ///
    /// Today's globals are **copied** into a `default` workspace (never moved),
    /// so a downgrade still finds them, and every capability keeps the state it
    /// had — a disabled MCP server does not come back enabled, a tool that was
    /// on does not come back off (§24.3, acceptance 8).
    pub(crate) fn migrate(&mut self) {
        if self.workspaces.is_empty() {
            let now = crate::session::unix_now();
            self.workspaces
                .push(Workspace::adopted_from(self, "default", now));
        }
        if !self
            .workspaces
            .iter()
            .any(|w| w.id == self.active_workspace)
        {
            self.active_workspace = self.workspaces[0].id.clone();
        }
    }
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
            project_instructions: true,
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
            context_budget: 32_768,
            memory_enabled: true,
            memory_path: String::new(),
            mcp_servers: Vec::new(),
            providers: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: String::new(),
            disabled_tools: Vec::new(),
            disabled_skills: Vec::new(),
            presets: Vec::new(),
            allowed_tools: Vec::new(),
        };

        settings.resolve_profile();
        settings
    }

    /// If `model` names an `[llm.<name>]` profile, pull its endpoint data.
    ///
    /// The discovered profiles, without their keys.
    ///
    /// Sorted by name so the UI's list is stable across restarts.
    pub(crate) fn profile_infos(&self) -> Vec<ProfileInfo> {
        let mut out: Vec<ProfileInfo> = self
            .profiles
            .iter()
            .map(|p| ProfileInfo {
                name: p.name.clone(),
                model: p.model.clone(),
                base_url: p.base_url.clone(),
                has_key: !p.api_key.trim().is_empty(),
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

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
        // The workspace in use has its own binding, and the environment is the
        // outermost override, so it has to reach that binding too.
        if let Some(w) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace)
        {
            if let Ok(v) = std::env::var("BOS_MODEL") {
                if !v.is_empty() {
                    w.model = v;
                }
            }
            if let Ok(v) = std::env::var("BOS_BASE_URL") {
                if !v.is_empty() {
                    w.base_url = v;
                }
            }
            if let Ok(v) = std::env::var("BOS_API_KEY") {
                if !v.is_empty() {
                    w.api_key = v;
                }
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
                settings.migrate();
                return settings.with_env_overrides();
            }
        }
        let mut settings = Self::from_config_value(&discover_config());
        settings.migrate();
        settings.with_env_overrides()
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
    ///
    /// The effective system prompt is the base prompt plus, when
    /// [`Self::project_instructions`] is on, the workspace's instruction
    /// files ([`crate::instructions`]), so a session created after the
    /// user edits `AGENTS.md` picks the new rules up immediately. An
    /// empty workspace keeps the prompt untouched.
    ///
    /// Enabled [`ProviderEntry`]s become an ordered fallback chain: each
    /// one is retried after the primary endpoint errors (invalid or
    /// disabled entries are skipped, never sent).
    pub(crate) fn agent_config(&self) -> AgentConfig {
        let mut prompt = self.system_prompt.clone();
        if self.project_instructions {
            let ws = self.bash_workspace.trim();
            if !ws.is_empty() {
                let root = PathBuf::from(shellexpand::tilde(ws).into_owned());
                if let Some(extra) = crate::instructions::load(&root) {
                    prompt.push_str("\n\n");
                    prompt.push_str(&extra);
                }
            }
        }
        let mut config = AgentConfig::default()
            .model(self.model.clone())
            .base_url(self.base_url.clone())
            .api_key(self.effective_api_key())
            .system_prompt(prompt)
            .temperature(self.temperature);
        if let Some(effort) = &self.reasoning_effort {
            config = config.reasoning_effort(effort.clone());
        }
        for entry in &self.providers {
            if !entry.enabled || entry.validate().is_err() {
                continue;
            }
            let api_key = if entry.api_key.is_empty() {
                self.effective_api_key()
            } else {
                entry.api_key.clone()
            };
            config = config.fallback(agent::FallbackProvider {
                base_url: entry.base_url.trim().to_string(),
                api_key,
                model: entry.model.trim().to_string(),
            });
        }
        config
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

    #[test]
    fn profile_infos_carry_names_and_never_keys() {
        let settings = Settings {
            profiles: vec![
                Profile {
                    name: "remote".into(),
                    model: "big/model".into(),
                    base_url: "https://example.test/v1".into(),
                    api_key: "sk-secret-value".into(),
                },
                Profile {
                    name: "local".into(),
                    model: "small/model".into(),
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    api_key: "   ".into(),
                },
            ],
            ..Settings::default()
        };
        let infos = settings.profile_infos();
        assert_eq!(
            infos.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            vec!["local", "remote"],
            "sorted by name, so the list does not move between restarts"
        );
        assert!(infos[1].has_key, "remote carries a key");
        assert!(!infos[0].has_key, "a blank key is not a key");
        let json = serde_json::to_string(&infos).expect("serializable");
        assert!(
            !json.contains("sk-secret-value"),
            "the webview payload must not contain key material: {json}"
        );
    }

    #[test]
    fn a_workspace_runtime_payload_covers_the_policy_fields() {
        let mut settings = Settings::default();
        settings.migrate();
        let id = settings.workspaces[0].id.clone();
        let before = settings.effective();
        // An absent field is not an answer: it must not reset policy the
        // workspace already had.
        settings
            .set_workspace_runtime(&id, &WorkspaceRuntime::default())
            .expect("known id");
        let after = settings.effective();
        assert_eq!(after.bash_enabled, before.bash_enabled);
        assert_eq!(after.file_tools_enabled, before.file_tools_enabled);
        assert_eq!(after.require_approval, before.require_approval);
        assert_eq!(after.context_budget, before.context_budget);
        assert_eq!(after.project_instructions, before.project_instructions);
        settings
            .set_workspace_runtime(
                &id,
                &WorkspaceRuntime {
                    bash_enabled: Some(!before.bash_enabled),
                    file_tools_enabled: Some(!before.file_tools_enabled),
                    require_approval: Some(!before.require_approval),
                    context_budget: Some(before.context_budget + 4096),
                    project_instructions: Some(!before.project_instructions),
                    ..Default::default()
                },
            )
            .expect("known id");
        let after = settings.effective();
        assert_eq!(after.bash_enabled, !before.bash_enabled);
        assert_eq!(after.file_tools_enabled, !before.file_tools_enabled);
        assert_eq!(after.require_approval, !before.require_approval);
        assert_eq!(after.context_budget, before.context_budget + 4096);
        assert_eq!(after.project_instructions, !before.project_instructions);
    }

    #[test]
    fn a_workspace_runtime_payload_can_flip_the_memory_switch() {
        let mut settings = Settings::default();
        settings.migrate();
        let id = settings.workspaces[0].id.clone();
        let before = settings.effective().memory_enabled;
        // `None` is "no opinion": the workspace keeps its own answer, which is
        // the only field in the payload with that meaning, because "off" and
        // "unset" are different states and only one of them is a bool.
        settings
            .set_workspace_runtime(&id, &WorkspaceRuntime::default())
            .expect("known id");
        assert_eq!(settings.effective().memory_enabled, before, "left alone");
        settings
            .set_workspace_runtime(
                &id,
                &WorkspaceRuntime {
                    memory_enabled: Some(!before),
                    ..Default::default()
                },
            )
            .expect("known id");
        assert_eq!(settings.effective().memory_enabled, !before, "flipped");
    }

    #[test]
    fn a_workspace_can_keep_its_own_skills_dir() {
        let mut settings = Settings::default();
        settings.migrate();
        let global = settings.skills_dir.clone();
        let id = settings.workspaces[0].id.clone();
        settings
            .set_workspace_runtime(&id, &WorkspaceRuntime::default())
            .expect("known id");
        assert_eq!(settings.effective().skills_dir, global);
        settings
            .set_workspace_runtime(
                &id,
                &WorkspaceRuntime {
                    skills_dir: "~/w1-skills".to_string(),
                    ..Default::default()
                },
            )
            .expect("known id");
        assert_eq!(settings.effective().skills_dir, "~/w1-skills");
    }

    #[test]
    fn a_workspace_can_keep_its_own_memory_file() {
        let mut settings = Settings::default();
        // A fresh `Settings` carries no workspaces until `migrate` adopts one for
        // the current folder.
        settings.migrate();
        let global = settings.memory_path.clone();
        let id = settings.workspaces[0].id.clone();
        // Blank means "the shared file", the same rule every runtime field follows.
        settings
            .set_workspace_runtime(&id, &WorkspaceRuntime::default())
            .expect("known id");
        assert_eq!(settings.effective().memory_path, global);
        settings
            .set_workspace_runtime(
                &id,
                &WorkspaceRuntime {
                    memory_path: "~/w1.jsonl".to_string(),
                    ..Default::default()
                },
            )
            .expect("known id");
        assert_eq!(settings.effective().memory_path, "~/w1.jsonl");
    }
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
    fn memory_defaults_to_enabled_file_store() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        assert!(settings.memory_enabled, "persistence on by default");
        assert_eq!(
            settings.memory_path, "",
            "empty means DEFAULT_MEMORY_PATH (~/.bos/gui/memory.jsonl)"
        );
        assert_eq!(DEFAULT_MEMORY_PATH, "~/.bos/gui/memory.jsonl");

        // The toggle and path survive a save/load round trip.
        let path = tmp_path("memory");
        let mut custom = settings;
        custom.memory_enabled = false;
        custom.memory_path = "/tmp/bos-mem-test.jsonl".to_string();
        custom.save_to(&path).expect("save");
        let loaded = Settings::load_from(&path);
        assert!(!loaded.memory_enabled);
        assert_eq!(loaded.memory_path, "/tmp/bos-mem-test.jsonl");
        let _ = std::fs::remove_file(&path);
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
    fn migration_copies_globals_without_flipping_capabilities() {
        let mut s = Settings::from_config_value(&serde_json::Value::Null);
        s.model = "example/model".to_string();
        s.bash_workspace = "/tmp/ws".to_string();
        s.bash_enabled = false;
        s.mcp_servers = vec![McpServerEntry {
            name: "off-by-choice".to_string(),
            enabled: false,
            ..McpServerEntry::default()
        }];
        s.migrate();

        assert_eq!(s.workspaces.len(), 1);
        let w = &s.workspaces[0];
        assert_eq!(w.id, "default");
        assert_eq!(s.active_workspace, "default");
        assert_eq!(w.root, "/tmp/ws");
        assert_eq!(w.model, "example/model");
        // Acceptance 8: a switched-off server stays off, a tool stays off.
        assert!(!w.mcp_servers[0].enabled);
        assert!(!w.bash_enabled);
        // The globals are copied, not moved, so a downgrade still finds them.
        assert_eq!(s.bash_workspace, "/tmp/ws");
        assert_eq!(s.model, "example/model");

        // Idempotent: migrating twice does not add a second workspace.
        s.migrate();
        assert_eq!(s.workspaces.len(), 1);
    }

    #[test]
    fn effective_settings_apply_the_workspace_and_inherit_blanks() {
        let mut s = Settings::from_config_value(&serde_json::Value::Null);
        s.model = "global/model".to_string();
        s.bash_workspace = "/global".to_string();
        s.migrate();
        s.workspaces[0].model = String::new(); // blank inherits
        s.workspaces[0].root = "/ws".to_string();
        s.workspaces[0].bash_enabled = false;
        s.workspaces[0].disabled_tools = vec!["bash".to_string()];
        s.workspaces[0].temperature = Some(0.2);
        let eff = s.effective();
        assert_eq!(eff.model, "global/model", "a blank field inherits");
        assert_eq!(eff.bash_workspace, "/ws", "an owned field replaces");
        assert!(!eff.bash_enabled);
        assert_eq!(eff.disabled_tools, vec!["bash".to_string()]);
        assert_eq!(eff.temperature, 0.2, "a workspace temperature wins");
        // The globals are untouched: resolution is a copy, not a mutation.
        assert_eq!(s.bash_workspace, "/global");
        assert!(s.disabled_tools.is_empty());
    }

    #[test]
    fn an_unknown_active_workspace_falls_back_to_the_first() {
        let mut s = Settings::from_config_value(&serde_json::Value::Null);
        s.migrate();
        s.active_workspace = "gone".to_string();
        s.migrate();
        assert_eq!(s.active_workspace, "default");
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

    #[test]
    fn agent_config_folds_workspace_instructions_and_honors_toggle() {
        let dir =
            std::env::temp_dir().join(format!("bos-gui-settings-instr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "always answer in French").unwrap();

        let mut settings = Settings {
            system_prompt: "base prompt".into(),
            bash_workspace: dir.to_string_lossy().into_owned(),
            ..Settings::default()
        };

        let folded = settings.agent_config().system_prompt;
        assert!(
            folded.starts_with("base prompt"),
            "base prompt stays first: {folded}"
        );
        assert!(folded.contains("always answer in French"));

        settings.project_instructions = false;
        let plain = settings.agent_config().system_prompt;
        assert!(!plain.contains("French"), "toggle must disable folding");
        assert_eq!(plain, "base prompt");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_config_leaves_prompt_alone_without_workspace() {
        let settings = Settings::default();
        assert!(settings.bash_workspace.trim().is_empty());
        // Even with an AGENTS.md on the real disk, no workspace means no
        // folding: the composed prompt equals the configured one.
        assert_eq!(
            settings.agent_config().system_prompt,
            settings.system_prompt
        );
    }

    /* ---- Fallback providers ---- */

    fn provider(name: &str, model: &str) -> ProviderEntry {
        ProviderEntry {
            name: name.to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            api_key: String::new(),
            model: model.to_string(),
            enabled: true,
        }
    }

    #[test]
    fn provider_entries_validate_and_dedupe() {
        let good = provider("local", "vendor/model");
        assert!(good.validate().is_ok());

        let mut no_url = good.clone();
        no_url.base_url = "  ".to_string();
        assert!(no_url.validate().is_err(), "blank url is rejected");

        let mut no_model = good.clone();
        no_model.model = String::new();
        assert!(no_model.validate().is_err(), "blank model is rejected");

        assert!(
            validate_providers(&[ProviderEntry::default()]).is_err(),
            "blank name is rejected"
        );
        assert!(validate_providers(&[good.clone(), good.clone()])
            .unwrap_err()
            .contains("duplicate provider"));
        assert!(validate_providers(&[good]).is_ok());
    }

    #[test]
    fn agent_config_maps_enabled_valid_providers_in_order() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        let disabled = ProviderEntry {
            enabled: false,
            ..provider("off", "skip/me")
        };
        let mut broken = provider("broken", " ");
        broken.model = " ".to_string();
        settings.providers = vec![
            provider("first", "vendor-a/model"),
            disabled,
            broken,
            ProviderEntry {
                api_key: "other-key".to_string(),
                ..provider("second", "vendor-b/model")
            },
        ];

        let config = settings.agent_config();
        assert_eq!(
            config.fallbacks.len(),
            2,
            "disabled and invalid entries must be skipped, never sent"
        );
        assert_eq!(config.fallbacks[0].model, "vendor-a/model");
        assert_eq!(
            config.fallbacks[0].api_key,
            settings.effective_api_key(),
            "a blank key inherits the primary key"
        );
        assert_eq!(config.fallbacks[1].model, "vendor-b/model");
        assert_eq!(config.fallbacks[1].api_key, "other-key");
        assert_eq!(
            config.fallbacks[1].base_url, "http://127.0.0.1:9/v1",
            "trailing whitespace is trimmed before dispatch"
        );
    }

    #[test]
    fn provider_entries_serde_roundtrip_and_legacy_toml_defaults_empty() {
        assert!(Settings::from_config_value(&serde_json::Value::Null)
            .providers
            .is_empty());

        let entry = provider("local", "nvidia/z-ai/glm-5.3-flash");
        let toml = toml::to_string(&Settings {
            providers: vec![entry.clone()],
            ..Settings::from_config_value(&serde_json::Value::Null)
        })
        .expect("serialize");
        let parsed: Settings = toml::from_str(&toml).expect("deserialize");
        assert_eq!(parsed.providers, vec![entry]);

        // A settings file written before failover support loads unchanged.
        let legacy: Settings = toml::from_str(r#"model = "m1""#).expect("parse");
        assert!(legacy.providers.is_empty());
    }
}

/// A reusable workspace configuration. Applying one creates a workspace from it,
/// so the result diverges immediately (§24.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Preset {
    /// Stable id of the preset itself.
    pub(crate) id: String,
    /// Name shown when offering the preset.
    pub(crate) name: String,
    /// The configuration copied into a new workspace.
    pub(crate) workspace: Workspace,
}
