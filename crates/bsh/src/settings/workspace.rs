//! The per-workspace settings file: how the agent runs in one directory.
//!
//! `<root>/.bsh/settings/settings.toml`. Everything the agent's behaviour
//! depends on lives here, because it is a property of the project rather than
//! of the machine. See `docs/bsh-settings-spec.md`.
//!
//! Absent is not the same as a value. Every inheritable field is an `Option`,
//! so `temperature = 0.0` means greedy decoding rather than "inherit" - the
//! sentinel scheme the previous flat file used is gone.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::global::{ModelRef, SettingsError};

/// The per-workspace settings file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceSettings {
    /// Format version; each file versions independently.
    pub(crate) schema_version: u32,
    /// The workspace's own selection: which model, preset and session.
    pub(crate) general: WorkspaceGeneral,
    /// How the agent is configured here.
    pub(crate) agent: AgentSection,
    /// What is switched off here.
    pub(crate) disabled: Disabled,
}

/// The workspace's selection out of the global definitions.
///
/// Each of these names something the global file defines, in the same way:
/// definitions live globally and the workspace records which one it uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceGeneral {
    /// The provider id this workspace uses by default.
    pub(crate) provider: String,
    /// The model nickname under that provider.
    pub(crate) model: String,
    /// The session last open in this workspace.
    pub(crate) active_session: String,
}

impl WorkspaceGeneral {
    /// The model this workspace selects, or none while it selects none.
    pub(crate) fn model_ref(&self) -> Option<ModelRef> {
        if self.provider.is_empty() || self.model.is_empty() {
            return None;
        }
        Some(ModelRef {
            provider: self.provider.clone(),
            model: self.model.clone(),
        })
    }
}

/// How the agent is configured in this workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentSection {
    /// The system prompt.
    pub(crate) system_prompt: String,
    /// Sampling temperature; absent inherits the engine's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    /// Reasoning effort; absent inherits the engine's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    /// The permission mode: "read-only", "workspace-write" or
    /// "danger-full-access".
    pub(crate) permission: String,
    /// Register the shell tool.
    pub(crate) bash_enabled: bool,
    /// Register the filesystem tools.
    pub(crate) file_tools_enabled: bool,
    /// Offer the model a hosted web-search tool.
    ///
    /// The provider performs the search, so this registers nothing locally; it
    /// only decides whether the tool is offered, and the engine then requires the
    /// Responses protocol, which is the only one that carries hosted tools.
    pub(crate) web_tools_enabled: bool,
    /// Run one assistant turn's tool calls concurrently.
    pub(crate) parallel_tool_calls: bool,
    /// Attach the file-backed memory store.
    pub(crate) memory_enabled: bool,
    /// Completion cap; absent leaves it to the engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    /// Send-side token ceiling; 0 disables compaction.
    pub(crate) context_budget: usize,
    /// ReAct steps one turn may take before the loop stops.
    pub(crate) max_steps: usize,
    /// Seconds one shell command may run before it is killed.
    pub(crate) shell_timeout_secs: u64,
    /// Bytes of one tool result kept before the rest is truncated.
    pub(crate) tool_output_cap: usize,
    /// The skill folder, overriding the global one when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skills_dir: Option<String>,
    /// The memory file, overriding the global one when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memory_path: Option<String>,
    /// MCP servers this workspace adds, merged with the global list.
    pub(crate) mcp_servers: Vec<serde_json::Value>,
}

impl Default for AgentSection {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            temperature: None,
            reasoning_effort: None,
            permission: "workspace-write".to_string(),
            bash_enabled: true,
            file_tools_enabled: true,
            web_tools_enabled: true,
            parallel_tool_calls: false,
            memory_enabled: true,
            max_tokens: None,
            context_budget: 32768,
            max_steps: 10,
            shell_timeout_secs: 300,
            tool_output_cap: 16384,
            skills_dir: None,
            memory_path: None,
            mcp_servers: Vec::new(),
        }
    }
}

/// What this workspace switches off.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Disabled {
    /// Tool names withheld from the build.
    pub(crate) tools: Vec<String>,
    /// Skill names withheld from the registry.
    pub(crate) skills: Vec<String>,
    /// Installed extensions switched off here. The global inventory decides
    /// what is installed; this only subtracts.
    pub(crate) extentions: Vec<String>,
}

impl WorkspaceSettings {
    /// Parse the workspace settings from TOML text.
    pub(crate) fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let settings: Self =
            toml::from_str(text).map_err(|e| SettingsError::Parse(e.to_string()))?;
        settings.validate()?;
        Ok(settings)
    }

    /// A workspace's settings file, under the workspace it configures.
    ///
    /// Per workspace by construction: the file travels with the checkout, so a
    /// second machine that clones it inherits the same agent configuration
    /// without carrying anyone's credentials (those stay in the global file).
    pub(crate) fn path(root: &Path) -> PathBuf {
        root.join(".bsh").join("settings").join("settings.toml")
    }

    /// Read a workspace's settings file, treating absence as a fresh workspace.
    pub(crate) fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::Io(e.to_string())),
        }
    }

    /// Write a workspace's settings file, creating its directory.
    pub(crate) fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SettingsError::Io(e.to_string()))?;
        }
        let text = self.to_toml()?;
        std::fs::write(path, text).map_err(|e| SettingsError::Io(e.to_string()))
    }

    /// Render the workspace settings as TOML.
    pub(crate) fn to_toml(&self) -> Result<String, SettingsError> {
        toml::to_string_pretty(self).map_err(|e| SettingsError::Parse(e.to_string()))
    }

    /// Refuse a file whose selection is half made.
    ///
    /// A provider with no model, or a model with no provider, is a selection
    /// that cannot resolve; it is refused rather than treated as "none", so a
    /// typo never silently starts a session on the wrong endpoint.
    pub(crate) fn validate(&self) -> Result<(), SettingsError> {
        let provider = self.general.provider.trim().is_empty();
        let model = self.general.model.trim().is_empty();
        if provider != model {
            return Err(SettingsError::HalfMadeSelection(format!(
                "{}/{}",
                self.general.provider, self.general.model
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKED: &str = r#"
schema_version = 1

[general]
provider = "nvidia"
model = "glm-5.3-flash"

[agent]
temperature = 0.0
permission = "read-only"

[disabled]
extentions = ["plan"]
"#;

    #[test]
    fn a_zero_temperature_is_a_value_not_a_sentinel() {
        let settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        assert_eq!(
            settings.agent.temperature,
            Some(0.0),
            "0.0 now means greedy decoding, which is why the field is an Option"
        );
    }

    #[test]
    fn an_absent_field_inherits_rather_than_defaulting() {
        let settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        assert_eq!(
            settings.agent.reasoning_effort, None,
            "absent inherits; it is not an empty value"
        );
        assert_eq!(settings.agent.skills_dir, None);
        assert_eq!(settings.agent.memory_path, None);
        assert!(
            settings.agent.mcp_servers.is_empty(),
            "a workspace adds to the global list, it does not replace it"
        );
    }

    #[test]
    fn the_workspace_names_a_model_it_does_not_define() {
        let settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        let reference = settings.general.model_ref().expect("a selection");
        assert_eq!(reference.provider, "nvidia");
        assert_eq!(reference.model, "glm-5.3-flash");
    }

    #[test]
    fn a_half_made_selection_is_refused() {
        let text = "[general]\nprovider = \"nvidia\"\n";
        assert_eq!(
            WorkspaceSettings::from_toml(text),
            Err(SettingsError::HalfMadeSelection("nvidia/".to_string())),
            "a provider without a model cannot resolve"
        );
    }

    #[test]
    fn no_selection_at_all_is_legal() {
        let settings = WorkspaceSettings::from_toml("").expect("a fresh workspace parses");
        assert_eq!(settings.general.model_ref(), None);
        assert_eq!(
            settings.agent.permission, "workspace-write",
            "the confinement default survives an empty file"
        );
    }

    #[test]
    fn the_workspace_file_round_trips() {
        let settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        let text = settings.to_toml().expect("renders");
        assert_eq!(
            WorkspaceSettings::from_toml(&text).expect("re-parses"),
            settings
        );
    }

    /// A scratch directory no other test shares.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-settings-tests")
            .join(format!("{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn the_workspace_path_sits_under_the_workspace_it_configures() {
        assert_eq!(
            WorkspaceSettings::path(std::path::Path::new("/tmp/proj")),
            std::path::PathBuf::from("/tmp/proj/.bsh/settings/settings.toml"),
            "per workspace by construction, so the file travels with the checkout"
        );
    }

    #[test]
    fn an_absent_workspace_file_is_a_fresh_workspace() {
        let path = scratch("absent").join("settings.toml");
        let loaded = WorkspaceSettings::load(&path).expect("absence is not an error");
        assert_eq!(loaded, WorkspaceSettings::default());
        assert!(loaded.general.provider.is_empty(), "it selects nothing");
        assert!(!path.exists(), "reading must not create the file it read");
    }

    #[test]
    fn the_workspace_file_round_trips_through_the_disk() {
        let original = WorkspaceSettings::from_toml(WORKED).expect("the fixture parses");
        let path = WorkspaceSettings::path(&scratch("ws-root"));
        original.save(&path).expect("saves, creating the directory");
        assert!(path.exists(), "save must create its directory");
        let reloaded = WorkspaceSettings::load(&path).expect("loads");
        assert_eq!(
            reloaded, original,
            "a save/load round trip must not lose a field"
        );
    }

    #[test]
    fn the_rendered_workspace_toml_reparses_to_the_same_value() {
        let original = WorkspaceSettings::from_toml(WORKED).expect("the fixture parses");
        let text = original.to_toml().expect("renders");
        let reparsed =
            WorkspaceSettings::from_toml(&text).expect("its own output must be valid input");
        assert_eq!(reparsed, original);
    }
}
