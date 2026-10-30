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
    /// A preset carries a level or a path scope that cannot be honoured.
    ///
    /// The message names the preset, the field and the value: a typo is a load
    /// error, never a silent default.
    InvalidPreset {
        /// The preset id.
        preset: String,
        /// What is wrong, with the field named.
        why: String,
    },
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
            Self::InvalidPreset { preset, why } => {
                write!(f, "the preset {preset:?} is invalid: {why}")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

/// One directory skills are discovered in.
///
/// The list of these is ORDERED and the order is the precedence: a skill name
/// found in an earlier source wins over the same name in a later one, so a
/// project directory can shadow a shared one without renaming anything. This
/// mirrors the reference's own root rank table (measured in
/// `@deepseek-ai/dsh-skill-filesystem`: project `.dsh/skills` 100, project
/// `.agents/skills` 200, custom 300, user `.dsh/skills` 400, user
/// `.agents/skills` 500, bundled 600), which the settings file flattens into
/// one list the reader can reorder and switch off.
///
/// `name` is the label the pane shows; `path` is stored as written (a leading
/// `~` is expanded at read time) so the file stays portable between machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillSource {
    /// The display name, e.g. `Bos` or `DSH`.
    pub(crate) name: String,
    /// The directory, as the reader typed it.
    pub(crate) path: String,
    /// Whether this source contributes skills. A switched-off source is kept in
    /// the list, with its path, so switching it back on needs no retyping.
    pub(crate) enabled: bool,
}

impl Default for SkillSource {
    fn default() -> Self {
        Self {
            name: String::new(),
            path: String::new(),
            enabled: true,
        }
    }
}

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

/// The local web-search tool's endpoint, budget and request shape.
///
/// The tool itself is registered by the build; this section is what it asks.
/// The endpoint is a MACHINE fact rather than a workspace one - the same
/// deployment serves every checkout - so it lives in the global file. The
/// credential is not here: it is a vault entry, named by
/// [`crate::vault::WEB_SEARCH_KEY`], because the settings tree stores no secret.
///
/// The shape fields exist because search providers disagree about the wire:
/// DeepSeek's Search API and Tavily are both `POST <base>/search` with
/// `Authorization: Bearer`, but Tavily names no count field, reads snippets
/// from `content` and dates from `published_date`, and a self-hosted SearXNG
/// wants GET with `q=`. Every field's default is the DeepSeek Search API, so a
/// file that names none of them speaks exactly the shipped wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebSearchSection {
    /// The Search API base, without the operation path.
    pub(crate) base_url: String,
    /// Results one search asks for; the tool clamps it to 1..=50.
    pub(crate) results_per_search: usize,
    /// Request method: "post" (JSON body) or "get" (query string).
    pub(crate) method: String,
    /// The operation appended to the base URL.
    pub(crate) path: String,
    /// The request field carrying the query.
    pub(crate) query_key: String,
    /// The request field carrying the result budget; empty omits it.
    pub(crate) count_key: String,
    /// Static extra fields merged into every request (e.g. `search_depth`).
    pub(crate) body: BTreeMap<String, serde_json::Value>,
    /// The response array holding the hits.
    pub(crate) results_key: String,
    /// The hit field carrying the URL.
    pub(crate) url_key: String,
    /// The hit field carrying a title; empty omits titles.
    pub(crate) title_key: String,
    /// The hit field carrying a snippet; empty omits snippets.
    pub(crate) snippet_key: String,
    /// The hit field carrying a publication date; empty omits dates.
    pub(crate) date_key: String,
}

impl Default for WebSearchSection {
    fn default() -> Self {
        Self {
            base_url: crate::web::DEFAULT_BASE_URL.to_string(),
            results_per_search: crate::web::DEFAULT_RESULTS,
            method: "post".to_string(),
            path: "search".to_string(),
            query_key: "query".to_string(),
            count_key: "count".to_string(),
            body: BTreeMap::from([("need_content".to_string(), serde_json::Value::Bool(false))]),
            results_key: "results".to_string(),
            url_key: "url".to_string(),
            title_key: "title".to_string(),
            snippet_key: "snippet".to_string(),
            date_key: "date".to_string(),
        }
    }
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
    ///
    /// Retired INPUT: a file that still carries a single directory parses, and
    /// its value is migrated into `skill_sources` once, at which point this is
    /// empty and never written back. Kept as an `Option` so "an old file has
    /// one" is distinguishable from "a new file has none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skills_dir: Option<String>,
    /// Every directory skills are discovered in, in precedence order.
    ///
    /// The first source wins a name collision. Empty means the reader has not
    /// opened the Skills pane yet, in which case `skill_sources()` supplies the
    /// discovered defaults rather than scanning nothing.
    pub(crate) skill_sources: Vec<SkillSource>,
    /// MCP servers every workspace gets, merged with each workspace's own.
    pub(crate) mcp_servers: Vec<serde_json::Value>,
    /// The provider tree, keyed by a concrete provider id.
    ///
    /// This is the reader's own document. A provider a plugin contributes does
    /// NOT live here - see `plugin_providers` - so a reload can rebuild that
    /// set from the packages without ever rewriting what the user typed.
    pub(crate) providers: BTreeMap<String, Provider>,
    /// Model providers contributed by installed extension packages, keyed by
    /// provider id.
    ///
    /// Runtime state, never serialized: the packages on disk are the source of
    /// truth, and this is the cache a build reads. It is rebuilt wholesale
    /// whenever the extension inventory is loaded, so uninstalling a package
    /// removes its providers without a migration.
    ///
    /// A plugin provider is looked up by every ``GlobalSettings`` read path
    /// ([`GlobalSettings::lookup`], the protocol resolution, the model lists)
    /// AFTER the reader's own tree, so a package can never shadow a provider the
    /// reader declared by hand.
    #[serde(skip)]
    pub(crate) plugin_providers: BTreeMap<String, Provider>,
    /// Whether a contributed provider needs a credential.
    ///
    /// A package that serves an open or anonymous lane declares
    /// `requiresKey: false`, and this is where the host remembers it. Absent
    /// ids read as key-bearing, which is the safe default: demanding a key that
    /// turns out to be unnecessary is a visible refusal, while omitting one that
    /// was needed is a request that fails at the endpoint.
    #[serde(skip)]
    pub(crate) plugin_provider_keys: BTreeMap<String, bool>,
    /// The extension ids that contributed each plugin provider.
    ///
    /// The Models pane draws a provider's origin from this, so a reader can tell
    /// a plugin's endpoint from their own at a glance, and a preset that allows
    /// only some extensions can be honoured without a second scan.
    #[serde(skip)]
    pub(crate) plugin_provider_owners: BTreeMap<String, String>,
    /// Agent preset declarations, keyed by preset id. A declaration with the
    /// same id as a shipped preset replaces it outright.
    pub(crate) presets: BTreeMap<String, Preset>,
    /// The installed extension inventory. Order is the reader's, and the ids
    /// are checked for repetition because an array of tables does not make
    /// them unique the way a map key would.
    #[serde(default, alias = "extentions")]
    pub(crate) extensions: Vec<InstalledExtension>,
    /// Remote plugin-market index URLs, in display order.
    ///
    /// The market fetches each URL on Refresh; nothing is written back into
    /// this list except the reader's own add/remove actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) market_indexes: Vec<String>,
    /// The automation tasks: a machine-level instruction and the session it
    /// runs in, on a cadence. A task names a session rather than belonging to
    /// one workspace, so the list is global.
    #[serde(default)]
    pub(crate) schedules: Vec<super::Schedule>,
    /// The workspace registry.
    pub(crate) workspaces: Vec<WorkspaceRef>,
    /// The local web-search tool's endpoint and per-search budget.
    #[serde(default)]
    pub(crate) web_search: WebSearchSection,
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
            skill_sources: Vec::new(),
            schedules: Vec::new(),
            mcp_servers: Vec::new(),
            providers: BTreeMap::new(),
            plugin_providers: BTreeMap::new(),
            plugin_provider_keys: BTreeMap::new(),
            plugin_provider_owners: BTreeMap::new(),
            presets: BTreeMap::new(),
            extensions: Vec::new(),
            market_indexes: Vec::new(),
            workspaces: Vec::new(),
            web_search: WebSearchSection::default(),
            default_preset: default_preset(),
        }
    }
}

/// The preset a fresh install selects.
fn default_preset() -> String {
    "standard".to_string()
}

/// The executor directories a fresh install looks in, in precedence order.
///
/// The framework's own directory leads because it is the one this build owns and
/// writes: `~/.bos/skills`. The rest are the directories other agent runtimes
/// on the machine keep their skills in, in the reference's own rank order
/// (measured in `@deepseek-ai/dsh-skill-filesystem`: user `~/.dsh/skills` 400,
/// user `~/.agents/skills` 500), followed by the wider set the pane offers.
/// Each is only a CANDIDATE: discovery keeps a source whose directory exists and
/// silently drops one that does not, so a machine that never installed Codex
/// shows no Codex card and, importantly, nothing is created.
pub(crate) const EXECUTOR_DIRS: &[(&str, &str)] = &[
    ("Bos", "~/.bos/skills"),
    ("DSH", "~/.dsh/skills"),
    ("Agents", "~/.agents/skills"),
    ("Claude", "~/.claude/skills"),
    ("Codebuddy", "~/.codebuddy/skills"),
    ("Codex", "~/.codex/skills"),
];

/// The display name of the executor that owns a directory, or the directory's
/// own last segment when it is not one of the known executors.
pub(crate) fn source_name_for(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    if let Some((name, _)) = EXECUTOR_DIRS.iter().find(|(_, p)| *p == trimmed) {
        return (*name).to_string();
    }
    trimmed
        .rsplit('/')
        .next()
        .filter(|leaf| !leaf.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// The executor directories that exist on this machine, in rank order.
///
/// Existence is the only test: a directory that is not there is not a source, so
/// a fresh install shows the framework directory (created by this build on
/// demand) and whatever else the machine actually has. Nothing is created here.
pub(crate) fn default_skill_sources() -> Vec<SkillSource> {
    EXECUTOR_DIRS
        .iter()
        .filter(|(_, path)| source_exists(path))
        .map(|(name, path)| SkillSource {
            name: (*name).to_string(),
            path: (*path).to_string(),
            enabled: true,
        })
        .collect()
}

/// Whether a stored directory path exists, expanding a leading tilde.
///
/// The framework directory counts as existing even before its first skill is
/// written, because this build owns it and the pane should show where skills
/// would go rather than an empty rail on a fresh install.
fn source_exists(path: &str) -> bool {
    let expanded = PathBuf::from(shellexpand::tilde(path).into_owned());
    expanded.is_dir() || path.trim().trim_end_matches('/') == "~/.bos/skills"
}

/// Machine preferences and window state.
///
/// None of these is workspace configuration: they are the machine's own
/// choices plus what was last on screen (`active_workspace`,
/// `active_session`), which several concurrent sessions make a view fact
/// rather than a workspace setting.
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
    /// Whether the Trajectory timeline draws each block at its actual duration.

    /// The reference keeps this in localStorage (`dsh.trajectory.duration`),
    /// so it is a preference about the reader rather than about a workspace;
    /// bsh stores it in the settings tree, which is where its other view
    /// preferences live.
    pub(crate) traj_actual_duration: bool,
    /// Whether the Trajectory timeline places blocks by real timestamp.
    pub(crate) traj_actual_time: bool,
    /// Where a link in a message opens.
    pub(crate) link_opening: String,
    /// What Enter does while a turn streams.
    pub(crate) composer_enter: String,
    /// How much performance and usage detail to show.
    pub(crate) performance_usage: String,
    /// Show the coding view: the trajectory tab and the Agent preset switch.
    pub(crate) developer_tools: bool,
    /// Whether presets apply at all: the master switch over the whole system.
    ///
    /// ABSENT means enabled, so a file written before this field existed keeps
    /// applying the preset it selects. `Some(false)` keeps the SELECTION - the
    /// session's own preset and the global default - exactly as it was while
    /// applying NONE of it: no permission ceiling, no read/write/deny scope, no
    /// preset egress intersection, no tool, skill, MCP or extension switching,
    /// no preset sandbox arm and no preset policy module. The declarations are
    /// still loaded and validated (a broken one is still a load error, so the
    /// reader can fix it); they are simply inert.
    ///
    /// One fact, one switch: the selection is never cleared to apply this, which
    /// is what makes turning presets back on restore the previous preset rather
    /// than fall back to the built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) presets_enabled: Option<bool>,
    /// Whether heel is applied at all: the master switch over the SANDBOX.
    ///
    /// ABSENT means enabled, so a file written before this field existed keeps
    /// confining the shell exactly as it did. `Some(false)` is a WIDENING that
    /// wins over every preset's `sandbox` arm: the shell and the terminal run
    /// unconfined (the bare spec), whatever a preset declares, including
    /// `sandbox = "heel"`. It does NOT switch presets off - the permission
    /// ceiling, the path scopes, the tool sets, the policy modules and
    /// `policy_mode` all still apply, the Rego tool boundary still refuses the
    /// paths and endpoints a command line names, and the browser's own loopback
    /// gate is untouched because it is not heel.
    ///
    /// A preset's `sandbox = "none"` is unaffected: it was already unconfined.
    /// At full access the switch changes nothing, so the attribution stays the
    /// permission's release rather than claiming this switch did it.
    ///
    /// One fact, one switch, and the same absent-means-on rule the preset switch
    /// uses, which is what makes an enabled file mirror a fresh one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox_enabled: Option<bool>,
    /// How the Diff pane presents a comparison: "unified" or "split".
    ///
    /// The reference's review pane carries the same two-way choice
    /// (`review.unified` / `review.split`) and keeps it for the reader rather
    /// than per file, so it is one preference here too. Unknown values fall
    /// back to unified, which is the reference's own default.
    pub(crate) diff_mode: String,
    /// Whether the Diff pane wraps long lines.
    ///
    /// The reference's third view control (`review.wrapAria`): a diff of a wide
    /// file is unreadable without it, and a diff of a narrow one is noisier
    /// with it, so the reader decides.
    pub(crate) diff_wrap: bool,
    /// The session-list ordering a reader chose.
    pub(crate) session_sort: String,
    /// The workspace that was open; empty while none is registered.
    pub(crate) active_workspace: String,
    /// The session last on screen; empty while none has been selected.
    ///
    /// A window/layout fact, not a workspace setting: several sessions in one
    /// workspace may run at the same time, and which one is being READ is the
    /// window's state. It therefore rides here with the other machine
    /// preferences, and the workspace document carries no session identity.
    pub(crate) active_session: String,
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
            traj_actual_duration: false,
            // The clock is on by default: the equal-width reading still places
            // every block at the instant it happened.
            traj_actual_time: true,
            link_opening: "sidebar".to_string(),
            composer_enter: "queue".to_string(),
            performance_usage: "detailed".to_string(),
            developer_tools: true,
            // Absent is enabled: a fresh file and every file written before the
            // master switch existed apply their preset.
            presets_enabled: None,
            // Absent is enabled: a fresh file confines the shell with heel, as
            // every file written before this switch existed does.
            sandbox_enabled: None,
            diff_mode: super::DEFAULT_DIFF_MODE.to_string(),
            diff_wrap: false,
            session_sort: String::new(),
            active_workspace: String::new(),
            active_session: String::new(),
        }
    }
}

impl General {
    /// Whether presets apply, with the absent value resolving to enabled.
    ///
    /// The ONE reader of the master switch: resolution, the pane and the
    /// composer chip all ask this instead of comparing `presets_enabled`
    /// against `Some(true)`, so "absent" cannot mean different things in
    /// different places.
    pub(crate) fn presets_active(&self) -> bool {
        self.presets_enabled.unwrap_or(true)
    }

    /// Whether heel is applied, with the absent value resolving to enabled.
    ///
    /// The ONE reader of the sandbox master switch: resolution, the General row,
    /// the preset cards, the confirmation gate and the composer all ask this
    /// instead of comparing `sandbox_enabled` against `Some(true)`, so "absent"
    /// cannot mean different things in different places.
    pub(crate) fn sandbox_active(&self) -> bool {
        self.sandbox_enabled.unwrap_or(true)
    }
}

/// Whether a bool is false, for `skip_serializing_if`.
fn is_false(value: &bool) -> bool {
    !*value
}

/// One provider: one connection, and the models reached through it.
///
/// The connection lives here rather than on each model so that a gateway is
/// declared once. Two models on one gateway share its base URL and the vault
/// entry named here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Provider {
    /// Display name. Absent shows the provider id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// The API base URL.
    pub(crate) base_url: String,
    /// The vault entry id this provider reads its API key from.
    ///
    /// The secret itself never lives in this file. When no id is named the
    /// provider falls back to the conventional `provider:<id>:api_key` entry,
    /// which is where the old migration filed a moved key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key_vault: Option<String>,
    /// A plaintext key from a settings document written before the vault became
    /// the only credential source.
    ///
    /// Accepted so an old file still parses, never written back, and never used
    /// to authenticate: a credential comes from `api_key_vault` or the
    /// conventional vault entry. It survives in memory only long enough for the
    /// session store to scrub it from transcripts.
    #[serde(rename = "api_key", default, skip_serializing)]
    pub(crate) legacy_api_key: Option<String>,
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

/// The shell sandbox a preset asks for.
///
/// This is a POLICY choice, not a build flag: heel remains the one sandbox
/// implementation and a required dependency. `Heel` asks for confinement even
/// when the session's permission would otherwise release it; `Off` asks for an
/// unconfined shell and is only permitted under a full-access ceiling, because
/// turning confinement off can otherwise only widen what the session may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum SandboxChoice {
    /// Confine shell commands with heel.
    #[serde(rename = "heel")]
    Heel,
    /// Run shell commands unconfined; requires a full-access ceiling.
    #[serde(rename = "none")]
    Off,
}

impl SandboxChoice {
    /// The value the preset's TOML uses.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Heel => "heel",
            Self::Off => "none",
        }
    }
}

impl<'de> Deserialize<'de> for SandboxChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = SandboxChoice;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("sandbox: \"heel\" or \"none\"")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "heel" => Ok(SandboxChoice::Heel),
                    "none" => Ok(SandboxChoice::Off),
                    other => Err(E::custom(format!(
                        "sandbox: expected \"heel\" or \"none\", got \"{other}\""
                    ))),
                }
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// What a policy refusal means for the call that met it.
///
/// This is a PRESET-level choice, and it is applied once, at the policy build
/// every tool's question passes through. `Required` - the default, and what
/// every preset written before this field meant - stops the call and returns
/// the refusal. `Advisory` records the same refusal in the audit and the pane's
/// warning ring, then lets the call proceed: the preset author asked to be told,
/// not obeyed.
///
/// Advisory relaxes only the modules THE PRESET contributes (its scope, its
/// egress, its named guard modules and its custom Rego). The baseline, the
/// session tier and the workspace policy are never advisory, so a preset cannot
/// use this mode to exceed the session's permission or leave the workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PolicyMode {
    /// A refusal stops the call.
    #[default]
    Required,
    /// A refusal is recorded and reported, and the call proceeds.
    Advisory,
}

impl PolicyMode {
    /// The value the preset's TOML uses.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Advisory => "advisory",
        }
    }
}

/// The highest authorization level a preset may declare.
pub(crate) const MAX_PRESET_LEVEL: u8 = 4;

/// Read a preset's `level`.
///
/// An integer 0..=4 is the documented form. The names an older declaration may
/// carry are accepted and mapped onto the same scale so an existing settings
/// file keeps loading; an unknown value is an error, never a silent default.
fn deserialize_level<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Option<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("level: an integer 0..=4")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(self)
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u8::try_from(value)
                .ok()
                .filter(|level| *level <= MAX_PRESET_LEVEL)
                .map(Some)
                .ok_or_else(|| {
                    E::custom(format!(
                        "level: expected an integer 0..={MAX_PRESET_LEVEL}, got {value}"
                    ))
                })
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if (0..=i64::from(MAX_PRESET_LEVEL)).contains(&value) {
                Ok(Some(value as u8))
            } else {
                Err(E::custom(format!(
                    "level: expected an integer 0..={MAX_PRESET_LEVEL}, got {value}"
                )))
            }
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let level = match value.trim() {
                "inspect" => 0,
                "read-only" | "review" | "research" => 1,
                "standard" | "code" | "ptc" | "creator" | "docs" => 2,
                "elevated" => 3,
                "admin" | "full-access" => 4,
                other => {
                    return Err(E::custom(format!(
                        "level: expected an integer 0..={MAX_PRESET_LEVEL} (or a legacy name), got '{other}'"
                    )));
                }
            };
            Ok(Some(level))
        }
    }
    deserializer.deserialize_option(Visitor)
}

/// One agent preset: the plugin composition a session runs under.
///
/// Measured from the reference (`@deepseek-ai/dsh-agent-preset`): the
/// declaration's fields are exactly `id`, `plugins`, `name`,
/// `description` and `order`. There is deliberately **no model, no system
/// prompt and no step cap** - the mounted plugins own the model-visible tools
/// and prompt sections, and the model is session and workspace state. A shipped
/// preset publishes no `name`: the dictionaries carry its copy.
///
/// BSH adds three fields the reference does not have: `policy`, a Rego module
/// the preset may contribute; `trusted`, the reader's explicit permission for
/// it to apply (an untrusted policy is ignored, never failed); and `sandbox`,
/// the per-preset heel choice, which may narrow to heel or widen to an
/// unconfined shell only under a full-access ceiling.
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
    /// Authorization level, 0..=4. Absent DERIVES it from what the preset
    /// actually grants ([`Preset::derived_level`]).
    ///
    /// The levels are documented in [`PRESET_LEVELS`] and in the crate's
    /// `docs/preset-authorization-plan.md`: 0 inspect grants nothing, 1
    /// read-only reads the workspace, 2 standard writes inside it with a
    /// confined shell, 3 elevated holds a full-access ceiling while the shell
    /// stays confined, and 4 full-access releases the shell. A value outside
    /// 0..=4 is a LOAD error naming the field, never a silent clamp.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_level"
    )]
    pub(crate) level: Option<u8>,
    /// The permission ceiling this preset grants: read_only,
    /// workspace_write or full_access. The session's permission is capped by
    /// this value; a preset can never raise it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permission: Option<String>,
    /// The shell sandbox this preset asks for, or none to inherit today's
    /// behaviour (heel whenever the effective permission confines).
    ///
    /// `heel` is the narrowing arm: it confines even a full-access session.
    /// `none` is the widening arm and is only permitted when the preset
    /// declares `permission = "danger-full-access"` AND the session resolves to
    /// full access; any narrower ceiling refuses the preset with rule
    /// `preset-sandbox-widens` and keeps the shell confined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sandbox: Option<SandboxChoice>,
    /// Read roots, workspace-relative. None inherits the workspace; an empty
    /// list allows no reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) read: Option<Vec<String>>,
    /// Write roots, workspace-relative. None inherits the workspace; an empty
    /// list allows no writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) write: Option<Vec<String>>,
    /// Denied path roots. Always unioned with the baseline and never widened.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deny: Vec<String>,
    /// Egress allowlist. None inherits the workspace allowlist; an empty list
    /// denies all egress; entries are intersected with the workspace list.
    ///
    /// "Inherits" is itself tri-state on the workspace side: no
    /// `.bsh/egress-allowlist` file leaves the reach UNRESTRICTED, a present
    /// file with no rule is a deliberate deny-all, and a present file with
    /// rules is the allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) egress: Option<Vec<String>>,
    /// Whether hosted web search is offered. None inherits the model setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) web_search: Option<bool>,
    /// Whether the turn may issue tool calls in parallel. None inherits the
    /// workspace/model setting; PTC sets it true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parallel_tool_calls: Option<bool>,
    /// Optional MCP tool allowlist, in namespace_tool form. None allows every
    /// tool of the allowed servers; an empty list allows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mcp_tools: Option<Vec<String>>,
    /// Binary-shipped Rego module ids. These are trusted by construction and
    /// are combined with the custom policy below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) builtin_policy: Vec<String>,
    /// Whether a policy refusal is required or advisory. Absent means
    /// required, which is what every older preset declaration meant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) policy_mode: Option<PolicyMode>,
    /// Explicit tool allowlist, or none for every tool the mounted bundles
    /// and the permission mode provide. An empty list mounts no tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tools: Option<Vec<String>>,
    /// Explicit skill allowlist, or none for every enabled skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skills: Option<Vec<String>>,
    /// Explicit MCP server allowlist (server namespaces), or none for every
    /// server the pool connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mcps: Option<Vec<String>>,
    /// Explicit installed-extension allowlist (settings ids), or none for
    /// every enabled extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) extensions: Option<Vec<String>>,
    /// The LLM persona/system prompt this preset applies, or none for the
    /// workspace prompt. When set it replaces the workspace prompt for the
    /// chats that select this preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) system_prompt: Option<String>,
    /// Whether this preset's persona IS the whole system prompt.
    ///
    /// The reference's persona row carries the same flag
    /// (`complete: true`, only its `minimal` preset sets it): the persona is
    /// restored as the sole prompt section, so the harness identity, the
    /// standing workflow and every tool-guidance section are suppressed. It is
    /// off by default, and it applies only when this preset actually ships a
    /// [`Self::system_prompt`]; otherwise the workspace persona and its own
    /// complete flag decide.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) complete: bool,
    /// A Rego module this preset contributes as a policy source.
    ///
    /// It may only NARROW the workspace/baseline policy: composition is
    /// intersection. It is ignored until [`Preset::trusted`] is true, so a
    /// preset edited into the settings file cannot silently take authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) policy: Option<String>,
    /// Whether the reader trusted this preset to contribute its policy.
    ///
    /// A preset with no policy needs no trust. The agent-presets pane writes
    /// this flag through an explicit control; the value is persisted with the
    /// preset declaration.
    #[serde(default)]
    pub(crate) trusted: bool,
}

impl Preset {
    /// The authorization level in force.
    ///
    /// A declared level wins; an absent one is DERIVED, never defaulted to a
    /// number the preset's own facts contradict.
    pub(crate) fn effective_level(&self) -> u8 {
        self.level.unwrap_or_else(|| self.derived_level())
    }

    /// Derive the level from what this preset actually grants.
    ///
    /// The derivation is deliberately conservative: it reads the DECLARATION,
    /// not a session, so the same preset grades the same way in every chat.
    ///
    /// | Level | Shipped | What it grants |
    /// |---|---|---|
    /// | 0 | - | nothing: no bundles, tools, MCP servers, extensions, reads, writes, egress or hosted search |
    /// | 1 | read-only | reads the workspace (or its declared read scope); no writes, no shell, no local egress |
    /// | 2 | standard, ptc, creator | the session tier capped by the preset: workspace writes and a confined shell |
    /// | 3 | - | a full-access CEILING with `sandbox = "heel"`: the tier is released but the shell stays confined |
    /// | 4 | - | a full-access ceiling with the shell released (`sandbox` absent or `none`) |
    pub(crate) fn derived_level(&self) -> u8 {
        let permission = self.permission.as_deref().map(str::trim);
        if permission == Some("danger-full-access") {
            return match self.sandbox {
                Some(SandboxChoice::Heel) => 3,
                _ => 4,
            };
        }
        if self.grants_nothing() {
            return 0;
        }
        if permission == Some("read-only") {
            return 1;
        }
        2
    }

    /// Whether this preset mounts nothing that can act at all.
    fn grants_nothing(&self) -> bool {
        self.plugins.is_empty()
            && self.tools.as_ref().is_none_or(Vec::is_empty)
            && self.mcps.as_ref().is_none_or(Vec::is_empty)
            && self.extensions.as_ref().is_none_or(Vec::is_empty)
            && self.read.as_ref().is_some_and(Vec::is_empty)
            && self.write.as_ref().is_some_and(Vec::is_empty)
            && self.egress.as_ref().is_some_and(Vec::is_empty)
            && self.web_search != Some(true)
    }

    /// Validate every path-scope entry in this declaration.
    ///
    /// Returns the first problem as (field, entry, why). The caller prefixes the
    /// preset id, so the load error names the declaration, the field and the
    /// value a reader has to fix. Nothing is ever dropped or defaulted.
    pub(crate) fn scope_problem(&self) -> Option<(&'static str, String, String)> {
        let fields: [(&'static str, Option<&Vec<String>>); 3] = [
            ("read", self.read.as_ref()),
            ("write", self.write.as_ref()),
            ("deny", Some(&self.deny)),
        ];
        for (field, entries) in fields {
            let Some(entries) = entries else {
                continue;
            };
            for entry in entries {
                if let Err(why) = agent::policy::validate_scope_entry(entry) {
                    return Some((field, entry.clone(), why));
                }
            }
        }
        None
    }

    /// Render this declaration as a full, annotated TOML template.
    ///
    /// Every field is present: a set field is an active assignment, an unset one
    /// is a commented example. Opening the editor and saving without changing a
    /// commented field therefore keeps "inherit" instead of silently turning it
    /// into a value, while a reader can still discover every option. The output
    /// parses back into the same declaration; a multi-line `policy` or
    /// `system_prompt` normalizes its trailing newline.
    pub(crate) fn to_toml_template(&self) -> String {
        let mut out = String::from(
            "# Custom agent preset. Every field is optional.\n\
             # A commented field is unset and inherits the workspace/default behaviour.\n\
             # Remove an assignment to inherit again. Save the preset, then Save settings.\n\
             \n[preset]\n\n",
        );

        out.push_str("# Display name and one-line description on the roster card.\n");
        push_optional_string(&mut out, "name", self.name.as_deref(), "\"Custom\"");
        push_optional_string(
            &mut out,
            "description",
            self.description.as_deref(),
            "\"What this preset is for.\"",
        );
        out.push_str(
            "# Roster order (lower first) and authorization level 0..=4.\n\
             # 0 inspect: grants nothing. 1 read-only: reads the workspace, no\n\
             # writes, no shell, no local egress. 2 standard: the session tier\n\
             # capped here, workspace writes and a confined shell (standard,\n\
             # ptc, creator). 3 elevated: a full-access ceiling with a confined\n\
             # shell. 4 full-access: a full-access ceiling with the shell\n\
             # released. Unset DERIVES the level from these fields.\n",
        );
        push_optional_order(&mut out, self.order);
        push_optional_level(&mut out, self.level);
        out.push('\n');

        out.push_str(
            "# Permission ceiling: read-only | workspace-write | danger-full-access.\n\
             # It caps the session's permission and can never raise it.\n",
        );
        push_optional_string(
            &mut out,
            "permission",
            self.permission.as_deref(),
            "\"read-only\"",
        );
        out.push_str(
            "# Shell sandbox. heel is the only sandbox implementation (a required\n\
             # dependency, with no build flag or fallback); this field chooses\n\
             # whether it is applied. \"heel\" confines commands even under full\n\
             # access. \"none\" runs SHELL commands unconfined (no kernel\n\
             # filesystem confinement and no egress allowlist) and requires\n\
             # permission = \"danger-full-access\" with a full-access session.\n\
             # Unset follows the effective permission. A narrower ceiling refuses\n\
             # sandbox = \"none\" with rule preset-sandbox-widens and keeps the\n\
             # shell confined; file tools keep their own permission and policy\n\
             # checks either way.\n",
        );
        push_optional_string(
            &mut out,
            "sandbox",
            self.sandbox.map(SandboxChoice::as_str),
            "\"heel\"",
        );
        out.push_str(
            "# Path scopes: workspace-relative or absolute paths and globs. Unset\n\
             # inherits the workspace; [] allows none. deny wins over read and\n\
             # write. A literal entry matches itself and everything below it; an\n\
             # entry with * ? [ or { is a glob (e.g. \"src/**/*.rs\"). An invalid\n\
             # entry is a load error naming the field, never a silent default.\n",
        );
        push_optional_list(&mut out, "read", self.read.as_deref(), "[\".\"]");
        push_optional_list(
            &mut out,
            "write",
            self.write.as_deref(),
            "[\"docs\", \"README.md\", \"src/**/*.rs\"]",
        );
        out.push_str("# Always denied, unioned with the baseline's .git/.bsh rules.\n");
        push_list(&mut out, "deny", &self.deny);
        out.push('\n');

        out.push_str("# Complete mounted plugin set: shell, files, plan. [] mounts nothing.\n");
        push_list(&mut out, "plugins", &self.plugins);
        out.push_str(
            "# Tool allowlist. Unset inherits the mounted bundles; [] mounts no tools.\n\
             # Vocabulary: bash, job_output, job_kill, read_file, write_file, edit_file,\n\
             # list_dir, update_plan, subagent. run_in_background follows bash.\n",
        );
        push_optional_list(
            &mut out,
            "tools",
            self.tools.as_deref(),
            "[\"read_file\", \"list_dir\", \"update_plan\"]",
        );
        out.push('\n');

        out.push_str(
            "# Network policy. Unset inherits the workspace allowlist, which is\n\
             # UNRESTRICTED when the workspace has no .bsh/egress-allowlist file; []\n\
             # denies all local egress even then. A non-empty list is INTERSECTED with\n\
             # the workspace list and never widens it. One rule per entry: an address\n\
             # or CIDR (the strong door, decided on the resolved address), a hostname\n\
             # with an optional leading \"*.\" (the weak door, decided on the name) or a\n\
             # bare port. Hosted web search is governed by web_search below.\n",
        );
        push_optional_list(&mut out, "egress", self.egress.as_deref(), "[]");
        out.push_str(
            "# Hosted web search and parallel tool calls. Unset inherits the\n\
             # model/workspace setting.\n",
        );
        push_optional_bool(&mut out, "web_search", self.web_search, "true");
        push_optional_bool(
            &mut out,
            "parallel_tool_calls",
            self.parallel_tool_calls,
            "false",
        );
        out.push('\n');

        out.push_str(
            "# Allowlists for skills, MCP servers, MCP tools (namespace_tool) and\n\
             # installed extensions. Unset inherits; [] allows none.\n",
        );
        push_optional_list(&mut out, "skills", self.skills.as_deref(), "[\"alpha\"]");
        push_optional_list(&mut out, "mcps", self.mcps.as_deref(), "[\"github\"]");
        push_optional_list(
            &mut out,
            "mcp_tools",
            self.mcp_tools.as_deref(),
            "[\"github_search\"]",
        );
        push_optional_list(
            &mut out,
            "extensions",
            self.extensions.as_deref(),
            "[\"dsh-echo\"]",
        );
        out.push('\n');

        out.push_str("# The LLM persona. Unset keeps the workspace prompt.\n");
        push_optional_text(
            &mut out,
            "system_prompt",
            self.system_prompt.as_deref(),
            "\"You are terse.\"",
        );
        out.push_str(
            "# Use the persona as the WHOLE system prompt: the harness identity,\n\
             # the standing workflow and every tool-guidance section are suppressed,\n\
             # exactly as the reference's complete persona arm does. Off is the\n\
             # default slot composition (identity < persona < workflow).\n",
        );
        push_optional_bool(&mut out, "complete", self.complete.then_some(true), "true");
        out.push('\n');

        out.push_str(
            "# Binary-shipped Rego modules: deny-secrets, read-only, no-shell,\n\
             # no-egress, docs-only, deny-git-writes.\n",
        );
        push_list(&mut out, "builtin_policy", &self.builtin_policy);
        out.push_str(
            "# Custom Rego policy (narrowing only). policy_mode: required | advisory.\n\
             # required + untrusted refuses the preset. advisory records the\n\
             # preset's own refusals in the audit and lets the call proceed; the\n\
             # baseline, the session tier and the workspace policy still refuse.\n",
        );
        push_optional_string(
            &mut out,
            "policy_mode",
            self.policy_mode.map(PolicyMode::as_str),
            "\"advisory\"",
        );
        out.push_str(&format!("trusted = {}\n", self.trusted));
        push_policy(&mut out, self.policy.as_deref());
        out
    }
}

/// One TOML basic string, quoted and escaped by the TOML serializer itself.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// One active string array.
fn push_list(out: &mut String, key: &str, values: &[String]) {
    let items: Vec<String> = values.iter().map(|item| toml_string(item)).collect();
    out.push_str(&format!("{key} = [{}]\n", items.join(", ")));
}

/// A set string, or the commented example that documents it.
fn push_optional_string(out: &mut String, key: &str, value: Option<&str>, example: &str) {
    match value {
        Some(value) => out.push_str(&format!("{key} = {}\n", toml_string(value))),
        None => out.push_str(&format!("# {key} = {example}\n")),
    }
}

/// A set array, or the commented example that documents it.
fn push_optional_list(out: &mut String, key: &str, value: Option<&[String]>, example: &str) {
    match value {
        Some(values) => push_list(out, key, values),
        None => out.push_str(&format!("# {key} = {example}\n")),
    }
}

/// A set boolean, or the commented example that documents it.
fn push_optional_bool(out: &mut String, key: &str, value: Option<bool>, example: &str) {
    match value {
        Some(value) => out.push_str(&format!("{key} = {value}\n")),
        None => out.push_str(&format!("# {key} = {example}\n")),
    }
}

/// A set level, or the commented example that documents it.
fn push_optional_level(out: &mut String, value: Option<u8>) {
    match value {
        Some(level) => out.push_str(&format!("level = {level}\n")),
        None => out.push_str("# level = 2\n"),
    }
}

/// A set order, or the commented example that documents it.
fn push_optional_order(out: &mut String, value: Option<i64>) {
    match value {
        Some(value) => out.push_str(&format!("order = {value}\n")),
        None => out.push_str("# order = 100\n"),
    }
}

/// A set string, using a readable literal block when it spans lines.
fn push_optional_text(out: &mut String, key: &str, value: Option<&str>, example: &str) {
    match value {
        Some(text) if text.contains('\n') && !text.contains("'''") => {
            out.push_str(&format!("{key} = '''\n{}\n'''\n", text.trim_end()));
        }
        Some(text) => out.push_str(&format!("{key} = {}\n", toml_string(text))),
        None => out.push_str(&format!("# {key} = {example}\n")),
    }
}

/// A set Rego module as a literal block, or the commented example to copy.
fn push_policy(out: &mut String, policy: Option<&str>) {
    match policy {
        Some(rego) if !rego.contains("'''") => {
            out.push_str(&format!("policy = '''\n{}\n'''\n", rego.trim_end()));
        }
        Some(rego) => out.push_str(&format!("policy = {}\n", toml_string(rego))),
        None => out.push_str(
            "# policy = '''\n\
             # package policy.custom\n\
             #\n\
             # # rule: custom-allow\n\
             # verdict := {\"allow\": true, \"rule\": \"custom-allow\"}\n\
             # '''\n",
        ),
    }
}

/// The presets the binary itself ships, as (id, mounted plugins).
///
/// The reference ships four - `standard`, `ptc`, `minimal` and
/// `cordis`. Only the two with backing here are shipped: `ptc` mounts
/// against a host-plane PTC runtime this deployment composes nowhere, and
/// `cordis` is a Cordis plugin composition with no counterpart, so shipping
/// either would be a name with nothing behind it.
const BUILT_IN_PRESET_IDS: &[&str] = &["standard", "ptc", "creator", "read-only"];

/// The sensitive path prefixes every shipped narrow preset denies.
fn preset_deny_paths() -> Vec<String> {
    [".git", ".bsh", ".env", "secrets", ".ssh", ".aws"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The shipped declaration for an id, or none.
///
/// Standard mounts the three core bundles and inherits every tool, skill, MCP
/// server, extension and the workspace prompt.
///
/// # The network policy of a shipped preset
///
/// Each shipped preset carries a network policy that matches what it actually
/// does, derived from the tools it keeps, not guessed:
///
/// - `standard` and `ptc` ALLOW NETWORK ACCESS BY DEFAULT: they keep the `shell`
///   bundle (`bash` can reach the network through its own client) and the MCP
///   servers, and both declare `web_search = true` (the LOCAL search tool -
///   see `crate::web`). They declare NO `egress`
///   field, which INHERITS the workspace allowlist - so the reach is the
///   workspace's own decision and the preset does not narrow it. On a FRESH
///   workspace there is no allowlist file, and the tri-state reads that as
///   UNRESTRICTED egress: that is what "allowed by default" means here. Once a
///   file exists the same inheritance is constrained by it, and a preset can
///   never widen that floor. `creator` is the same.
/// - `read-only` keeps only the read-side tools (`glob`, `grep`, `read_file`,
///   `list_dir`), `update_plan` and the two goal tools - none of which opens a
///   socket - so it declares `egress = []`, which DENIES ALL local egress. A
///   read of the workspace needs no network, and the derived proxy set is
///   empty, so the kernel denies outbound traffic with no proxy running.
///
/// The `egress` field governs every tool that opens a socket under this host -
/// the shell, MCP and the local `web_search` tool - and `web_search` decides
/// only whether the model is offered search at all. That is why `read-only` can
/// deny egress and stay coherent: it also declines the search grant.
///
/// A preset may also declare its network reach as a non-empty list: entries are
/// intersected with the workspace allowlist, never unioned with it, so a preset
/// can narrow the workspace's reach and never widen it.
pub(crate) fn built_in_preset(id: &str) -> Option<Preset> {
    let standard_bundles = vec!["shell".to_string(), "files".to_string(), "plan".to_string()];
    let standard_guard = || {
        (
            preset_deny_paths(),
            vec!["deny-secrets".to_string(), "deny-git-writes".to_string()],
        )
    };
    match id {
        "standard" => Some(Preset {
            level: Some(2),
            plugins: standard_bundles,
            deny: standard_guard().0,
            builtin_policy: standard_guard().1,
            // No `egress` field: Standard keeps the shell and the MCP servers,
            // so it ALLOWS NETWORK ACCESS by inheriting the workspace
            // allowlist. It cannot grant more than the workspace does - a
            // preset only ever narrows - so "allowed by default" means it does
            // not restrict here, not that it widens the workspace floor.
            //
            // Web search is STATED rather than left to the workspace default:
            // Standard is the everyday preset and search is part of what it is
            // for, so the Details row shows the grant instead of a reader
            // inferring it from a switch somewhere else.
            web_search: Some(true),
            order: Some(10),
            ..Preset::default()
        }),
        "ptc" => Some(Preset {
            level: Some(2),
            plugins: standard_bundles,
            deny: standard_guard().0,
            builtin_policy: standard_guard().1,
            // No `egress` field, exactly as Standard: PTC batches the same
            // tools including the shell, so it ALLOWS NETWORK ACCESS by
            // inheriting the workspace allowlist and narrows nothing.
            web_search: Some(true),
            parallel_tool_calls: Some(true),
            system_prompt: Some(crate::prompts::PTC_PERSONA.to_string()),
            order: Some(20),
            ..Preset::default()
        }),
        "creator" => Some(Preset {
            level: Some(2),
            plugins: standard_bundles,
            deny: standard_guard().0,
            builtin_policy: standard_guard().1,
            // No `egress` field: Creator authors extensions by installing
            // packages through the shell, so it needs the workspace's reach and
            // inherits it.
            web_search: Some(true),
            system_prompt: Some(crate::prompts::CREATOR_PERSONA.to_string()),
            order: Some(40),
            ..Preset::default()
        }),
        "read-only" => Some(Preset {
            level: Some(1),
            permission: Some("read-only".to_string()),
            read: Some(vec![".".to_string()]),
            write: Some(Vec::new()),
            deny: preset_deny_paths(),
            plugins: vec!["files".to_string(), "plan".to_string()],
            tools: Some(vec![
                "get_goal".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "read_file".to_string(),
                "list_dir".to_string(),
                "update_goal".to_string(),
                "update_plan".to_string(),
            ]),
            skills: Some(Vec::new()),
            mcps: Some(Vec::new()),
            extensions: Some(Vec::new()),
            // Local egress is DENIED: every tool this preset keeps reads the
            // workspace and none opens a socket, so the reach it needs is none.
            // The empty list is declared, not absent, which is the difference
            // between "no network" and "inherit the workspace"; the derived
            // proxy set is empty, so the kernel denies with no proxy running.
            // Search is OFF for the same reason: the local `web_search` tool is
            // local network access, and a preset only ever narrows.
            egress: Some(Vec::new()),
            web_search: Some(false),
            builtin_policy: vec![
                "deny-secrets".to_string(),
                "read-only".to_string(),
                "no-shell".to_string(),
                "no-egress".to_string(),
            ],
            policy_mode: Some(PolicyMode::Required),
            system_prompt: Some(crate::prompts::READ_ONLY_PERSONA.to_string()),
            order: Some(50),
            ..Preset::default()
        }),
        _ => None,
    }
}

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
    BUILT_IN_PRESET_IDS.contains(&id)
}

/// The standard an installed package was read as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InstalledStandard {
    /// A Codex plugin package.
    Codex,
    /// A DeepSeek Harness package.
    Dsh,
    /// A bare inventory entry, or a package whose markers had not been read
    /// when the record was written. The resolved view replaces this with the
    /// standard the manifest declares whenever the package loads.
    #[default]
    Local,
}

impl InstalledStandard {
    /// The label the disclosure and the market card show.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Dsh => "DeepSeek Harness",
            Self::Local => "Local",
        }
    }
}

/// One entry in the installed extension inventory.
///
/// `path` is authoritative. When it names a package directory, the manifest is
/// re-read from that directory on every build and every list refresh, so a
/// stale `name`, `version` or `description` here can never decide whether the
/// package loads. The other fields are cached metadata: they make the plugin
/// list readable without a parse per frame and keep a record readable after its
/// package directory has gone missing.
///
/// Enablement is deliberately NOT a field here. It is the workspace's own
/// `disabled.extensions` list: enabling is per workspace (a machine-wide flag
/// would contradict a workspace that switched the package off) and that same
/// list is what the agent build and the MCP derivation read. Installing,
/// uninstalling and toggling all persist the inventory and that list together
/// through `AppState::persist_extension_state`, so a reload sees the same
/// state without Save.
///
/// An old file carrying only `{ id, path }` still parses: every other field is
/// serde-defaulted, and the missing metadata is re-derived from the package on
/// first use.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledExtension {
    /// The extension id. It names a built-in bundle, or an installed package.
    pub(crate) id: String,
    /// The package name from the manifest, when known (for example
    /// `@scope/name`). For a DSH package this is also the id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) name: String,
    /// The display name from the manifest, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) display_name: String,
    /// The manifest description, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    /// The manifest version, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) version: String,
    /// The standard the package was installed as.
    #[serde(default)]
    pub(crate) standard: InstalledStandard,
    /// The market source the row came from, for example `dshmarket`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) source_label: String,
    /// The npm spec or `repository#path` the installer was given, for display.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) install_target: String,
    /// The package directory of an installed DSH or Codex extension.
    ///
    /// The ONLY field that decides whether a package can load. A path is
    /// tilde-expanded and read through `agent::extensions` on each build; the
    /// standard is detected from the package markers, so the same inventory
    /// entry serves both standards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    /// Whether the package directory was copied into BOS's managed extension
    /// root. Only a managed directory is ever deleted on uninstall; a local
    /// package registered in place is the user's and is never removed.
    #[serde(default)]
    pub(crate) managed: bool,
    /// Unix seconds the record was written.
    #[serde(default)]
    pub(crate) installed_at: u64,
    /// Whether the reader approved the third-party commands this package
    /// declares (its MCP servers).
    ///
    /// Trust is the gate between installing a stranger's package and running
    /// its child process: an untrusted package contributes no MCP server, so a
    /// fresh install - and every record written before this field existed -
    /// executes nothing. Serialized only when true, so a TOML document keeps
    /// the old shape until someone actually trusts a package.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) trusted: bool,
    /// The package directory the reader approved.
    ///
    /// Trust is per (id, path): keeping the approved path beside the flag means
    /// a record whose directory was repointed - by a reinstall elsewhere, by an
    /// update, or by hand - is untrusted again even if `trusted` survived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trusted_path: Option<String>,
}

impl InstalledExtension {
    /// Whether this package's declared commands may run.
    ///
    /// True only when the reader trusted the package AND the directory it is
    /// installed at is the one they approved. A legacy record (no field), a
    /// fresh install, and a moved directory are all untrusted.
    pub(crate) fn is_trusted(&self) -> bool {
        if !self.trusted {
            return false;
        }
        let Some(path) = self.trusted_root() else {
            return false;
        };
        let Some(current) = self.path_root() else {
            return false;
        };
        path == current
    }

    /// The stored package directory, expanded, when it names one.
    pub(crate) fn path_root(&self) -> Option<std::path::PathBuf> {
        self.path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(crate::caps::expand_path)
    }

    /// The directory the reader approved, expanded, when one is recorded.
    fn trusted_root(&self) -> Option<std::path::PathBuf> {
        self.trusted_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(crate::caps::expand_path)
    }
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
        for extension in &self.extensions {
            if extension.id.trim().is_empty() {
                return Err(SettingsError::EmptyExtention);
            }
            if !installed.insert(extension.id.clone()) {
                return Err(SettingsError::DuplicateExtention(extension.id.clone()));
            }
        }
        // A preset's level and path scopes are checked HERE, at load, so an
        // unparsable glob or an out-of-range level names the preset and the
        // field instead of quietly becoming an allow-everything or a dropped
        // deny entry.
        for (id, preset) in &self.presets {
            if let Some(level) = preset.level {
                if level > MAX_PRESET_LEVEL {
                    return Err(SettingsError::InvalidPreset {
                        preset: id.clone(),
                        why: format!(
                            "level: expected an integer 0..={MAX_PRESET_LEVEL}, got {level}"
                        ),
                    });
                }
            }
            if let Some((field, entry, why)) = preset.scope_problem() {
                return Err(SettingsError::InvalidPreset {
                    preset: id.clone(),
                    why: format!("{field}: {entry:?}: {why}"),
                });
            }
        }
        Ok(())
    }

    /// Validate one preset declaration on its own, for the editor's save path.
    ///
    /// The editor parses a `[preset]` table directly, so it never passes
    /// through [`GlobalSettings::validate`]; this is the same check with the id
    /// the reader is editing in the message.
    pub(crate) fn validate_preset(id: &str, preset: &Preset) -> Result<(), SettingsError> {
        if let Some(level) = preset.level {
            if level > MAX_PRESET_LEVEL {
                return Err(SettingsError::InvalidPreset {
                    preset: id.to_string(),
                    why: format!("level: expected an integer 0..={MAX_PRESET_LEVEL}, got {level}"),
                });
            }
        }
        match preset.scope_problem() {
            Some((field, entry, why)) => Err(SettingsError::InvalidPreset {
                preset: id.to_string(),
                why: format!("{field}: {entry:?}: {why}"),
            }),
            None => Ok(()),
        }
    }

    /// Every skill source, migrated and defaulted.
    ///
    /// Three states fold into one answer, in this order:
    ///
    /// 1. a file that still carries the retired single `skills_dir` is read as
    ///    ONE source, placed first, because it is the directory that build was
    ///    actually scanning - a migration that dropped it would lose skills;
    /// 2. a file that already names sources answers with exactly those, in
    ///    order, so a source the reader removed stays removed;
    /// 3. a file with neither is a fresh install, and the answer is the
    ///    discovered defaults.
    ///
    /// The migration is a READ, not a write: `Settings::load` never touches the
    /// disk, so opening the app cannot rewrite a configuration it merely read.
    /// The pane persists the migrated list the first time it saves.
    pub(crate) fn skill_sources(&self) -> Vec<SkillSource> {
        if !self.skill_sources.is_empty() {
            return self.skill_sources.clone();
        }
        match self
            .skills_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
        {
            Some(legacy) => {
                let mut sources = vec![SkillSource {
                    name: source_name_for(legacy),
                    path: legacy.to_string(),
                    enabled: true,
                }];
                // The other executor directories still exist beside the legacy
                // one, so the migration keeps them rather than narrowing the
                // install to the single directory an old file happened to name.
                for built in default_skill_sources() {
                    if !sources.iter().any(|s| s.path == built.path) {
                        sources.push(built);
                    }
                }
                sources
            }
            None => default_skill_sources(),
        }
    }

    /// Replace the source list outright.
    pub(crate) fn set_skill_sources(&mut self, sources: Vec<SkillSource>) {
        self.skill_sources = sources;
        // The retired field would otherwise keep re-seeding the list on the next
        // read of a file that has already been migrated.
        self.skills_dir = None;
    }

    /// The preset an id names, or none while nothing defines it.
    ///
    /// A declaration wins outright: the reference's own rule is that a user
    /// override replaces the complete child list and does not merge with it.
    pub(crate) fn preset(&self, id: &str) -> Option<Preset> {
        if let Some(declared) = self.presets.get(id) {
            return Some(declared.clone());
        }
        built_in_preset(id)
    }

    /// Every preset id on the roster, shipped ones included, in display order.
    pub(crate) fn preset_ids(&self) -> Vec<String> {
        let mut rows: Vec<(i64, String)> = self
            .presets
            .iter()
            .map(|(id, preset)| (preset.order.unwrap_or(0), id.clone()))
            .collect();
        for id in BUILT_IN_PRESET_IDS {
            if !self.presets.contains_key(*id) {
                let order = built_in_preset(id)
                    .and_then(|preset| preset.order)
                    .unwrap_or(0);
                rows.push((order, (*id).to_string()));
            }
        }
        rows.sort();
        rows.into_iter().map(|(_, id)| id).collect()
    }

    /// The provider a reference names, with the model entry it points at.
    ///
    /// The reader's own tree wins: a contributed provider is consulted only
    /// when no hand-declared one carries the id, so a plugin can never shadow a
    /// provider the reader wrote by hand. A half-made reference (a provider
    /// with no such model) resolves to none either way, which is the state a
    /// session reports as unresolvable rather than silently replacing.
    pub(crate) fn lookup(&self, reference: &ModelRef) -> Option<(&Provider, &ModelEntry)> {
        let provider = self
            .providers
            .get(&reference.provider)
            .or_else(|| self.plugin_providers.get(&reference.provider))?;
        let entry = provider.models.get(&reference.model)?;
        Some((provider, entry))
    }

    /// The provider node an id names, from the reader's tree or a plugin's.
    ///
    /// This is the one accessor a surface that only wants the node uses, so
    /// "is this provider real" has a single answer that already accounts for
    /// contributions.
    pub(crate) fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers
            .get(id)
            .or_else(|| self.plugin_providers.get(id))
    }

    /// Whether an id names a provider a plugin contributed.
    pub(crate) fn is_plugin_provider(&self, id: &str) -> bool {
        !self.providers.contains_key(id) && self.plugin_providers.contains_key(id)
    }

    /// The extension id that contributed a provider, when one did.
    pub(crate) fn plugin_provider_owner(&self, id: &str) -> Option<&str> {
        self.plugin_provider_owners.get(id).map(String::as_str)
    }

    /// Whether a provider needs a credential.
    ///
    /// A contributed provider answers with what its package declared; every
    /// other provider is key-bearing, because the reader's own tree is entered
    /// through the form that requires one.
    pub(crate) fn provider_requires_key(&self, id: &str) -> bool {
        if self.is_plugin_provider(id) {
            return self.plugin_provider_keys.get(id).copied().unwrap_or(true);
        }
        true
    }

    /// Replace the contributed provider set in one shot.
    ///
    /// Called once per extension-inventory load, never per frame: the three
    /// maps are rebuilt together so a provider is never momentarily visible
    /// without its owner or its key policy.
    pub(crate) fn set_plugin_providers(
        &mut self,
        providers: BTreeMap<String, Provider>,
        keys: BTreeMap<String, bool>,
        owners: BTreeMap<String, String>,
    ) {
        self.plugin_providers = providers;
        self.plugin_provider_keys = keys;
        self.plugin_provider_owners = owners;
    }

    /// Every provider id, the reader's first then the contributed ones.
    ///
    /// Order is the contract: the panes and the composer draw this, and a
    /// plugin's rows must not displace the reader's own.
    pub(crate) fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.providers.keys().cloned().collect();
        ids.extend(
            self.plugin_providers
                .keys()
                .filter(|id| !self.providers.contains_key(*id))
                .cloned(),
        );
        ids
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
api_key_vault = "keys/nvidia"

[providers.nvidia."glm-5.3-flash"]
model = "nvidia/z-ai/glm-5.3-flash"
api_mode = "chat"
input_types = ["text", "image"]

[providers.nvidia."kimi-k3"]
model = "nvidia/moonshotai/kimi-k3"

[[extensions]]
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
        assert_eq!(provider.api_key_vault.as_deref(), Some("keys/nvidia"));
    }

    /// A document written before the vault landed still parses, but its
    /// plaintext key is never written back and never becomes the credential.
    #[test]
    fn a_legacy_plaintext_key_is_read_then_dropped() {
        let text =
            "[providers.legacy]\nbase_url = \"https://legacy.example\"\napi_key = \"sk-legacy\"\n";
        let settings = GlobalSettings::from_toml(text).expect("the old document parses");
        let provider = settings.providers.get("legacy").expect("the provider");
        assert_eq!(provider.legacy_api_key.as_deref(), Some("sk-legacy"));
        assert_eq!(provider.api_key_vault, None);
        let rendered = settings.to_toml().expect("renders");
        assert!(
            !rendered
                .lines()
                .any(|line| line.trim_start().starts_with("api_key =")),
            "the plaintext field is not written back: {rendered}"
        );
        assert!(!rendered.contains("sk-legacy"));
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

    /// The master preset switch: absent means ENABLED (so every file written
    /// before the field existed keeps applying its preset), an explicit false
    /// survives a save/load restart, and turning it back on writes the field
    /// away so the file reads exactly like a fresh one.
    #[test]
    fn the_presets_switch_defaults_to_enabled_and_survives_a_restart() {
        let fresh = GlobalSettings::from_toml("").expect("an empty file parses");
        assert_eq!(fresh.general.presets_enabled, None);
        assert!(
            fresh.general.presets_active(),
            "a fresh settings file defaults to enabled"
        );
        // An older file that names no field is enabled for the same reason.
        let old = GlobalSettings::from_toml("[general]\nlang = \"zh\"\n").expect("parses");
        assert!(old.general.presets_active());

        let mut switched_off = fresh;
        switched_off.general.presets_enabled = Some(false);
        let text = switched_off.to_toml().expect("renders");
        assert!(
            text.contains("presets_enabled = false"),
            "the switch is written where a reader can see it:\n{text}"
        );
        let path = scratch("presets-switch").join("settings.toml");
        switched_off.save(&path).expect("saves");
        let reloaded = GlobalSettings::load(&path).expect("loads");
        assert_eq!(reloaded.general.presets_enabled, Some(false));
        assert!(
            !reloaded.general.presets_active(),
            "the flag survives a restart"
        );
        // The preset DECLARATIONS are still loaded and validated while off, so
        // the reader can fix a broken one.
        assert_eq!(reloaded.default_preset, "standard");
        assert!(reloaded.preset("read-only").is_some());

        // On again is ABSENT again: the file reads like a fresh install.
        let mut switched_on = reloaded;
        switched_on.general.presets_enabled = None;
        let text = switched_on.to_toml().expect("renders");
        assert!(
            !text.contains("presets_enabled"),
            "an enabled file carries no switch field:\n{text}"
        );
        assert_eq!(
            GlobalSettings::from_toml(&text).expect("re-parses"),
            switched_on
        );
    }

    /// The sandbox master switch, with the same three properties the preset
    /// switch has: absent means ON, an explicit false survives a restart, and
    /// turning it back on writes the field away so an enabled file is
    /// byte-identical in meaning to a fresh one.
    #[test]
    fn the_sandbox_switch_defaults_to_on_and_survives_a_restart() {
        let fresh = GlobalSettings::from_toml("").expect("an empty file parses");
        assert_eq!(fresh.general.sandbox_enabled, None);
        assert!(
            fresh.general.sandbox_active(),
            "a fresh settings file applies the sandbox"
        );
        // An older file that names no field applies it for the same reason.
        let old = GlobalSettings::from_toml("[general]\nlang = \"zh\"\n").expect("parses");
        assert!(old.general.sandbox_active());

        let mut switched_off = fresh;
        switched_off.general.sandbox_enabled = Some(false);
        let text = switched_off.to_toml().expect("renders");
        assert!(
            text.contains("sandbox_enabled = false"),
            "the switch is written where a reader can see it:\n{text}"
        );
        let path = scratch("sandbox-switch").join("settings.toml");
        switched_off.save(&path).expect("saves");
        let reloaded = GlobalSettings::load(&path).expect("loads");
        assert_eq!(reloaded.general.sandbox_enabled, Some(false));
        assert!(
            !reloaded.general.sandbox_active(),
            "the switch survives a restart"
        );
        // The PRESET switch is a different fact and is untouched by this one.
        assert!(
            reloaded.general.presets_active(),
            "turning the sandbox off must not turn presets off"
        );

        // On again is ABSENT again: the file reads like a fresh install.
        let mut switched_on = reloaded;
        switched_on.general.sandbox_enabled = None;
        let text = switched_on.to_toml().expect("renders");
        assert!(
            !text.contains("sandbox_enabled"),
            "an enabled file carries no switch field:\n{text}"
        );
        assert_eq!(
            GlobalSettings::from_toml(&text).expect("re-parses"),
            switched_on
        );
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
presets_enabled = false
sandbox_enabled = false
session_sort = "name"
active_workspace = "w1"

[providers.nvidia]
base_url = "https://integrate.api.nvidia.com/v1"
api_key_vault = "keys/nvidia"

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

[[extensions]]
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

    /// An installed market plugin keeps its package path through the file, so a
    /// restart still knows what to load.
    #[test]
    fn an_installed_extension_path_round_trips() {
        let mut settings = GlobalSettings::default();
        settings.extensions.push(InstalledExtension {
            id: "codex-deploy".to_string(),
            name: "codex-deploy".to_string(),
            display_name: "Codex Deploy".to_string(),
            description: "Deploys things in tests.".to_string(),
            version: "0.4.0".to_string(),
            standard: InstalledStandard::Codex,
            source_label: "Codex Marketplace".to_string(),
            install_target: "github.example/codex-deploy#plugins/deploy".to_string(),
            path: Some("/tmp/codex-deploy".to_string()),
            managed: true,
            installed_at: 1_700_000_000,
            trusted: false,
            trusted_path: None,
        });
        settings
            .market_indexes
            .push("https://market.example/plugins.json".to_string());
        let text = settings.to_toml().expect("the document");
        assert!(text.contains("[[extensions]]"), "{text}");
        assert!(text.contains("path = \"/tmp/codex-deploy\""), "{text}");
        assert!(
            text.contains("https://market.example/plugins.json"),
            "{text}"
        );
        let read = GlobalSettings::from_toml(&text).expect("it parses back");
        assert_eq!(read.extensions, settings.extensions);
        assert_eq!(read.market_indexes, settings.market_indexes);
    }

    /// An inventory written before the record carried metadata still parses.
    /// The only authoritative field - the path - survives; everything else
    /// defaults and is re-derived from the package on first use, and the
    /// workspace disabled list (not the record) decides whether it is on.
    #[test]
    fn a_legacy_id_and_path_extension_still_loads() {
        let text = "[[extensions]]\nid = \"shell\"\npath = \"/tmp/shell\"\n";
        let settings = GlobalSettings::from_toml(text).expect("the old document parses");
        let entry = &settings.extensions[0];
        assert_eq!(entry.id, "shell");
        assert_eq!(entry.path.as_deref(), Some("/tmp/shell"));
        assert_eq!(entry.name, "");
        assert_eq!(entry.version, "");
        assert_eq!(entry.standard, InstalledStandard::Local);
        assert!(!entry.managed, "an old entry is not assumed managed");
        assert_eq!(entry.installed_at, 0);
        assert!(!entry.is_trusted(), "an old record loads untrusted");
    }

    /// A fresh record is untrusted, and the flag is written only once someone
    /// trusts a package: the document keeps the pre-trust shape until then.
    #[test]
    fn an_extension_is_untrusted_until_the_flag_is_written() {
        let mut settings = GlobalSettings::default();
        settings.extensions.push(InstalledExtension {
            id: "mcp-fixture".to_string(),
            path: Some("/tmp/mcp-fixture".to_string()),
            ..InstalledExtension::default()
        });
        assert!(!settings.extensions[0].is_trusted());
        let text = settings.to_toml().expect("the document");
        assert!(
            !text.contains("trusted"),
            "an untrusted record does not write the field: {text}"
        );

        settings.extensions[0].trusted = true;
        settings.extensions[0].trusted_path = Some("/tmp/mcp-fixture".to_string());
        assert!(settings.extensions[0].is_trusted());
        let text = settings.to_toml().expect("the document");
        assert!(text.contains("trusted = true"), "{text}");
        assert!(
            text.contains("trusted_path = \"/tmp/mcp-fixture\""),
            "{text}"
        );
        let read = GlobalSettings::from_toml(&text).expect("it parses back");
        assert!(read.extensions[0].is_trusted());
    }

    /// Trust is bound to the approved directory: repointing the record at
    /// another package makes it untrusted again, even if the flag survives.
    #[test]
    fn moving_an_extension_clears_its_trust() {
        let mut entry = InstalledExtension {
            id: "mcp-fixture".to_string(),
            path: Some("/tmp/mcp-fixture-source".to_string()),
            trusted: true,
            trusted_path: Some("/tmp/mcp-fixture-source".to_string()),
            ..InstalledExtension::default()
        };
        assert!(entry.is_trusted(), "the premise: trusted at its path");
        entry.path = Some("/tmp/mcp-fixture-other".to_string());
        assert!(
            !entry.is_trusted(),
            "a path change is a different package to approve"
        );
        entry.path = None;
        assert!(!entry.is_trusted(), "a record with no path is untrusted");
        entry.path = Some("/tmp/mcp-fixture-source".to_string());
        assert!(entry.is_trusted(), "the original approval still holds");
    }

    #[test]
    fn the_global_path_is_the_settings_file_in_the_data_directory() {
        assert_eq!(
            GlobalSettings::path(),
            crate::settings::dir().join("settings.toml")
        );
    }

    /// A provider whose wire is not the shipped shape is a settings edit, not
    /// a recompile: the Tavily section from the settings spec parses, and the
    /// fields the tool asks are exactly the ones the section names.
    #[test]
    fn a_tavily_web_search_section_parses_and_keeps_its_shape() {
        let text = r#"
schema_version = 1

[web_search]
base_url = "https://api.tavily.com"
results_per_search = 5
method = "POST"
path = "search"
query_key = "query"
count_key = ""
snippet_key = "content"
date_key = "published_date"

[web_search.body]
search_depth = "advanced"
"#;
        let settings = GlobalSettings::from_toml(text).expect("the section parses");
        let section = &settings.web_search;
        assert_eq!(section.base_url, "https://api.tavily.com");
        assert_eq!(section.results_per_search, 5);
        assert_eq!(section.method, "POST");
        assert_eq!(section.count_key, "", "Tavily defines no count field");
        assert_eq!(section.snippet_key, "content");
        assert_eq!(section.date_key, "published_date");
        assert_eq!(
            section
                .body
                .get("search_depth")
                .and_then(serde_json::Value::as_str),
            Some("advanced")
        );
        // And an absent section is the shipped wire: defaults, not zeros.
        let defaults = GlobalSettings::from_toml("schema_version = 1").expect("bare parses");
        assert_eq!(
            defaults.web_search,
            WebSearchSection::default(),
            "a file that names no shape speaks the shipped wire"
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

    /// One TOML file wrapper, so a template can be parsed as the editor does.
    #[derive(Debug, serde::Deserialize)]
    struct TemplateFile {
        preset: Preset,
    }

    /// The editor's template documents every field, and a fully populated
    /// declaration survives a render/parse round trip. Multi-line strings
    /// normalize their trailing newline by construction.
    #[test]
    fn the_preset_template_lists_every_field_and_round_trips() {
        let preset = Preset {
            plugins: vec!["shell".to_string(), "files".to_string()],
            name: Some("Full".to_string()),
            description: Some("All fields set".to_string()),
            order: Some(7),
            level: Some(2),
            permission: Some("workspace-write".to_string()),
            sandbox: Some(SandboxChoice::Heel),
            read: Some(vec![".".to_string()]),
            write: Some(vec!["docs".to_string()]),
            deny: vec![".git".to_string()],
            egress: Some(vec!["1.2.3.4".to_string()]),
            web_search: Some(false),
            parallel_tool_calls: Some(true),
            mcp_tools: Some(vec!["github_search".to_string()]),
            builtin_policy: vec!["deny-secrets".to_string()],
            policy_mode: Some(PolicyMode::Advisory),
            tools: Some(vec!["read_file".to_string(), "subagent".to_string()]),
            skills: Some(vec!["alpha".to_string()]),
            mcps: Some(vec!["github".to_string()]),
            extensions: Some(vec!["dsh-echo".to_string()]),
            system_prompt: Some("You are terse.\nAnswer briefly.".to_string()),
            complete: true,
            policy: Some(
                "package policy.custom\n\n# rule: custom-allow\nverdict := {\"allow\": true, \"rule\": \"custom-allow\"}\n"
                    .to_string(),
            ),
            trusted: true,
        };
        let text = preset.to_toml_template();
        for key in [
            "name",
            "description",
            "order",
            "level",
            "permission",
            "sandbox",
            "read",
            "write",
            "deny",
            "plugins",
            "tools",
            "egress",
            "web_search",
            "parallel_tool_calls",
            "skills",
            "mcps",
            "mcp_tools",
            "extensions",
            "system_prompt",
            "complete",
            "builtin_policy",
            "policy_mode",
            "trusted",
            "policy",
        ] {
            assert!(
                text.lines().any(|line| {
                    line.trim_start_matches("# ")
                        .trim_start()
                        .starts_with(&format!("{key} ="))
                }),
                "the template must document {key}:\n{text}"
            );
        }
        let parsed: TemplateFile = toml::from_str(&text).expect("the template parses");
        let normalized =
            |value: &Option<String>| value.as_ref().map(|text| format!("{}\n", text.trim_end()));
        let mut expected = preset.clone();
        expected.system_prompt = normalized(&preset.system_prompt);
        expected.policy = normalized(&preset.policy);
        assert_eq!(parsed.preset, expected, "the template round-trips:\n{text}");
    }

    /// A commented field is what keeps "inherit" intact: a default preset
    /// renders every optional field as a comment, and parses back unchanged.
    #[test]
    fn the_preset_template_keeps_unset_fields_unset() {
        let preset = Preset::default();
        let text = preset.to_toml_template();
        for key in [
            "name",
            "description",
            "order",
            "level",
            "permission",
            "sandbox",
            "read",
            "write",
            "egress",
            "web_search",
            "parallel_tool_calls",
            "mcp_tools",
            "policy_mode",
            "tools",
            "skills",
            "mcps",
            "extensions",
            "system_prompt",
            "complete",
            "policy",
        ] {
            assert!(
                text.lines()
                    .any(|line| line.trim_start().starts_with(&format!("# {key} ="))),
                "an unset {key} must stay a commented example:\n{text}"
            );
        }
        for key in ["plugins", "deny", "builtin_policy", "trusted"] {
            assert!(
                text.lines()
                    .any(|line| line.trim_start().starts_with(&format!("{key} ="))),
                "{key} has a concrete default and must be active:\n{text}"
            );
        }
        let parsed: TemplateFile = toml::from_str(&text).expect("the template parses");
        assert_eq!(parsed.preset, preset);
    }

    /// A sandbox value outside the two names is a LOAD error naming the field.
    ///
    /// The alternative - silently treating an unknown word as "inherit" - would
    /// turn a misspelled `none` into a confined shell or a misspelled `heel`
    /// into an unconfined one, so the parser refuses by name.
    #[test]
    fn a_malformed_sandbox_value_is_a_load_error_naming_the_field() {
        let text = "[presets.custom]\nplugins = []\nsandbox = \"seatbelt\"\n";
        let error = GlobalSettings::from_toml(text).expect_err("seatbelt is not a sandbox");
        let message = error.to_string();
        assert!(message.contains("sandbox"), "{message}");
        assert!(message.contains("seatbelt"), "{message}");
        assert!(
            message.contains("heel") && message.contains("none"),
            "the error names both accepted values: {message}"
        );

        // The preset editor's own wrapper parses the same field, so its error
        // carries the same field name rather than a bare enum message.
        let editor = toml::from_str::<TemplateFile>("[preset]\nsandbox = \"seatbelt\"\n")
            .expect_err("the editor refuses the same value");
        let message = editor.to_string();
        assert!(message.contains("sandbox"), "{message}");
        assert!(message.contains("seatbelt"), "{message}");
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
