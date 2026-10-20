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
    /// The workspace's own facts: the session it last had open.
    pub(crate) general: WorkspaceGeneral,
    /// How the agent is configured here.
    pub(crate) agent: AgentSection,
    /// What is switched off here.
    pub(crate) disabled: Disabled,
}

/// The workspace's own facts.
///
/// Neither a model nor a session identity belongs here: a model is chosen per
/// session, and which session is on screen is the window's state (several
/// sessions can run at once). What stays here is the workspace's agent
/// configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceGeneral {
    /// A provider id an older build wrote here, when the model was a workspace
    /// setting rather than a session one.
    ///
    /// Deserialize-only, the same shape as `Provider::legacy_api_key`: a file
    /// that still carries it parses, it is read once to seed the session that
    /// was open, and nothing ever writes it again.
    #[serde(default, rename = "provider", skip_serializing)]
    pub(crate) legacy_provider: String,
    /// The model nickname that accompanied [`Self::legacy_provider`].
    #[serde(default, rename = "model", skip_serializing)]
    pub(crate) legacy_model: String,
    /// The session last open in this workspace, as an older build recorded it.
    ///
    /// Deserialize-only like the model pair above. Which session is on screen
    /// is a WINDOW fact - several sessions can run at once - so it lives on
    /// the global preferences now, and this field only carries an old file's
    /// answer into that preference once.
    #[serde(default, rename = "active_session", skip_serializing)]
    pub(crate) legacy_active_session: String,
}

impl WorkspaceGeneral {
    /// The pair an older file wrote, when it wrote both halves.
    ///
    /// A half-made pair is no selection: it could not resolve, so it is not
    /// migrated onto a session that would then report an unknown name.
    pub(crate) fn legacy_pair(&self) -> Option<ModelRef> {
        let provider = self.legacy_provider.trim();
        let model = self.legacy_model.trim();
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some(ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
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
    pub(crate) extensions: Vec<String>,
}

impl WorkspaceSettings {
    /// Parse the workspace settings from TOML text.
    pub(crate) fn from_toml(text: &str) -> Result<Self, SettingsError> {
        toml::from_str(text).map_err(|e| SettingsError::Parse(e.to_string()))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKED: &str = r#"
schema_version = 1

[general]
provider = "nvidia"
model = "glm-5.3-flash"
active_session = "chat-1"

[agent]
temperature = 0.0
permission = "read-only"

[disabled]
extensions = ["plan"]
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

    /// The pair an older file wrote is READ - it is the one fact a migration
    /// needs - but nothing ever writes it again.
    #[test]
    fn a_legacy_selection_is_read_but_never_written() {
        let settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        let reference = settings.general.legacy_pair().expect("the old pair");
        assert_eq!(reference.provider, "nvidia");
        assert_eq!(reference.model, "glm-5.3-flash");
        assert_eq!(
            settings.general.legacy_active_session, "chat-1",
            "the session it remembered is read too"
        );

        let text = settings.to_toml().expect("renders");
        assert!(
            !text.contains("provider ="),
            "a workspace no longer names a provider: {text}"
        );
        assert!(
            !text.contains("model ="),
            "a workspace no longer names a model: {text}"
        );
        assert!(
            !text.contains("active_session ="),
            "a workspace no longer names the session on screen: {text}"
        );
    }

    /// A half-made legacy pair could not resolve, so it is not migrated.
    #[test]
    fn a_half_made_legacy_selection_is_not_a_choice() {
        let text = "[general]\nprovider = \"nvidia\"\n";
        let settings = WorkspaceSettings::from_toml(text).expect("an old file still parses");
        assert_eq!(settings.general.legacy_pair(), None);
    }

    #[test]
    fn no_selection_at_all_is_legal() {
        let settings = WorkspaceSettings::from_toml("").expect("a fresh workspace parses");
        assert_eq!(settings.general.legacy_pair(), None);
        assert_eq!(
            settings.agent.permission, "workspace-write",
            "the confinement default survives an empty file"
        );
    }

    /// The written document no longer carries the pair, so what comes back is
    /// the file WITHOUT the legacy input: a second save is the same document.
    #[test]
    fn the_workspace_file_round_trips_without_the_legacy_pair() {
        let mut settings = WorkspaceSettings::from_toml(WORKED).expect("parses");
        settings.general.legacy_provider.clear();
        settings.general.legacy_model.clear();
        settings.general.legacy_active_session.clear();
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
        assert!(
            loaded.general.legacy_provider.is_empty(),
            "it selects nothing"
        );
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
            reloaded.agent, original.agent,
            "a save/load round trip must not lose an agent field"
        );
        assert_eq!(
            reloaded.general.legacy_pair(),
            None,
            "the legacy pair is not written, so it does not come back"
        );
        assert_eq!(
            reloaded.general.legacy_active_session, "",
            "and neither does the retired session id"
        );
    }

    #[test]
    fn the_rendered_workspace_toml_reparses_to_the_same_value() {
        let original = WorkspaceSettings::from_toml(WORKED).expect("the fixture parses");
        let text = original.to_toml().expect("renders");
        let reparsed =
            WorkspaceSettings::from_toml(&text).expect("its own output must be valid input");
        assert_eq!(reparsed.agent, original.agent);
        assert_eq!(reparsed.disabled, original.disabled);
        assert_eq!(reparsed.general.legacy_pair(), None);
    }
}
