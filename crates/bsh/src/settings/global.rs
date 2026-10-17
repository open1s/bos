//! The global settings file: everything that crosses workspaces.
//!
//! `~/.bos/bsh/settings.toml` holds exactly the information that is the same
//! whichever workspace is open, or that belongs to no workspace at all:
//! machine preferences, the provider tree, preset definitions, the extension
//! inventory and the workspace registry. See `docs/bsh-settings-spec.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One automation task: an instruction that runs in a session on a cadence.
///
/// The reference keeps a task beside the session it was created in and lists
/// it on its Automation tasks panel, which is the `session_id` link. The
/// cadence here is the "every N" family bsh supports, with
/// `every_seconds == 0` meaning the one-shot `once` rule. The reference also
/// offers daily, weekdays, weekly and cron rules, which need date and
/// time-zone arithmetic bsh does not do yet, so they are not drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Schedule {
    /// Stable identifier, unique within the settings file.
    pub(crate) id: String,
    /// Task name, bounded by [`GlobalSettings::NAME_LIMIT`].
    pub(crate) name: String,
    /// The instruction the task sends to its session.
    pub(crate) prompt: String,
    /// The session the task runs in.
    #[serde(default)]
    pub(crate) session_id: String,
    /// Seconds between runs; zero means the task runs once.
    #[serde(default)]
    pub(crate) every_seconds: u64,
    /// Whether the task still triggers.
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    /// Unix seconds of the last run; zero when it has never run.
    #[serde(default)]
    pub(crate) last_run: u64,
}

/// A boolean that is on when unset, for task fields that default to enabled.
fn default_true() -> bool {
    true
}

/// Why a task could not be created, under the reference own refusals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduleRefusal {
    /// The name is empty or longer than [`GlobalSettings::NAME_LIMIT`].
    Name,
    /// The instruction is empty.
    Prompt,
}

impl GlobalSettings {
    /// The longest task name the reference accepts.
    pub(crate) const NAME_LIMIT: usize = 120;

    /// Append an automation task, answering with its id.
    pub(crate) fn add_schedule(
        &mut self,
        name: &str,
        prompt: &str,
        session_id: &str,
        every_seconds: u64,
    ) -> Result<String, ScheduleRefusal> {
        let name = name.trim();
        let prompt = prompt.trim();
        if name.is_empty() || name.chars().count() > Self::NAME_LIMIT {
            return Err(ScheduleRefusal::Name);
        }
        if prompt.is_empty() {
            return Err(ScheduleRefusal::Prompt);
        }
        let id = format!("task-{}", unix_now());
        self.schedules.push(Schedule {
            id: id.clone(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            session_id: session_id.to_string(),
            every_seconds,
            enabled: true,
            last_run: 0,
        });
        Ok(id)
    }

    /// Drop a task; false when no task carries the id.
    pub(crate) fn remove_schedule(&mut self, id: &str) -> bool {
        let before = self.schedules.len();
        self.schedules.retain(|entry| entry.id != id);
        before != self.schedules.len()
    }

    /// Flip a task between enabled and inactive; false when the id is unknown.
    pub(crate) fn toggle_schedule(&mut self, id: &str) -> bool {
        let Some(entry) = self.schedules.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        entry.enabled = !entry.enabled;
        true
    }

    /// Whether a task triggers at `now`.
    pub(crate) fn is_due(&self, schedule: &Schedule, now: u64) -> bool {
        if !schedule.enabled {
            return false;
        }
        if schedule.last_run == 0 {
            return true;
        }
        schedule.every_seconds != 0
            && now.saturating_sub(schedule.last_run) >= schedule.every_seconds
    }

    /// The tasks due at `now`, in file order.
    pub(crate) fn due_schedules(&self, now: u64) -> Vec<String> {
        self.schedules
            .iter()
            .filter(|entry| self.is_due(entry, now))
            .map(|entry| entry.id.clone())
            .collect()
    }

    /// When a task runs next: `now` while it is due, `None` when it is off or
    /// its single run has already happened.
    pub(crate) fn next_run(&self, schedule: &Schedule, now: u64) -> Option<u64> {
        if !schedule.enabled {
            return None;
        }
        if schedule.last_run == 0 {
            return Some(now);
        }
        if schedule.every_seconds == 0 {
            return None;
        }
        Some(schedule.last_run + schedule.every_seconds)
    }

    /// Record that a task ran at `now`. A one-shot task turns itself off, so a
    /// restart does not run it a second time.
    pub(crate) fn mark_schedule_ran(&mut self, id: &str, now: u64) {
        if let Some(entry) = self.schedules.iter_mut().find(|entry| entry.id == id) {
            entry.last_run = now;
            if entry.every_seconds == 0 {
                entry.enabled = false;
            }
        }
    }
}

/// Unix seconds, or zero if the clock is before the epoch.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Why a settings file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsError {
    /// The file could not be read.
    Io(String),
    /// The file did not parse.
    Parse(String),
    /// A model leaf carried no model id.
    EmptyModel(String),
    /// A provider carried no base URL.
    EmptyBaseUrl(String),
    /// An extension entry carried no id.
    EmptyExtention,
    /// Two extension entries carried the same id.
    DuplicateExtention(String),
    /// A workspace named a provider without a model, or a model without a
    /// provider.
    HalfMadeSelection(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(why) => write!(f, "cannot read the settings file: {why}"),
            Self::Parse(why) => write!(f, "cannot parse the settings file: {why}"),
            Self::EmptyModel(id) => write!(f, "the model {id:?} carries no model id"),
            Self::EmptyBaseUrl(id) => write!(f, "the provider {id:?} carries no base URL"),
            Self::EmptyExtention => write!(f, "an extension entry carries no id"),
            Self::DuplicateExtention(id) => {
                write!(f, "the extension {id:?} is installed twice")
            }
            Self::HalfMadeSelection(id) => {
                write!(f, "the selection {id:?} names only half a model")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

/// A reference to one model in the provider tree.
///
/// The session and the workspace both name a model this way. It is a pointer,
/// never a copy: no base URL or API key travels with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelRef {
    /// The provider id, as it appears in the tree.
    pub(crate) provider: String,
    /// The model's nickname under that provider.
    pub(crate) model: String,
}

/// The global settings file, as `~/.bos/bsh/settings.toml` holds them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobalSettings {
    /// Format version; each file versions independently.
    pub(crate) schema_version: u32,
    /// Machine preferences: the app's appearance and behaviour.
    pub(crate) general: General,
    /// The skill folder every workspace inherits unless it sets its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skills_dir: Option<String>,
    /// MCP servers every workspace gets, merged with each workspace's own.
    pub(crate) mcp_servers: Vec<serde_json::Value>,
    /// The provider tree, keyed by a concrete provider id.
    pub(crate) providers: BTreeMap<String, Provider>,
    /// Agent preset declarations, keyed by preset id. A declaration with the
    /// same id as a shipped preset replaces it outright.
    pub(crate) presets: BTreeMap<String, Preset>,
    /// The installed extension inventory. Order is the reader's, and the ids
    /// are checked for repetition because an array of tables does not make
    /// them unique the way a map key would.
    pub(crate) extentions: Vec<Extension>,
    /// The automation tasks: a machine-level instruction and the session it
    /// runs in, on a cadence. A task names a session rather than belonging to
    /// one workspace, so the list is global.
    #[serde(default)]
    pub(crate) schedules: Vec<super::Schedule>,
    /// The workspace registry.
    pub(crate) workspaces: Vec<WorkspaceRef>,
    /// The preset a session takes when it names none. The reference keeps this
    /// as the registry's selectedDefault, resolved over the deployment default.
    #[serde(default = "default_preset")]
    pub(crate) default_preset: String,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            schema_version: 0,
            general: General::default(),
            skills_dir: None,
            schedules: Vec::new(),
            mcp_servers: Vec::new(),
            providers: BTreeMap::new(),
            presets: BTreeMap::new(),
            extentions: Vec::new(),
            workspaces: Vec::new(),
            default_preset: default_preset(),
        }
    }
}

/// The preset a fresh install selects.
fn default_preset() -> String {
    "standard".to_string()
}

/// Machine preferences. None of these follow a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct General {
    /// The palette mode: "light", "dark" or "system".
    pub(crate) theme_mode: String,
    /// The interface language.
    pub(crate) lang: String,
    /// The conversation content size in px.
    pub(crate) content_font_size: u32,
    /// The content width a reader dragged out, or none while the column
    /// follows the reference's own clamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content_width: Option<f32>,
    /// The sidebar width a reader dragged out, or none while the column takes
    /// the reference's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sidebar_width: Option<f32>,
    /// How much of a turn the conversation renders.
    pub(crate) transcript_view: String,
    /// Where a link in a message opens.
    pub(crate) link_opening: String,
    /// What Enter does while a turn streams.
    pub(crate) composer_enter: String,
    /// How much performance and usage detail to show.
    pub(crate) performance_usage: String,
    /// Show the coding view: the trajectory tab and the Agent preset switch.
    pub(crate) developer_tools: bool,
    /// The session-list ordering a reader chose.
    pub(crate) session_sort: String,
    /// The workspace that was open; empty while none is registered.
    pub(crate) active_workspace: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            theme_mode: "system".to_string(),
            lang: "en".to_string(),
            content_font_size: 14,
            content_width: None,
            sidebar_width: None,
            transcript_view: super::DEFAULT_TRANSCRIPT_VIEW.to_string(),
            link_opening: "sidebar".to_string(),
            composer_enter: "queue".to_string(),
            performance_usage: "detailed".to_string(),
            developer_tools: true,
            session_sort: String::new(),
            active_workspace: String::new(),
        }
    }
}

/// Whether a bool is false, for `skip_serializing_if`.
fn is_false(value: &bool) -> bool {
    !*value
}

/// One provider: one connection, and the models reached through it.
///
/// The connection lives here rather than on each model so that a gateway is
/// declared once. Two models on one gateway share its base URL and API key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Provider {
    /// Display name. Absent shows the provider id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// The API base URL.
    pub(crate) base_url: String,
    /// The API key.
    pub(crate) api_key: String,
    /// Whether the user declared this connection by hand instead of picking
    /// the provider from the shipped catalog. Written only when true.
    #[serde(skip_serializing_if = "is_false")]
    pub(crate) declared: bool,
    /// The wire protocol a new model of this family speaks, written only when
    /// the pane changed it. A shipped family carries its own here, so the
    /// family choice survives without a leaf to hold it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_mode: Option<String>,
    /// The models this provider offers, keyed by nickname.
    ///
    /// Flattened: a model sits directly under its provider, so the file reads
    /// `[providers.nvidia."glm-5.3-flash"]` rather than requiring a `models`
    /// table between them. A misspelled provider field is therefore collected
    /// here and then refused for carrying no model id, rather than ignored.
    #[serde(flatten)]
    pub(crate) models: BTreeMap<String, ModelEntry>,
}

/// One model under a provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelEntry {
    /// The model id the API is called with. Required.
    pub(crate) model: String,
    /// Display name. Absent shows the model id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Output cap for this model. Absent leaves it to the engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    /// The wire protocol; absent uses the engine's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) api_mode: Option<String>,
    /// Reasoning effort; absent uses the engine's own default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    /// Declared input types. Absent means undeclared, which reads as
    /// permissive, so a model written before this field keeps sending images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_types: Option<Vec<String>>,
}

/// One agent preset: the plugin composition a session runs under.
///
/// Measured from the reference (`@deepseek-ai/dsh-agent-preset`): the
/// declaration's fields are exactly `id`, `plugins`, `name`,
/// `description` and `order`. There is deliberately **no model, no system
/// prompt and no step cap** - the mounted plugins own the model-visible tools
/// and prompt sections, and the model is session and workspace state. A shipped
/// preset publishes no `name`: the dictionaries carry its copy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preset {
    /// The plugins this preset mounts. The *complete* list, not a delta: an
    /// empty list mounts nothing, and a declaration replaces the shipped list
    /// rather than merging with it.
    pub(crate) plugins: Vec<String>,
    /// Display name. Unset on a shipped preset, whose copy is in the locale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// Display description. Unset on a shipped preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// Roster order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) order: Option<i64>,
}

/// The presets the binary itself ships, as (id, mounted plugins).
///
/// The reference ships four - `standard`, `ptc`, `minimal` and
/// `cordis`. Only the two with backing here are shipped: `ptc` mounts
/// against a host-plane PTC runtime this deployment composes nowhere, and
/// `cordis` is a Cordis plugin composition with no counterpart, so shipping
/// either would be a name with nothing behind it.
const BUILT_IN_PRESETS: &[(&str, &[&str])] =
    &[("standard", &["shell", "files", "plan"]), ("minimal", &[])];

/// The providers the binary itself knows how to speak to, as
/// (id, display name, default base URL, default protocol).
///
/// The reference ships a provider directory and offers its entries in the add
/// card's first mode; this is the honest counterpart here, because these five
/// are the only vendors `crates/react` can build a client for. A provider that
/// is not on this list is not refused - it is what the card's custom mode is
/// for, where the endpoint and the protocol are typed instead of looked up.
///
/// The protocol is the value that reaches `AgentConfig::api_mode`: the first
/// three vendors speak their own wire format and ignore it, while `openai`
/// dispatches on it, so the default recorded here is the one its own transport
/// implements.
pub(crate) const BUILT_IN_PROVIDERS: &[(&str, &str, &str, &str)] = &[
    (
        "deepseek",
        "DeepSeek",
        "https://api.deepseek.com",
        "responses",
    ),
    ("openai", "OpenAI", "https://api.openai.com/v1", "chat"),
    (
        "anthropic",
        "Anthropic",
        "https://api.anthropic.com",
        "anthropic",
    ),
    (
        "nvidia",
        "NVIDIA",
        "https://integrate.api.nvidia.com/v1",
        "chat",
    ),
    (
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        "chat",
    ),
];

/// The shipped provider matching an id, as (display name, base URL, protocol).
///
/// The id is the catalog's own key and the prefix the engine's router is
/// registered under, so the id a user picks is the id the wire model carries.
pub(crate) fn built_in(id: &str) -> Option<(&'static str, &'static str, &'static str)> {
    BUILT_IN_PROVIDERS
        .iter()
        .find(|(candidate, ..)| *candidate == id)
        .map(|(_, name, base_url, protocol)| (*name, *base_url, *protocol))
}

/// Whether a preset id is one the build ships.
///
/// A declaration with the same id replaces the shipped row rather than adding
/// to it, so a pane asks this to tell a restored default from a user's own.
pub(crate) fn ships_preset(id: &str) -> bool {
    BUILT_IN_PRESETS.iter().any(|(name, _)| *name == id)
}

/// One entry in the installed extension inventory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Extension {
    /// The extension id. It must name something the build ships.
    pub(crate) id: String,
}

/// One workspace in the registry.
///
/// The registry holds the identity and the root; everything about how the
/// agent runs there lives in the workspace's own settings file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceRef {
    /// Stable id, also the key sessions are tagged with.
    pub(crate) id: String,
    /// Display name.
    pub(crate) name: String,
    /// Filesystem root the tools are confined to.
    pub(crate) root: String,
    /// Unix seconds created.
    pub(crate) created_at: u64,
    /// Unix seconds last changed.
    pub(crate) updated_at: u64,
}

impl GlobalSettings {
    /// Parse the global settings from TOML text.
    pub(crate) fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let settings: Self =
            toml::from_str(text).map_err(|e| SettingsError::Parse(e.to_string()))?;
        settings.validate()?;
        Ok(settings)
    }

    /// The global settings file inside the harness data directory.
    pub(crate) fn path() -> PathBuf {
        super::dir().join("settings.toml")
    }

    /// Read the global settings file.
    ///
    /// An absent file is a fresh install, not an error: nothing is shipped, so
    /// the defaults ARE the configuration until something writes a file. Any
    /// other read failure is reported, because then the file exists and asking
    /// the reader to guess which one it is would be the wrong move.
    pub(crate) fn load(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::Io(e.to_string())),
        }
    }

    /// Write the global settings file, creating its directory.
    pub(crate) fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SettingsError::Io(e.to_string()))?;
        }
        let text = self.to_toml()?;
        std::fs::write(path, text).map_err(|e| SettingsError::Io(e.to_string()))
    }

    /// Render the global settings as TOML.
    pub(crate) fn to_toml(&self) -> Result<String, SettingsError> {
        toml::to_string_pretty(self).map_err(|e| SettingsError::Parse(e.to_string()))
    }

    /// Refuse a file whose references cannot be honoured.
    ///
    /// Duplicate provider, preset and extension ids cannot reach here: they are
    /// map keys, and a repeated key is a TOML parse error. What is left is a
    /// leaf with no model id, a provider with no base URL, and an extension id
    /// that is blank or repeated.
    pub(crate) fn validate(&self) -> Result<(), SettingsError> {
        for (id, provider) in &self.providers {
            if provider.base_url.trim().is_empty() {
                return Err(SettingsError::EmptyBaseUrl(id.clone()));
            }
            for (nickname, entry) in &provider.models {
                if entry.model.trim().is_empty() {
                    return Err(SettingsError::EmptyModel(format!("{id}/{nickname}")));
                }
            }
        }
        let mut installed = std::collections::BTreeSet::new();
        for extension in &self.extentions {
            if extension.id.trim().is_empty() {
                return Err(SettingsError::EmptyExtention);
            }
            if !installed.insert(extension.id.clone()) {
                return Err(SettingsError::DuplicateExtention(extension.id.clone()));
            }
        }
        Ok(())
    }

    /// The preset an id names, or none while nothing defines it.
    ///
    /// A declaration wins outright: the reference's own rule is that a user
    /// override replaces the complete child list and does not merge with it.
    pub(crate) fn preset(&self, id: &str) -> Option<Preset> {
        if let Some(declared) = self.presets.get(id) {
            return Some(declared.clone());
        }
        BUILT_IN_PRESETS
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, plugins)| Preset {
                plugins: plugins.iter().map(|p| (*p).to_string()).collect(),
                ..Preset::default()
            })
    }

    /// Every preset id on the roster, shipped ones included, in display order.
    pub(crate) fn preset_ids(&self) -> Vec<String> {
        let mut rows: Vec<(i64, String)> = self
            .presets
            .iter()
            .map(|(id, preset)| (preset.order.unwrap_or(0), id.clone()))
            .collect();
        for (id, _) in BUILT_IN_PRESETS {
            if !self.presets.contains_key(*id) {
                rows.push((0, (*id).to_string()));
            }
        }
        rows.sort();
        rows.into_iter().map(|(_, id)| id).collect()
    }

    /// The provider a reference names, with the model entry it points at.
    ///
    /// Resolves to none when either half is missing, which is the state a
    /// session reports as unresolvable rather than silently replacing.
    pub(crate) fn lookup(&self, reference: &ModelRef) -> Option<(&Provider, &ModelEntry)> {
        let provider = self.providers.get(&reference.provider)?;
        let entry = provider.models.get(&reference.model)?;
        Some((provider, entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOTED: &str = r#"
schema_version = 1

[general]
theme_mode = "dark"
lang = "zh"

[providers.nvidia]
base_url = "http://127.0.0.1:11436/v1"
api_key  = "sk-local"

[providers.nvidia."glm-5.3-flash"]
model = "nvidia/z-ai/glm-5.3-flash"
api_mode = "chat"
input_types = ["text", "image"]

[providers.nvidia."kimi-k3"]
model = "nvidia/moonshotai/kimi-k3"

[[extentions]]
id = "shell"

[[workspaces]]
id = "w1"
name = "bsh"
root = "/tmp/bsh"
"#;

    #[test]
    fn a_provider_owns_one_connection_and_many_models() {
        let settings = GlobalSettings::from_toml(ROOTED).expect("parses");
        let provider = settings.providers.get("nvidia").expect("the provider");
        assert_eq!(provider.models.len(), 2, "two models, one connection");
        assert_eq!(provider.base_url, "http://127.0.0.1:11436/v1");
        assert_eq!(provider.api_key, "sk-local");
    }

    #[test]
    fn a_quoted_leaf_key_holds_a_dotted_nickname() {
        let settings = GlobalSettings::from_toml(ROOTED).expect("parses");
        let entry = settings
            .providers
            .get("nvidia")
            .and_then(|p| p.models.get("glm-5.3-flash"))
            .expect("the dotted nickname survives quoting");
        assert_eq!(entry.model, "nvidia/z-ai/glm-5.3-flash");
        assert_eq!(entry.api_mode.as_deref(), Some("chat"));
    }

    #[test]
    fn a_model_without_input_types_is_undeclared_not_empty() {
        let settings = GlobalSettings::from_toml(ROOTED).expect("parses");
        let entry = settings
            .providers
            .get("nvidia")
            .and_then(|p| p.models.get("kimi-k3"))
            .expect("the second model");
        assert_eq!(
            entry.input_types, None,
            "undeclared reads as permissive; an empty list would read as text-only"
        );
    }

    #[test]
    fn a_root_key_written_after_a_table_header_is_refused() {
        let text = "[general]\nlang = \"zh\"\nmcp_servers = []\n";
        assert!(
            matches!(
                GlobalSettings::from_toml(text),
                Err(SettingsError::Parse(_))
            ),
            "without this refusal the key vanishes into [general] and the list \
             silently reads as empty, which is exactly how a hand-written file \
             loses its MCP servers"
        );
    }

    #[test]
    fn a_repeated_provider_id_is_a_parse_error() {
        let text = "[providers.nvidia]\nbase_url = \"http://a\"\n\n[providers.nvidia]\nbase_url = \"http://b\"\n";
        assert!(
            matches!(
                GlobalSettings::from_toml(text),
                Err(SettingsError::Parse(_))
            ),
            "one node per provider id"
        );
    }

    #[test]
    fn a_leaf_without_a_model_id_is_refused() {
        let text = "[providers.nvidia]\nbase_url = \"http://a\"\n\n[providers.nvidia.glm]\napi_mode = \"chat\"\n";
        assert_eq!(
            GlobalSettings::from_toml(text),
            Err(SettingsError::EmptyModel("nvidia/glm".to_string())),
            "the nickname is display, the model id is the payload"
        );
    }

    #[test]
    fn a_provider_without_a_base_url_is_refused() {
        let text = "[providers.nvidia]\napi_key = \"k\"\n";
        assert_eq!(
            GlobalSettings::from_toml(text),
            Err(SettingsError::EmptyBaseUrl("nvidia".to_string()))
        );
    }

    #[test]
    fn an_empty_file_is_the_fresh_install() {
        let settings = GlobalSettings::from_toml("").expect("an empty file is legal");
        assert!(
            settings.providers.is_empty(),
            "a fresh install ships nothing"
        );
        assert!(settings.workspaces.is_empty(), "and registers no workspace");
        assert_eq!(
            settings.general.theme_mode, "system",
            "the machine preferences still carry their defaults"
        );
    }

    #[test]
    fn a_reference_that_no_longer_resolves_is_none() {
        let settings = GlobalSettings::from_toml(ROOTED).expect("parses");
        let live = ModelRef {
            provider: "nvidia".to_string(),
            model: "kimi-k3".to_string(),
        };
        assert!(settings.lookup(&live).is_some());
        let gone = ModelRef {
            provider: "nvidia".to_string(),
            model: "retired".to_string(),
        };
        assert!(
            settings.lookup(&gone).is_none(),
            "an unresolvable session must say so, never fall back"
        );
        let no_provider = ModelRef {
            provider: "openrouter".to_string(),
            model: "kimi-k3".to_string(),
        };
        assert!(settings.lookup(&no_provider).is_none());
    }

    #[test]
    fn the_global_file_round_trips() {
        let settings = GlobalSettings::from_toml(ROOTED).expect("parses");
        let text = settings.to_toml().expect("renders");
        assert_eq!(
            GlobalSettings::from_toml(&text).expect("re-parses"),
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

    /// One of every surface the global file carries, so a dropped field shows.
    const RICH: &str = r#"
schema_version = 1
skills_dir = "~/.bos/skills"
mcp_servers = []
default_preset = "tight"

[general]
theme_mode = "dark"
lang = "zh"
content_font_size = 16
content_width = 900
sidebar_width = 300
transcript_view = "compact"
link_opening = "new-tab"
composer_enter = "steer"
performance_usage = "compact"
developer_tools = false
session_sort = "name"
active_workspace = "w1"

[providers.nvidia]
base_url = "https://integrate.api.nvidia.com/v1"
api_key = "nvapi-x"

  [providers.nvidia."glm-5.3-flash"]
  model = "zai-org/glm-5.3-flash"
  api_mode = "chat"
  reasoning_effort = "high"
  input_types = ["text", "image"]

[presets.tight]
plugins = ["shell"]
name = "Tight"
description = "as small as it gets"
order = 3

[[extentions]]
id = "shell"

[[workspaces]]
id = "w1"
name = "bsh"
root = "/tmp/bsh"
created_at = 1
updated_at = 2
"#;

    #[test]
    fn an_absent_global_file_is_a_fresh_install() {
        let path = scratch("absent").join("settings.toml");
        let loaded = GlobalSettings::load(&path).expect("absence is not an error");
        assert_eq!(loaded, GlobalSettings::default());
        assert!(
            loaded.providers.is_empty(),
            "a fresh install ships no endpoint, so the first launch must not invent one"
        );
        assert_eq!(
            loaded.default_preset, "standard",
            "but it does ship a preset default, so a session always has a composition"
        );
        assert!(!path.exists(), "reading must not create the file it read");
    }

    #[test]
    fn the_global_path_is_the_settings_file_in_the_data_directory() {
        assert_eq!(
            GlobalSettings::path(),
            crate::settings::dir().join("settings.toml")
        );
    }

    #[test]
    fn the_global_file_round_trips_through_the_disk() {
        let original = GlobalSettings::from_toml(RICH).expect("the fixture parses");
        let path = scratch("round-trip").join("nested").join("settings.toml");
        original.save(&path).expect("saves, creating the directory");
        assert!(path.exists(), "save must create its directory");
        let reloaded = GlobalSettings::load(&path).expect("loads");
        assert_eq!(
            reloaded, original,
            "a save/load round trip must not lose a field"
        );
    }

    #[test]
    fn the_rendered_global_toml_reparses_to_the_same_value() {
        let original = GlobalSettings::from_toml(RICH).expect("the fixture parses");
        let text = original.to_toml().expect("renders");
        let reparsed =
            GlobalSettings::from_toml(&text).expect("its own output must be valid input");
        assert_eq!(
            reparsed, original,
            "a field that renders under one name and parses under another would vanish"
        );
    }
}
