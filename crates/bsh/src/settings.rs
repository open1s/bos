//! Persisted harness settings: the files the shell reads and writes.
//!
//! Two documents, each the truth for what it owns. The global file
//! (`~/.bos/bsh/settings.toml`) holds everything that is the same whichever
//! workspace is open: appearance, language, the provider tree, the preset and
//! extension inventories, the workspace registry and the task list. One file per
//! workspace (`<root>/.bsh/settings/settings.toml`) holds that workspace's own
//! agent configuration, and one session file per session holds which model and
//! preset that session last used.
//!
//! This module is the seam the panes edit. Nothing here is a projection, so a
//! round trip through the UI cannot drop a field a pane does not draw: what a
//! pane writes is what the file holds.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) mod global;
pub(crate) mod mcp;
pub(crate) mod provider;
pub(crate) mod resolve;
pub(crate) mod workspace;

use global::GlobalSettings;
use workspace::WorkspaceSettings;

// The automation tasks, the clock they read and their refusals live on the
// global document, because a task is a machine-level instruction that names
// a session rather than belonging to one workspace.
pub(crate) use self::global::{unix_now, Schedule, ScheduleRefusal};

/// The harness settings, held as the two documents they are stored in.
///
/// The panes edit this structure directly: a pane that changes a provider edits
/// [`GlobalSettings::providers`], and a pane that changes how one workspace
/// behaves edits [`WorkspaceSettings::agent`]. Nothing here is a projection back
/// out of these trees, so a round trip through the UI cannot drop a field a pane
/// does not draw - what a pane writes is what the file holds.
///
/// The one computed value is [`Settings::effective`], which folds the two trees
/// into the concrete numbers a build uses. That is a derivation, not a second
/// storage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    /// The global document: preferences, the provider tree, the preset and
    /// extension inventories, and the workspace registry.
    #[serde(default)]
    pub(crate) global: GlobalSettings,
    /// The active workspace's document: what it selects and how it configures
    /// the agent here.
    #[serde(default)]
    pub(crate) workspace: WorkspaceSettings,
    /// The directory the global document is written to.
    ///
    /// This is a location, never a setting: serde skips it so the file never
    /// claims to live somewhere else, and a test points it at its own scratch
    /// directory. Without it a test that changes a preference writes the user's
    /// real configuration, which is how a shell ends up confined to a
    /// temporary directory a test made.
    #[serde(skip)]
    pub(crate) save_root: Option<PathBuf>,
    /// The last scanned skill catalogue, keyed by the settings that name it.
    ///
    /// Derivation, not storage: the scan's inputs are the two skill-directory
    /// lists and the workspace's switched-off names, and the cache compares a
    /// fingerprint of exactly those on every read. It exists because a
    /// resolution runs several times per frame and each one used to walk every
    /// skill directory and parse every `SKILL.md` - the synchronous disk read
    /// that made a keystroke lag.
    #[serde(skip)]
    pub(crate) skill_cache: resolve::SkillCatalogueCache,
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
    /// Workspace root.
    pub(crate) root: String,
    /// The provider id the model came from, which is the wire prefix the
    /// engine registers the vendor under.
    pub(crate) provider: String,
    /// The model reference that resolved, or none while nothing is chosen.
    pub(crate) model_ref: Option<global::ModelRef>,
    /// Model identifier, as the API is called with it.
    pub(crate) model: String,
    /// The name the chip and its menu show for the resolved choice.
    ///
    /// The leaf's display name when it declares one, else the nickname the
    /// menu rows carry, so the label and the checked row never describe two
    /// different things. Empty while nothing is chosen.
    pub(crate) model_name: String,
    /// API base URL.
    pub(crate) base_url: String,
    /// The vault entry the provider named, if any.
    pub(crate) api_key_vault: Option<String>,
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
    /// Offer the model a hosted web-search tool.
    pub(crate) web_tools_enabled: bool,
    /// The session's permission capped by the preset ceiling.
    ///
    /// The one resolution the chip, the policy data and the audit read. The
    /// requested value is kept beside the effective one so the chip can explain
    /// a ceiling and the menu can check the reader's own choice.
    pub(crate) permission: resolve::PermissionResolution,
    /// Every skill source in precedence order, with its count and existence.
    ///
    /// Switched-OFF sources are kept here so the pane can draw them and switch
    /// them back on; \`enabled_skills\` is what a build reads.
    pub(crate) skill_sources: Vec<resolve::ResolvedSource>,
    /// The discovered, deduplicated catalogue: every skill, with its source and
    /// its own on/off flag. The pane lists this and the build registers from it.
    ///
    /// Shared with the cache, so a frame's several resolutions clone a pointer
    /// instead of the catalogue.
    pub(crate) skills: std::sync::Arc<Vec<resolve::DiscoveredSkill>>,
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
    /// The plugin ids the preset mounts. The complete set, not a delta.
    pub(crate) preset_plugins: Vec<String>,
    /// The preset id in effect (empty when no preset is selected). This is the
    /// authority half of a policy principal: a preset may narrow what a turn
    /// may touch, and the policy input has to carry which preset was active.
    pub(crate) preset_id: String,
    /// Permission ceiling, or none to inherit the session's permission.
    pub(crate) preset_permission: Option<String>,
    /// Read roots; none inherits the workspace, empty allows nothing.
    pub(crate) preset_read: Option<Vec<String>>,
    /// Write roots; none inherits the workspace, empty allows nothing.
    pub(crate) preset_write: Option<Vec<String>>,
    /// Denied path roots, always unioned.
    pub(crate) preset_deny: Vec<String>,
    /// Egress allowlist; none inherits, empty denies, entries intersect.
    pub(crate) preset_egress: Option<Vec<String>>,
    /// Hosted web-search override.
    pub(crate) preset_web_search: Option<bool>,
    /// Parallel tool-call override.
    pub(crate) preset_parallel_tool_calls: Option<bool>,
    /// MCP tool allowlist, or none for every tool of the allowed servers.
    pub(crate) preset_mcp_tools: Option<Vec<String>>,
    /// Binary-shipped Rego module ids.
    pub(crate) preset_builtin_policy: Vec<String>,
    /// required or advisory custom-policy mode.
    pub(crate) preset_policy_mode: Option<String>,
    /// Explicit tool allowlist, or none for all bundle tools.
    pub(crate) preset_tools: Option<Vec<String>>,
    /// Explicit skill allowlist, or none for every enabled skill.
    pub(crate) preset_skills: Option<Vec<String>>,
    /// Explicit MCP server allowlist, or none for every connected server.
    pub(crate) preset_mcps: Option<Vec<String>>,
    /// Explicit installed-extension allowlist, or none for every enabled one.
    pub(crate) preset_extensions: Option<Vec<String>>,
    /// The preset's LLM persona, or none for the workspace prompt.
    pub(crate) preset_system_prompt: Option<String>,
    /// The Rego module the active preset contributes, when it has one.
    pub(crate) preset_policy: Option<String>,
    /// Whether the active preset's policy is trusted to apply.
    pub(crate) preset_trusted: bool,
}

impl Effective {
    /// A required custom policy that the reader has not trusted: the preset
    /// cannot be applied, because applying it without the policy would be
    /// wider than the preset declares.
    pub(crate) fn preset_policy_required_untrusted(&self) -> bool {
        self.preset_policy_mode.as_deref() == Some("required")
            && self.preset_policy.is_some()
            && !self.preset_trusted
    }

    /// The prompt the LLM call receives: the selected preset's persona when it
    /// has one, otherwise the workspace prompt. The build and the context meter
    /// read this one value so they cannot disagree.
    pub(crate) fn llm_system_prompt(&self) -> &str {
        self.preset_system_prompt
            .as_deref()
            .unwrap_or(&self.system_prompt)
    }

    /// The skills a build should register: the discovered catalogue with the
    /// switched-off ones removed.
    ///
    /// This is the whole of the skill gate. A skill whose source is off never
    /// reaches the catalogue at all (the scan skips it), and a skill switched off
    /// on its own is dropped here, so the agent can only receive a directory that
    /// survived both flags.
    pub(crate) fn enabled_skills(&self) -> impl Iterator<Item = &resolve::DiscoveredSkill> {
        let allowed = self.preset_skills.as_deref();
        self.skills.iter().filter(move |skill| {
            skill.enabled
                && allowed.is_none_or(|names| names.iter().any(|name| name == &skill.name))
        })
    }
}

/// The harness data directory.
pub(crate) fn dir() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~/.bos/bsh").into_owned())
}

/// The settings file path.
///
/// One definition, owned by the type that reads and writes it, so the path the
/// settings model uses and the path the reveal action opens cannot drift apart.
pub(crate) fn settings_path() -> PathBuf {
    global::GlobalSettings::path()
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

/// One config error as something a caller in the io world can carry.
fn as_io(error: global::SettingsError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Move a document this build cannot read out of the live directory.
///
/// The live directory holds exactly the documents the app reads, so a file it
/// has just refused to parse must not keep living there under a name that looks
/// like one of them. It used to be renamed to `settings.toml.bak` in place -
/// an old-format document beside the live one for good, and every later reader
/// had to work out which was which.
///
/// It goes to `archive/legacy-flat/` with the epoch in its name, because two
/// runs on two days are two different records and neither should overwrite the
/// other. Nothing is deleted: the archive is the one place a lost configuration
/// can still be recovered from.
fn archive_unreadable(path: &std::path::Path) {
    let Some(parent) = path.parent() else {
        let _ = std::fs::rename(path, path.with_extension("toml.old"));
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let dir = parent.join("archive").join("legacy-flat");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::rename(path, dir.join(format!("settings-flat-{stamp}.toml.old")));
}

/// The root the global file's active workspace points at, empty while none does.
fn active_root(global: &GlobalSettings) -> String {
    global
        .workspaces
        .iter()
        .find(|w| w.id == global.general.active_workspace)
        .map(|w| w.root.clone())
        .unwrap_or_default()
}

impl Settings {
    /// Read the two documents, and do nothing else.
    ///
    /// The files are the truth. The global file carries the machine
    /// preferences, the provider tree, the preset declarations, the extension
    /// inventory, the workspace registry and the task list; one file per
    /// workspace carries that workspace's own agent configuration. Both are read
    /// whole, and this structure holds them whole, so what a pane writes is what
    /// the file holds on the way back out.
    ///
    /// Nothing is written on the way in. A first run leaves the disk alone, so
    /// asking what the configuration is can never destroy it.
    pub(crate) fn load() -> Self {
        let global = GlobalSettings::load(&GlobalSettings::path()).unwrap_or_default();
        let root = active_root(&global);
        let workspace =
            WorkspaceSettings::load(&WorkspaceSettings::path(std::path::Path::new(&root)))
                .unwrap_or_default();
        let mut read = Self {
            global,
            workspace,
            // The settings were read from their own location, so that is where
            // they are written back to.
            save_root: None,
            // Nothing has been scanned yet; the first resolution fills it.
            skill_cache: resolve::SkillCatalogueCache::default(),
        };
        read.adopt_legacy_session();
        read.ensure_workspace();
        read
    }

    /// Bring a retired workspace `active_session` into the machine preference.
    ///
    /// Which session is on screen is a window fact - several sessions in one
    /// workspace can run at once - so it lives on the global document now. An
    /// older workspace file still names one; this carries that answer over
    /// once, and never lets it overwrite a session the reader has since opened.
    fn adopt_legacy_session(&mut self) {
        if !self.global.general.active_session.trim().is_empty() {
            return;
        }
        let legacy = self.workspace.general.legacy_active_session.trim();
        if !legacy.is_empty() {
            self.global.general.active_session = legacy.to_string();
        }
    }

    /// Write both documents back, each whole.
    ///
    /// No round trip goes through a projection, so there is nothing to merge:
    /// the whole tree is written and the whole tree comes back. A file that
    /// exists but does not parse is moved aside rather than overwritten - that
    /// is not a migration and does not try to read the old format; it is the one
    /// guard that keeps a redesign from silently eating a configuration it
    /// cannot read.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        // The documents may now name directories the cached catalogue was not
        // scanned from, and a skill install may have moved files under an
        // unchanged list, so the next resolution re-reads the catalogue.
        self.invalidate_skills();
        let global_path = self.writable_global_path()?;
        let root = self.active_workspace_root();
        let workspace = self.workspace.clone();
        self.global.validate().map_err(as_io)?;
        self.global.save(&global_path).map_err(as_io)?;
        if root.is_empty() {
            // Nothing to place is nothing to report. Something to place with no
            // root is a loss the reader has to see: the dialog would otherwise
            // say "saved" about a document that never landed anywhere, and the
            // only copy of that workspace's configuration would be the window
            // that just closed.
            if workspace == WorkspaceSettings::default() {
                return Ok(());
            }
            return Err(std::io::Error::other(
                "the active workspace has no root, so its settings were not written",
            ));
        }
        workspace
            .save(&WorkspaceSettings::path(std::path::Path::new(&root)))
            .map_err(as_io)?;
        Ok(())
    }

    /// Write the machine preferences alone.
    ///
    /// Selecting a session changes a WINDOW fact - which chat is on screen - and
    /// the workspace document no longer carries session identity, so there is no
    /// reason to rewrite that file every time the reader switches chats.
    pub(crate) fn save_global(&self) -> std::io::Result<()> {
        self.invalidate_skills();
        let global_path = self.writable_global_path()?;
        self.global.validate().map_err(as_io)?;
        self.global.save(&global_path).map_err(as_io)
    }

    /// The path a machine preference is written to, after the write guards.
    ///
    /// Every global write goes through here, so the test guard and the
    /// unreadable-file archive cannot be forgotten by one of the writers.
    fn writable_global_path(&self) -> std::io::Result<PathBuf> {
        // A TEST BUILD MUST NEVER WRITE THE REAL CONFIGURATION.
        //
        // Every test redirects the write by setting save_root to its scratch
        // store, and this is the belt behind that brace: one checkout's test run
        // shares $HOME with every other checkout on the machine, so a single
        // forgotten redirect does not merely fail a test - it overwrites the
        // reader's actual settings with a fixture. That is not hypothetical: a
        // test run in a second worktree wrote this file while it was being
        // repaired, replacing the reader's provider with a "work.example"
        // fixture. The guard costs one comparison and makes the class
        // impossible in this build.
        if cfg!(test) && self.save_root.is_none() {
            return Err(std::io::Error::other(
                "a test build has no save_root and must not write the real settings file",
            ));
        }
        // The location is carried, not recomputed, so a redirected settings
        // tree writes exactly where its reader looks.
        let path = match &self.save_root {
            Some(root) => root.join("settings.toml"),
            None => GlobalSettings::path(),
        };
        if path.exists() && GlobalSettings::load(&path).is_err() {
            archive_unreadable(&path);
        }
        Ok(path)
    }

    /// Plaintext provider keys an older settings document still carries.
    ///
    /// The tree no longer stores a credential; vault entries are the only
    /// source. A legacy `api_key` is kept in memory just long enough for the
    /// session store to scrub it from transcripts, and is never serialized or
    /// used to authenticate.
    pub(crate) fn secrets(&self) -> Vec<String> {
        self.global
            .providers
            .values()
            .filter_map(|provider| {
                // Only a legacy plaintext key remains readable here; it is
                // scrubbed from transcripts but never used to authenticate.
                let key = provider.legacy_api_key.as_deref()?.trim();
                (!key.is_empty()).then(|| key.to_string())
            })
            .collect()
    }

    /// The root of the workspace the registry says is active.
    pub(crate) fn active_workspace_root(&self) -> String {
        active_root(&self.global)
    }

    /// Make sure the registry names a workspace it actually carries.
    ///
    /// A fresh install has no registry at all, so the first save names the
    /// working directory rather than writing a file nobody can point at.
    pub(crate) fn ensure_workspace(&mut self) {
        if !self.global.general.active_workspace.is_empty()
            && self
                .global
                .workspaces
                .iter()
                .any(|w| w.id == self.global.general.active_workspace)
        {
            return;
        }
        if self.global.workspaces.is_empty() {
            // A fresh install has nothing to point at, and a shell with no
            // workspace record has no root to confine its tools to and no
            // session it can create. The directory the shell was launched from
            // is the one fact that needs no settings file, so it is named here
            // rather than left for a save nobody has triggered yet.
            let root = std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let name = root
                .rsplit('/')
                .next()
                .filter(|leaf| !leaf.is_empty())
                .unwrap_or(&root)
                .to_string();
            let id = self.note_workspace(&name, &root);
            self.global.general.active_workspace = id;
            return;
        }
        if let Some(first) = self.global.workspaces.first() {
            self.global.general.active_workspace = first.id.clone();
        }
    }

    /// The workspace record in effect, from the registry.
    pub(crate) fn active_workspace(&self) -> Option<&global::WorkspaceRef> {
        self.global
            .workspaces
            .iter()
            .find(|w| w.id == self.global.general.active_workspace)
    }

    /// Why a turn cannot be sent for one session's own choice, or none while
    /// it can.
    ///
    /// The refusal is the seam that keeps a stale name honest: a model or
    /// preset the trees no longer define is reported rather than quietly
    /// falling back to some other definition, because the reader who chose
    /// that name is the one who can correct it.
    ///
    /// The resolution is the one the build uses - the session's pair against
    /// the global tree - so the send gate and the agent can never disagree
    /// about whether a turn has an endpoint. A session that names nothing is
    /// refused with the composer's "choose a model" copy.
    pub(crate) fn refusal_for(&self, session: &resolve::SessionChoice) -> Option<&'static str> {
        let entry = self
            .global
            .workspaces
            .iter()
            .find(|w| w.id == self.global.general.active_workspace);
        let (skill_sources, skills) = self.skill_cache.resolve(&self.global, &self.workspace);
        let resolution = resolve::resolve_with_catalogue(
            &self.global,
            &self.workspace,
            entry,
            session,
            skill_sources,
            skills,
        );
        if resolution.preset_policy_mode.as_deref() == Some("required")
            && resolution.preset_policy.is_some()
            && !resolution.preset_trusted
        {
            return Some("composer.presetPolicyUntrusted");
        }
        resolution.refusal().map(|refusal| match refusal {
            resolve::Refusal::NoModel => "composer.chooseModel",
            resolve::Refusal::UnknownModel(_) => "composer.modelUnavailable",
            resolve::Refusal::UnknownPreset(_) => "composer.presetUnavailable",
        })
    }

    /// Fold the two documents into the concrete values a build uses.
    ///
    /// This is a derivation, never a second storage: every field is read from
    /// the trees, so a value one pane wrote is the value every pane and the
    /// agent both see.
    /// The recycle bin's directory.
    ///
    /// Beside the settings file, derived from `save_root` rather than recomputed from
    /// `$HOME`, so a scratch configuration gets a scratch bin and a test can never
    /// empty the reader's.
    pub(crate) fn trash_dir(&self) -> PathBuf {
        match &self.save_root {
            Some(root) => root.join("trash"),
            // A test build never gets a path it could write the reader's bin from:
            // the guard finds the state that lost its root instead of quietly
            // emptying somebody's recycle bin.
            None if cfg!(test) => panic!("a test reached the real recycle bin"),
            None => crate::settings::dir().join("trash"),
        }
    }

    /// The admin-key vault's file.
    ///
    /// Beside the settings document, derived from 'save_root' rather than
    /// recomputed from '$HOME', so a scratch configuration gets a scratch vault
    /// and a test can never open the reader's. This is the one place the path is
    /// defined; 'Vault' is constructed from it.
    pub(crate) fn vault_path(&self) -> PathBuf {
        // A vault written before the file was hidden is renamed once, so a
        // reader who already stored secrets does not silently get an empty one.
        let current = match &self.save_root {
            Some(root) => root.join(".vault.bin"),
            None if cfg!(test) => panic!("a test reached the real vault"),
            None => crate::settings::dir().join(".vault.bin"),
        };
        if !current.exists() {
            let legacy = current.with_file_name("vault.bin");
            if legacy.is_file() {
                let _ = std::fs::rename(&legacy, &current);
            }
        }
        current
    }

    pub(crate) fn effective(&self) -> Effective {
        self.effective_for(&resolve::SessionChoice::default())
    }

    /// The generation of the skill catalogue this tree resolves to.
    ///
    /// It moves whenever a scan produced a new catalogue or the cache was
    /// invalidated, so a view that caches rows derived from the catalogue (the
    /// composer's slash palette) can tell whether its rows are still the ones
    /// these settings describe.
    pub(crate) fn skill_generation(&self) -> u64 {
        self.skill_cache.generation()
    }

    /// How many filesystem scans this tree's catalogue has needed.
    ///
    /// The performance test's observable: a keystroke that re-resolved the
    /// catalogue would move it. It counts SCANS, not resolutions, because a
    /// resolution served from the cache does no disk work and copies no rows.
    #[cfg(test)]
    pub(crate) fn skill_scans(&self) -> usize {
        self.skill_cache.scans()
    }

    /// Forget the scanned skill catalogue.
    ///
    /// A settings EDIT needs no call: the cache's fingerprint covers every
    /// field a scan reads. This is for the changes the settings cannot see -
    /// the directories on disk moving under an unchanged configuration.
    pub(crate) fn invalidate_skills(&self) {
        self.skill_cache.invalidate();
    }

    /// Fold the two documents with one session's own model choice folded in.
    ///
    /// The session names the model and the global tree defines what the name
    /// means. There is no workspace-level default: a session that names
    /// nothing has no model rather than inheriting someone else's.
    pub(crate) fn effective_for(&self, session: &resolve::SessionChoice) -> Effective {
        let entry = self
            .global
            .workspaces
            .iter()
            .find(|w| w.id == self.global.general.active_workspace);
        // The catalogue comes from the cache, keyed by the settings that name
        // it: a frame resolves several times, and only the first may touch disk.
        let (skill_sources, skills) = self.skill_cache.resolve(&self.global, &self.workspace);
        let resolution = resolve::resolve_with_catalogue(
            &self.global,
            &self.workspace,
            entry,
            session,
            skill_sources,
            skills,
        );
        let connection = resolution.model.connection();
        let provider_id = resolution
            .model
            .reference()
            .map(|named| named.provider.clone())
            .unwrap_or_default();
        let general = &self.global.general;
        let model_name = resolution
            .model
            .reference()
            .map(|reference| self.model_display_name(reference))
            .unwrap_or_default();
        Effective {
            provider: provider_id,
            model_ref: resolution.model.reference().cloned(),
            model_name,
            max_steps: resolution.max_steps,
            parallel_tool_calls: resolution.parallel_tool_calls,
            api_mode: connection
                .and_then(|c| c.api_mode.clone())
                .unwrap_or_default(),
            root: resolution.root,
            model: connection.map(|c| c.model.clone()).unwrap_or_default(),
            base_url: connection.map(|c| c.base_url.clone()).unwrap_or_default(),
            api_key_vault: connection.and_then(|c| c.api_key_vault.clone()),
            system_prompt: resolution.system_prompt,
            temperature: resolution.temperature.unwrap_or(0.7),
            reasoning_effort: resolution.reasoning_effort,
            max_tokens: resolution.max_tokens,
            bash_enabled: resolution.bash_enabled,
            file_tools_enabled: resolution.file_tools_enabled,
            web_tools_enabled: resolution.web_tools_enabled,
            permission: resolution.permission.clone(),
            skill_sources: resolution.skill_sources.clone(),
            skills: resolution.skills.clone(),
            context_budget: resolution.context_budget,
            shell_timeout_secs: resolution.shell_timeout_secs,
            tool_output_cap: resolution.tool_output_cap,
            memory_enabled: resolution.memory_enabled,
            transcript_view: resolution.transcript_view,
            performance_usage: general.performance_usage.clone(),
            disabled_tools: resolution.disabled_tools,
            preset_plugins: resolution.plugins,
            preset_id: resolution.preset_id.unwrap_or_default(),
            preset_permission: resolution.preset_permission,
            preset_read: resolution.preset_read,
            preset_write: resolution.preset_write,
            preset_deny: resolution.preset_deny,
            preset_egress: resolution.preset_egress,
            preset_web_search: resolution.preset_web_search,
            preset_parallel_tool_calls: resolution.preset_parallel_tool_calls,
            preset_mcp_tools: resolution.preset_mcp_tools,
            preset_builtin_policy: resolution.preset_builtin_policy,
            preset_policy_mode: resolution.preset_policy_mode,
            preset_tools: resolution.preset_tools,
            preset_skills: resolution.preset_skills,
            preset_mcps: resolution.preset_mcps,
            preset_extensions: resolution.preset_extensions,
            preset_system_prompt: resolution.preset_system_prompt,
            preset_policy: resolution.preset_policy,
            preset_trusted: resolution.preset_trusted,
        }
    }

    /// The memory file in effect.
    ///
    /// An explicit workspace path wins. Otherwise the file sits beside the
    /// settings document this configuration writes - `save_root` for a test or
    /// a redirected install, the bsh data directory for the app - exactly like
    /// the vault and the recycle bin. Recomputing it from `$HOME` made every
    /// test build read (and, on a remember, rewrite) the reader's real memory
    /// file, which is shared state no test owns.
    pub(crate) fn memory_file(&self) -> PathBuf {
        let raw = self
            .workspace
            .agent
            .memory_path
            .as_deref()
            .unwrap_or("")
            .trim();
        if !raw.is_empty() {
            return PathBuf::from(shellexpand::tilde(raw).into_owned());
        }
        match &self.save_root {
            Some(root) => root.join("memory.jsonl"),
            None => dir().join("memory.jsonl"),
        }
    }

    /// Every effective MCP server: the global entries in file order, with a
    /// workspace entry of the same name replacing its global twin.
    pub(crate) fn mcp_servers(&self) -> Vec<mcp::McpServer> {
        mcp::merge(
            &mcp::parse_list(&self.global.mcp_servers, mcp::McpScope::Global),
            &mcp::parse_list(&self.workspace.agent.mcp_servers, mcp::McpScope::Workspace),
        )
    }

    /// The entries one document holds.
    pub(crate) fn mcp_servers_of(&self, scope: mcp::McpScope) -> Vec<mcp::McpServer> {
        match scope {
            mcp::McpScope::Global => {
                mcp::parse_list(&self.global.mcp_servers, mcp::McpScope::Global)
            }
            mcp::McpScope::Workspace => {
                mcp::parse_list(&self.workspace.agent.mcp_servers, mcp::McpScope::Workspace)
            }
        }
    }

    /// The mutable list one document owns.
    fn mcp_list_mut(&mut self, scope: mcp::McpScope) -> &mut Vec<serde_json::Value> {
        match scope {
            mcp::McpScope::Global => &mut self.global.mcp_servers,
            mcp::McpScope::Workspace => &mut self.workspace.agent.mcp_servers,
        }
    }

    /// Add a server to one document.
    ///
    /// The pane edits the DRAFT, so nothing reaches disk until the dialog
    /// saves; a refusal leaves the list exactly as it was.
    pub(crate) fn add_mcp_server(
        &mut self,
        scope: mcp::McpScope,
        server: mcp::McpServer,
    ) -> Result<(), mcp::McpRefusal> {
        let others = self.mcp_servers_of(scope);
        server.validate(&others, None)?;
        self.mcp_list_mut(scope).push(server.to_value());
        Ok(())
    }

    /// Replace the entry called `original` in one document.
    pub(crate) fn update_mcp_server(
        &mut self,
        scope: mcp::McpScope,
        original: &str,
        server: mcp::McpServer,
    ) -> Result<(), mcp::McpRefusal> {
        let others = self.mcp_servers_of(scope);
        server.validate(&others, Some(original))?;
        let list = self.mcp_list_mut(scope);
        let Some(index) = list.iter().position(|value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(original))
        }) else {
            return Err(mcp::McpRefusal::NotFound);
        };
        list[index] = server.to_value();
        Ok(())
    }

    /// Drop one server from one document.
    pub(crate) fn remove_mcp_server(&mut self, scope: mcp::McpScope, name: &str) -> bool {
        let list = self.mcp_list_mut(scope);
        let before = list.len();
        list.retain(|value| {
            !value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|entry| entry.eq_ignore_ascii_case(name))
        });
        before != list.len()
    }

    /// The model nicknames one provider carries, for the menu and the pane.
    pub(crate) fn catalog(&self, provider: &str) -> Vec<String> {
        self.global.models_of(provider).keys().cloned().collect()
    }

    /// Every provider that carries models, with its catalogue, for the menu.
    ///
    /// Grouping by provider id keeps the rows unambiguous when two providers
    /// carry a model of the same name. `active` is the provider the choice in
    /// effect names; it leads the list so the common case is one row away. The
    /// model is session state now, so the caller passes the provider in rather
    /// than the workspace carrying a selection.
    pub(crate) fn catalogs_by_provider(&self, active: &str) -> Vec<(String, Vec<String>)> {
        let mut groups: Vec<(String, Vec<String>)> = self
            .global
            .providers
            .keys()
            .map(|provider| (provider.clone(), self.catalog(provider)))
            .filter(|(_, models)| !models.is_empty())
            .collect();
        groups.sort_by_key(|(provider, _)| provider != active);
        groups
    }

    /// The name a resolved choice is shown under, in the chip and its menu.
    ///
    /// A leaf that declares a display name owns what the reader sees; one that
    /// declares none is named by its nickname. The nickname is the key the
    /// menu rows carry, so the label and the checked row describe the same
    /// thing - the API model id is a different fact and stays on the wire.
    pub(crate) fn model_display_name(&self, reference: &global::ModelRef) -> String {
        self.global
            .lookup(reference)
            .and_then(|(_, leaf)| leaf.name.clone())
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| reference.model.clone())
    }

    /// Whether one session's model accepts an image attachment.
    ///
    /// An undeclared model is permissive; only a model that names its input
    /// types has something to answer. A session with no model accepts none.
    pub(crate) fn accepts_image_for(&self, session: &resolve::SessionChoice) -> bool {
        let Some(named) = self.effective_for(session).model_ref else {
            return false;
        };
        self.global.accepts_image(&named.provider, &named.model)
    }

    /// Register the workspace the shell is running from, if it is new.
    pub(crate) fn note_workspace(&mut self, name: &str, root: &str) -> String {
        if let Some(id) = self
            .global
            .workspaces
            .iter()
            .find(|w| w.root == root)
            .map(|w| w.id.clone())
        {
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.global.workspaces.push(global::WorkspaceRef {
            id: id.clone(),
            name: name.to_string(),
            root: root.to_string(),
            created_at: unix_now(),
            updated_at: unix_now(),
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::global::{self, GlobalSettings, ModelEntry};
    use super::mcp::{McpRefusal, McpScope, McpServer, McpTransport};
    use super::workspace::WorkspaceSettings;
    use super::Settings;

    /// The reference font-size default and the appearance default, both on the
    /// global tier because neither is a workspace fact.
    #[test]
    fn the_reference_chrome_defaults_survive() {
        let settings = Settings::default();
        assert_eq!(settings.global.general.content_font_size, 14);
        assert_eq!(settings.global.general.theme_mode, "system");
        assert_eq!(settings.global.general.lang, "en");
        assert_eq!(settings.global.general.transcript_view, "standard");
    }

    /// A fresh install ships no endpoint, so the first run asks for one rather
    /// than pretending a provider is configured.
    #[test]
    fn a_fresh_install_ships_nothing() {
        let settings = Settings::default();
        assert!(
            settings.global.providers.is_empty(),
            "no provider is seeded"
        );
        assert!(
            settings.global.presets.is_empty(),
            "no preset is seeded from a retired directory"
        );
        assert!(
            settings.workspace.general.legacy_pair().is_none(),
            "no workspace holds a model selection: it lives on the session"
        );
    }

    /// A name the trees no longer define is reported, never substituted: the
    /// session that names it is the one that can correct it.
    #[test]
    fn a_stale_name_is_refused_rather_than_substituted() {
        let mut settings = Settings::default();
        let id = settings.note_workspace("scratch", "/tmp");
        settings.global.general.active_workspace = id;
        let choice = |provider: &str, model: &str| crate::settings::resolve::SessionChoice {
            model: Some(global::ModelRef {
                provider: provider.to_string(),
                model: model.to_string(),
            }),
            preset: None,
        };
        assert_eq!(
            settings.refusal_for(&crate::settings::resolve::SessionChoice::default()),
            Some("composer.chooseModel"),
            "a chat that names no model asks for one"
        );

        // A model nickname that no provider tree defines.
        let ghost = choice("work", "ghost");
        assert_eq!(
            settings.refusal_for(&ghost),
            Some("composer.modelUnavailable"),
            "a name the tree dropped is not silently swapped for another"
        );

        // The very same nickname, now defined, resolves.
        let mut global = settings.global.clone();
        global.add_provider("work", "", "https://work.example/v1", &[]);
        let _ = global.add_model("work", "ghost", "", "gpt-4o", "");
        settings.global = global;
        assert_eq!(
            settings.refusal_for(&ghost),
            None,
            "a resolved name is not refused"
        );

        // A preset nothing defines is the harder stop: it decides what the
        // model sees, so a missing mount cannot be answered by a guess.
        settings.global.default_preset = "missing-preset".to_string();
        assert_eq!(
            settings.refusal_for(&ghost),
            Some("composer.presetUnavailable"),
            "a preset the tree cannot build is refused outright"
        );
    }

    /// A session that names a model is sendable; one that names none is not,
    /// and the composer reads the "choose a model" copy.
    #[test]
    fn a_session_choice_is_the_only_way_to_name_a_model() {
        let mut settings = Settings::default();
        let id = settings.note_workspace("scratch", "/tmp");
        settings.global.general.active_workspace = id;
        settings
            .global
            .add_provider("work", "", "https://work.example/v1", &[]);
        let _ = settings.global.add_model("work", "fast", "", "fast-id", "");
        assert_eq!(
            settings.refusal_for(&crate::settings::resolve::SessionChoice::default()),
            Some("composer.chooseModel"),
            "with no session choice there is no model, even when one is defined"
        );

        let choice = crate::settings::resolve::SessionChoice {
            model: Some(global::ModelRef {
                provider: "work".to_string(),
                model: "fast".to_string(),
            }),
            preset: None,
        };
        assert_eq!(
            settings.refusal_for(&choice),
            None,
            "the session's own pair is enough to send"
        );
    }

    /// A provider node owns its endpoint, and a model leaf keys the nickname
    /// to the full API id the wire speaks.
    #[test]
    fn a_provider_node_owns_the_endpoint_and_the_model_ids() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("work", "", "https://work.example/v1", &[]);
        let _ = settings.add_model("work", "fast", "Fast one", "gpt-4o-mini", "chat");
        let _ = settings.add_model("work", "brilliant", "", "o3", "responses");
        let node = settings.providers.get("work").expect("seeded");
        assert_eq!(node.base_url, "https://work.example/v1");
        assert_eq!(node.api_key_vault, None);
        assert_eq!(
            node.models.get("fast").expect("nickname").model,
            "gpt-4o-mini",
            "the nickname keys the leaf, and the field is the API id"
        );
        assert_eq!(
            node.models.get("brilliant").expect("nickname").model,
            "o3",
            "a blank nickname falls back to the model id"
        );
        assert_eq!(
            node.models
                .get("brilliant")
                .and_then(|leaf| leaf.name.as_deref()),
            None,
            "an untyped display name is None, not an empty string"
        );
        assert_eq!(node.models.get("fast").expect("lm").model, "gpt-4o-mini");
        assert_eq!(
            settings.protocol("work", "fast"),
            "chat",
            "the leaf carries the protocol it was added with"
        );
    }

    /// The provider id is the registry key, so it is what the wire model
    /// carries: a rename moves the display, never the prefix.
    #[test]
    fn the_provider_id_stays_the_registry_key() {
        let mut settings = GlobalSettings::default();
        settings.add_custom_provider(
            "relay",
            "",
            "https://relay.example/v1",
            &[ModelEntry::named("m")],
        );
        settings.set_provider_name("relay", "Relay");
        let node = settings.providers.get("relay").expect("seeded");
        assert_eq!(node.name.as_deref(), Some("Relay"));
        assert!(
            node.models.keys().any(|key| key == "m"),
            "the model leaf is keyed by the id it carries"
        );
        assert!(settings.providers.contains_key("relay"), "the key is fixed");
    }

    /// Input types are undeclared until the pane says otherwise, so a model
    /// that never declared them takes everything.
    #[test]
    fn an_undeclared_model_takes_everything() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings.add_model("p", "m", "", "m", "");
        assert!(settings.accepts_image("p", "m"), "undeclared is permissive");
        assert_eq!(settings.input_types_of("p", "m"), ["text", "image"]);
    }

    /// Turning image off refuses it, and the text leaf can never be turned off,
    /// because a model that takes no text has nothing to send.
    #[test]
    fn text_never_leaves_the_declared_input_types() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings.add_model("p", "m", "", "m", "");
        settings.set_input_type("p", "m", "image");
        assert!(
            !settings.accepts_image("p", "m"),
            "turning image off refuses it"
        );
        settings.set_input_type("p", "m", "image");
        assert!(
            settings.accepts_image("p", "m"),
            "turning it back on restores it"
        );
        assert_eq!(settings.input_types_of("p", "m"), ["text", "image"]);
        settings.set_input_type("p", "m", "text");
        assert!(
            settings.shows_input_type("p", "m", "text"),
            "text stays standing"
        );
        assert!(settings.accepts_image("p", "m"), "and image is untouched");
    }

    /// A declaration survives a file round trip, because the tree is the
    /// document rather than a view the pane rebuilds from.
    #[test]
    fn a_declared_input_type_survives_a_reload() {
        let mut settings = Settings {
            global: {
                let mut global = GlobalSettings::default();
                global.add_provider("p", "", "https://p.example/v1", &[]);
                let _ = global.add_model("p", "m", "", "m", "");
                global
            },
            ..Settings::default()
        };
        settings.global.set_input_type("p", "m", "image");
        let text = toml::to_string(&settings.global).expect("serialize");
        let back = GlobalSettings::from_toml(&text).expect("deserialize");
        assert!(
            !back.accepts_image("p", "m"),
            "the declaration came back too"
        );
    }

    /// Dropping the model drops its declaration, so a re-added leaf starts
    /// permissive rather than inheriting a stale answer.
    #[test]
    fn dropping_a_model_drops_its_declaration() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings.add_model("p", "m", "", "m", "");
        settings.set_input_type("p", "m", "image");
        assert!(!settings.accepts_image("p", "m"));
        settings.remove_model("p", "m");
        let _ = settings.add_model("p", "m", "", "m", "");
        assert!(settings.accepts_image("p", "m"), "gone means undeclared");
    }

    /// A repeated model id under a different key is refused, so two leaves
    /// cannot answer the same wire model on one provider.
    #[test]
    fn a_model_id_appears_once_on_a_provider() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        settings.add_model("p", "m", "", "same", "").expect("first");
        let repeat = settings.add_model("p", "n", "", "same", "");
        assert!(
            repeat.is_err(),
            "a second key repeating the id is a duplicate"
        );
    }

    /// Re-adding an id under the key it already has merges rather than
    /// duplicating, so a rename of the nickname is not a refusal.
    #[test]
    fn re_adding_an_id_under_its_own_key_merges() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        settings.add_model("p", "m", "", "same", "").expect("first");
        settings.add_model("p", "m", "", "same", "").expect("merge");
        assert_eq!(settings.model_ids("p").len(), 1, "one leaf, not two");
        assert_eq!(
            settings.model_ids("p").iter().next().map(String::as_str),
            Some("same")
        );
    }

    /// A blank model id is refused, because a leaf with no id has nothing for
    /// the wire to speak.
    #[test]
    fn a_blank_model_id_is_refused() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let outcome = settings.add_model("p", "m", "", "   ", "");
        assert!(outcome.is_err(), "a whitespace id is not a model");
    }

    /// A typed but blank display name is refused, so a half-rename is visible
    /// rather than silently dropped.
    #[test]
    fn a_typed_but_blank_display_name_is_refused() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let outcome = settings.add_model("p", "m", "  ", "m", "");
        assert!(
            outcome.is_err(),
            "a typed blank name is a refusal, not a None"
        );
    }

    /// The endpoint is validated on add, so an unspeakable address never
    /// reaches the file.
    #[test]
    fn an_unspeakable_endpoint_is_refused() {
        let mut settings = GlobalSettings::default();
        settings.add_provider("p", "", "https://p.example/v1", &[]);
        let outcome = settings.add_model("p", "m", "", "m", "");
        assert!(outcome.is_ok(), "a good address is accepted");
        let malformed = super::provider::base_url_refusal("not an endpoint");
        assert!(malformed.is_some(), "a malformed address is refused");
    }

    /// The ids the tree does not carry yet are the catalogue the add card
    /// offers; an id already taken is not offered again.
    #[test]
    fn only_the_unconfigured_families_are_offered() {
        let mut settings = GlobalSettings::default();
        assert!(
            settings.addable_providers().len() >= 2,
            "a fresh tree offers families"
        );
        settings.add_provider("deepseek", "", "https://api.deepseek.com", &[]);
        let offered = settings.addable_providers();
        assert!(
            !offered.iter().any(|id| id == "deepseek"),
            "taken is not offered"
        );
        assert!(
            offered.iter().any(|id| id == "openai"),
            "the rest still are"
        );
    }

    /// A session records a nickname the global tree defines, and the effective
    /// connection carries that leaf's API id rather than the nickname.
    #[test]
    fn a_session_records_a_reference_not_a_model_string() {
        let mut settings = Settings::default();
        settings
            .global
            .add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings
            .global
            .add_model("p", "fast", "", "gpt-4o-mini", "");
        let choice = crate::settings::resolve::SessionChoice {
            model: Some(global::ModelRef {
                provider: "p".to_string(),
                model: "fast".to_string(),
            }),
            preset: None,
        };
        let effective = settings.effective_for(&choice);
        assert_eq!(effective.provider, "p");
        assert_eq!(effective.model, "gpt-4o-mini");
        assert_eq!(
            effective.model_name, "fast",
            "the chip names the nickname when the leaf declares no display name"
        );
        assert!(
            settings.effective().model_ref.is_none(),
            "without a session choice there is no reference at all"
        );
    }

    /// A session reference the tree cannot resolve refuses to send rather
    /// than falling back to a model the reader never asked for.
    #[test]
    fn an_unresolvable_reference_is_reported_not_faked() {
        let settings = Settings::default();
        assert!(
            settings.effective().model_ref.is_none(),
            "no provider means no reference"
        );
    }

    /// The image answer follows the SESSION's choice.
    #[test]
    fn the_image_answer_follows_the_session_choice() {
        let mut settings = Settings::default();
        settings
            .global
            .add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings.global.add_model("p", "m", "", "m", "");
        let choice = crate::settings::resolve::SessionChoice {
            model: Some(global::ModelRef {
                provider: "p".to_string(),
                model: "m".to_string(),
            }),
            preset: None,
        };
        assert!(
            settings.accepts_image_for(&choice),
            "undeclared is permissive"
        );
        settings.global.set_input_type("p", "m", "image");
        assert!(
            !settings.accepts_image_for(&choice),
            "the declaration reached the session's answer"
        );
        assert!(
            !settings.accepts_image_for(&crate::settings::resolve::SessionChoice::default()),
            "a chat with no model accepts no image"
        );
    }

    /// The catalog a pane lists is the nicknames one provider carries, so the
    /// menu and the row cannot drift.
    #[test]
    fn the_catalog_is_the_provider_s_nicknames() {
        let mut settings = Settings::default();
        settings
            .global
            .add_provider("p", "", "https://p.example/v1", &[]);
        let _ = settings.global.add_model("p", "fast", "", "a", "");
        let _ = settings.global.add_model("p", "slow", "", "b", "");
        assert_eq!(
            settings.catalog("p"),
            vec!["fast".to_string(), "slow".to_string()]
        );
    }

    /// MCP entries live in the document that owns them; the effective list lets
    /// a workspace entry override a global one by name.
    #[test]
    fn the_mcp_documents_hold_their_own_servers() {
        let http = |url: &str| McpTransport::Http {
            url: url.to_string(),
        };
        let mut settings = Settings::default();
        let global = McpServer {
            name: "github".to_string(),
            transport: http("https://a.example"),
            enabled: true,
            scope: McpScope::Global,
        };
        settings
            .add_mcp_server(McpScope::Global, global.clone())
            .expect("the first entry is added");
        assert_eq!(settings.global.mcp_servers.len(), 1);
        assert!(settings.mcp_servers_of(McpScope::Workspace).is_empty());
        assert_eq!(settings.mcp_servers().len(), 1);

        // The same name in the workspace document overrides, rather than
        // duplicating, its global twin.
        settings
            .add_mcp_server(
                McpScope::Workspace,
                McpServer {
                    transport: http("https://b.example"),
                    scope: McpScope::Workspace,
                    ..global.clone()
                },
            )
            .expect("an override is a different document");
        assert_eq!(settings.mcp_servers().len(), 1);
        assert_eq!(
            settings.mcp_servers()[0].transport,
            http("https://b.example")
        );
        assert_eq!(settings.mcp_servers()[0].scope, McpScope::Workspace);

        // A duplicate in the SAME document is refused and changes nothing.
        assert_eq!(
            settings.add_mcp_server(
                McpScope::Global,
                McpServer {
                    transport: http("https://c.example"),
                    ..global.clone()
                },
            ),
            Err(McpRefusal::NameTaken)
        );
        assert_eq!(
            settings.global.mcp_servers.len(),
            1,
            "the refused add wrote nothing"
        );

        // An in-place edit changes the override, not its twin.
        let mut edited = settings.mcp_servers_of(McpScope::Workspace)[0].clone();
        edited.transport = http("https://d.example");
        settings
            .update_mcp_server(McpScope::Workspace, "github", edited)
            .expect("the edit lands");
        assert_eq!(
            settings.mcp_servers()[0].transport,
            http("https://d.example")
        );
        assert_eq!(settings.global.mcp_servers[0]["url"], "https://a.example");

        // A rename that no longer collides becomes a second effective row,
        // and deleting each row addresses its own document.
        let mut renamed = settings.mcp_servers_of(McpScope::Workspace)[0].clone();
        renamed.name = "docs".to_string();
        settings
            .update_mcp_server(McpScope::Workspace, "github", renamed)
            .expect("the rename lands");
        let names: Vec<String> = settings
            .mcp_servers()
            .into_iter()
            .map(|server| server.name)
            .collect();
        assert_eq!(names, ["github", "docs"]);
        assert!(settings.remove_mcp_server(McpScope::Workspace, "docs"));
        assert_eq!(
            settings.mcp_servers()[0].name,
            "github",
            "removing the override reveals the global entry"
        );
        assert!(settings.remove_mcp_server(McpScope::Global, "github"));
        assert!(settings.mcp_servers().is_empty());
        assert!(!settings.remove_mcp_server(McpScope::Global, "gone"));
    }

    /// A task is registered against the file, and the cadence it carries is
    /// what the shell scheduler reads.
    #[test]
    fn a_task_is_registered_and_runs_on_its_cadence() {
        let mut settings = GlobalSettings::default();
        let id = settings
            .add_schedule("Nightly digest", "summarize the day", "session-1", 60)
            .expect("a schedule");
        assert_eq!(settings.schedules.len(), 1);
        let task = settings.schedules[0].clone();
        assert_eq!(task.prompt, "summarize the day");
        assert_eq!(task.every_seconds, 60);

        // A task that never ran is due whenever it is asked.
        assert!(settings.is_due(&task, 0));
        assert!(settings.is_due(&task, 60));

        // Once it has run, the cadence holds.
        // A zero stamp would read as never ran, so the clock starts at one.
        settings.mark_schedule_ran(&id, 1);
        assert!(!settings.is_due(&settings.schedules[0], 60));
        assert!(settings.is_due(&settings.schedules[0], 61));
        assert_eq!(settings.next_run(&settings.schedules[0], 30), Some(61));

        // A repeated name on another session is a second task, not a clash.
        assert!(settings
            .add_schedule("Nightly digest", "x", "s-2", 1)
            .is_ok());
        assert_eq!(settings.schedules.len(), 2);
        assert!(settings.toggle_schedule(&id));
        assert!(!settings.is_due(&settings.schedules[0], 10_000));
        assert!(settings.remove_schedule(&id));
        assert!(!settings.remove_schedule(&id));
        assert!(settings.schedules.is_empty());
    }

    /// A task with no name or no instruction is refused by the same spelling
    /// the reference uses, so the pane can show a reason.
    #[test]
    fn an_empty_task_is_refused() {
        let mut settings = GlobalSettings::default();
        assert!(settings.add_schedule("", "prompt", "s", 60).is_err());
        assert!(settings.add_schedule("name", "   ", "s", 60).is_err());
    }

    /// Presets are declared in the file, so the retired preset directory has
    /// nothing left to read.
    #[test]
    fn presets_live_in_the_settings_file() {
        let mut settings = Settings::default();
        settings.global.presets.insert(
            "tight".to_string(),
            super::global::Preset {
                name: Some("Tight".to_string()),
                plugins: vec!["shell".to_string()],
                description: None,
                order: None,
                policy: None,
                trusted: false,
                ..Default::default()
            },
        );
        assert!(settings.global.preset_ids().contains(&"tight".to_string()));
        assert!(settings.global.preset("tight").is_some());
        assert!(
            settings.global.preset("standard").is_some(),
            "the shipped two stay available"
        );
    }

    /// The workspace registry records each root once, so reopening a folder
    /// returns the workspace it already has.
    #[test]
    fn a_workspace_root_registers_once() {
        let mut settings = Settings::default();
        let first = settings.note_workspace("first", "/tmp/one");
        let second = settings.note_workspace("renamed", "/tmp/one");
        assert_eq!(first, second, "the root identifies the workspace");
        assert_eq!(settings.global.workspaces.len(), 1);
        let other = settings.note_workspace("second", "/tmp/two");
        assert_ne!(other, first, "a different root is a different workspace");
    }

    /// The global file and the workspace file are separate documents, so a
    /// workspace switch does not rewrite the global one.
    #[test]
    fn the_two_level_files_are_separate_documents() {
        let global = GlobalSettings::default().to_toml().expect("global");
        assert!(
            !global.contains("[providers."),
            "an empty tree writes no provider"
        );
        let workspace = super::workspace::WorkspaceSettings::default()
            .to_toml()
            .expect("ws");
        assert!(
            workspace.trim().lines().count() > 0,
            "the workspace writes its own file"
        );
    }

    /// A test build refuses to write the real configuration.
    ///
    /// The scratch store redirects every save, and this is the guard behind that
    /// redirect: one checkout's test run shares $HOME with every other checkout,
    /// so a forgotten redirect overwrites the reader's settings with a fixture.
    /// A second worktree did exactly that to this file while it was being
    /// repaired.
    #[cfg(test)]
    #[test]
    fn a_test_build_never_writes_the_real_configuration() {
        let bare = Settings::default();
        assert!(bare.save_root.is_none(), "the premise: no redirect is set");
        let refused = bare.save();
        assert!(
            refused.is_err(),
            "a test build with no save_root must refuse, not write the user's file"
        );
    }

    /// An unreadable document leaves the live directory for the archive.
    ///
    /// It used to be renamed to `settings.toml.bak` in place, so an old-format
    /// file sat beside the live one for good and every later reader had to work
    /// out which was which.
    #[test]
    fn an_unreadable_document_is_archived_beside_the_tree() {
        let base = std::env::temp_dir().join(format!("bsh-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch");
        let settings = Settings {
            save_root: Some(base.clone()),
            ..Settings::default()
        };
        // A file this build cannot read, which is what the old flat format is.
        let old = base.join("settings.toml");
        std::fs::write(&old, "schema_version = 1\ntheme_mode = \"light\"\n")
            .expect("the old document");
        settings.save().expect("the save");

        // The live document is the one this build wrote...
        let live = std::fs::read_to_string(&old).expect("the live document");
        assert!(
            live.contains("[general]"),
            "the live file is the new format the app reads"
        );
        assert!(
            !base.join("settings.toml.bak").exists(),
            "nothing is left beside the live file under a settings-like name"
        );
        // ...and the old one is in the archive, named so it cannot be mistaken
        // for a live document, with its content intact.
        let archived = std::fs::read_dir(base.join("archive").join("legacy-flat"))
            .expect("the archive")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(archived.len(), 1, "one record, got {archived:?}");
        let name = archived[0]
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            name.starts_with("settings-flat-") && name.ends_with(".toml.old"),
            "the archived name cannot be read as a live document, got {name}"
        );
        let kept = std::fs::read_to_string(&archived[0]).expect("the archived content");
        assert!(
            kept.contains("theme_mode"),
            "the refused document is kept whole, not eaten"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A save puts each fact in the file that owns it, at the path that file is
    /// documented to live at.
    ///
    /// This is the assertion the split is worth: the global document carries the
    /// machine preference and the registry, one workspace document carries that
    /// workspace's selection and agent configuration, and neither leaks into the
    /// other. A save that wrote one file and skipped the other would still leave
    /// every pane green, so the paths and both directions of the split are read
    /// back from disk here.
    #[test]
    fn a_save_writes_each_fact_only_to_its_own_file() {
        let base = std::env::temp_dir().join(format!("bsh-two-level-{}", std::process::id()));
        let root = base.join("workspace");
        let home = base.join("home");
        std::fs::create_dir_all(&root).expect("the workspace root");
        // The redirected location is what a test and a scratch store use, so the
        // global file is never the developer's own.
        let mut settings = Settings {
            save_root: Some(home.clone()),
            ..Settings::default()
        };
        let id = settings.note_workspace("probe", &root.to_string_lossy());
        settings.global.general.active_workspace = id.clone();
        // One fact that belongs to the machine...
        settings.global.general.lang = "zh".to_string();
        // ...one more that is also a machine preference (which session was on
        // screen), and one that belongs to this workspace.
        settings.global.general.active_session = "s1".to_string();
        settings.workspace.agent.permission = crate::caps::READ_ONLY.to_string();
        settings.save().expect("the save reaches both files");

        // The global document, at the documented path.
        let global_path = home.join("settings.toml");
        assert!(global_path.exists(), "the global file is written");
        let global = GlobalSettings::load(&global_path).expect("the global file parses");
        assert_eq!(global.general.lang, "zh", "the machine preference landed");
        assert_eq!(
            global.general.active_session, "s1",
            "the session on screen is a machine preference"
        );
        assert!(
            global.workspaces.iter().any(|workspace| workspace.id == id),
            "the registry landed"
        );

        // The workspace document, under the workspace it configures.
        let workspace_path = WorkspaceSettings::path(&root);
        assert_eq!(
            workspace_path,
            root.join(".bsh").join("settings").join("settings.toml"),
            "the workspace file lives under the workspace"
        );
        assert!(workspace_path.exists(), "the workspace file is written");
        let workspace = WorkspaceSettings::load(&workspace_path).expect("the workspace parses");
        assert_eq!(
            workspace.agent.permission,
            crate::caps::READ_ONLY,
            "the agent configuration landed"
        );
        assert_eq!(
            workspace.general.legacy_active_session, "",
            "the workspace document carries no session identity"
        );

        // The split, in both directions: neither file carries the other's facts.
        let global_text = std::fs::read_to_string(&global_path).expect("global text");
        assert!(
            !global_text.contains("permission"),
            "the workspace's agent configuration stays out of the global file"
        );
        let workspace_text = std::fs::read_to_string(&workspace_path).expect("workspace text");
        assert!(
            !workspace_text.contains("lang"),
            "the machine preference stays out of the workspace file"
        );
        assert!(
            !workspace_text.contains("workspaces"),
            "the registry stays out of the workspace file"
        );
        assert!(
            !workspace_text.contains("provider =")
                && !workspace_text.contains("model =")
                && !workspace_text.contains("active_session"),
            "neither a model nor a session identity reaches the workspace \
             document: {workspace_text}"
        );

        // Reading the two files back is what a relaunch does, and it finds the
        // same facts — so the split survives the round trip, not only the write.
        let read_back = Settings {
            global: GlobalSettings::load(&global_path).expect("global back"),
            workspace: WorkspaceSettings::load(&workspace_path).expect("workspace back"),
            save_root: None,
            ..Settings::default()
        };
        assert_eq!(read_back.global.general.lang, "zh");
        assert_eq!(read_back.workspace.agent.permission, crate::caps::READ_ONLY);
        assert_eq!(read_back.active_workspace_root(), root.to_string_lossy());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Recording which session is on screen rewrites the MACHINE preference and
    /// leaves the workspace document byte-identical.
    #[test]
    fn recording_the_open_session_leaves_the_workspace_file_alone() {
        let base = std::env::temp_dir().join(format!("bsh-open-session-{}", std::process::id()));
        let root = base.join("workspace");
        let home = base.join("home");
        std::fs::create_dir_all(&root).expect("the workspace root");
        let mut settings = Settings {
            save_root: Some(home.clone()),
            ..Settings::default()
        };
        let id = settings.note_workspace("probe", &root.to_string_lossy());
        settings.global.general.active_workspace = id;
        settings.save().expect("the baseline");
        let workspace_path = WorkspaceSettings::path(&root);
        let before = std::fs::read_to_string(&workspace_path).expect("the baseline document");

        // What selecting a chat does.
        settings.global.general.active_session = "s9".to_string();
        settings.save_global().expect("the preference write");

        let global = GlobalSettings::load(&home.join("settings.toml")).expect("the global file");
        assert_eq!(global.general.active_session, "s9");
        let after = std::fs::read_to_string(&workspace_path).expect("the workspace document");
        assert_eq!(
            before, after,
            "switching chats must not rewrite the workspace a chat happens to live in"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A workspace file written before this move named the session itself. It is
    /// carried into the machine preference once, and never over a newer choice.
    #[test]
    fn a_legacy_workspace_session_becomes_the_machine_preference_once() {
        let mut settings = Settings::default();
        settings.workspace.general.legacy_active_session = "old-chat".to_string();
        settings.adopt_legacy_session();
        assert_eq!(
            settings.global.general.active_session, "old-chat",
            "the old file's answer is adopted, so a relaunch opens the same chat"
        );

        settings.global.general.active_session = "newer-chat".to_string();
        settings.workspace.general.legacy_active_session = "old-chat".to_string();
        settings.adopt_legacy_session();
        assert_eq!(
            settings.global.general.active_session, "newer-chat",
            "a preference already recorded is never overwritten by the retired field"
        );
    }

    /// A pane edit lives in the draft until it is committed.
    ///
    /// The actions that preview a preference (language, palette, font size, the
    /// coding-view switch) used to write the COMMITTED tree and save it, so a
    /// language pick persisted every other field from the copy the reader had
    /// not applied yet - a pending permission change was overwritten by it. The
    /// contract this pins is the one the crate already states: a pane edits the
    /// draft and Save is the only writer.
    #[test]
    fn a_draft_edit_writes_nothing_until_it_is_committed() {
        let base = std::env::temp_dir().join(format!("bsh-draft-{}", std::process::id()));
        let root = base.join("workspace");
        let home = base.join("home");
        std::fs::create_dir_all(&root).expect("the workspace root");

        let mut settings = Settings {
            save_root: Some(home.clone()),
            ..Settings::default()
        };
        let id = settings.note_workspace("probe", &root.to_string_lossy());
        settings.global.general.active_workspace = id;
        settings.save().expect("the baseline");

        let global_path = home.join("settings.toml");
        let workspace_path = WorkspaceSettings::path(&root);
        let global_before = std::fs::read_to_string(&global_path).expect("global baseline");
        let workspace_before =
            std::fs::read_to_string(&workspace_path).expect("workspace baseline");

        // What a pane does: the draft moves, nothing is written.
        let mut draft = settings.clone();
        draft.global.general.lang = "zh".to_string();
        draft.workspace.agent.permission = crate::caps::READ_ONLY.to_string();
        assert_eq!(
            std::fs::read_to_string(&global_path).expect("global unchanged"),
            global_before,
            "editing the draft must not touch the global file"
        );
        assert_eq!(
            std::fs::read_to_string(&workspace_path).expect("workspace unchanged"),
            workspace_before,
            "editing the draft must not touch the workspace file"
        );

        // What Save does: the draft becomes the document, and both files take
        // exactly the facts they own.
        settings = draft;
        settings.save().expect("the commit");
        let global = GlobalSettings::load(&global_path).expect("global after");
        assert_eq!(global.general.lang, "zh", "the language is committed");
        let workspace = WorkspaceSettings::load(&workspace_path).expect("workspace after");
        assert_eq!(
            workspace.agent.permission,
            crate::caps::READ_ONLY,
            "the permission the reader picked survives a language change beside it"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A workspace document with nowhere to go is reported, not dropped.
    ///
    /// The only copy of that configuration is the window holding it, so a silent
    /// skip would lose it while telling the reader the save succeeded.
    #[test]
    fn a_save_with_no_workspace_root_reports_instead_of_dropping() {
        let base = std::env::temp_dir().join(format!("bsh-no-root-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("scratch");
        let mut settings = Settings {
            save_root: Some(base.clone()),
            ..Settings::default()
        };
        // Nothing is registered, so there is no root to place a document under.
        settings.global.workspaces.clear();
        settings.global.general.active_workspace = String::new();
        settings.workspace.agent.permission = crate::caps::READ_ONLY.to_string();
        let refused = settings.save();
        assert!(
            refused.is_err(),
            "a workspace fact with no root is a failure, not a silent skip"
        );
        assert!(
            base.join("settings.toml").exists(),
            "the global file is still written before the refusal"
        );

        // A workspace document with nothing in it has nothing to lose, so an
        // empty one is not a failure.
        let mut empty = Settings {
            save_root: Some(base.clone()),
            ..Settings::default()
        };
        empty.global.workspaces.clear();
        empty.global.general.active_workspace = String::new();
        assert!(
            empty.save().is_ok(),
            "an empty workspace document has nothing to place"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
