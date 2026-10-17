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
}

/// Build the agent for the active workspace.
pub(crate) fn build_agent(settings: &Settings) -> BuiltAgent {
    let eff = settings.effective();
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
        api_key: eff.api_key.clone(),
        system_prompt: eff.system_prompt.clone(),
        temperature: eff.temperature,
        reasoning_effort: eff.reasoning_effort.clone(),
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

    if !eff.skills_dir.trim().is_empty() {
        let dir = expand_path(&eff.skills_dir);
        if dir.is_dir() {
            let _ = agent.register_skills_from_dir_denying(dir, &eff.disabled_skills);
        }
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
    }
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
            let description = registry
                .get(&name)
                .map(|tool| tool.description().to_string())
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
    use super::{allows_writes, build_agent, is_confined, FULL_ACCESS, READ_ONLY, WORKSPACE_WRITE};
    use crate::settings::global::{ModelEntry, Preset};
    use crate::settings::Settings;

    /// A bare tree, which is what a fresh install carries.
    fn bare() -> Settings {
        Settings::default()
    }

    /// The meter tools figure is the serialized request array: nonzero for a
    /// built-in tool set, and strictly smaller once a tool is switched off.
    #[test]
    fn the_tool_schema_length_follows_the_registered_tools() {
        let all = build_agent(&bare()).tools_schema_chars;
        let mut settings = bare();
        settings.workspace.agent.bash_enabled = false;
        let fewer = build_agent(&settings).tools_schema_chars;
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
            build_agent(&settings).agent.config().web_search,
            "the setting reaches the agent config"
        );

        settings.workspace.agent.web_tools_enabled = false;
        assert!(
            !build_agent(&settings).agent.config().web_search,
            "switching it off withdraws the tool"
        );
    }

    /// A settings selection of a provider is the endpoint the agent is built
    /// against, so what the models pane lists really reaches the agent.
    #[test]
    fn a_selected_provider_is_the_endpoint() {
        let mut settings = bare();
        settings.global.add_provider(
            "work",
            "Work",
            "https://work.example",
            "work-key",
            &[ModelEntry::named("work-model")],
        );
        settings.workspace.general.provider = "work".to_string();
        settings.workspace.general.model = "work-model".to_string();
        let built = build_agent(&settings);
        let config = built.agent.config();
        // The wire form is the prefix the engine registers a vendor on.
        assert_eq!(config.model, "work/work-model");
        assert_eq!(config.base_url, "https://work.example");
    }

    /// The agent-loop switch reaches the agent config, not just the file.
    #[test]
    fn the_parallel_switch_reaches_the_agent_config() {
        let off = bare();
        assert!(!build_agent(&off).agent.config().parallel_tool_calls);
        let mut on = bare();
        on.workspace.agent.parallel_tool_calls = true;
        assert!(build_agent(&on).agent.config().parallel_tool_calls);
    }

    /// The API protocol the user picks reaches the agent config, not just the
    /// file - which is what makes the models pane row a control rather than a
    /// label.
    #[test]
    fn the_api_protocol_reaches_the_agent_config() {
        let mut settings = bare();
        settings
            .global
            .add_provider("p", "", "https://p.example", "", &[ModelEntry::named("m")]);
        settings.workspace.general.provider = "p".to_string();
        settings.workspace.general.model = "m".to_string();
        settings.global.set_family_protocol("p", "responses");
        assert_eq!(build_agent(&settings).agent.config().api_mode, "responses");
        settings.global.set_family_protocol("p", "chat");
        assert_eq!(build_agent(&settings).agent.config().api_mode, "chat");
    }

    /// The Max output tokens row reaches the agent config. Two explicit values
    /// are paired so the assertion cannot be satisfied by a default the BOS
    /// config happens to carry for this model; an empty field clears the cap,
    /// which leaves the provider own default in charge.
    #[test]
    fn the_output_cap_reaches_the_agent_config() {
        let mut capped = bare();
        capped.workspace.agent.max_tokens = Some(4096);
        assert_eq!(build_agent(&capped).agent.config().max_tokens, Some(4096));
        let mut smaller = bare();
        smaller.workspace.agent.max_tokens = Some(2048);
        assert_eq!(build_agent(&smaller).agent.config().max_tokens, Some(2048));
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
        let built = build_agent(&narrow);
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
        let built = build_agent(&settings);
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
        let on = build_agent(&bare());
        assert!(on
            .plugins
            .iter()
            .any(|plugin| plugin.enabled && plugin.tools.iter().any(|tool| tool == "bash")));

        let mut off_tree = bare();
        off_tree.workspace.disabled.extentions = vec!["shell".to_string()];
        let off = build_agent(&off_tree);
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

    /// Read Only is not a label: the side-effecting tools never reach the
    /// request, so the registry is smaller than the confined build and holds
    /// neither the shell nor the file writer.
    #[test]
    fn read_only_withholds_the_side_effecting_tools() {
        let confined = bare();
        let mut read_only = bare();
        read_only.workspace.agent.permission = READ_ONLY.to_string();

        let names = |settings: &Settings| {
            let built = build_agent(settings);
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
        assert_eq!(build_agent(&settings).agent.config().max_steps, 1);
    }
}
