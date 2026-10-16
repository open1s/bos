//! Persisted harness settings and per-workspace overrides.
//!
//! Loaded from ~/.bos/bsh/settings.toml; when that file does not exist the
//! model defaults are discovered from the BOS config (~/.bos/conf/config.toml).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A named alternate endpoint discovered from the BOS config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Profile {
    /// Profile name as used in [llm.<name>].
    pub(crate) name: String,
    /// Model identifier.
    pub(crate) model: String,
    /// API base URL.
    pub(crate) base_url: String,
    /// API key.
    pub(crate) api_key: String,
}

/// One configured workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Workspace {
    /// Stable id.
    pub(crate) id: String,
    /// Display name.
    pub(crate) name: String,
    /// Filesystem root the tools are confined to.
    pub(crate) root: String,
    /// Model override; empty inherits the global model.
    pub(crate) model: String,
    /// Base URL override.
    pub(crate) base_url: String,
    /// API key override.
    pub(crate) api_key: String,
    /// System prompt override.
    pub(crate) system_prompt: String,
    /// Temperature override; 0 inherits.
    pub(crate) temperature: f32,
    /// Reasoning effort override.
    pub(crate) reasoning_effort: Option<String>,
    /// Unix seconds created.
    pub(crate) created_at: u64,
    /// Unix seconds last changed.
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
            temperature: 0.0,
            reasoning_effort: None,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// One provider's customized model catalog.
///
/// The reference keeps this list inside its provider editor: the model ids a
/// user added by hand, which is what the pane offers to switch the active model
/// between. It is keyed by provider name, and an empty key is the plain
/// endpoint rather than a named provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ModelCatalog {
    /// The provider the list belongs to; empty is the plain endpoint.
    pub(crate) provider: String,
    /// The model ids the user added, in the order they were added.
    pub(crate) models: Vec<String>,
    /// Declared input types per model id ("text", "image").
    ///
    /// Absent means undeclared, which bsh reads as permissive, so a catalog
    /// written before this field keeps sending images.
    #[serde(default)]
    pub(crate) input_types: BTreeMap<String, Vec<String>>,
}

/// Why a model id was refused by a provider's catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogError {
    /// The typed id was blank.
    Empty,
    /// The catalog already holds the id.
    Duplicate,
}

/// Why the provider form refused to add a provider.
///
/// The reference refuses an incomplete custom provider instead of filling the
/// gaps in, so each variant maps to the sentence it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderRefusal {
    /// The display name was blank.
    NameEmpty,
    /// No base URL was entered.
    NeedsBaseUrl,
    /// No model id was entered.
    NeedsModel,
    /// A saved provider already carries the name.
    IdTaken,
}

impl ProviderRefusal {
    /// The locale key of the sentence this refusal draws.
    pub(crate) fn key(self) -> &'static str {
        match self {
            ProviderRefusal::NameEmpty => "settings.providerNameEmpty",
            ProviderRefusal::NeedsBaseUrl => "settings.providerNeedsBaseUrl",
            ProviderRefusal::NeedsModel => "settings.providerNeedsModel",
            ProviderRefusal::IdTaken => "settings.providerIdTaken",
        }
    }
}

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
    /// Task name, bounded by [`Settings::NAME_LIMIT`].
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
    /// The name is empty or longer than [`Settings::NAME_LIMIT`].
    Name,
    /// The instruction is empty.
    Prompt,
}

impl Settings {
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

/// The persisted harness settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// Format version.
    pub(crate) schema_version: u32,
    /// The palette mode: "light", "dark" or "system".
    ///
    /// The reference defaults to following the system preference.
    #[serde(default = "default_theme_mode")]
    pub(crate) theme_mode: String,
    /// The chat content width a reader dragged out, or none while the column
    /// follows the reference's own clamp. Measured: the reference keeps this as
    /// dsh.conversation.contentWidth in its client store and resolves it
    /// through resolveContentWidth; bsh's store is this file.
    #[serde(default)]
    pub(crate) content_width: Option<f32>,
    /// The sidebar width a reader dragged out, or none while the column takes
    /// the reference's own 280 default. Measured: setSidebar keeps
    /// layoutInfo.sidebar in the client store, clamped to 264..420.
    #[serde(default)]
    pub(crate) sidebar_width: Option<f32>,
    /// The conversation content size in px (the reference Font size setting).
    #[serde(default = "default_content_font_size")]
    pub(crate) content_font_size: u32,
    /// How much of a turn the conversation renders: "compact", "standard",
    /// "detailed" or "verbose" (the reference's Work details row).
    #[serde(default = "default_transcript_view")]
    pub(crate) transcript_view: String,
    /// Where a link in a message opens: "sidebar" or "new-tab".
    #[serde(default = "default_link_opening")]
    pub(crate) link_opening: String,
    /// What Enter does while a turn streams: "queue" or "steer".
    #[serde(default = "default_composer_enter")]
    pub(crate) composer_enter: String,
    /// How much performance and usage detail to show: "compact" or "detailed".
    #[serde(default = "default_performance_usage")]
    pub(crate) performance_usage: String,
    /// Show the coding view: the trajectory tab and the Agent preset switch.
    #[serde(default = "default_developer_tools")]
    pub(crate) developer_tools: bool,
    /// The interface language: "en" or "zh".
    #[serde(default = "default_lang")]
    pub(crate) lang: String,
    /// Default model identifier.
    pub(crate) model: String,
    /// Default API base URL.
    pub(crate) base_url: String,
    /// Default API key.
    pub(crate) api_key: String,
    /// Default system prompt.
    pub(crate) system_prompt: String,
    /// Default sampling temperature.
    pub(crate) temperature: f32,
    /// Reasoning effort for reasoning models.
    pub(crate) reasoning_effort: Option<String>,
    /// Optional completion cap.
    pub(crate) max_tokens: Option<u32>,
    /// ReAct steps one turn may take before the loop stops.
    #[serde(default = "default_max_steps")]
    pub(crate) max_steps: usize,
    /// Register the shell tool.
    pub(crate) bash_enabled: bool,
    /// Register the filesystem tools.
    pub(crate) file_tools_enabled: bool,
    /// Register the web tools.
    pub(crate) web_tools_enabled: bool,
    /// Require approval before side-effecting tools run.
    pub(crate) require_approval: bool,
    /// The default permission mode for new sessions, the reference's own
    /// trinary: "read-only", "workspace-write" or "danger-full-access".
    #[serde(default = "default_permission")]
    pub(crate) permission: String,
    /// Where skill folders live.
    pub(crate) skills_dir: String,
    /// Send-side token ceiling; 0 disables compaction.
    pub(crate) context_budget: usize,
    /// Seconds one shell command may run before it is killed.
    pub(crate) shell_timeout_secs: u64,
    /// Bytes of one tool result kept before the rest is truncated.
    pub(crate) tool_output_cap: usize,
    /// Attach the file-backed memory store.
    pub(crate) memory_enabled: bool,
    /// Memory file; empty uses the shared default.
    pub(crate) memory_path: String,
    /// MCP server entries.
    pub(crate) mcp_servers: Vec<serde_json::Value>,
    /// Provider entries.
    pub(crate) providers: Vec<Profile>,
    /// Configured workspaces.
    pub(crate) workspaces: Vec<Workspace>,
    /// Active workspace id.
    pub(crate) active_workspace: String,
    /// Provider the models pane selected; empty uses the plain endpoint.
    #[serde(default)]
    pub(crate) active_provider: String,
    /// Customized model catalogs, one per provider key.
    #[serde(default)]
    pub(crate) catalogs: Vec<ModelCatalog>,
    /// Automation tasks: instructions that run in a session on a cadence.
    #[serde(default)]
    pub(crate) schedules: Vec<Schedule>,
    /// The declarative presets found under the BSH directory.
    #[serde(default)]
    pub(crate) presets: Vec<agent::plugins::presets::Preset>,
    /// Preset the agent-presets pane selected; empty uses no preset.
    #[serde(default)]
    pub(crate) preset: String,
    /// Run one assistant turn's tool calls concurrently instead of one at a time.
    #[serde(default)]
    pub(crate) parallel_tool_calls: bool,

    /// The session reopened at launch; empty starts on the welcome screen.
    #[serde(default)]
    pub(crate) active_session: String,
    /// Tool names switched off by name.
    pub(crate) disabled_tools: Vec<String>,
    /// Skill names switched off by name.
    pub(crate) disabled_skills: Vec<String>,
    /// Plugin names switched off by name.
    pub(crate) disabled_plugins: Vec<String>,
    /// API protocol the agent speaks: empty follows the model, else chat, responses,
    /// or anthropic (the Messages API).
    #[serde(default)]
    pub(crate) api_mode: String,
    /// Workspace tree order the sidebar last chose; empty means the default.
    #[serde(default)]
    pub(crate) session_sort: String,
    /// Endpoints discovered from the BOS config (never serialized).
    #[serde(skip)]
    pub(crate) profiles: Vec<Profile>,
}

fn default_lang() -> String {
    "en".to_string()
}

fn default_theme_mode() -> String {
    "system".to_string()
}

/// The reference stepper default, which is the baseline body size.
fn default_content_font_size() -> u32 {
    crate::theme::BODY_SIZE as u32
}

fn default_max_steps() -> usize {
    10
}

/// The reference's own default permission mode for a new session.
fn default_permission() -> String {
    "workspace-write".to_string()
}

/// The desktop shell's measured Work details default, which is `dshDesktop`
/// in the reference's `TranscriptViewPolicy` rather than the shared
/// `DEFAULT_TRANSCRIPT_VIEW_MODE`.
fn default_transcript_view() -> String {
    DEFAULT_TRANSCRIPT_VIEW.to_string()
}

/// Where a message's links open; the reference's measured default.
fn default_link_opening() -> String {
    "sidebar".to_string()
}

/// What Enter does while a turn streams; the reference's measured default.
fn default_composer_enter() -> String {
    "queue".to_string()
}

/// How much performance and usage detail to show; the reference's default.
fn default_performance_usage() -> String {
    "detailed".to_string()
}

/// The reference's `DeveloperToolsSettingsFields` default, which is on.
fn default_developer_tools() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            theme_mode: "system".to_string(),
            content_width: None,
            sidebar_width: None,
            content_font_size: 14,
            transcript_view: default_transcript_view(),
            link_opening: default_link_opening(),
            composer_enter: default_composer_enter(),
            performance_usage: default_performance_usage(),
            developer_tools: default_developer_tools(),
            lang: "en".to_string(),
            model: "gpt-4o".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            system_prompt: "You are a helpful assistant.".to_string(),
            temperature: 0.7,
            reasoning_effort: None,
            max_tokens: None,
            max_steps: default_max_steps(),
            bash_enabled: true,
            file_tools_enabled: true,
            web_tools_enabled: true,
            require_approval: true,
            permission: default_permission(),
            skills_dir: "~/.bos/skills".to_string(),
            context_budget: 32768,
            shell_timeout_secs: 300,
            tool_output_cap: 16 * 1024,
            memory_enabled: true,
            memory_path: String::new(),
            mcp_servers: Vec::new(),
            providers: Vec::new(),
            workspaces: Vec::new(),
            active_workspace: String::new(),
            active_provider: String::new(),
            catalogs: Vec::new(),
            schedules: Vec::new(),
            presets: Vec::new(),
            preset: String::new(),
            parallel_tool_calls: false,
            active_session: String::new(),
            disabled_tools: Vec::new(),
            disabled_skills: Vec::new(),
            disabled_plugins: Vec::new(),
            api_mode: String::new(),
            session_sort: String::new(),
            profiles: Vec::new(),
        }
    }
}

/// How a completed turn's process steps group, from the reference's own
/// `stepGrouping` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepGrouping {
    /// Every step folds under one disclosure.
    Collapsed,
    /// A closed turn keeps the history it accumulated while it ran.
    History,
    /// Every step stands on its own.
    None,
}

/// The reference's `TranscriptViewPolicy`, measured from its
/// `dsh-client-ui-chat` bundle.
///
/// `derivePresentationPolicy` is a pure table lookup, and none of its readers
/// compares the mode enum, so these four readings are the whole vocabulary:
/// completed turns fold, the running turn keeps its steps, a settled reasoning
/// block keeps its preview, and a turn's steps group by one of three rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptPolicy {
    /// Fold a completed turn's live process away.
    pub(crate) fold_completed_turns: bool,
    /// How a completed turn's steps group.
    pub(crate) step_grouping: StepGrouping,
    /// Keep the running turn's own steps visible while it streams.
    pub(crate) live_process_detail: bool,
    /// Keep a settled reasoning block's one-line preview.
    pub(crate) settled_reasoning_preview: bool,
}

/// The mode a client that saves nothing starts on.
///
/// The bundle's own `DEFAULT_TRANSCRIPT_VIEW_MODE` is `detailed`, but that is
/// the non-Desktop-Web default; the desktop shell ships `standard`, so the
/// desktop reading is the one this shell uses.
pub(crate) const DEFAULT_TRANSCRIPT_VIEW: &str = "standard";

impl TranscriptPolicy {
    /// The policy a stored mode resolves to, legacy spellings included.
    ///
    /// `normal` and `expanded` are a two-mode generation's saved values: the
    /// reference reads both as `detailed`, never offers them and never writes
    /// them back. An unknown mode falls back to the desktop default.
    pub(crate) fn resolve(mode: &str) -> Self {
        match mode {
            "compact" => Self {
                fold_completed_turns: true,
                step_grouping: StepGrouping::Collapsed,
                live_process_detail: false,
                settled_reasoning_preview: false,
            },
            "detailed" | "normal" | "expanded" => Self {
                fold_completed_turns: true,
                step_grouping: StepGrouping::History,
                live_process_detail: true,
                settled_reasoning_preview: true,
            },
            "verbose" => Self {
                fold_completed_turns: false,
                step_grouping: StepGrouping::None,
                live_process_detail: false,
                settled_reasoning_preview: true,
            },
            _ => Self {
                fold_completed_turns: true,
                step_grouping: StepGrouping::Collapsed,
                live_process_detail: true,
                settled_reasoning_preview: true,
            },
        }
    }
}

/// The concrete values a build uses once workspace overrides are applied.
#[derive(Debug, Clone)]
pub(crate) struct Effective {
    /// ReAct steps one turn may take before the loop stops.
    pub(crate) max_steps: usize,
    /// Run one turn tool calls concurrently when the model asks for several.
    pub(crate) parallel_tool_calls: bool,
    /// API protocol, or empty to follow the model.
    pub(crate) api_mode: String,
    /// Workspace display name.
    pub(crate) name: String,
    /// Workspace root.
    pub(crate) root: String,
    /// Model identifier.
    pub(crate) model: String,
    /// API base URL.
    pub(crate) base_url: String,
    /// API key.
    pub(crate) api_key: String,
    /// System prompt.
    pub(crate) system_prompt: String,
    /// Sampling temperature.
    pub(crate) temperature: f32,
    /// Reasoning effort.
    pub(crate) reasoning_effort: Option<String>,
    /// Completion cap.
    pub(crate) max_tokens: Option<u32>,
    /// Register the shell tool.
    pub(crate) bash_enabled: bool,
    /// Register the filesystem tools.
    pub(crate) file_tools_enabled: bool,
    /// The default permission mode for new sessions.
    pub(crate) permission: String,
    /// Skills directory.
    pub(crate) skills_dir: String,
    /// Send-side token ceiling.
    pub(crate) context_budget: usize,
    /// Shell command timeout in seconds.
    pub(crate) shell_timeout_secs: u64,
    /// Tool result truncation ceiling in bytes.
    pub(crate) tool_output_cap: usize,
    /// Attach memory.
    pub(crate) memory_enabled: bool,
    /// How much of a turn the conversation renders.
    pub(crate) transcript_view: TranscriptPolicy,
    /// How much performance and usage detail to show.
    pub(crate) performance_usage: String,
    /// Tool names switched off.
    pub(crate) disabled_tools: Vec<String>,
    /// Skill names switched off.
    pub(crate) disabled_skills: Vec<String>,
}

/// The harness data directory.
pub(crate) fn dir() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~/.bos/bsh").into_owned())
}

/// Load every preset a directory holds, skipping the files that do not parse.
///
/// The store drops unreadable session files silently for the same reason: a
/// stray file in the directory must not take the pane down with it.
pub(crate) fn presets_in(dir: &std::path::Path) -> Vec<agent::plugins::presets::Preset> {
    let mut presets: Vec<agent::plugins::presets::Preset> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("json") | Some("toml")
            )
        })
        .filter_map(|path| agent::plugins::presets::Preset::load(&path).ok())
        .collect();
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    presets
}

/// The presets under the BSH directory, which is where the pane reads them.
pub(crate) fn discover_presets() -> Vec<agent::plugins::presets::Preset> {
    presets_in(&dir().join("presets"))
}

/// The settings file path.
pub(crate) fn settings_path() -> PathBuf {
    dir().join("settings.toml")
}

/// The platform's file browser launcher.
#[cfg(target_os = "macos")]
pub(crate) const OPENER: &str = "open";
/// The platform's file browser launcher.
#[cfg(target_os = "linux")]
pub(crate) const OPENER: &str = "xdg-open";
/// The platform's file browser launcher.
#[cfg(target_os = "windows")]
pub(crate) const OPENER: &str = "explorer";

/// Show the settings file in the platform's file browser.
///
/// A file that does not exist yet is not an error: the directory is revealed
/// instead, which is where the reader would create it.
pub(crate) fn reveal_config() -> std::io::Result<()> {
    let path = settings_path();
    let target = if path.exists() { path } else { dir() };
    std::process::Command::new(OPENER)
        .arg(target)
        .spawn()
        .map(|_| ())
}

impl Settings {
    /// Read the saved settings, discovering model defaults on first run.
    pub(crate) fn load() -> Self {
        let path = settings_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut parsed) = toml::from_str::<Settings>(&text) {
                parsed.profiles = discover_profiles();
                parsed.presets = discover_presets();
                parsed.ensure_workspace();
                return parsed;
            }
        }
        let mut settings = Settings::discover();
        settings.ensure_workspace();
        let _ = settings.save();
        settings
    }

    /// Build settings from the BOS config defaults.
    pub(crate) fn discover() -> Self {
        let profiles = discover_profiles();
        let first = profiles.first().cloned();
        let mut settings = Settings {
            profiles,
            presets: discover_presets(),
            ..Settings::default()
        };
        if let Some(profile) = first {
            settings.model = profile.model;
            settings.base_url = profile.base_url;
            settings.api_key = profile.api_key;
        }
        settings
    }

    /// Persist the settings.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        let dir = dir();
        std::fs::create_dir_all(&dir)?;
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(settings_path(), text)
    }

    /// Guarantee at least one workspace and a valid active id.
    pub(crate) fn ensure_workspace(&mut self) {
        if self.workspaces.is_empty() {
            let root = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = PathBuf::from(&root)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "workspace".to_string());
            let ws = Workspace {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                root,
                created_at: crate::state::now(),
                updated_at: crate::state::now(),
                ..Default::default()
            };
            self.active_workspace = ws.id.clone();
            self.workspaces.push(ws);
        }
        if self.active_workspace.is_empty()
            || !self
                .workspaces
                .iter()
                .any(|w| w.id == self.active_workspace)
        {
            self.active_workspace = self.workspaces[0].id.clone();
        }
    }

    /// The active workspace.
    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.id == self.active_workspace)
            .or_else(|| self.workspaces.first())
    }

    /// The preset the agent-presets pane selected, if it is still on disk.
    pub(crate) fn active_preset(&self) -> Option<&agent::plugins::presets::Preset> {
        if self.preset.is_empty() {
            return None;
        }
        self.presets
            .iter()
            .find(|preset| preset.name == self.preset)
    }

    /// The model ids one provider's catalog holds, in insertion order.
    pub(crate) fn catalog(&self, provider: &str) -> &[String] {
        self.catalogs
            .iter()
            .find(|entry| entry.provider == provider)
            .map(|entry| entry.models.as_slice())
            .unwrap_or(&[])
    }

    /// Add a model id to one provider's catalog.
    ///
    /// A blank and a repeat are both refused rather than quietly stored, because
    /// the pane explains the refusal; the catalog is created on first use.
    /// Add one provider, or say why it cannot be added.
    ///
    /// A refused call leaves the settings exactly as they were: the reference
    /// never silently replaces a provider that is already saved.
    /// Update the provider behind `original` from the current fields, or say
    /// why it cannot be updated.
    ///
    /// An edit may rename the provider, but never onto another saved one; the
    /// reference refuses that with the same sentence it refuses a duplicate on.
    pub(crate) fn update_provider(
        &mut self,
        original: &str,
        name: &str,
    ) -> Result<(), ProviderRefusal> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ProviderRefusal::NameEmpty);
        }
        if self.base_url.trim().is_empty() {
            return Err(ProviderRefusal::NeedsBaseUrl);
        }
        if self.model.trim().is_empty() {
            return Err(ProviderRefusal::NeedsModel);
        }
        if name != original && self.providers.iter().any(|saved| saved.name == name) {
            return Err(ProviderRefusal::IdTaken);
        }
        let Some(index) = self
            .providers
            .iter()
            .position(|saved| saved.name == original)
        else {
            return self.add_provider(&name);
        };
        let model = self.model.clone();
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let entry = &mut self.providers[index];
        entry.name = name.clone();
        entry.model = model;
        entry.base_url = base_url;
        entry.api_key = api_key;
        self.active_provider = name;
        Ok(())
    }

    pub(crate) fn add_provider(&mut self, name: &str) -> Result<(), ProviderRefusal> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ProviderRefusal::NameEmpty);
        }
        if self.base_url.trim().is_empty() {
            return Err(ProviderRefusal::NeedsBaseUrl);
        }
        if self.model.trim().is_empty() {
            return Err(ProviderRefusal::NeedsModel);
        }
        if self.providers.iter().any(|saved| saved.name == name) {
            return Err(ProviderRefusal::IdTaken);
        }
        self.providers.push(Profile {
            name: name.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
        });
        self.active_provider = name;
        Ok(())
    }

    pub(crate) fn add_catalog_model(
        &mut self,
        provider: &str,
        model: &str,
    ) -> Result<(), CatalogError> {
        let model = model.trim();
        if model.is_empty() {
            return Err(CatalogError::Empty);
        }
        let index = match self
            .catalogs
            .iter()
            .position(|entry| entry.provider == provider)
        {
            Some(index) => index,
            None => {
                self.catalogs.push(ModelCatalog {
                    provider: provider.to_string(),
                    models: Vec::new(),
                    input_types: BTreeMap::new(),
                });
                self.catalogs.len() - 1
            }
        };
        let entry = &mut self.catalogs[index];
        if entry.models.iter().any(|existing| existing == model) {
            return Err(CatalogError::Duplicate);
        }
        entry.models.push(model.to_string());
        Ok(())
    }

    /// Drop a model id from one provider's catalog, dropping an emptied catalog.
    pub(crate) fn remove_catalog_model(&mut self, provider: &str, model: &str) {
        if let Some(entry) = self
            .catalogs
            .iter_mut()
            .find(|entry| entry.provider == provider)
        {
            entry.models.retain(|existing| existing != model);
            entry.input_types.remove(model);
        }
        self.catalogs.retain(|entry| !entry.models.is_empty());
    }

    /// The input types declared for one catalog model, empty when undeclared.
    pub(crate) fn input_types_for(&self, provider: &str, model: &str) -> &[String] {
        self.catalogs
            .iter()
            .find(|entry| entry.provider == provider)
            .and_then(|entry| entry.input_types.get(model))
            .map(|types| types.as_slice())
            .unwrap_or(&[])
    }

    /// Whether one catalog model shows an input type as declared.
    ///
    /// Undeclared reads as both, because bsh sends either until the user says
    /// otherwise; that keeps the chips lit on a catalog that predates them.
    pub(crate) fn shows_input_type(&self, provider: &str, model: &str, kind: &str) -> bool {
        match self.input_types_for(provider, model) {
            [] => true,
            declared => declared.iter().any(|entry| entry == kind),
        }
    }

    /// Whether one catalog model accepts image input.
    ///
    /// Undeclared is permissive: bsh only refuses an image the user explicitly
    /// said the model cannot take.
    pub(crate) fn accepts_image(&self, provider: &str, model: &str) -> bool {
        self.shows_input_type(provider, model, "image")
    }

    /// Whether the active model of the active provider accepts image input.
    pub(crate) fn active_accepts_image(&self) -> bool {
        if self.active_provider.is_empty() {
            return true;
        }
        self.accepts_image(&self.active_provider, &self.model)
    }

    /// Toggle one input type declared on a catalog model.
    ///
    /// The first toggle materializes the declaration: a model that declares
    /// nothing shows both chips lit, and turning one off records the other.
    /// Text is never dropped, because a model that takes no text cannot answer.
    pub(crate) fn toggle_input_type(&mut self, provider: &str, model: &str, kind: &str) {
        if !self.catalogs.iter().any(|entry| entry.provider == provider) {
            self.catalogs.push(ModelCatalog {
                provider: provider.to_string(),
                models: Vec::new(),
                input_types: BTreeMap::new(),
            });
        }
        let declared: Vec<String> = match self.input_types_for(provider, model) {
            [] => vec!["text".to_string(), "image".to_string()],
            current => current.to_vec(),
        };
        let mut next: Vec<String> = declared
            .iter()
            .filter(|entry| *entry != kind)
            .cloned()
            .collect();
        if next.len() == declared.len() {
            next.push(kind.to_string());
        }
        if !next.iter().any(|entry| entry == "text") {
            next.insert(0, "text".to_string());
        }
        if let Some(entry) = self
            .catalogs
            .iter_mut()
            .find(|entry| entry.provider == provider)
        {
            entry.input_types.insert(model.to_string(), next);
        }
    }
    /// The provider the models pane selected, looked up across both lists.
    ///
    /// Saved providers are searched first, then the profiles discovered from
    /// the BOS config, so a saved entry wins over a discovered one of the same
    /// name. An empty selection is no selection.
    pub(crate) fn active_profile(&self) -> Option<&Profile> {
        if self.active_provider.is_empty() {
            return None;
        }
        self.providers
            .iter()
            .chain(self.profiles.iter())
            .find(|profile| profile.name == self.active_provider)
    }

    /// Resolve the effective values for the active workspace.
    pub(crate) fn effective(&self) -> Effective {
        let mut eff = Effective {
            name: String::new(),
            root: String::new(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            system_prompt: self.system_prompt.clone(),
            temperature: self.temperature,
            reasoning_effort: self.reasoning_effort.clone(),
            max_tokens: self.max_tokens,
            max_steps: self.max_steps,
            parallel_tool_calls: self.parallel_tool_calls,
            api_mode: self.api_mode.clone(),
            bash_enabled: self.bash_enabled,
            file_tools_enabled: self.file_tools_enabled,
            permission: self.permission.clone(),
            skills_dir: self.skills_dir.clone(),
            context_budget: self.context_budget,
            shell_timeout_secs: self.shell_timeout_secs,
            tool_output_cap: self.tool_output_cap,
            memory_enabled: self.memory_enabled,
            transcript_view: TranscriptPolicy::resolve(&self.transcript_view),
            performance_usage: self.performance_usage.clone(),
            disabled_tools: self.disabled_tools.clone(),
            disabled_skills: self.disabled_skills.clone(),
        };
        if let Some(profile) = self.active_profile() {
            eff.model = profile.model.clone();
            eff.base_url = profile.base_url.clone();
            eff.api_key = profile.api_key.clone();
        }
        if let Some(preset) = self.active_preset() {
            if let Some(model) = &preset.model {
                eff.model = model.clone();
            }
            if let Some(prompt) = &preset.system_prompt {
                eff.system_prompt = prompt.clone();
            }
            if let Some(steps) = preset.max_steps {
                eff.max_steps = steps as usize;
            }
        }
        if let Some(ws) = self.active_workspace() {
            eff.name = ws.name.clone();
            if !ws.root.is_empty() {
                eff.root = ws.root.clone();
            }
            if !ws.model.is_empty() {
                eff.model = ws.model.clone();
            }
            if !ws.base_url.is_empty() {
                eff.base_url = ws.base_url.clone();
            }
            if !ws.api_key.is_empty() {
                eff.api_key = ws.api_key.clone();
            }
            if !ws.system_prompt.is_empty() {
                eff.system_prompt = ws.system_prompt.clone();
            }
            if ws.temperature > 0.0 {
                eff.temperature = ws.temperature;
            }
            if ws.reasoning_effort.is_some() {
                eff.reasoning_effort = ws.reasoning_effort.clone();
            }
        }
        eff
    }

    /// The memory file in effect.
    pub(crate) fn memory_file(&self) -> PathBuf {
        let raw = self.memory_path.trim();
        let path = if raw.is_empty() {
            dir().join("memory.jsonl")
        } else {
            PathBuf::from(shellexpand::tilde(raw).into_owned())
        };
        path
    }
}

/// Read the [llm.<name>] profiles from the discovered BOS config.
fn discover_profiles() -> Vec<Profile> {
    let mut profiles = Vec::new();
    let mut loader = config::ConfigLoader::new().discover();
    let Ok(value) = loader.load_sync() else {
        return profiles;
    };
    if let Some(gm) = value.get("global_model") {
        let model = gm
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let base_url = gm
            .get("base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let api_key = gm
            .get("api_key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !model.is_empty() {
            profiles.push(Profile {
                name: "global".to_string(),
                model,
                base_url,
                api_key,
            });
        }
    }
    if let Some(llm) = value.get("llm").and_then(|v| v.as_object()) {
        for (name, entry) in llm {
            let model = entry.get("model").and_then(|v| v.as_str()).unwrap_or("");
            if model.is_empty() {
                continue;
            }
            profiles.push(Profile {
                name: name.clone(),
                model: model.to_string(),
                base_url: entry
                    .get("base_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                api_key: entry
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    profiles
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_content_font_size_defaults_to_the_reference_step() {
        let settings = super::Settings::default();
        assert_eq!(
            settings.content_font_size, 14,
            "the reference stepper default"
        );
        assert_eq!(
            settings.theme_mode, "system",
            "the reference default preference"
        );
        assert_eq!(super::default_content_font_size(), 14);
        assert_eq!(super::default_theme_mode(), "system");
    }

    #[test]
    fn input_types_are_undeclared_until_the_pane_says_otherwise() {
        let mut settings = super::Settings {
            active_provider: "p".to_string(),
            model: "m".to_string(),
            ..super::Settings::default()
        };
        assert!(settings.active_accepts_image(), "undeclared is permissive");
        settings.toggle_input_type("p", "m", "image");
        assert!(
            !settings.active_accepts_image(),
            "turning Image off refuses images"
        );
        assert_eq!(settings.input_types_for("p", "m"), ["text".to_string()]);
    }

    #[test]
    fn an_image_can_be_turned_off_and_back_on() {
        let mut settings = super::Settings::default();
        settings.toggle_input_type("p", "m", "image");
        assert!(!settings.accepts_image("p", "m"));
        settings.toggle_input_type("p", "m", "image");
        assert!(settings.accepts_image("p", "m"));
        assert_eq!(
            settings.input_types_for("p", "m"),
            ["text".to_string(), "image".to_string()]
        );
    }

    #[test]
    fn text_never_leaves_the_declared_input_types() {
        let mut settings = super::Settings::default();
        settings.toggle_input_type("p", "m", "text");
        assert!(settings.shows_input_type("p", "m", "text"));
        assert!(settings.accepts_image("p", "m"));
    }

    #[test]
    fn a_declared_input_type_survives_a_reload() {
        let mut settings = super::Settings::default();
        settings.toggle_input_type("p", "m", "image");
        settings.toggle_input_type("p", "m", "image");
        let text = serde_json::to_string(&settings).expect("serialize");
        let back: super::Settings = serde_json::from_str(&text).expect("deserialize");
        assert!(back.accepts_image("p", "m"));
    }

    #[test]
    fn dropping_a_catalog_model_drops_its_declaration() {
        let mut settings = super::Settings::default();
        settings.add_catalog_model("p", "m").expect("add");
        settings.toggle_input_type("p", "m", "image");
        assert!(!settings.accepts_image("p", "m"));
        settings.remove_catalog_model("p", "m");
        assert!(settings.accepts_image("p", "m"), "gone means undeclared");
    }
    use super::{presets_in, Profile, Settings};

    /// A preset with a name, a model, a prompt and a step ceiling.
    fn preset(name: &str) -> agent::plugins::presets::Preset {
        agent::plugins::presets::Preset {
            name: name.to_string(),
            tools: vec!["bash".to_string()],
            model: Some(format!("{name}-model")),
            system_prompt: Some(format!("{name} prompt")),
            max_steps: Some(21),
        }
    }

    /// A selected preset names the agent, and a stale name falls back to the
    /// fields rather than blanking them.
    #[test]
    fn a_preset_supplies_the_model_prompt_and_ceiling() {
        let settings = Settings {
            model: "plain".to_string(),
            max_steps: 4,
            presets: vec![preset("careful")],
            preset: "careful".to_string(),
            ..Settings::default()
        };
        let eff = settings.effective();
        assert_eq!(eff.model, "careful-model");
        assert_eq!(eff.system_prompt, "careful prompt");
        assert_eq!(eff.max_steps, 21);

        let stale = Settings {
            preset: "gone".to_string(),
            ..settings
        };
        let eff = stale.effective();
        assert_eq!(eff.model, "plain");
        assert_eq!(eff.max_steps, 4);
    }

    /// The directory loader reads what parses, in name order, and skips the rest.
    #[test]
    fn the_preset_loader_reads_the_directory() {
        let dir = std::path::PathBuf::from("target/bsh-preset-dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(
            dir.join("careful.json"),
            r#"{"name":"careful","tools":["bash"],"model":"m","max_steps":3}"#,
        )
        .expect("write");
        std::fs::write(dir.join("broken.json"), "{ not json").expect("write");
        std::fs::write(dir.join("ignored.txt"), "hello").expect("write");

        let found = presets_in(&dir);
        assert_eq!(found.len(), 1, "only the parseable preset is read");
        assert_eq!(found[0].name, "careful");
        assert_eq!(found[0].max_steps, Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A selected provider is the endpoint tier, and a stale name falls back to
    /// the plain fields rather than blanking the endpoint.
    #[test]
    fn effective_prefers_the_active_provider() {
        let settings = Settings {
            model: "plain".to_string(),
            base_url: "https://plain".to_string(),
            api_key: "plain-key".to_string(),
            providers: vec![Profile {
                name: "work".to_string(),
                model: "work-model".to_string(),
                base_url: "https://work".to_string(),
                api_key: "work-key".to_string(),
            }],
            active_provider: "work".to_string(),
            ..Settings::default()
        };
        let eff = settings.effective();
        assert_eq!(eff.model, "work-model");
        assert_eq!(eff.base_url, "https://work");
        assert_eq!(eff.api_key, "work-key");

        let stale = Settings {
            active_provider: "gone".to_string(),
            ..settings
        };
        let eff = stale.effective();
        assert_eq!(eff.base_url, "https://plain");
        assert_eq!(eff.api_key, "plain-key");
    }

    /// The provider form refuses what the reference refuses, and a refusal never
    /// touches a provider that is already saved.
    #[test]
    fn the_provider_form_refuses_and_never_replaces() {
        let mut settings = super::Settings {
            base_url: String::new(),
            model: String::new(),
            ..super::Settings::default()
        };
        assert_eq!(
            settings.add_provider("  "),
            Err(super::ProviderRefusal::NameEmpty)
        );
        assert_eq!(
            settings.add_provider("work"),
            Err(super::ProviderRefusal::NeedsBaseUrl)
        );
        settings.base_url = "https://api.example/v1".to_string();
        assert_eq!(
            settings.add_provider("work"),
            Err(super::ProviderRefusal::NeedsModel)
        );
        settings.model = "deepseek-chat".to_string();
        assert_eq!(settings.add_provider("work"), Ok(()));
        assert_eq!(settings.providers.len(), 1);
        assert_eq!(settings.active_provider, "work");

        settings.base_url = "https://elsewhere.example/v1".to_string();
        assert_eq!(
            settings.add_provider("work"),
            Err(super::ProviderRefusal::IdTaken),
            "a saved provider is never replaced"
        );
        assert_eq!(
            settings.providers[0].base_url, "https://api.example/v1",
            "and the refusal left it untouched"
        );
    }

    /// Each refusal draws the reference own sentence, so the mapping is pinned.
    #[test]
    fn every_refusal_names_the_sentence_it_draws() {
        let pairs = [
            (
                super::ProviderRefusal::NameEmpty,
                "settings.providerNameEmpty",
            ),
            (
                super::ProviderRefusal::NeedsBaseUrl,
                "settings.providerNeedsBaseUrl",
            ),
            (
                super::ProviderRefusal::NeedsModel,
                "settings.providerNeedsModel",
            ),
            (super::ProviderRefusal::IdTaken, "settings.providerIdTaken"),
        ];
        for (refusal, key) in pairs {
            assert_eq!(refusal.key(), key);
        }
    }

    /// An edit changes the provider in place and may rename it, but never onto
    /// another saved provider.
    #[test]
    fn editing_a_provider_updates_it_in_place() {
        let mut settings = super::Settings {
            base_url: "https://api.example/v1".to_string(),
            model: "deepseek-chat".to_string(),
            ..super::Settings::default()
        };
        assert_eq!(settings.add_provider("work"), Ok(()));
        assert_eq!(settings.add_provider("spare"), Ok(()));

        settings.base_url = "https://elsewhere.example/v1".to_string();
        assert_eq!(
            settings.update_provider("work", "spare"),
            Err(super::ProviderRefusal::IdTaken)
        );
        assert_eq!(settings.providers.len(), 2, "a refused edit adds nothing");
        assert_eq!(settings.providers[0].name, "work", "and renames nothing");

        assert_eq!(settings.update_provider("work", "  home  "), Ok(()));
        assert_eq!(settings.providers.len(), 2, "an edit never duplicates");
        assert_eq!(settings.providers[0].name, "home");
        assert_eq!(
            settings.providers[0].base_url,
            "https://elsewhere.example/v1"
        );
        assert_eq!(settings.active_provider, "home");

        settings.base_url = String::new();
        assert_eq!(
            settings.update_provider("home", "home"),
            Err(super::ProviderRefusal::NeedsBaseUrl)
        );
        assert_eq!(
            settings.providers[0].base_url, "https://elsewhere.example/v1",
            "the refused edit left the provider alone"
        );
    }

    /// The catalog refuses a blank id and a repeat, keeps insertion order, and
    /// drops an emptied list, because the pane derives its hints from exactly
    /// these answers.
    #[test]
    fn the_model_catalog_refuses_blanks_and_repeats() {
        let mut settings = Settings::default();
        assert!(settings.catalog("work").is_empty());
        assert_eq!(
            settings.add_catalog_model("work", "   "),
            Err(super::CatalogError::Empty)
        );
        assert!(
            settings.catalog("work").is_empty(),
            "a blank id stores nothing"
        );
        assert_eq!(settings.add_catalog_model("work", "gpt-4o"), Ok(()));
        assert_eq!(
            settings.add_catalog_model("work", "gpt-4o"),
            Err(super::CatalogError::Duplicate)
        );
        assert_eq!(settings.add_catalog_model("work", "o3"), Ok(()));
        assert_eq!(
            settings.catalog("work"),
            ["gpt-4o".to_string(), "o3".to_string()],
            "insertion order is the user order"
        );
        assert!(settings.catalog("personal").is_empty());
        assert_eq!(settings.add_catalog_model("personal", "llama"), Ok(()));
        assert_eq!(
            settings.catalog("work").len(),
            2,
            "catalogs are per provider"
        );
        settings.remove_catalog_model("work", "gpt-4o");
        assert_eq!(settings.catalog("work"), ["o3".to_string()]);
        settings.remove_catalog_model("work", "o3");
        assert!(
            !settings
                .catalogs
                .iter()
                .any(|entry| entry.provider == "work"),
            "an emptied catalog is dropped rather than kept as a husk"
        );
    }

    #[test]
    fn a_new_task_is_due_once_and_then_waits_for_its_interval() {
        let mut settings = Settings::default();
        let id = settings
            .add_schedule("Digest", "Summarize the inbox.", "s1", 60)
            .expect("a named task with an instruction is accepted");
        let task = settings.schedules[0].clone();
        assert!(settings.is_due(&task, 1_000), "a fresh task is due at once");
        settings.mark_schedule_ran(&id, 1_000);
        let task = settings.schedules[0].clone();
        assert!(
            !settings.is_due(&task, 1_059),
            "the interval has not elapsed"
        );
        assert!(settings.is_due(&task, 1_060), "the interval has elapsed");
        assert_eq!(settings.next_run(&task, 1_060), Some(1_060));
    }

    #[test]
    fn a_one_shot_task_runs_once_and_turns_itself_off() {
        let mut settings = Settings::default();
        let id = settings
            .add_schedule("Ping", "Say hello.", "s1", 0)
            .expect("a single run needs no interval");
        let task = settings.schedules[0].clone();
        assert!(settings.is_due(&task, 10));
        settings.mark_schedule_ran(&id, 10);
        let task = settings.schedules[0].clone();
        assert!(!task.enabled, "a single run has happened");
        assert!(!settings.is_due(&task, 10_000));
        assert_eq!(settings.next_run(&task, 10_000), None);
    }

    #[test]
    fn an_inactive_task_never_triggers() {
        let mut settings = Settings::default();
        let id = settings
            .add_schedule("Watch", "Check the queue.", "s1", 30)
            .expect("the task is valid before it is switched off");
        assert!(settings.toggle_schedule(&id));
        let task = settings.schedules[0].clone();
        assert!(!settings.is_due(&task, 10_000));
        assert_eq!(settings.next_run(&task, 10_000), None);
        assert!(settings.toggle_schedule(&id), "it can be switched back on");
        let task = settings.schedules[0].clone();
        assert!(settings.is_due(&task, 10_000));
    }

    #[test]
    fn a_task_needs_a_name_and_an_instruction() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.add_schedule("  ", "Do it.", "s1", 60),
            Err(super::ScheduleRefusal::Name)
        );
        let long = "n".repeat(Settings::NAME_LIMIT + 1);
        assert_eq!(
            settings.add_schedule(&long, "Do it.", "s1", 60),
            Err(super::ScheduleRefusal::Name)
        );
        assert_eq!(
            settings.add_schedule("Named", "   ", "s1", 60),
            Err(super::ScheduleRefusal::Prompt)
        );
        assert!(settings.schedules.is_empty(), "a refused task is not kept");
    }

    #[test]
    fn the_work_details_modes_are_the_references_own_table() {
        use super::{StepGrouping, TranscriptPolicy};
        let compact = TranscriptPolicy::resolve("compact");
        assert!(compact.fold_completed_turns);
        assert_eq!(compact.step_grouping, StepGrouping::Collapsed);
        assert!(!compact.live_process_detail);
        assert!(!compact.settled_reasoning_preview);

        let standard = TranscriptPolicy::resolve("standard");
        assert!(standard.fold_completed_turns);
        assert_eq!(standard.step_grouping, StepGrouping::Collapsed);
        assert!(standard.live_process_detail);
        assert!(standard.settled_reasoning_preview);

        let detailed = TranscriptPolicy::resolve("detailed");
        assert!(detailed.fold_completed_turns);
        assert_eq!(detailed.step_grouping, StepGrouping::History);
        assert!(detailed.live_process_detail);
        assert!(detailed.settled_reasoning_preview);

        let verbose = TranscriptPolicy::resolve("verbose");
        assert!(!verbose.fold_completed_turns);
        assert_eq!(verbose.step_grouping, StepGrouping::None);
        assert!(!verbose.live_process_detail);
        assert!(verbose.settled_reasoning_preview);
    }

    #[test]
    fn a_legacy_work_details_value_reads_as_detailed() {
        let detailed = super::TranscriptPolicy::resolve("detailed");
        for legacy in ["normal", "expanded"] {
            assert_eq!(
                super::TranscriptPolicy::resolve(legacy),
                detailed,
                "{legacy} is a saved value of the two-mode generation"
            );
        }
        assert_eq!(
            super::TranscriptPolicy::resolve("standard"),
            super::TranscriptPolicy::resolve("not-a-mode"),
            "an unknown mode falls back to the desktop default"
        );
        assert_eq!(super::DEFAULT_TRANSCRIPT_VIEW, "standard");
        assert_eq!(super::default_transcript_view(), "standard");
    }

    #[test]
    fn dropping_a_task_takes_it_out_of_the_file() {
        let mut settings = Settings::default();
        let id = settings
            .add_schedule("Digest", "Summarize.", "s1", 60)
            .expect("the task is valid");
        assert!(settings.remove_schedule(&id));
        assert!(
            !settings.remove_schedule(&id),
            "a second drop finds nothing"
        );
        assert!(settings.schedules.is_empty());
    }
}
