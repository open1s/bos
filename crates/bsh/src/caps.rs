//! Building the chat agent from settings.
//!
//! The agent is rebuilt on every turn from the persisted settings, so a
//! settings change reaches the very next send without an app restart. How much
//! the agent may do is the permission mode, and every arm of it lands on a real
//! seam: Read Only withholds the side-effecting tools from the request, Workspace
//! Write confines the shell and the file tools to the active workspace, and Full
//! access releases both.

use std::path::PathBuf;
use std::sync::Arc;

use agent::memory::{FileMemory, MemoryStore};
use agent::tools::{BashTool, FunctionTool, PlanTool};
use agent::{Agent, AgentConfig, ToolError};

use crate::plugins::{self, PluginInfo};
use crate::settings::{Effective, Settings};
use crate::vault::{self, Vault, VaultError};

/// The permission mode that confines a turn to the active workspace.
pub(crate) const WORKSPACE_WRITE: &str = "workspace-write";

/// The permission mode that withholds every side-effecting tool.
pub(crate) const READ_ONLY: &str = "read-only";

/// The permission mode that releases the workspace confinement.
pub(crate) const FULL_ACCESS: &str = "danger-full-access";

/// Whether a permission mode confines a turn to the active workspace.
///
/// Confined is what installs the shell's `WorkspaceValidator` and gives the file
/// tools their root; the released mode passes `None` to both, and
/// `BashTool::with_config` then leaves its validator unset, which is the switch
/// that drops the destructive and elevated-privilege guards.
pub(crate) fn is_confined(permission: &str) -> bool {
    permission != FULL_ACCESS
}

/// Whether a permission mode lets a turn write anything at all.
///
/// Read Only withholds the shell and the file writer, so an uncooperative model
/// cannot reach them: they are absent from the request rather than refused at
/// call time.
pub(crate) fn allows_writes(permission: &str) -> bool {
    permission != READ_ONLY
}

/// A capability the built agent exposes, for the settings inventory.
#[derive(Debug, Clone)]
pub(crate) struct CapInfo {
    /// Tool or skill name.
    pub(crate) name: String,
    /// One-line description.
    pub(crate) description: String,
    /// "tool" or "skill".
    pub(crate) category: String,
}

/// A built agent together with the plugins its tools came from.
pub(crate) struct BuiltAgent {
    /// The agent the shell runs turns against.
    pub(crate) agent: Arc<Agent>,
    /// Serialized length of the tool definitions the agent carries.
    ///
    /// The composer's context meter prices the request's tool schemas from this
    /// array, the way the reference's `contextBreakdown` follows the latest
    /// request header.
    pub(crate) tools_schema_chars: usize,
    /// The installed plugins, for the settings inventory.
    pub(crate) plugins: Vec<PluginInfo>,
    /// Why the provider credential could not be resolved, if it could not.
    ///
    /// A locked vault is named here rather than silently handing the engine an
    /// empty key. The field is absent for every build that needs no vault - a
    /// plaintext key, an empty key awaiting environment auth, or no vault file.
    pub(crate) credential_error: Option<VaultError>,
}

/// Build the agent for one session.
///
/// 'choice' is the session's own model pair, and the only source of a model:
/// a session that names none is built with nothing rather than a fallback. It
/// is the same resolution the composer chip, the menu and the send gate read.
///
/// 'vault' is the in-app credential store and the only credential source: the
/// settings tree stores no API key. The build reads the entry the SELECTED
/// provider names, falling back to provider:ID:api_key when no entry is
/// named. A vault that exists but is locked - and a named entry that does not
/// exist - is reported through 'BuiltAgent::credential_error' instead of
/// silently leaving the key empty.
///
/// `mcp` is the live server pool: its tools are registered under their
/// server namespace, so a configured MCP server reaches the model on the very
/// next turn without the settings pane being the only place that knows it.
pub(crate) fn build_agent(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
    mcp: &crate::mcp::McpPool,
) -> BuiltAgent {
    let eff = settings.effective_for(choice);
    let (api_key, credential_error) = provider_api_key(&eff, vault);
    // The wire form is `<provider>/<model>`: the engine registers one vendor
    // per provider id and resolves the prefix out of this string, so the id the
    // Models pane shows is the id the engine dispatches on.
    let model = if eff.provider.is_empty() {
        eff.model.clone()
    } else {
        format!("{}/{}", eff.provider, eff.model)
    };
    let mut config = AgentConfig {
        name: "bsh".to_string(),
        model,
        base_url: eff.base_url.clone(),
        api_key,
        system_prompt: eff.system_prompt.clone(),
        temperature: eff.temperature,
        // The composer's "Off" level is stored as the literal `off` so it can
        // beat the model's declared effort; on the wire it means NO effort field
        // rather than a string a provider would reject.
        reasoning_effort: eff
            .reasoning_effort
            .clone()
            .filter(|level| !level.eq_ignore_ascii_case("off")),
        max_tokens: eff.max_tokens,
        // A zero-step loop would return nothing, so the floor is one.
        max_steps: eff.max_steps.max(1),
        parallel_tool_calls: eff.parallel_tool_calls,
        ..AgentConfig::default()
    };
    // BSH does not read the ambient BOS config: the settings files are the
    // whole truth for this shell. The model's declared protocol arrives through
    // the settings instead, so the pane and the wire agree by construction.
    config.api_mode = eff.api_mode.clone();
    // The hosted web-search tool. The engine still requires the Responses
    // protocol before it offers the tool, because a chat or anthropic endpoint
    // would reject the object; the setting decides whether the reader wants it.
    config.web_search = eff.web_tools_enabled;

    let mut agent = Agent::from_config(config);
    let root = workspace_root(&eff);
    // A preset names the BUNDLES it wants, not their tools: the shipped
    // presets say `shell`, `files` and `plan`, while the bundle that carries the
    // shell is named `shell` and its own tool `bash`. Comparing a tool name
    // against a bundle name would switch every tool off, which is exactly the
    // wrong-model symptom of an agent that answers with no tools at all. So the
    // preset answers by bundle, and the tool-level switch answers by tool.
    let preset_bundles = (!eff.preset_plugins.is_empty()).then(|| eff.preset_plugins.clone());
    let off_bundle = |name: &str| {
        if let Some(allowed) = &preset_bundles {
            if !allowed.iter().any(|bundle| bundle == name) {
                return true;
            }
        }
        false
    };
    let off_tool = |name: &str| eff.disabled_tools.iter().any(|d| d == name);
    let mut bundles: Vec<plugins::Bundle> = Vec::new();

    // The permission mode is the reference's own trinary, and each arm lands on
    // a real seam rather than a label:
    //
    // - Read Only withholds the two side-effecting tool families, so the shell
    //   and every write are absent from the request, not merely refused.
    // - Workspace Write is the confined build: the shell validator and the file
    //   tools' root are both installed.
    // - Full access hands the shell `workspace_root: None`, which is the one
    //   switch that drops BashTool's destructive and elevated-privilege guards,
    //   and releases the file tools' escape refusal.
    let confined = is_confined(&eff.permission);
    let writes = allows_writes(&eff.permission);

    if eff.bash_enabled && writes && !off_bundle("shell") && !off_tool("bash") {
        let config = agent::tools::bash::BashToolConfig {
            workspace_root: if confined {
                root.as_ref().map(|r| r.to_string_lossy().to_string())
            } else {
                None
            },
            timeout_secs: eff.shell_timeout_secs,
            ..Default::default()
        };
        let tool: Arc<dyn agent::tools::Tool> = Arc::new(BashTool::new("bash").with_config(config));
        bundles.push(plugins::Bundle::new("shell", vec![tool]));
    }
    if eff.file_tools_enabled && !off_bundle("files") {
        let files_root = if confined { root.clone() } else { None };
        let mut files: Vec<Arc<dyn agent::tools::Tool>> = Vec::new();
        if !off_tool("read_file") {
            files.push(Arc::new(read_file_tool(root.clone())));
        }
        if writes && !off_tool("write_file") {
            files.push(Arc::new(write_file_tool(files_root.clone())));
        }
        if !off_tool("list_dir") {
            files.push(Arc::new(list_dir_tool(root.clone())));
        }
        if !files.is_empty() {
            bundles.push(plugins::Bundle::new("files", files));
        }
    }
    if !off_bundle("plan") && !off_tool("update_plan") {
        let tool: Arc<dyn agent::tools::Tool> = Arc::new(PlanTool::new(agent.plan()));
        bundles.push(plugins::Bundle::new("plan", vec![tool]));
    }

    // The registry is the source of the agent's tools: install the bundles,
    // then hand every tool they contributed to the agent.
    let (registry, plugins) = plugins::install(bundles, &settings.workspace.disabled.extentions);
    for name in registry.tool_names() {
        if let Some(tool) = registry.tool(&name) {
            agent.add_tool(tool);
        }
    }

    // MCP tools come from the live pool rather than from the document: the
    // connection is the expensive part and it is shared across turns. A name
    // the registry already holds is skipped by the registration itself.
    for tool in mcp.tools() {
        let _ = agent.add_mcp_tool(&tool.namespace, &tool.name, tool.adapter.clone());
    }

    // Skills come from the ENABLED sources the resolution already scanned, and
    // only from the skills it kept: turning a source off removes its skills from
    // this list, and turning a skill off removes that one. Registering from the
    // resolved catalogue rather than re-walking the directories is what makes
    // the pane's toggle and the agent's build one fact instead of two scans that
    // agree by luck.
    for skill in eff.enabled_skills() {
        let _ = agent.register_skill_manifest(skill.manifest.clone());
    }

    if eff.memory_enabled {
        if let Some(memory) = open_memory(settings) {
            agent = agent.with_memory(memory);
        }
    }

    // The meter's tools bucket follows the request's own tool array, so it is
    // priced from the registry that request will serialize.
    let tools_schema_chars = agent
        .registry()
        .map(|registry| tools_schema_chars(registry))
        .unwrap_or(0);

    BuiltAgent {
        agent: Arc::new(agent),
        plugins,
        tools_schema_chars,
        credential_error,
    }
}

/// Resolve one provider's credential from the vault, and the refusal when a
/// vault that should supply it cannot.
///
/// The settings tree no longer carries an API key, so the order is:
///
/// 1. when the provider named a vault entry in `api_key_vault`, that exact
///    entry is read. A name the unlocked vault does not hold is an explicit
///    refusal carrying the id - never a silent empty key;
/// 2. otherwise the conventional `provider:<id>:api_key` entry is read, which
///    is where an older migration filed a moved key.
///
/// A vault that exists but is locked is an explicit 'Locked' refusal on every
/// vault path, because an empty key would send the turn unauthenticated. With
/// no vault file at all, a named entry is reported missing (naming the id),
/// while an unnamed provider keeps the empty key, preserving environment
/// authentication for installs that never created a vault.
pub(crate) fn provider_credential(
    provider: &str,
    api_key_vault: Option<&str>,
    vault: &Vault,
) -> (String, Option<VaultError>) {
    let named = api_key_vault
        .map(str::trim)
        .filter(|entry| !entry.is_empty());
    if let Some(entry) = named {
        if !vault.is_app_available() {
            // No app access key means the encrypted entries cannot be read. A
            // locked file is a different refusal from a name no file could
            // hold: the first says grant app access, the second names the id
            // that is missing.
            if vault.exists() {
                return (String::new(), Some(VaultError::Locked));
            }
            return (
                String::new(),
                Some(VaultError::MissingEntry(entry.to_string())),
            );
        }
        return match vault.resolve(entry) {
            Some(secret) => (secret.as_str().to_string(), None),
            None => (
                String::new(),
                Some(VaultError::MissingEntry(entry.to_string())),
            ),
        };
    }
    if provider.is_empty() || !vault.exists() {
        return (String::new(), None);
    }
    if !vault.is_app_available() {
        return (String::new(), Some(VaultError::Locked));
    }
    let name = vault::provider_key(provider);
    match vault.resolve(&name) {
        Some(secret) => (secret.as_str().to_string(), None),
        None => (String::new(), None),
    }
}

/// The effective provider's credential, derived from the same resolution the
/// build uses.
fn provider_api_key(eff: &Effective, vault: &Vault) -> (String, Option<VaultError>) {
    provider_credential(&eff.provider, eff.api_key_vault.as_deref(), vault)
}

/// Why the next build would have no credential, if it would have none.
///
/// The send path asks this before it opens a turn, so a locked vault is refused
/// where the reader can see it rather than producing an unauthenticated request.
pub(crate) fn credential_error(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
) -> Option<VaultError> {
    let eff = settings.effective_for(choice);
    provider_api_key(&eff, vault).1
}

/// The serialized length of the tool definitions an agent sends.
///
/// `contextBreakdown`'s tools bucket follows the latest request header, so this
/// measures exactly what that request carries: every registered tool's OpenAI
/// projection, sync and async alike, serialized as one array.
fn tools_schema_chars(registry: &agent::tools::ToolRegistry) -> usize {
    let mut definitions: Vec<react::tool::ToolDefinition> = Vec::new();
    for (_, tool) in registry.iter() {
        definitions.push(tool.to_openai_definition());
    }
    for name in registry.async_tool_names() {
        if let Some(tool) = registry.get_async(&name) {
            definitions.push(tool.to_openai_definition());
        }
    }
    serde_json::to_string(&definitions)
        .map(|json| json.chars().count())
        .unwrap_or(0)
}

/// The capabilities the agent exposes, for the tools-and-skills pane.
pub(crate) fn capabilities(agent: &Agent) -> Vec<CapInfo> {
    let mut caps = Vec::new();
    if let Some(registry) = agent.registry() {
        for name in registry.list() {
            // Async tools (MCP among them) live in their own map, so a
            // sync-only lookup left every MCP row in the Tools pane blank.
            let description = registry
                .get(&name)
                .map(|tool| tool.description().to_string())
                .or_else(|| registry.get_async(&name).map(|tool| tool.description()))
                .unwrap_or_default();
            caps.push(CapInfo {
                name,
                description,
                category: "tool".to_string(),
            });
        }
    }
    for (name, instructions) in agent.get_skills_content() {
        let summary = instructions
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .to_string();
        caps.push(CapInfo {
            name: name.to_string(),
            description: summary,
            category: "skill".to_string(),
        });
    }
    caps.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    caps
}

/// The workspace root, expanded, when one is configured.
fn workspace_root(eff: &Effective) -> Option<PathBuf> {
    let raw = eff.root.trim();
    (!raw.is_empty()).then(|| expand_path(raw))
}

/// Expand a leading tilde.
pub(crate) fn expand_path(raw: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(raw).into_owned())
}

/// Resolve a tool path against the workspace root, refusing escapes.
fn resolve_path(root: &Option<PathBuf>, raw: &str) -> Result<PathBuf, String> {
    let raw_path = PathBuf::from(raw);
    let joined = if raw_path.is_absolute() {
        raw_path
    } else if let Some(root) = root {
        root.join(raw_path)
    } else {
        raw_path
    };
    if let Some(root) = root {
        let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let probe = if joined.exists() {
            joined.canonicalize().unwrap_or_else(|_| joined.clone())
        } else {
            let parent = joined
                .parent()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
                .unwrap_or_default();
            parent.join(joined.file_name().unwrap_or_default())
        };
        if !probe.starts_with(&canon_root) {
            return Err(format!("path escapes the workspace: {raw}"));
        }
    }
    Ok(joined)
}

fn arg_str(args: &serde_json::Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidInput(format!("{key} is required")))
}

/// read_file - read a UTF-8 file inside the workspace.
fn read_file_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "read_file",
        "Read a UTF-8 text file. Paths may be absolute or relative to the workspace.",
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
        move |args| {
            let raw = arg_str(args, "path")?;
            let target = resolve_path(&root, &raw).map_err(ToolError::InvalidInput)?;
            let content = std::fs::read_to_string(&target)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            Ok(serde_json::json!({ "path": target.display().to_string(), "content": content }))
        },
    )
}

/// write_file - write a UTF-8 file inside the workspace.
fn write_file_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "write_file",
        "Write a UTF-8 text file, creating parent directories as needed.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        }),
        move |args| {
            let raw = arg_str(args, "path")?;
            let content = arg_str(args, "content")?;
            let target = resolve_path(&root, &raw).map_err(ToolError::InvalidInput)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| ToolError::Failed(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&target, &content)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            Ok(serde_json::json!({
                "path": target.display().to_string(),
                "bytes": content.len(),
            }))
        },
    )
}

/// list_dir - list a directory's entries.
fn list_dir_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "list_dir",
        "List a directory's entries (name and kind).",
        serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
        move |args| {
            let raw = arg_str(args, "path")?;
            let target = resolve_path(&root, &raw).map_err(ToolError::InvalidInput)?;
            let mut entries = Vec::new();
            let reader = std::fs::read_dir(&target)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            for entry in reader.flatten() {
                let kind = if entry.path().is_dir() { "dir" } else { "file" };
                entries.push(serde_json::json!({
                    "name": entry.file_name().to_string_lossy(),
                    "kind": kind,
                }));
            }
            Ok(serde_json::json!({ "path": target.display().to_string(), "entries": entries }))
        },
    )
}

/// Open the file-backed memory store on a short-lived runtime.
///
/// FileMemory::open is async while the build is sync and may run inside an
/// async context, so the open happens on its own current-thread runtime.
fn open_memory(settings: &Settings) -> Option<Arc<dyn MemoryStore>> {
    let path = settings.memory_file();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let opened = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| rt.block_on(FileMemory::open(path)).ok())
            .map(|memory| memory.with_max_items(500));
        let _ = tx.send(opened);
    });
    let opened = rx.recv().ok().flatten()?;
    Some(Arc::new(opened) as Arc<dyn MemoryStore>)
}

#[cfg(test)]
mod tests {
    use super::{
        allows_writes, build_agent, is_confined, BuiltAgent, FULL_ACCESS, READ_ONLY,
        WORKSPACE_WRITE,
    };
    use crate::settings::global::{ModelEntry, Preset};
    use crate::settings::resolve::SessionChoice;
    use crate::settings::Settings;
    use crate::vault::Vault;

    /// A bare tree, which is what a fresh install carries.
    fn bare() -> Settings {
        Settings::default()
    }

    /// A build that consults no vault, which is what every non-credential test
    /// wants.
    fn build(settings: &Settings) -> BuiltAgent {
        build_agent(
            settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        )
    }

    /// A build for one explicit session choice.
    fn build_for(settings: &Settings, choice: &SessionChoice) -> BuiltAgent {
        build_agent(
            settings,
            &Vault::disabled(),
            choice,
            &crate::mcp::McpPool::default(),
        )
    }

    /// The choice the credential fixtures name: the `work` provider's leaf.
    fn work_choice() -> SessionChoice {
        SessionChoice {
            model: Some(crate::settings::global::ModelRef {
                provider: "work".to_string(),
                model: "work-model".to_string(),
            }),
            preset: None,
        }
    }

    /// The meter tools figure is the serialized request array: nonzero for a
    /// built-in tool set, and strictly smaller once a tool is switched off.
    #[test]
    fn the_tool_schema_length_follows_the_registered_tools() {
        let all = build(&bare()).tools_schema_chars;
        let mut settings = bare();
        settings.workspace.agent.bash_enabled = false;
        let fewer = build(&settings).tools_schema_chars;
        assert!(all > 0, "a built agent carries tool definitions");
        assert!(
            fewer > 0 && fewer < all,
            "switching off a tool must shrink the array ({fewer} vs {all})"
        );
    }

    /// The web-tools switch reaches the agent config, and the hosted tool only
    /// appears when the protocol can carry it.
    ///
    /// The setting is the reader's half of the decision; the engine holds the
    /// other half, so this pins that the value arrives and that a chat-protocol
    /// provider is not handed a hosted tool it would reject.
    #[test]
    fn the_web_tools_switch_reaches_the_agent() {
        let mut settings = bare();
        assert!(
            settings.workspace.agent.web_tools_enabled,
            "the shipped default offers web search"
        );
        assert!(
            build(&settings).agent.config().web_search,
            "the setting reaches the agent config"
        );

        settings.workspace.agent.web_tools_enabled = false;
        assert!(
            !build(&settings).agent.config().web_search,
            "switching it off withdraws the tool"
        );
    }

    /// A session's chosen provider is the endpoint the agent is built against,
    /// so what the model menu lists really reaches the agent.
    #[test]
    fn a_sessions_provider_is_the_endpoint() {
        let mut settings = bare();
        settings.global.add_provider(
            "work",
            "Work",
            "https://work.example",
            &[ModelEntry::named("work-model")],
        );
        let built = build_for(&settings, &work_choice());
        let config = built.agent.config();
        // The wire form is the prefix the engine registers a vendor on.
        assert_eq!(config.model, "work/work-model");
        assert_eq!(config.base_url, "https://work.example");
    }

    /// The session's pair is the only source of a model at the caps seam.
    ///
    /// This is the same resolution the composer chip, the menu's checked row and
    /// the send gate read, so the model a reader picks in a chat is the model
    /// that chat's next turn is built against - and a chat with no choice is
    /// built with no model rather than someone else's.
    #[test]
    fn a_sessions_choice_is_the_only_model_source() {
        let mut settings = bare();
        settings.global.add_provider(
            "p",
            "P",
            "https://p.example",
            &[ModelEntry::named("session-model")],
        );
        assert_eq!(
            build(&settings).agent.config().model,
            "",
            "a session that names no model is built with no model"
        );

        let choice = SessionChoice {
            model: Some(crate::settings::global::ModelRef {
                provider: "p".to_string(),
                model: "session-model".to_string(),
            }),
            preset: None,
        };
        assert_eq!(
            build_for(&settings, &choice).agent.config().model,
            "p/session-model",
            "the session's own pair is the endpoint the next turn runs on"
        );
    }

    /// A connected MCP server's tools reach the agent, namespaced by server.
    ///
    /// This is the seam that makes the settings pane real: the same build a turn
    /// runs registers what the pool connected.
    #[test]
    fn a_connected_mcp_server_reaches_the_agent() {
        use react::tool::registry::AsyncTool;
        use std::sync::Arc;
        let echo: Arc<dyn AsyncTool> = Arc::new(agent::tools::AsyncFunctionTool::from_fn(
            "github_search",
            "Search GitHub",
            serde_json::json!({ "type": "object", "properties": {} }),
            |_args| async move { Ok(serde_json::json!({"ok": true})) },
        ));
        let pool = crate::mcp::McpPool::with_tools(vec![crate::mcp::McpTool {
            namespace: "github".to_string(),
            name: "search".to_string(),
            adapter: echo,
        }]);
        let built = build_agent(
            &bare(),
            &Vault::disabled(),
            &SessionChoice::default(),
            &pool,
        );
        let names = built
            .agent
            .registry()
            .map(|registry| registry.async_tool_names())
            .unwrap_or_default();
        assert!(
            names.contains(&"github_search".to_string()),
            "the server's tool is registered: {names:?}"
        );
        // And it is callable through the same registry the engine dispatches
        // on, not merely listed.
        let value = built
            .agent
            .registry()
            .expect("the build has a registry")
            .execute("github_search", &serde_json::json!({}))
            .expect("the MCP-backed tool runs");
        assert_eq!(value["ok"], serde_json::json!(true));
        // And the Tools pane can describe it, which only works when the
        // capability projection reads the async map too.
        let caps = crate::caps::capabilities(&built.agent);
        assert!(
            caps.iter()
                .any(|cap| cap.name == "github_search" && !cap.description.is_empty()),
            "the MCP tool is listed with its description: {caps:?}"
        );
    }

    /// The agent-loop switch reaches the agent config, not just the file.
    #[test]
    fn the_parallel_switch_reaches_the_agent_config() {
        let off = bare();
        assert!(!build(&off).agent.config().parallel_tool_calls);
        let mut on = bare();
        on.workspace.agent.parallel_tool_calls = true;
        assert!(build(&on).agent.config().parallel_tool_calls);
    }

    /// The API protocol the user picks reaches the agent config, not just the
    /// file - which is what makes the models pane row a control rather than a
    /// label.
    #[test]
    fn the_api_protocol_reaches_the_agent_config() {
        let mut settings = bare();
        settings
            .global
            .add_provider("p", "", "https://p.example", &[ModelEntry::named("m")]);
        let choice = SessionChoice {
            model: Some(crate::settings::global::ModelRef {
                provider: "p".to_string(),
                model: "m".to_string(),
            }),
            preset: None,
        };
        settings.global.set_family_protocol("p", "responses");
        assert_eq!(
            build_for(&settings, &choice).agent.config().api_mode,
            "responses"
        );
        settings.global.set_family_protocol("p", "chat");
        assert_eq!(
            build_for(&settings, &choice).agent.config().api_mode,
            "chat"
        );
    }

    /// The Max output tokens row reaches the agent config. Two explicit values
    /// are paired so the assertion cannot be satisfied by a default the BOS
    /// config happens to carry for this model; an empty field clears the cap,
    /// which leaves the provider own default in charge.
    #[test]
    fn the_output_cap_reaches_the_agent_config() {
        let mut capped = bare();
        capped.workspace.agent.max_tokens = Some(4096);
        assert_eq!(build(&capped).agent.config().max_tokens, Some(4096));
        let mut smaller = bare();
        smaller.workspace.agent.max_tokens = Some(2048);
        assert_eq!(build(&smaller).agent.config().max_tokens, Some(2048));
    }

    /// A selected preset narrows the tools the agent is built with, so the
    /// agent-presets pane changes the agent rather than a label.
    #[test]
    fn a_preset_narrows_the_tools() {
        let mut narrow = bare();
        narrow.global.presets.insert(
            "shell-only".to_string(),
            Preset {
                name: Some("shell-only".to_string()),
                plugins: vec!["shell".to_string()],
                description: None,
                order: None,
            },
        );
        narrow.global.default_preset = "shell-only".to_string();
        let built = build(&narrow);
        assert_eq!(built.agent.config().max_steps, 10);
        let mut carrying: Vec<String> = built
            .plugins
            .iter()
            .filter(|plugin| !plugin.tools.is_empty())
            .map(|plugin| plugin.name.clone())
            .collect();
        carrying.sort();
        assert_eq!(
            carrying,
            vec!["shell".to_string()],
            "the preset allows bash only, so every other bundle keeps no tool"
        );
        let shell = built
            .plugins
            .iter()
            .find(|plugin| plugin.name == "shell")
            .expect("the shell bundle");
        assert!(shell.tools.iter().any(|tool| tool == "bash"));
    }

    /// The registry is the source of the agent tools, and a disabled tool is
    /// filtered out before its bundle is installed.
    #[test]
    fn plugins_carry_the_filtered_tools() {
        let mut settings = bare();
        settings.workspace.disabled.tools = vec!["read_file".to_string()];
        settings.workspace.agent.max_steps = 17;
        let built = build(&settings);
        let names = |plugin: &str| {
            let mut tools = built
                .plugins
                .iter()
                .find(|p| p.name == plugin)
                .map(|p| p.tools.clone())
                .unwrap_or_default();
            tools.sort();
            tools
        };
        assert_eq!(names("shell"), vec!["bash".to_string()]);
        assert_eq!(
            names("files"),
            vec!["list_dir".to_string(), "write_file".to_string()]
        );
        assert_eq!(names("plan"), vec!["update_plan".to_string()]);
        assert_eq!(
            built.agent.config().max_steps,
            17,
            "the loop depth must reach the agent"
        );
    }

    /// A disabled plugin contributes nothing, and the inventory still lists it
    /// so the pane can switch it back on. Paired with the enabled case, so the
    /// filter cannot pass by dropping every tool.
    #[test]
    fn a_disabled_plugin_contributes_no_tools() {
        let on = build(&bare());
        assert!(on
            .plugins
            .iter()
            .any(|plugin| plugin.enabled && plugin.tools.iter().any(|tool| tool == "bash")));

        let mut off_tree = bare();
        off_tree.workspace.disabled.extentions = vec!["shell".to_string()];
        let off = build(&off_tree);
        let shell = off
            .plugins
            .iter()
            .find(|plugin| plugin.name == "shell")
            .expect("shell is still listed");
        assert!(!shell.enabled, "a disabled plugin reads as off");
        assert!(
            shell.tools.is_empty(),
            "the registry must hold none of its tools, and caps adds exactly those"
        );
    }

    /// A scratch directory no other test shares.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-caps-skill-tests")
            .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Write one skill directory the way the agent crate reads it.
    fn write_skill(root: &std::path::Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("the skill directory");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n"),
        )
        .expect("the skill manifest");
    }

    /// A settings tree whose only skill source is \`root\`, named \`First\`.
    fn with_one_source(root: &std::path::Path) -> Settings {
        let mut settings = bare();
        settings
            .global
            .set_skill_sources(vec![crate::settings::global::SkillSource {
                name: "First".to_string(),
                path: root.to_string_lossy().to_string(),
                enabled: true,
            }]);
        settings
    }

    /// The skill names the built agent carries, sorted.
    fn skill_names(built: &BuiltAgent) -> Vec<String> {
        let mut names: Vec<String> = built
            .agent
            .get_skills_content()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();
        names.sort();
        names
    }

    /// A source the reader switched off takes its skills out of the AGENT, not
    /// just out of the pane. Paired with the enabled case, so the filter cannot
    /// pass by registering nothing at all.
    #[test]
    fn a_disabled_source_is_absent_from_the_built_agent() {
        let root = scratch("source-off");
        write_skill(&root, "alpha", "one");
        write_skill(&root, "beta", "two");

        let on = with_one_source(&root);
        assert_eq!(
            skill_names(&build(&on)),
            vec!["alpha".to_string(), "beta".to_string()],
            "the premise: an enabled source registers its skills"
        );

        let mut off = with_one_source(&root);
        off.global.skill_sources[0].enabled = false;
        assert!(
            skill_names(&build(&off)).is_empty(),
            "turning the source off must remove its skills from the agent itself"
        );
    }

    /// A skill switched off on its own is absent from the agent while its
    /// siblings stay, and it is still discoverable so the pane can list it.
    #[test]
    fn a_disabled_skill_is_absent_from_the_built_agent() {
        let root = scratch("skill-off");
        write_skill(&root, "alpha", "one");
        write_skill(&root, "beta", "two");

        let mut settings = with_one_source(&root);
        settings.workspace.disabled.skills = vec!["alpha".to_string()];

        let built = build(&settings);
        assert_eq!(
            skill_names(&built),
            vec!["beta".to_string()],
            "the switched-off skill is gone and its sibling is not"
        );
        let eff = settings.effective();
        assert!(
            eff.skills
                .iter()
                .any(|skill| skill.name == "alpha" && !skill.enabled),
            "the catalogue still carries it, so the pane can switch it back on"
        );
        assert_eq!(
            eff.enabled_skills().count(),
            1,
            "and exactly one skill is buildable"
        );
    }

    /// A source whose directory does not exist registers nothing rather than
    /// failing the build: the pane must not be able to break a turn by existing.
    #[test]
    fn a_source_that_is_not_there_registers_nothing() {
        let missing = scratch("source-missing").join("absent");
        let settings = with_one_source(&missing);
        assert!(
            skill_names(&build(&settings)).is_empty(),
            "a directory that is not there contributes no skills and no error"
        );
    }

    /// Read Only is not a label: the side-effecting tools never reach the
    /// request, so the registry is smaller than the confined build and holds
    /// neither the shell nor the file writer.
    #[test]
    fn read_only_withholds_the_side_effecting_tools() {
        let confined = bare();
        let mut read_only = bare();
        read_only.workspace.agent.permission = READ_ONLY.to_string();

        let names = |settings: &Settings| {
            let built = build(settings);
            let mut names: Vec<String> = built
                .plugins
                .iter()
                .flat_map(|plugin| plugin.tools.clone())
                .collect();
            names.sort();
            names
        };

        assert_eq!(
            names(&confined),
            vec![
                "bash".to_string(),
                "list_dir".to_string(),
                "read_file".to_string(),
                "update_plan".to_string(),
                "write_file".to_string(),
            ]
        );
        assert_eq!(
            names(&read_only),
            vec![
                "list_dir".to_string(),
                "read_file".to_string(),
                "update_plan".to_string(),
            ],
            "Read Only must drop the shell and the writer, and keep the readers"
        );
    }

    /// Full access is the mode that releases the confinement, and the shell
    /// own config is where it lands: an unset workspace root is what leaves
    /// the shell tool validator unset and drops its two guards.
    #[test]
    fn full_access_releases_the_workspace_confinement() {
        assert!(is_confined(WORKSPACE_WRITE));
        assert!(is_confined("something-unknown"));
        assert!(!is_confined(FULL_ACCESS));
        assert!(allows_writes(WORKSPACE_WRITE));
        assert!(allows_writes(FULL_ACCESS));
        assert!(!allows_writes(READ_ONLY));
        assert!(
            allows_writes("something-unknown"),
            "only Read Only withholds the writers"
        );
    }

    /// A zero step ceiling cannot run a loop, so it is floored at one.
    #[test]
    fn a_zero_step_ceiling_is_floored() {
        let mut settings = bare();
        settings.workspace.agent.max_steps = 0;
        assert_eq!(build(&settings).agent.config().max_steps, 1);
    }

    /// A vault in a scratch directory, unlocked, holding one entry.
    fn vault_holding(label: &str, name: &str, secret: &str) -> Vault {
        let directory =
            std::env::temp_dir().join(format!("bsh-caps-vault-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory");
        let mut vault = Vault::new(directory.join(".vault.bin"));
        vault.create("a test passphrase").expect("the vault");
        vault.put(name, secret).expect("the entry");
        vault
    }

    /// A settings tree whose endpoint carries no credential source yet.
    fn provider_without_a_credential() -> Settings {
        let mut settings = bare();
        settings.global.add_provider(
            "work",
            "Work",
            "https://work.example",
            &[ModelEntry::named("work-model")],
        );
        settings
    }

    /// The agent resolves provider:ID:api_key from the vault at build time,
    /// and the settings tree never receives the secret.
    #[test]
    fn a_vault_key_reaches_the_agent_when_no_entry_is_named() {
        let settings = provider_without_a_credential();
        assert!(
            settings
                .global
                .providers
                .get("work")
                .is_some_and(|provider| provider.api_key_vault.is_none()),
            "the premise: no named entry, so the convention supplies the key"
        );
        let secret = "sk-vault-only-value";
        let vault = vault_holding("agent", &crate::vault::provider_key("work"), secret);

        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert_eq!(
            built.agent.config().api_key,
            secret,
            "the vault supplied it"
        );
        assert!(
            built.credential_error.is_none(),
            "a resolved key is not a refusal"
        );

        // The settings file, serialized, carries neither the secret nor a copy
        // of it: the tree was never written to.
        let text = toml::to_string(&settings.global).expect("the settings document");
        assert!(!text.contains(secret), "the secret is in the settings tree");
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with("api_key =")),
            "the settings tree has no plaintext key field: {text}"
        );
    }

    /// A vault with no app-access key is an explicit refusal, not an empty key
    /// handed to the provider.
    #[test]
    fn a_vault_without_app_access_refuses_instead_of_an_empty_key() {
        let settings = provider_without_a_credential();
        let directory =
            std::env::temp_dir().join(format!("bsh-caps-no-app-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the scratch directory");
        let path = directory.join(".vault.bin");
        let mut created = Vault::new(path.clone());
        created.create("a test passphrase").expect("the vault");
        created
            .put(&crate::vault::provider_key("work"), "sk-locked")
            .expect("the entry");
        std::fs::remove_file(path.with_file_name(".vault.access")).expect("the app access file");
        drop(created);
        let vault = Vault::new(path);

        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert!(
            matches!(
                &built.credential_error,
                Some(crate::vault::VaultError::Locked)
            ),
            "a locked vault names itself, got {:?}",
            built.credential_error
        );
        assert!(
            built.agent.config().api_key.is_empty(),
            "the engine is not handed a secret the vault could not open"
        );
        assert!(
            matches!(
                crate::caps::credential_error(&settings, &vault, &work_choice()),
                Some(crate::vault::VaultError::Locked)
            ),
            "the send path reads the same refusal before opening a turn"
        );
        let text = toml::to_string(&settings.global).expect("the settings document");
        assert!(!text.contains("sk-locked"));
    }

    /// Locking the Vault pane does not take provider keys away: the app-access
    /// key keeps the table available for resolution.
    #[test]
    fn a_locked_admin_view_still_supplies_provider_keys() {
        let settings = provider_without_a_credential();
        let mut vault = vault_holding(
            "admin-locked-app-access",
            &crate::vault::provider_key("work"),
            "sk-app-still-works",
        );
        vault.lock();
        assert!(!vault.is_unlocked(), "the Vault pane reads locked");
        assert!(vault.is_app_available(), "the app still has its table");

        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert_eq!(built.agent.config().api_key, "sk-app-still-works");
        assert!(
            built.credential_error.is_none(),
            "a locked admin view is not a credential refusal"
        );
        assert!(crate::caps::credential_error(&settings, &vault, &work_choice()).is_none());
    }

    /// A plaintext key left in an old document is not a credential: the build
    /// reads only the vault, and the old value is never written back.
    #[test]
    fn a_legacy_plaintext_key_is_not_a_credential() {
        let mut settings = provider_without_a_credential();
        if let Some(provider) = settings.global.providers.get_mut("work") {
            provider.legacy_api_key = Some("sk-plaintext-legacy".to_string());
        }
        let vault = vault_holding("plaintext", "keys/other", "sk-vault-other");
        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert!(
            built.agent.config().api_key.is_empty(),
            "the legacy value never authenticates a request"
        );
        assert!(built.credential_error.is_none());
        let text = toml::to_string(&settings.global).expect("the settings document");
        assert!(!text.contains("sk-plaintext-legacy"));
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with("api_key =")),
            "the legacy field is not written back: {text}"
        );
    }

    /// A named vault entry is read and beats the conventional
    /// `provider:<id>:api_key` entry.
    #[test]
    fn a_named_vault_entry_supplies_the_secret() {
        let mut settings = provider_without_a_credential();
        if let Some(provider) = settings.global.providers.get_mut("work") {
            provider.api_key_vault = Some("keys/chosen".to_string());
        }
        let secret = "sk-named-entry-value";
        let mut vault = vault_holding("named", "keys/chosen", secret);
        vault
            .put(&crate::vault::provider_key("work"), "sk-conventional-stale")
            .expect("the conventional entry");
        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert_eq!(built.agent.config().api_key, secret);
        assert!(
            built.credential_error.is_none(),
            "a resolved named entry is not a refusal"
        );
        assert!(
            crate::caps::credential_error(&settings, &vault, &work_choice()).is_none(),
            "the send path reads the same answer"
        );
    }

    /// A named entry the vault does not hold is an explicit refusal carrying
    /// the id, never a silent empty key.
    #[test]
    fn a_missing_named_vault_entry_names_the_id() {
        let mut settings = provider_without_a_credential();
        if let Some(provider) = settings.global.providers.get_mut("work") {
            provider.api_key_vault = Some("keys/absent".to_string());
        }
        let vault = vault_holding("named-missing", "keys/other", "sk-unrelated");
        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert!(
            built.agent.config().api_key.is_empty(),
            "an unresolved named entry cannot become an empty-key request"
        );
        match &built.credential_error {
            Some(crate::vault::VaultError::MissingEntry(name)) => {
                assert_eq!(name, "keys/absent");
            }
            other => panic!("expected a named miss, got {other:?}"),
        }
        let described = built
            .credential_error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            described.contains("keys/absent"),
            "the refusal names the id: {described}"
        );
        assert!(
            matches!(
                crate::caps::credential_error(&settings, &vault, &work_choice()),
                Some(crate::vault::VaultError::MissingEntry(ref name)) if name == "keys/absent"
            ),
            "the send path names the same id before opening a turn"
        );
        // Locking the admin view leaves the app access table in place, so the
        // named miss is still the answer rather than a generic lock refusal.
        let mut locked = vault;
        locked.lock();
        assert!(
            matches!(
                crate::caps::credential_error(&settings, &locked, &work_choice()),
                Some(crate::vault::VaultError::MissingEntry(ref name)) if name == "keys/absent"
            ),
            "the admin lock does not hide the named miss"
        );
        // A file that does not exist cannot hold the named entry, so the id is
        // still the answer rather than a generic absence.
        let absent = Vault::new(
            std::env::temp_dir().join(format!("bsh-caps-absent-{}", std::process::id())),
        );
        assert!(
            matches!(
                crate::caps::credential_error(&settings, &absent, &work_choice()),
                Some(crate::vault::VaultError::MissingEntry(ref name)) if name == "keys/absent"
            ),
            "no vault file names the missing id for a provider that chose one"
        );
    }

    /// A provider carries the choice as an id, and the settings document that
    /// writes it never carries the secret.
    #[test]
    fn a_vault_choice_round_trips_without_the_secret() {
        let mut settings = provider_without_a_credential();
        if let Some(provider) = settings.global.providers.get_mut("work") {
            provider.api_key_vault = Some("keys/chosen".to_string());
        }
        let secret = "sk-never-in-settings";
        let vault = vault_holding("roundtrip", "keys/chosen", secret);
        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert_eq!(built.agent.config().api_key, secret);

        let text = toml::to_string(&settings.global).expect("the settings document");
        assert!(
            text.contains("api_key_vault = \"keys/chosen\""),
            "the choice is serialized as an id: {text}"
        );
        assert!(!text.contains(secret), "the secret is in the settings tree");
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with("api_key =")),
            "only the vault id is stored: {text}"
        );
        let back = crate::settings::global::GlobalSettings::from_toml(&text)
            .expect("the provider round-trips");
        assert_eq!(
            back.providers
                .get("work")
                .and_then(|provider| provider.api_key_vault.as_deref()),
            Some("keys/chosen"),
            "the choice survives a round trip"
        );
    }

    /// With no vault file, an empty key stays empty: environment authentication
    /// is a real mode and the vault must not invent a refusal.
    #[test]
    fn no_vault_leaves_the_environment_path_alone() {
        let settings = provider_without_a_credential();
        let directory =
            std::env::temp_dir().join(format!("bsh-caps-no-vault-{}", std::process::id()));
        let vault = Vault::new(directory.join(".vault.bin"));
        let built = build_agent(
            &settings,
            &vault,
            &work_choice(),
            &crate::mcp::McpPool::default(),
        );
        assert!(built.agent.config().api_key.is_empty());
        assert!(
            built.credential_error.is_none(),
            "no vault is not a refusal"
        );
        assert!(crate::caps::credential_error(&settings, &vault, &work_choice()).is_none());
    }
}
