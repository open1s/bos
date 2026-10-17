//! The resolution layer: one workspace's concrete values.
//!
//! Three levels fold into one answer, and the precedence is the design's own.
//! The session names a model, the workspace names a default, the global file
//! defines what those names mean, and the framework supplies what nobody set.
//!
//! Nothing is ever silently substituted. A name that no longer resolves comes
//! back as a missing definition for the view to report, because a quiet
//! fallback would send a turn on an endpoint the reader never chose.

use std::path::PathBuf;

use serde_json::Value;

use super::global::{GlobalSettings, ModelRef, Preset, WorkspaceRef};
use super::workspace::WorkspaceSettings;
use super::{dir, TranscriptPolicy};

/// The skill folder used when neither level names one.
const FRAMEWORK_SKILLS_DIR: &str = "~/.bos/skills";

/// Which level named the model in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The session carries its own choice, which beats the workspace's.
    Session,
    /// The workspace's own default.
    Workspace,
}

/// The connection and model facts one request needs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Connection {
    /// The API base URL, owned by the provider rather than by the model.
    pub(crate) base_url: String,
    /// The API key, owned by the provider.
    pub(crate) api_key: String,
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

/// The model a build will use, or why it cannot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ModelChoice {
    /// A name resolved against the provider tree.
    Found {
        /// Which level named it.
        source: Source,
        /// The name that resolved.
        reference: ModelRef,
        /// The connection it resolved to.
        connection: Connection,
    },
    /// Nothing names a model.
    Unchosen,
    /// A name that no longer resolves. Never replaced by another.
    Missing {
        /// Which level named it.
        source: Source,
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
    /// The permission mode for new sessions.
    pub(crate) permission: String,
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
    /// Where skill folders live.
    pub(crate) skills_dir: PathBuf,
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
    pub(crate) disabled_extentions: Vec<String>,
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

/// Fold the three levels into the values a build uses.
pub(crate) fn resolve(
    global: &GlobalSettings,
    workspace: &WorkspaceSettings,
    entry: Option<&WorkspaceRef>,
    session: &SessionChoice,
) -> Resolution {
    let model = choose_model(global, workspace, session.model.as_ref());

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

    let raw_skills = workspace
        .agent
        .skills_dir
        .clone()
        .or_else(|| global.skills_dir.clone())
        .unwrap_or_else(|| FRAMEWORK_SKILLS_DIR.to_string());

    Resolution {
        model,
        name: entry.map(|e| e.name.clone()).unwrap_or_default(),
        root: entry.map(|e| e.root.clone()).unwrap_or_default(),
        preset_id,
        preset,
        preset_missing,
        plugins,
        system_prompt: workspace.agent.system_prompt.clone(),
        reasoning_effort: workspace.agent.reasoning_effort.clone().or(declared_effort),
        temperature: workspace.agent.temperature,
        max_tokens: workspace.agent.max_tokens,
        max_steps: workspace.agent.max_steps,
        parallel_tool_calls: workspace.agent.parallel_tool_calls,
        permission: workspace.agent.permission.clone(),
        bash_enabled: workspace.agent.bash_enabled,
        file_tools_enabled: workspace.agent.file_tools_enabled,
        web_tools_enabled: workspace.agent.web_tools_enabled,
        memory_enabled: workspace.agent.memory_enabled,
        memory_path: match workspace.agent.memory_path.as_deref().map(str::trim) {
            Some(path) if !path.is_empty() => expand(path),
            _ => dir().join("memory.jsonl"),
        },
        skills_dir: expand(&raw_skills),
        context_budget: workspace.agent.context_budget,
        shell_timeout_secs: workspace.agent.shell_timeout_secs,
        tool_output_cap: workspace.agent.tool_output_cap,
        mcp_servers: merge_mcp(&global.mcp_servers, &workspace.agent.mcp_servers),
        disabled_tools: workspace.disabled.tools.clone(),
        disabled_skills: workspace.disabled.skills.clone(),
        disabled_extentions: workspace.disabled.extentions.clone(),
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

/// Pick the model: the session's own choice, else the workspace's, else none.
fn choose_model(
    global: &GlobalSettings,
    workspace: &WorkspaceSettings,
    session_model: Option<&ModelRef>,
) -> ModelChoice {
    let chosen = match session_model {
        Some(reference) => Some((Source::Session, reference.clone())),
        None => workspace
            .general
            .model_ref()
            .map(|reference| (Source::Workspace, reference)),
    };
    let Some((source, reference)) = chosen else {
        return ModelChoice::Unchosen;
    };
    match global.lookup(&reference) {
        Some((provider, leaf)) => ModelChoice::Found {
            source,
            reference: reference.clone(),
            connection: Connection {
                base_url: provider.base_url.clone(),
                api_key: provider.api_key.clone(),
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
        None => ModelChoice::Missing { source, reference },
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

    fn bare() -> Resolution {
        resolve(
            &global(),
            &workspace(),
            Some(&entry()),
            &SessionChoice::default(),
        )
    }

    #[test]
    fn the_workspace_default_is_used_when_the_session_names_none() {
        let r = bare();
        let ModelChoice::Found {
            source, connection, ..
        } = &r.model
        else {
            panic!("the workspace fixture selects a model that resolves");
        };
        assert_eq!(*source, Source::Workspace);
        assert_eq!(
            connection.model, "nvidia/z-ai/glm-5.3-flash",
            "the connection carries the API id, not the nickname"
        );
        assert_eq!(
            connection.base_url, "http://127.0.0.1:11436/v1",
            "the connection comes from the provider, not the leaf"
        );
    }

    #[test]
    fn the_sessions_own_choice_beats_the_workspace_default() {
        let chosen = ModelRef {
            provider: "nvidia".to_string(),
            model: "kimi".to_string(),
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
        let ModelChoice::Found {
            source, connection, ..
        } = &r.model
        else {
            panic!("the session's choice resolves");
        };
        assert_eq!(*source, Source::Session);
        assert_eq!(connection.model, "nvidia/moonshotai/kimi-k3");
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
                source: Source::Session,
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
            &SessionChoice::default(),
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

    #[test]
    fn the_skill_folder_goes_workspace_global_then_framework() {
        let global = global();
        assert!(
            bare().skills_dir.ends_with(".bos/skills"),
            "the global value applies when the workspace names none"
        );

        let text = "[agent]\nskills_dir = \"/tmp/mine\"\n";
        let own = WorkspaceSettings::from_toml(text).expect("parses");
        assert_eq!(
            resolve(&global, &own, None, &SessionChoice::default()).skills_dir,
            PathBuf::from("/tmp/mine")
        );

        let none = GlobalSettings::from_toml("").expect("parses");
        let empty = WorkspaceSettings::from_toml("").expect("parses");
        assert!(
            resolve(&none, &empty, None, &SessionChoice::default())
                .skills_dir
                .ends_with(".bos/skills"),
            "the framework default is the last resort"
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
            vec!["minimal", "standard", "tight"],
            "the roster carries what the binary ships plus what was declared"
        );
        assert!(
            !ids.iter().any(|id| id == "ptc" || id == "cordis"),
            "a shipped preset with no backing here is not a roster row"
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
}
