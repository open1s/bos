//! The resolution layer: one session's concrete values.
//!
//! The SESSION names the model, and the global file defines what a name means.
//! There is no workspace-level model: the selection is per chat, so two chats
//! in one workspace can run different models. Nothing is ever silently
//! substituted - a name that no longer resolves comes back as a missing
//! definition for the view to report, and a session that names nothing is
//! unchosen - because a quiet fallback would send a turn on an endpoint the
//! reader never chose.

use std::path::PathBuf;

use serde_json::Value;

use super::global::{GlobalSettings, ModelRef, Preset, WorkspaceRef};
use super::workspace::WorkspaceSettings;
use super::{dir, TranscriptPolicy};

/// One skill source after resolution: the label, the expanded path, and whether
/// the directory is actually there.
///
/// `exists` is carried rather than re-tested per row so a render never touches
/// the filesystem: the scan that fills the pane already asked, and the answer is
/// a fact about a moment, not about the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSource {
    /// The display name the pane shows and the card's initial comes from.
    pub(crate) name: String,
    /// The path as stored, with the tilde still in it, for display.
    pub(crate) path: String,
    /// The expanded directory a scan would read.
    pub(crate) dir: PathBuf,
    /// Whether the reader left it switched on.
    pub(crate) enabled: bool,
    /// Whether the directory is there.
    pub(crate) exists: bool,
    /// How many skills this source contributed AFTER dedupe. A source that lost
    /// every name to an earlier one counts what it still owns, which is what the
    /// card's number means to a reader: "what do I get from here".
    pub(crate) count: usize,
}

/// One skill, already deduplicated, with the source it came from.
///
/// The name is the key: a directory per skill with the agent crate's own
/// manifest reader, and the FIRST source in precedence order wins a collision,
/// so a project directory shadows a shared one without a rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredSkill {
    /// The skill's own name, from its frontmatter.
    pub(crate) name: String,
    /// The one-line description from its frontmatter.
    pub(crate) description: String,
    /// The skill's own directory.
    pub(crate) dir: PathBuf,
    /// The skill's manifest, `<dir>/SKILL.md`. This is what the build registers:
    /// the agent crate parses frontmatter from the file, so handing it the file
    /// keeps one reader authoritative.
    pub(crate) manifest: PathBuf,
    /// The name of the source that supplied it.
    pub(crate) source: String,
    /// Whether the reader switched THIS skill off. A skill that lost the name to
    /// an earlier source is not listed at all, so this is only ever the shadowing
    /// copy's flag.
    pub(crate) enabled: bool,
    /// Whether the manifest restricts the skill to user invocation
    /// (`disable-model-invocation: true`). The agent crate's own frontmatter
    /// reader sets it, so the palette and the build cannot disagree.
    pub(crate) user_only: bool,
}

/// The connection and model facts one request needs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Connection {
    /// The API base URL, owned by the provider rather than by the model.
    pub(crate) base_url: String,
    /// The vault entry the provider named, if any.
    pub(crate) api_key_vault: Option<String>,
    /// The full API model id, not the nickname the reader sees.
    pub(crate) model: String,
    /// The wire protocol, or none to follow the model.
    pub(crate) api_mode: Option<String>,
    /// Declared input types; none is undeclared, which reads as permissive.
    pub(crate) input_types: Option<Vec<String>>,
}

/// Why a turn cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// No level names a model: a fresh install, or a workspace that never chose.
    NoModel,
    /// A name the provider tree no longer defines.
    UnknownModel(ModelRef),
    /// A preset id nothing defines. The reference rejects a missing definition
    /// rather than falling back, because the preset decides what the model sees.
    UnknownPreset(String),
}

/// What the session carries into resolution.
///
/// The reference saves both of these on the session log rather than on the
/// workspace: the preset decides the tool schemas and prompt sections the model
/// sees, so it has to be reconstructible from the session alone, and a session
/// may change preset while it is still blank.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionChoice {
    /// The model the session recorded, if any.
    pub(crate) model: Option<ModelRef>,
    /// The preset id the session recorded, if any.
    pub(crate) preset: Option<String>,
}

/// The session's permission, capped by the selected preset's ceiling.
///
/// This is the ONE place the minimum is computed. The composer chip shows
/// `effective`, the policy data hands `effective` to Rego as `data.permission`,
/// and every audit row records it, so a reducer cannot be left out of one
/// consumer while another disagrees.
///
/// A preset can narrow `read-only < workspace-write < danger-full-access` but
/// can never widen the session's own permission. An unknown value ranks as
/// read-only and is normalised to it, so a typo fails closed instead of
/// reaching a mode whose Rust arm treats a stray string as writable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionResolution {
    /// The mode the workspace agent section asks for, as stored.
    pub(crate) requested: String,
    /// The ceiling the selected preset declares, when it declares one.
    pub(crate) preset_ceiling: Option<String>,
    /// The mode in force: the lower of the request and the ceiling.
    pub(crate) effective: String,
    /// The ceiling that lowered the request, when one did.
    ///
    /// The chip annotates with this and the menu explains rows above it; no
    /// reader re-derives the comparison.
    pub(crate) reduced_by: Option<String>,
}

impl PermissionResolution {
    /// Resolve one requested mode against one preset ceiling.
    ///
    /// The ceiling wins only when it ranks strictly below the request; a
    /// ceiling that ranks above it is not a grant and changes nothing.
    pub(crate) fn resolve(requested: &str, preset_ceiling: Option<&str>) -> Self {
        let request = normalize_permission(requested);
        let ceiling = preset_ceiling
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(normalize_permission);
        let reduced_by = ceiling.filter(|value| permission_rank(value) < permission_rank(&request));
        let effective = reduced_by.clone().unwrap_or_else(|| request.clone());
        Self {
            requested: requested.to_string(),
            preset_ceiling: preset_ceiling
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            effective,
            reduced_by,
        }
    }
}

/// Rank one permission string for the preset ceiling.
///
/// Unknown values rank as read-only, which is the fail-closed arm.
pub(crate) fn permission_rank(value: &str) -> u8 {
    match value.trim() {
        "danger-full-access" => 2,
        "workspace-write" => 1,
        _ => 0,
    }
}

/// Map a stored permission string onto one of the three known tiers.
///
/// Anything unrecognised becomes read-only: the stored string is preserved in
/// `requested` so a reader can see the typo, but the value in force is a tier
/// the build, the policy and the chip all understand.
fn normalize_permission(value: &str) -> String {
    match value.trim() {
        "danger-full-access" => "danger-full-access".to_string(),
        "workspace-write" => "workspace-write".to_string(),
        _ => "read-only".to_string(),
    }
}

/// The model a build will use, or why it cannot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModelChoice {
    /// A name resolved against the provider tree.
    Found {
        /// The name that resolved.
        reference: ModelRef,
        /// The connection it resolved to.
        connection: Connection,
    },
    /// This session names no model.
    Unchosen,
    /// A name that no longer resolves. Never replaced by another.
    Missing {
        /// The name that did not resolve.
        reference: ModelRef,
    },
}

impl ModelChoice {
    /// The connection a turn would use, or none while it cannot be sent.
    pub(crate) fn connection(&self) -> Option<&Connection> {
        match self {
            Self::Found { connection, .. } => Some(connection),
            _ => None,
        }
    }

    /// The name in effect, whether or not it resolves.
    pub(crate) fn reference(&self) -> Option<&ModelRef> {
        match self {
            Self::Found { reference, .. } | Self::Missing { reference, .. } => Some(reference),
            Self::Unchosen => None,
        }
    }

    /// Why a turn is refused, or none while it can be sent.
    pub(crate) fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Found { .. } => None,
            Self::Unchosen => Some(Refusal::NoModel),
            Self::Missing { reference, .. } => Some(Refusal::UnknownModel(reference.clone())),
        }
    }
}

/// Every value a build uses, once the three levels are folded together.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolution {
    /// Which model, or why there is none.
    pub(crate) model: ModelChoice,
    /// The workspace display name.
    pub(crate) name: String,
    /// The workspace root the tools are confined to.
    pub(crate) root: String,
    /// The preset id in effect, after the user default has been applied.
    pub(crate) preset_id: Option<String>,
    /// The preset it resolved to, once found.
    pub(crate) preset: Option<Preset>,
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
    /// The Rego module the resolved preset contributes.
    pub(crate) preset_policy: Option<String>,
    /// Whether the resolved preset's policy is trusted to apply.
    pub(crate) preset_trusted: bool,
    /// A preset id nothing defines.
    pub(crate) preset_missing: Option<String>,
    /// The plugins the preset mounts. The complete set, not a delta.
    pub(crate) plugins: Vec<String>,
    /// The system prompt.
    pub(crate) system_prompt: String,
    /// Reasoning effort, after the workspace's own override.
    pub(crate) reasoning_effort: Option<String>,
    /// Sampling temperature; none leaves it to the engine.
    pub(crate) temperature: Option<f32>,
    /// Completion cap; none leaves it to the engine.
    pub(crate) max_tokens: Option<u32>,
    /// ReAct steps one turn may take before the loop stops.
    pub(crate) max_steps: usize,
    /// Run one turn's tool calls concurrently.
    pub(crate) parallel_tool_calls: bool,
    /// The session's permission and the preset ceiling folded into the mode in
    /// force. The one resolution every consumer reads.
    pub(crate) permission: PermissionResolution,
    /// Register the shell tool.
    pub(crate) bash_enabled: bool,
    /// Register the filesystem tools.
    pub(crate) file_tools_enabled: bool,
    /// Offer the model a hosted web-search tool.
    pub(crate) web_tools_enabled: bool,
    /// Attach the file-backed memory store.
    pub(crate) memory_enabled: bool,
    /// The memory file in effect.
    pub(crate) memory_path: PathBuf,
    /// Every skill source in effect, in precedence order (first wins).
    ///
    /// The workspace's own directory, when it names one, is PREPENDED: it is the
    /// most specific answer, and the reference ranks project roots above user
    /// roots for exactly that reason. Sources the reader switched off are
    /// absent, so a build reading this list cannot see a disabled directory.
    pub(crate) skill_sources: Vec<ResolvedSource>,

    /// Every discovered skill, deduplicated first-wins, with its source.
    ///
    /// This is the catalogue the pane lists and the build registers from, so the
    /// count the pane shows and the skills the agent receives come from one scan.
    ///
    /// Shared rather than owned: a frame resolves the settings several times
    /// (the composer chip, the transcript, the preset chip), and cloning the
    /// whole catalogue on each of those was O(catalogue) per keystroke. An
    /// `Arc` makes every reader after the first scan O(1).
    pub(crate) skills: std::sync::Arc<Vec<DiscoveredSkill>>,
    /// Send-side token ceiling; 0 disables compaction.
    pub(crate) context_budget: usize,
    /// Seconds one shell command may run before it is killed.
    pub(crate) shell_timeout_secs: u64,
    /// Bytes of one tool result kept before the rest is truncated.
    pub(crate) tool_output_cap: usize,
    /// MCP servers, the global list merged with the workspace's.
    pub(crate) mcp_servers: Vec<Value>,
    /// Tool names switched off.
    pub(crate) disabled_tools: Vec<String>,
    /// Skill names switched off.
    pub(crate) disabled_skills: Vec<String>,
    /// Installed extensions switched off here.
    pub(crate) disabled_extensions: Vec<String>,
    /// How much of a turn the conversation renders.
    pub(crate) transcript_view: TranscriptPolicy,
    /// The interface language.
    pub(crate) lang: String,
    /// The palette mode.
    pub(crate) theme_mode: String,
    /// The conversation content size in px.
    pub(crate) content_font_size: u32,
    /// The content width a reader dragged out, or none for the reference clamp.
    pub(crate) content_width: Option<f32>,
    /// The sidebar width a reader dragged out, or none for the reference default.
    pub(crate) sidebar_width: Option<f32>,
    /// Show the coding view.
    pub(crate) developer_tools: bool,
    /// How much performance and usage detail to show.
    pub(crate) performance_usage: String,
    /// The session-list ordering a reader chose.
    pub(crate) session_sort: String,
}

/// Fold the three levels into the values a build uses, scanning the catalogue.
///
/// This is the TEST seam: it has no cache, so it always walks the sources.
/// `Settings::effective_for` goes through `SkillCatalogueCache` instead, which
/// is what keeps a keystroke away from `discover_skills`.
#[cfg(test)]
pub(crate) fn resolve(
    global: &GlobalSettings,
    workspace: &WorkspaceSettings,
    entry: Option<&WorkspaceRef>,
    session: &SessionChoice,
) -> Resolution {
    let (skill_sources, skills) = discover_skills(global, workspace, &workspace.disabled.skills);
    resolve_with_catalogue(
        global,
        workspace,
        entry,
        session,
        skill_sources,
        std::sync::Arc::new(skills),
    )
}

/// Fold the three levels with an already-scanned catalogue.
///
/// The scan is the only O(catalogue), disk-touching part of a resolution, so
/// this is the seam a cache hands its answer to.
pub(crate) fn resolve_with_catalogue(
    global: &GlobalSettings,
    workspace: &WorkspaceSettings,
    entry: Option<&WorkspaceRef>,
    session: &SessionChoice,
    skill_sources: Vec<ResolvedSource>,
    skills: std::sync::Arc<Vec<DiscoveredSkill>>,
) -> Resolution {
    let model = choose_model(global, session.model.as_ref());

    // The session's own preset wins; one that names none takes the user's
    // default, which is the reference's selectedDefault over its deployment
    // default of standard.
    let named = session
        .preset
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| global.default_preset.trim().to_string());
    let (preset, preset_missing) = if named.is_empty() {
        (None, None)
    } else {
        let found = global.preset(&named);
        let missing = if found.is_none() {
            Some(named.clone())
        } else {
            None
        };
        (found, missing)
    };
    let plugins = preset
        .as_ref()
        .map(|p| p.plugins.clone())
        .unwrap_or_default();
    let preset_id = if named.is_empty() { None } else { Some(named) };

    // The model node describes the model; the workspace says what it wants here.
    let declared_effort = match &model {
        ModelChoice::Found { reference, .. } => global
            .lookup(reference)
            .and_then(|(_, leaf)| leaf.reasoning_effort.clone()),
        _ => None,
    };

    // One permission resolution for this fold. The chip, the policy data and
    // the audit all read this value; nothing else computes the minimum.
    let permission = PermissionResolution::resolve(
        &workspace.agent.permission,
        preset
            .as_ref()
            .and_then(|preset| preset.permission.as_deref()),
    );

    Resolution {
        model,
        name: entry.map(|e| e.name.clone()).unwrap_or_default(),
        root: entry.map(|e| e.root.clone()).unwrap_or_default(),
        preset_id,
        preset_permission: preset.as_ref().and_then(|preset| preset.permission.clone()),
        preset_read: preset.as_ref().and_then(|preset| preset.read.clone()),
        preset_write: preset.as_ref().and_then(|preset| preset.write.clone()),
        preset_deny: preset
            .as_ref()
            .map(|preset| preset.deny.clone())
            .unwrap_or_default(),
        preset_egress: preset.as_ref().and_then(|preset| preset.egress.clone()),
        preset_web_search: preset.as_ref().and_then(|preset| preset.web_search),
        preset_parallel_tool_calls: preset
            .as_ref()
            .and_then(|preset| preset.parallel_tool_calls),
        preset_mcp_tools: preset.as_ref().and_then(|preset| preset.mcp_tools.clone()),
        preset_builtin_policy: preset
            .as_ref()
            .map(|preset| preset.builtin_policy.clone())
            .unwrap_or_default(),
        preset_policy_mode: preset
            .as_ref()
            .and_then(|preset| preset.policy_mode.clone()),
        preset_tools: preset.as_ref().and_then(|preset| preset.tools.clone()),
        preset_skills: preset.as_ref().and_then(|preset| preset.skills.clone()),
        preset_mcps: preset.as_ref().and_then(|preset| preset.mcps.clone()),
        preset_extensions: preset.as_ref().and_then(|preset| preset.extensions.clone()),
        preset_system_prompt: preset
            .as_ref()
            .and_then(|preset| preset.system_prompt.clone()),
        preset_policy: preset.as_ref().and_then(|preset| preset.policy.clone()),
        preset_trusted: preset
            .as_ref()
            .map(|preset| preset.trusted)
            .unwrap_or(false),
        preset,
        preset_missing,
        plugins,
        system_prompt: workspace.agent.system_prompt.clone(),
        reasoning_effort: workspace.agent.reasoning_effort.clone().or(declared_effort),
        temperature: workspace.agent.temperature,
        max_tokens: workspace.agent.max_tokens,
        max_steps: workspace.agent.max_steps,
        parallel_tool_calls: workspace.agent.parallel_tool_calls,
        permission,
        bash_enabled: workspace.agent.bash_enabled,
        file_tools_enabled: workspace.agent.file_tools_enabled,
        web_tools_enabled: workspace.agent.web_tools_enabled,
        memory_enabled: workspace.agent.memory_enabled,
        memory_path: match workspace.agent.memory_path.as_deref().map(str::trim) {
            Some(path) if !path.is_empty() => expand(path),
            _ => dir().join("memory.jsonl"),
        },
        skill_sources,
        skills,
        context_budget: workspace.agent.context_budget,
        shell_timeout_secs: workspace.agent.shell_timeout_secs,
        tool_output_cap: workspace.agent.tool_output_cap,
        mcp_servers: merge_mcp(&global.mcp_servers, &workspace.agent.mcp_servers),
        disabled_tools: workspace.disabled.tools.clone(),
        disabled_skills: workspace.disabled.skills.clone(),
        disabled_extensions: workspace.disabled.extensions.clone(),
        transcript_view: TranscriptPolicy::resolve(&global.general.transcript_view),
        lang: global.general.lang.clone(),
        theme_mode: global.general.theme_mode.clone(),
        content_font_size: global.general.content_font_size,
        content_width: global.general.content_width,
        sidebar_width: global.general.sidebar_width,
        developer_tools: global.general.developer_tools,
        performance_usage: global.general.performance_usage.clone(),
        session_sort: global.general.session_sort.clone(),
    }
}

impl Resolution {
    /// Why a turn is refused, or none while it can be sent.
    ///
    /// The preset is checked first: a mount that cannot be built is a harder
    /// stop than a model that cannot be reached.
    pub(crate) fn refusal(&self) -> Option<Refusal> {
        if let Some(id) = &self.preset_missing {
            return Some(Refusal::UnknownPreset(id.clone()));
        }
        self.model.refusal()
    }
}

/// Every source in effect and the deduplicated skills they hold.
///
/// The scan is the agent crate's OWN reader ([\`agent::skills::SkillLoader\`]), not a
/// second parser: the manifest format, the frontmatter keys and the \`SKILL.md\`
/// convention are defined once, and a pane that agreed with the agent only by
/// coincidence would eventually list a skill the agent refuses to load.
///
/// Order is the rule. Sources are walked in precedence order and a name already
/// seen is skipped, so the FIRST source wins and the later copy contributes
/// nothing. \`disabled\` filters per skill, and a skill that lost its name to an
/// earlier source never appears, even if the reader switched the winner off -
/// shadowing is about which directory is authoritative, not about which copy is
/// convenient.
///
/// Nothing is created: a source whose directory is absent is reported with
/// \`exists: false\` and contributes no skills, so merely opening the pane cannot
/// scatter \`.claude\` directories across a machine that never had them.
pub(crate) fn discover_skills(
    global: &GlobalSettings,
    workspace: &WorkspaceSettings,
    disabled: &[String],
) -> (Vec<ResolvedSource>, Vec<DiscoveredSkill>) {
    let mut sources: Vec<ResolvedSource> = Vec::new();
    // The workspace's own directory, when it names one, leads: it is the most
    // specific answer about this project.
    if let Some(own) = workspace
        .agent
        .skills_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        sources.push(resolved_source(
            super::global::source_name_for(own),
            own.to_string(),
            true,
        ));
    }
    for source in global.skill_sources() {
        // A repeat of the workspace's own directory is one source, not two: the
        // pane would otherwise show the same path twice and count it twice.
        if sources.iter().any(|existing| existing.path == source.path) {
            continue;
        }
        sources.push(resolved_source(source.name, source.path, source.enabled));
    }

    let mut skills: Vec<DiscoveredSkill> = Vec::new();
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for source in sources.iter_mut() {
        if !source.enabled || !source.exists {
            continue;
        }
        for meta in scan_dir(&source.dir) {
            if !claimed.insert(meta.name.clone()) {
                continue;
            }
            source.count += 1;
            let enabled = !disabled.iter().any(|off| off == &meta.name);
            skills.push(DiscoveredSkill {
                name: meta.name,
                description: meta.description,
                dir: meta.dir,
                manifest: meta.manifest,
                source: source.name.clone(),
                enabled,
                user_only: meta.user_only,
            });
        }
    }
    (sources, skills)
}

/// One scanned catalogue and the fingerprint of the settings that produced it.
#[derive(Debug, Clone)]
struct CatalogueEntry {
    fingerprint: u64,
    sources: Vec<ResolvedSource>,
    skills: std::sync::Arc<Vec<DiscoveredSkill>>,
}

/// A fingerprint of every input a catalogue scan reads.
///
/// Hashed rather than stored as a clone of the key: this runs on every
/// resolution, so it must not allocate. `discover_skills` reads exactly these
/// four facts, and each is hashed in a form that is stable across a frame.
fn catalogue_fingerprint(global: &GlobalSettings, workspace: &WorkspaceSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    global.skills_dir.hash(&mut hasher);
    for source in &global.skill_sources {
        source.name.hash(&mut hasher);
        source.path.hash(&mut hasher);
        source.enabled.hash(&mut hasher);
    }
    // The list boundary: without it ("ab", "c") and ("a", "bc") would hash the
    // same, which is harmless here only because the fields are read positionally
    // - separating the lists keeps that true even if a field is added later.
    0xffu8.hash(&mut hasher);
    workspace.agent.skills_dir.hash(&mut hasher);
    0xfeu8.hash(&mut hasher);
    workspace.disabled.skills.hash(&mut hasher);
    hasher.finish()
}

/// The skill catalogue, cached until the settings that name it move.
///
/// `discover_skills` walks every source directory and parses each `SKILL.md`;
/// on a real catalogue that is milliseconds of filesystem work. Resolution runs
/// several times per frame (the composer chip, the transcript, the preset
/// chip), so without this cache the input path paid that scan three times per
/// keystroke. The cache is what makes a resolution O(settings), never
/// O(catalogue) and never a disk read.
///
/// The key is a fingerprint of the exact inputs a scan reads, so a settings
/// edit invalidates itself with no version plumbing to forget. The one change
/// the fingerprint cannot see is the DISK moving under an unchanged
/// configuration (a skill directory added, removed or edited in place), so
/// `invalidate` is called from the places that do that behind the settings'
/// back: a save, and a package install or uninstall.
#[derive(Debug, Default)]
pub(crate) struct SkillCatalogueCache {
    entry: std::sync::Mutex<Option<CatalogueEntry>>,
    generation: std::sync::atomic::AtomicU64,
    /// How many times this cache actually walked the sources.
    ///
    /// A scan is the one O(catalogue), disk-touching stage of a resolution, so
    /// this counter is the performance test's observable: a run of typing that
    /// resolved the catalogue again would move it. Per-cache rather than
    /// process-global so tests running in parallel cannot read each other's
    /// scans.
    scans: std::sync::atomic::AtomicUsize,
}

impl SkillCatalogueCache {
    /// The catalogue for these settings, scanning only when an input moved.
    ///
    /// The returned `Arc` is shared with the cache, so a frame that resolves
    /// the settings three times clones three pointers rather than the catalogue.
    pub(crate) fn resolve(
        &self,
        global: &GlobalSettings,
        workspace: &WorkspaceSettings,
    ) -> (Vec<ResolvedSource>, std::sync::Arc<Vec<DiscoveredSkill>>) {
        let fingerprint = catalogue_fingerprint(global, workspace);
        if let Some(entry) = self.lock().as_ref() {
            if entry.fingerprint == fingerprint {
                return (entry.sources.clone(), entry.skills.clone());
            }
        }
        let (sources, skills) = discover_skills(global, workspace, &workspace.disabled.skills);
        self.scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let skills = std::sync::Arc::new(skills);
        *self.lock() = Some(CatalogueEntry {
            fingerprint,
            sources: sources.clone(),
            skills: skills.clone(),
        });
        // A NEW catalogue is a new generation, which is what the palette keys
        // its own cached rows on.
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (sources, skills)
    }

    /// How many times the sources have actually been walked.
    pub(crate) fn scans(&self) -> usize {
        self.scans.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many catalogues this cache has produced (or been told to forget).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Forget the cached scan.
    ///
    /// Called where the catalogue can change without any settings field moving:
    /// a settings save (the documents on disk may now name different
    /// directories) and a package install/uninstall (a directory appeared or
    /// went away). A settings EDIT does not need this - the fingerprint covers
    /// every field a scan reads.
    pub(crate) fn invalidate(&self) {
        *self.lock() = None;
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// The entry, recovering from a poisoned lock.
    ///
    /// A panic inside a scan would otherwise turn every later resolution into a
    /// second panic; the cache is derived data, so taking the inner value is
    /// always sound.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<CatalogueEntry>> {
        self.entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Clone for SkillCatalogueCache {
    fn clone(&self) -> Self {
        Self {
            entry: std::sync::Mutex::new(self.lock().clone()),
            generation: std::sync::atomic::AtomicU64::new(self.generation()),
            scans: std::sync::atomic::AtomicUsize::new(self.scans()),
        }
    }
}

impl PartialEq for SkillCatalogueCache {
    /// The cache is DERIVED state: two settings trees with the same fields are
    /// the same settings whether or not one of them has scanned yet.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// One stored source, with its directory expanded and tested for existence.
fn resolved_source(name: String, path: String, enabled: bool) -> ResolvedSource {
    let dir = expand(&path);
    let exists = dir.is_dir();
    let name = if name.trim().is_empty() {
        super::global::source_name_for(&path)
    } else {
        name
    };
    ResolvedSource {
        name,
        path,
        dir,
        enabled,
        exists,
        count: 0,
    }
}

/// The skills one directory holds, through the agent crate's own reader.
///
/// An unreadable directory is an empty one: a pane that failed to open a
/// directory has nothing to show for it, and refusing the whole scan because one
/// source is unreadable would hide every other source's skills.
fn scan_dir(dir: &std::path::Path) -> Vec<ScannedSkill> {
    let mut loader = agent::skills::SkillLoader::new(dir.to_path_buf());
    let Ok(metas) = loader.discover() else {
        return Vec::new();
    };
    metas
        .into_iter()
        .map(|meta| ScannedSkill {
            name: meta.name,
            description: meta.description,
            dir: meta
                .path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| dir.to_path_buf()),
            manifest: meta.path,
            user_only: meta.user_only,
        })
        .collect()
}

/// A skill as its directory reported it, before dedupe assigns a source.
struct ScannedSkill {
    name: String,
    description: String,
    dir: PathBuf,
    manifest: PathBuf,
    user_only: bool,
}

/// Pick the model: the session's own choice, else none.
///
/// There is no workspace-level default any more. An older settings file may
/// still carry one; it is migrated onto the open session at launch, so this
/// function never reads it.
fn choose_model(global: &GlobalSettings, session_model: Option<&ModelRef>) -> ModelChoice {
    let Some(reference) = session_model.cloned() else {
        return ModelChoice::Unchosen;
    };
    match global.lookup(&reference) {
        Some((provider, leaf)) => ModelChoice::Found {
            reference: reference.clone(),
            connection: Connection {
                base_url: provider.base_url.clone(),
                api_key_vault: provider.api_key_vault.clone(),
                model: leaf.model.clone(),
                // A leaf that names no protocol speaks its family's, so
                // the protocol the pane set on the provider node reaches the
                // wire without a leaf to hold it.
                api_mode: leaf
                    .api_mode
                    .clone()
                    .or_else(|| global.family_protocol_of(&reference.provider).into()),
                input_types: leaf.input_types.clone(),
            },
        },
        None => ModelChoice::Missing { reference },
    }
}

/// Merge the two MCP lists, workspace last so it wins.
///
/// A server is identified by its own name field. Two entries with the same name
/// collapse into the workspace's; an entry with no name cannot be matched, so it
/// is always appended rather than silently dropped.
fn merge_mcp(global: &[Value], workspace: &[Value]) -> Vec<Value> {
    let mut merged = global.to_vec();
    for server in workspace {
        match mcp_name(server) {
            Some(name) => match merged
                .iter_mut()
                .find(|existing| mcp_name(existing) == Some(name))
            {
                Some(slot) => *slot = server.clone(),
                None => merged.push(server.clone()),
            },
            None => merged.push(server.clone()),
        }
    }
    merged
}

/// The name an MCP server entry is identified by.
fn mcp_name(server: &Value) -> Option<&str> {
    server
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

/// Resolve a config path, expanding a leading tilde.
fn expand(raw: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(raw).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::global::SkillSource;

    const GLOBAL: &str = r#"
schema_version = 1
skills_dir = "~/.bos/skills"
mcp_servers = [
  { name = "shared", url = "global" },
  { name = "only-global", url = "g" },
]

[general]
lang = "zh"

[providers.nvidia]
base_url = "http://127.0.0.1:11436/v1"
api_key = "sk-local"

[providers.nvidia."glm"]
model = "nvidia/z-ai/glm-5.3-flash"
reasoning_effort = "high"

[providers.nvidia."kimi"]
model = "nvidia/moonshotai/kimi-k3"

[presets.tight]
plugins = ["shell"]

[[workspaces]]
id = "w1"
name = "bsh"
root = "/tmp/bsh"
"#;

    /// A workspace file as an older build wrote it: the model pair is legacy
    /// INPUT now, so this fixture doubles as the "an old file still parses"
    /// case. The pair must not become a fallback.
    const WORKSPACE: &str = r#"
[general]
provider = "nvidia"
model = "glm"

[agent]
temperature = 0.0
mcp_servers = [
  { name = "shared", url = "local" },
  { name = "only-local", url = "l" },
]
"#;

    fn global() -> GlobalSettings {
        GlobalSettings::from_toml(GLOBAL).expect("the global fixture parses")
    }

    fn workspace() -> WorkspaceSettings {
        WorkspaceSettings::from_toml(WORKSPACE).expect("the workspace fixture parses")
    }

    fn bare_ids() -> Vec<String> {
        global().preset_ids()
    }

    fn entry() -> WorkspaceRef {
        let g = global();
        g.workspaces.first().cloned().expect("the registry entry")
    }

    /// The choice one session carries into resolution.
    fn choice(provider: &str, model: &str) -> SessionChoice {
        SessionChoice {
            model: Some(ModelRef {
                provider: provider.to_string(),
                model: model.to_string(),
            }),
            preset: None,
        }
    }

    fn bare() -> Resolution {
        resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &choice("nvidia", "glm"),
        )
    }

    #[test]
    fn a_legacy_workspace_pair_is_not_a_fallback() {
        // The fixture still carries provider/model under [general], which an
        // older build wrote. They parse, and they select NOTHING.
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice::default(),
        );
        assert_eq!(
            r.model,
            ModelChoice::Unchosen,
            "the retired workspace selection must not choose a model"
        );
        assert_eq!(r.model.refusal(), Some(Refusal::NoModel));
    }

    #[test]
    fn the_sessions_choice_resolves_against_the_global_tree() {
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &choice("nvidia", "kimi"),
        );
        let ModelChoice::Found { connection, .. } = &r.model else {
            panic!("the session's choice resolves");
        };
        assert_eq!(
            connection.model, "nvidia/moonshotai/kimi-k3",
            "the connection carries the API id, not the nickname"
        );
        assert_eq!(
            connection.base_url, "http://127.0.0.1:11436/v1",
            "the connection comes from the provider, not the leaf"
        );
    }

    #[test]
    fn no_model_anywhere_is_unchosen_and_refuses() {
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let r = resolve(&global(), &empty, Some(&entry()), &SessionChoice::default());
        assert_eq!(r.model, ModelChoice::Unchosen);
        assert_eq!(r.model.refusal(), Some(Refusal::NoModel));
        assert!(r.model.connection().is_none());
    }

    #[test]
    fn a_name_that_no_longer_resolves_is_reported_and_never_replaced() {
        let chosen = ModelRef {
            provider: "nvidia".to_string(),
            model: "retired".to_string(),
        };
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice {
                model: Some(chosen.clone()),
                preset: None,
            },
        );
        assert_eq!(
            r.model,
            ModelChoice::Missing {
                reference: chosen.clone(),
            },
            "the reader is told what it wanted, not quietly given something else"
        );
        assert_eq!(r.model.refusal(), Some(Refusal::UnknownModel(chosen)));
        assert!(
            r.model.connection().is_none(),
            "an unresolvable model must not yield a connection"
        );
    }

    #[test]
    fn a_zero_temperature_survives_resolution() {
        let r = bare();
        assert_eq!(
            r.temperature,
            Some(0.0),
            "0.0 is greedy decoding, which is why it is not an absence"
        );
        assert_eq!(r.max_tokens, None, "an absent cap stays absent");
    }

    #[test]
    fn the_model_node_declares_an_effort_the_workspace_may_override() {
        assert_eq!(bare().reasoning_effort.as_deref(), Some("high"));

        let text = "[general]\nprovider = \"nvidia\"\nmodel = \"glm\"\n\n[agent]\nreasoning_effort = \"low\"\n";
        let overridden = WorkspaceSettings::from_toml(text).expect("parses");
        let r = resolve(
            &global(),
            &overridden,
            Some(&entry()),
            &choice("nvidia", "glm"),
        );
        assert_eq!(
            r.reasoning_effort.as_deref(),
            Some("low"),
            "the workspace's intent for this project beats the model's own note"
        );
    }

    #[test]
    fn mcp_lists_merge_and_the_workspace_wins_by_name() {
        let r = bare();
        let names: Vec<&str> = r
            .mcp_servers
            .iter()
            .filter_map(|s| s.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec!["shared", "only-global", "only-local"],
            "the global order survives, a collision is replaced in place, and the \
             workspace's own additions land last"
        );
        assert_eq!(r.mcp_servers[0]["url"], "local");
        assert_eq!(r.mcp_servers[1]["url"], "g");
    }

    #[test]
    fn an_unnamed_mcp_entry_is_appended_rather_than_dropped() {
        let named = vec![serde_json::json!({ "name": "a", "url": "1" })];
        let anonymous = vec![serde_json::json!({ "url": "2" })];
        let merged = merge_mcp(&named, &anonymous);
        assert_eq!(merged.len(), 2, "a server with no name cannot be matched");
    }

    /// A scratch directory no other test shares, removed on the way in.
    fn scratch(label: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-skill-tests")
            .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Write one skill directory: \`<root>/<name>/SKILL.md\` with frontmatter.
    ///
    /// The manifest is the agent crate's own shape - a \`---\` fence, \`name\` and
    /// \`description\` - so the fixture proves the SAME reader is used rather than
    /// a second parser that happens to agree.
    fn write_skill(root: &std::path::Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("the skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n"),
        )
        .expect("the skill manifest");
    }

    /// A global tree with two sources, \`first\` then \`second\`.
    fn two_sources(first: &std::path::Path, second: &std::path::Path) -> GlobalSettings {
        let mut built = GlobalSettings::from_toml("").expect("parses");
        built.set_skill_sources(vec![
            SkillSource {
                name: "First".to_string(),
                path: first.to_string_lossy().to_string(),
                enabled: true,
            },
            SkillSource {
                name: "Second".to_string(),
                path: second.to_string_lossy().to_string(),
                enabled: true,
            },
        ]);
        built
    }

    /// The first source wins a name collision, and the shadowed copy does not
    /// appear twice.
    #[test]
    fn the_first_source_wins_a_skill_name_collision() {
        let first = scratch("dedupe-first");
        let second = scratch("dedupe-second");
        write_skill(&first, "shared", "from the first source");
        write_skill(&second, "shared", "from the second source");
        write_skill(&second, "only-second", "unique to the second");

        let global = two_sources(&first, &second);
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let (sources, skills) = discover_skills(&global, &empty, &[]);

        assert_eq!(skills.len(), 2, "the shadowed copy is not listed twice");
        let shared = skills
            .iter()
            .find(|skill| skill.name == "shared")
            .expect("the shared name");
        assert_eq!(
            shared.description, "from the first source",
            "the FIRST source in order wins, not the last scanned"
        );
        assert_eq!(shared.source, "First");
        assert_eq!(
            sources[0].count, 1,
            "the winner counts it; the loser does not"
        );
        assert_eq!(sources[1].count, 1, "the second keeps only its unique name");
    }

    /// A source the reader switched off contributes nothing, and its skills are
    /// absent rather than merely marked.
    #[test]
    fn a_disabled_source_contributes_no_skills() {
        let first = scratch("off-first");
        let second = scratch("off-second");
        write_skill(&first, "a", "one");
        write_skill(&second, "b", "two");

        let mut global = two_sources(&first, &second);
        global.skill_sources[0].enabled = false;
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let (sources, skills) = discover_skills(&global, &empty, &[]);

        assert!(!sources[0].enabled, "the flag survives the scan");
        assert_eq!(sources[0].count, 0, "a switched-off source holds nothing");
        assert!(
            skills.iter().all(|skill| skill.source != "First"),
            "the disabled source's skills are absent, not just flagged: {skills:?}"
        );
        assert_eq!(skills.len(), 1);
    }

    /// A disabled skill is listed with \`enabled: false\` so the pane can switch it
    /// back on, while \`enabled_skills\` - what a build reads - drops it.
    #[test]
    fn a_disabled_skill_is_listed_but_not_buildable() {
        let first = scratch("off-skill");
        write_skill(&first, "keep", "kept");
        write_skill(&first, "off", "switched off");

        let global = two_sources(&first, &scratch("off-skill-empty"));
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let (_, skills) = discover_skills(&global, &empty, &["off".to_string()]);

        let off = skills
            .iter()
            .find(|skill| skill.name == "off")
            .expect("the switched-off skill is still listed, so it can come back");
        assert!(!off.enabled, "and it reads as off");
        let (sources, _) = discover_skills(&global, &empty, &["off".to_string()]);
        assert_eq!(
            sources
                .iter()
                .find(|source| source.name == "First")
                .map(|source| source.count),
            Some(2),
            "the source still counts it: it is discovered, just not registered"
        );
    }

    /// A source whose directory is absent is reported as absent and creates
    /// nothing on disk: a pane that opened \`~/.codex\` into existence would be a
    /// side effect nobody asked for.
    #[test]
    fn a_missing_directory_is_not_a_source_and_is_not_created() {
        let missing = scratch("absent-parent").join("never-there");
        let mut global = GlobalSettings::from_toml("").expect("parses");
        global.set_skill_sources(vec![SkillSource {
            name: "Codex".to_string(),
            path: missing.to_string_lossy().to_string(),
            enabled: true,
        }]);
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let (sources, skills) = discover_skills(&global, &empty, &[]);

        assert_eq!(sources.len(), 1, "it is listed, so the reader sees the row");
        assert!(!sources[0].exists, "and it says the directory is not there");
        assert!(skills.is_empty());
        assert!(
            !missing.exists(),
            "discovery must not create the directory it looked for"
        );
    }

    /// A file that still carries the retired single directory parses, and its
    /// value becomes ONE source - first, because it is what that build scanned.
    #[test]
    fn an_old_single_dir_settings_file_migrates_to_the_list() {
        let legacy = GlobalSettings::from_toml("skills_dir = \"/tmp/legacy-skills\"\n")
            .expect("an old file still parses");
        assert_eq!(
            legacy.skills_dir.as_deref(),
            Some("/tmp/legacy-skills"),
            "the retired field is still read"
        );
        let sources = legacy.skill_sources();
        assert_eq!(
            sources[0].path, "/tmp/legacy-skills",
            "the migrated directory LEADS: it is what the old build was scanning"
        );
        assert!(sources[0].enabled);
        assert!(
            sources.iter().any(|source| source.path == "~/.bos/skills"),
            "and the executor defaults are kept beside it rather than replacing it"
        );

        // Once the pane saves, the retired field is cleared so it cannot seed the
        // list a second time on the next read.
        let mut migrated = legacy;
        migrated.set_skill_sources(sources);
        assert!(migrated.skills_dir.is_none());
        let text = migrated.to_toml().expect("renders");
        assert!(
            !text.contains("skills_dir"),
            "the retired field is not written back: {text}"
        );
        assert!(text.contains("legacy-skills"));
    }

    /// A file with no \`skills_dir\` and no sources falls back to the executor
    /// defaults, always leading with the framework directory this build owns.
    #[test]
    fn a_fresh_install_offers_the_framework_directory_first() {
        let fresh = GlobalSettings::from_toml("").expect("parses");
        let sources = fresh.skill_sources();
        assert!(
            !sources.is_empty(),
            "a fresh install still knows where skills would go"
        );
        assert_eq!(
            sources[0].path, "~/.bos/skills",
            "the framework directory is the one this build owns, so it leads"
        );
    }

    /// The workspace's own directory is the most specific answer, so it is
    /// prepended and a build sees it before every global source.
    #[test]
    fn the_workspace_directory_leads_the_sources() {
        let own =
            WorkspaceSettings::from_toml("[agent]\nskills_dir = \"/tmp/mine\"\n").expect("parses");
        let global = GlobalSettings::from_toml("").expect("parses");
        let (sources, _) = discover_skills(&global, &own, &[]);
        assert_eq!(
            sources[0].path, "/tmp/mine",
            "the project's own directory outranks the machine's"
        );
    }

    /// The same directory named twice is ONE source: the pane would otherwise
    /// draw the same path on two cards and count its skills twice.
    #[test]
    fn a_directory_named_twice_is_one_source() {
        let own = WorkspaceSettings::from_toml("[agent]\nskills_dir = \"~/.bos/skills\"\n")
            .expect("parses");
        let global = GlobalSettings::from_toml("").expect("parses");
        let (sources, _) = discover_skills(&global, &own, &[]);
        assert_eq!(
            sources
                .iter()
                .filter(|source| source.path == "~/.bos/skills")
                .count(),
            1,
            "the workspace's copy folds into the global one, it does not double it"
        );
    }

    #[test]
    fn a_session_that_names_no_preset_takes_the_user_default() {
        let r = bare();
        assert_eq!(r.preset_id.as_deref(), Some("standard"));
        assert_eq!(
            r.plugins,
            vec!["shell", "files", "plan"],
            "the shipped standard preset mounts the three bundles"
        );
        assert!(r.refusal().is_none());
    }

    #[test]
    fn a_declaration_replaces_the_shipped_preset_outright() {
        let text = "[presets.standard]\nplugins = [\"shell\"]\n";
        let declared = GlobalSettings::from_toml(text).expect("parses");
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        let r = resolve(&declared, &empty, None, &SessionChoice::default());
        assert_eq!(
            r.plugins,
            vec!["shell"],
            "an override replaces the complete child list; it must NOT merge with \
             the shipped one, or a user could never remove a plugin"
        );
    }

    #[test]
    fn the_sessions_preset_beats_the_user_default() {
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice {
                model: None,
                preset: Some("tight".to_string()),
            },
        );
        assert_eq!(r.preset_id.as_deref(), Some("tight"));
        assert_eq!(r.plugins, vec!["shell"]);
    }

    #[test]
    fn an_empty_plugin_list_mounts_nothing() {
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice {
                model: None,
                preset: Some("minimal".to_string()),
            },
        );
        assert!(
            r.plugins.is_empty(),
            "an empty list is a composition of nothing, not a request for defaults"
        );
    }

    #[test]
    fn a_preset_nothing_defines_refuses_the_turn() {
        let r = resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice {
                model: None,
                preset: Some("gone".to_string()),
            },
        );
        assert_eq!(r.preset_missing.as_deref(), Some("gone"));
        assert!(r.preset.is_none());
        assert!(r.plugins.is_empty());
        assert_eq!(
            r.refusal(),
            Some(Refusal::UnknownPreset("gone".to_string())),
            "the reference rejects a missing definition rather than falling back"
        );
    }

    #[test]
    fn the_roster_lists_shipped_and_declared_presets() {
        let mut ids = bare_ids();
        ids.sort();
        assert_eq!(
            ids,
            vec!["creator", "ptc", "read-only", "standard", "tight"],
            "the roster carries what the binary ships plus what was declared"
        );
        assert!(
            !ids.iter().any(|id| id == "cordis"),
            "Cordis has no backing here and is not a roster row"
        );
    }

    #[test]
    fn a_declared_preset_that_publishes_a_name_owns_its_copy() {
        let text = "[presets.mine]\nname = \"Mine\"\nplugins = []\n";
        let declared = GlobalSettings::from_toml(text).expect("parses");
        assert_eq!(
            declared.preset("mine").and_then(|p| p.name),
            Some("Mine".to_string()),
            "user-authored metadata is never translated"
        );
        assert_eq!(
            declared.preset("standard").and_then(|p| p.name),
            None,
            "a shipped preset publishes no name: the dictionaries carry its copy"
        );
    }

    #[test]
    fn the_workspace_identity_comes_from_the_registry() {
        let r = bare();
        assert_eq!(r.name, "bsh");
        assert_eq!(r.root, "/tmp/bsh");
    }

    #[test]
    fn machine_preferences_come_from_the_global_file() {
        let r = bare();
        assert_eq!(r.lang, "zh");
        assert_eq!(r.transcript_view, TranscriptPolicy::resolve("standard"));
    }

    /// The one minimum: a ceiling below the request wins, a ceiling at or above
    /// it does not, and the reduction names its cause.
    #[test]
    fn the_permission_resolution_is_the_minimum() {
        let none = PermissionResolution::resolve("workspace-write", None);
        assert_eq!(none.effective, "workspace-write");
        assert!(none.reduced_by.is_none(), "no ceiling reduces nothing");

        let capped = PermissionResolution::resolve("danger-full-access", Some("read-only"));
        assert_eq!(capped.requested, "danger-full-access");
        assert_eq!(capped.preset_ceiling.as_deref(), Some("read-only"));
        assert_eq!(capped.effective, "read-only");
        assert_eq!(capped.reduced_by.as_deref(), Some("read-only"));

        let slack = PermissionResolution::resolve("read-only", Some("danger-full-access"));
        assert_eq!(
            slack.effective, "read-only",
            "a higher ceiling cannot widen the session"
        );
        assert!(slack.reduced_by.is_none());

        let typo = PermissionResolution::resolve("typo", None);
        assert_eq!(
            typo.effective, "read-only",
            "an unknown mode fails closed instead of reaching a writable arm"
        );
        assert_eq!(typo.requested, "typo", "while the stored value is visible");
    }
}
