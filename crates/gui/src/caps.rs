//! Capability layer: what the chat agent can actually do.
//!
//! [`build_agent`] turns the user's [`Settings`] into a fully-equipped
//! [`Agent`] — a bash tool, filesystem tools, an always-on `update_plan`
//! tool whose shared store backs the plan panel, skills discovered from the
//! skills directory, and (when enabled) a persistent [`agent::memory::FileMemory`]
//! with auto-remember, so the GUI chat is a working coding agent with
//! long-term memory rather than a bare LLM chat. [`capabilities_of`] reports
//! what an agent carries so the frontend can display tools, skills, and
//! plugins.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::tools::FunctionTool;
use agent::{Agent, BashTool};
use serde::Serialize;

use crate::settings::Settings;

/// Largest file a `read_file` call will return, in bytes.
const MAX_READ_BYTES: usize = 256 * 1024;

/// One registered tool, plugin, or skill for display.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CapInfo {
    /// Registered name (for tools this is the callable name).
    pub(crate) name: String,
    /// Human-readable description.
    pub(crate) description: String,
    /// Grouping category (`builtin`, `skill`, …).
    pub(crate) category: String,
}

/// Snapshot of everything the agent can do, for the capabilities view.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Capabilities {
    /// Synchronous tools with descriptions.
    pub(crate) tools: Vec<CapInfo>,
    /// Asynchronous tools (e.g. MCP-backed) with descriptions.
    pub(crate) async_tools: Vec<CapInfo>,
    /// Skills loaded from the skills directory.
    pub(crate) skills: Vec<CapInfo>,
    /// Registered agent plugin names.
    pub(crate) plugins: Vec<String>,
    /// The skills directory the agent was built from.
    pub(crate) skills_dir: String,
    /// Whether the bash tool is enabled.
    pub(crate) bash_enabled: bool,
    /// Whether the filesystem tools are enabled.
    pub(crate) file_tools_enabled: bool,
}

/// Expand a leading `~` the same way the settings paths expect.
fn expand_tilde(raw: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(raw.trim()).into_owned())
}

/// Resolve `raw` against the optional workspace root.
///
/// Without a root, paths resolve against the process working directory. With
/// a root, relative paths join the root and the result must stay inside it —
/// the containment check canonicalizes the existing prefix so `..` and
/// symlinks cannot escape a locked-down workspace.
///
/// `pub(crate)` so the approval gate can resolve the same path the write
/// tool will target and show a before/after diff of it.
pub(crate) fn resolve_path(root: &Option<PathBuf>, raw: &str) -> Result<PathBuf, String> {
    let raw_path = Path::new(raw);
    let Some(root) = root else {
        let abs = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("cannot read working directory: {e}"))?
                .join(raw_path)
        };
        return Ok(abs);
    };
    let joined = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        root.join(raw_path)
    };
    let canon_root = root
        .canonicalize()
        .map_err(|e| format!("workspace {} unavailable: {e}", root.display()))?;
    // Canonicalize the deepest existing ancestor, then re-append the rest, so
    // writes to not-yet-created files still pass the containment check.
    let mut probe = joined.clone();
    let mut tail: Vec<PathBuf> = Vec::new();
    while !probe.exists() {
        match (probe.file_name(), probe.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(PathBuf::from(name));
                probe = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut checked = probe
        .canonicalize()
        .map_err(|e| format!("{}: {e}", probe.display()))?;
    for part in tail.iter().rev() {
        checked.push(part);
    }
    if !checked.starts_with(&canon_root) {
        return Err(format!("path escapes the workspace: {raw}"));
    }
    Ok(checked)
}

fn json_arg_err(msg: &str) -> String {
    msg.to_string()
}

/// `read_file` — return a UTF-8 file's contents (capped at 256 KiB).
fn read_file_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "read_file",
        "Read a UTF-8 text file and return its contents. Paths may be absolute or relative to the workspace.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to read" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        move |args| {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| agent::ToolError::InvalidInput(json_arg_err("path is required")))?;
            let target = resolve_path(&root, path).map_err(agent::ToolError::InvalidInput)?;
            let bytes =
                std::fs::read(&target).map_err(|e| agent::ToolError::Failed(format!("{}: {e}", target.display())))?;
            let (content, truncated) = if bytes.len() > MAX_READ_BYTES {
                (String::from_utf8_lossy(&bytes[..MAX_READ_BYTES]).into_owned(), true)
            } else {
                (String::from_utf8_lossy(&bytes).into_owned(), false)
            };
            Ok(serde_json::json!({
                "path": target.display().to_string(),
                "content": content,
                "truncated": truncated,
                "bytes": bytes.len(),
            }))
        },
    )
}

/// `write_file` — create parent directories and write UTF-8 content.
fn write_file_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "write_file",
        "Write a UTF-8 text file, creating parent directories as needed. Paths may be absolute or relative to the workspace.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to write" },
                "content": { "type": "string", "description": "Full file contents" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        move |args| {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| agent::ToolError::InvalidInput(json_arg_err("path is required")))?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| agent::ToolError::InvalidInput(json_arg_err("content is required")))?;
            let target = resolve_path(&root, path).map_err(agent::ToolError::InvalidInput)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| agent::ToolError::Failed(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(&target, content)
                .map_err(|e| agent::ToolError::Failed(format!("{}: {e}", target.display())))?;
            Ok(serde_json::json!({
                "path": target.display().to_string(),
                "bytes": content.len(),
            }))
        },
    )
}

/// `list_dir` — list a directory's entries with their kind.
fn list_dir_tool(root: Option<PathBuf>) -> FunctionTool {
    FunctionTool::new(
        "list_dir",
        "List a directory's entries (name and kind). Paths may be absolute or relative to the workspace.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        move |args| {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| agent::ToolError::InvalidInput(json_arg_err("path is required")))?;
            let target = resolve_path(&root, path).map_err(agent::ToolError::InvalidInput)?;
            let mut entries = Vec::new();
            let reader = std::fs::read_dir(&target)
                .map_err(|e| agent::ToolError::Failed(format!("{}: {e}", target.display())))?;
            for entry in reader.flatten() {
                let kind = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    "dir"
                } else {
                    "file"
                };
                entries.push(serde_json::json!({
                    "name": entry.file_name().to_string_lossy(),
                    "kind": kind,
                }));
            }
            entries.sort_by_key(|e| e.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string());
            Ok(serde_json::json!({
                "path": target.display().to_string(),
                "entries": entries,
            }))
        },
    )
}

/// Build the chat agent from settings: LLM config plus tools and skills.
///
/// The bash tool, the filesystem tools, and the skills directory are each
/// controlled by a settings flag/field; a missing skills directory simply
/// yields no skills. Side-effecting tools (`bash`, `write_file`) are wrapped
/// in an approval gate when `require_approval` is on.
pub(crate) fn build_agent(
    settings: &Settings,
    broker: Arc<crate::approval::ApprovalBroker>,
) -> Arc<Agent> {
    let mut agent = Agent::from_config(settings.agent_config());

    let ws = settings.bash_workspace.trim();
    let root: Option<PathBuf> = (!ws.is_empty()).then(|| expand_tilde(ws));
    // The approval gate resolves write targets against this root to show
    // before/after review diffs in the webview.
    broker.set_workspace(root.clone());

    if settings.bash_enabled {
        let tool: Arc<dyn agent::tools::Tool> = match &root {
            Some(r) => Arc::new(BashTool::new("bash").with_workspace(&r.to_string_lossy())),
            None => Arc::new(BashTool::new("bash")),
        };
        crate::approval::add_gated_tool(
            &mut agent,
            tool,
            settings.require_approval,
            broker.clone(),
        );
    }
    if settings.file_tools_enabled {
        // Read-only tools never need approval; writes and commands do.
        let read: Arc<dyn agent::tools::Tool> = Arc::new(read_file_tool(root.clone()));
        agent.add_tool(read);
        let write: Arc<dyn agent::tools::Tool> = Arc::new(write_file_tool(root.clone()));
        crate::approval::add_gated_tool(
            &mut agent,
            write,
            settings.require_approval,
            broker.clone(),
        );
        let list: Arc<dyn agent::tools::Tool> = Arc::new(list_dir_tool(root.clone()));
        agent.add_tool(list);
    }

    // Planning is always on: the model tracks its steps in the agent's own
    // shared plan store (`Agent::plan`), which the plan panel renders and
    // sessions persist — read-only state, so no approval gate.
    let plan: Arc<dyn agent::tools::Tool> = Arc::new(agent::tools::PlanTool::new(agent.plan()));
    agent.add_tool(plan);

    let skills = settings.skills_dir.trim();
    if !skills.is_empty() {
        // A missing directory discovers nothing; that is not an error.
        let _ = agent.register_skills_from_dir(expand_tilde(skills));
    }

    if settings.memory_enabled {
        let raw = settings.memory_path.trim();
        let path = if raw.is_empty() {
            expand_tilde(crate::settings::DEFAULT_MEMORY_PATH)
        } else {
            expand_tilde(raw)
        };
        if let Some(memory) = open_file_memory(&path) {
            agent = agent.with_memory(Arc::new(memory));
            // The low-level agent stores each completed exchange itself
            // (Agent::remember_exchange), so the GUI stream path stays thin.
            agent.set_auto_remember(true);
        }
    }

    Arc::new(agent)
}

/// Open (and cap) a persistent [`agent::memory::FileMemory`] at `path`.
///
/// A missing file opens empty and the parent directory is created lazily by
/// the store's first write, so opening never touches the filesystem beyond
/// reading. Any failure returns `None`: a chat agent without memory is still
/// fully functional, so a broken path degrades instead of aborting the build.
/// The open runs on a short-lived worker thread with its own current-thread
/// runtime because `FileMemory::open` is async while `build_agent` is sync
/// and may itself be called from inside an async context (the MCP rebuild
/// path does), where blocking on a nested runtime would panic.
fn open_file_memory(path: &Path) -> Option<agent::memory::FileMemory> {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let memory = runtime
            .block_on(agent::memory::FileMemory::open(&path))
            .ok()?;
        Some(memory.with_max_items(500))
    })
    .join()
    .ok()
    .flatten()
}

/// Describe an agent's registered tools, skills, and plugins.
///
/// `settings` supplies the display-only fields (skills directory and the
/// tool toggles) that are not stored on the agent itself.
pub(crate) fn capabilities_of(agent: &Agent, settings: &Settings) -> Capabilities {
    let mut tools = Vec::new();
    let mut async_tools = Vec::new();
    if let Some(reg) = agent.registry() {
        for (name, tool) in reg.iter() {
            tools.push(CapInfo {
                name: name.clone(),
                description: tool.description(),
                category: tool.category(),
            });
        }
        for name in reg.async_tool_names() {
            // MCP tools are external ("plugin") tools; show them apart.
            let category = if reg.is_mcp_tool(&name) {
                "mcp"
            } else {
                "async"
            };
            let info = reg.get_async(&name).map(|tool| CapInfo {
                name: name.clone(),
                description: tool.description(),
                category: category.to_string(),
            });
            async_tools.push(info.unwrap_or(CapInfo {
                name,
                description: String::new(),
                category: category.to_string(),
            }));
        }
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    async_tools.sort_by(|a, b| a.name.cmp(&b.name));

    let mut skills: Vec<CapInfo> = agent
        .get_skills_content()
        .into_iter()
        .map(|(name, instructions)| {
            let summary = instructions
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("")
                .to_string();
            CapInfo {
                name: name.to_string(),
                description: summary,
                category: "skill".to_string(),
            }
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    Capabilities {
        tools,
        async_tools,
        skills,
        plugins: agent.plugins().plugin_names_blocking(),
        skills_dir: settings.skills_dir.clone(),
        bash_enabled: settings.bash_enabled,
        file_tools_enabled: settings.file_tools_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_names(agent: &Agent) -> Vec<String> {
        agent.registry().map(|r| r.list()).unwrap_or_default()
    }

    #[test]
    fn default_settings_register_tools() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        let names = tool_names(&agent);
        assert!(
            names.contains(&"bash".to_string()),
            "bash missing: {names:?}"
        );
        assert!(
            names.contains(&"read_file".to_string()),
            "read_file missing: {names:?}"
        );
        assert!(
            names.contains(&"write_file".to_string()),
            "write_file missing: {names:?}"
        );
        assert!(
            names.contains(&"list_dir".to_string()),
            "list_dir missing: {names:?}"
        );
    }

    #[test]
    fn toggles_remove_tools() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.bash_enabled = false;
        settings.file_tools_enabled = false;
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        // The plan tool is the one always-on capability: it only mutates
        // in-memory state, so no flag turns it off.
        assert_eq!(
            tool_names(&agent),
            vec!["update_plan".to_string()],
            "only the plan tool should remain"
        );
    }

    /// Planning is registered on every build and shares the agent's store:
    /// driving the tool like the model would is visible through `plan_items`.
    #[test]
    fn plan_tool_is_registered_and_shares_the_store() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        let names = tool_names(&agent);
        assert!(
            names.contains(&"update_plan".to_string()),
            "update_plan missing: {names:?}"
        );

        let reg = agent.registry().expect("registry");
        let tool = reg.get("update_plan").expect("plan tool registered");
        tool.run(&serde_json::json!({
            "items": [{"text": "check capabilities", "status": "in_progress"}]
        }))
        .expect("plan update");

        let items = agent.plan_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "check capabilities");
    }

    #[test]
    fn missing_skills_dir_is_not_an_error() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.skills_dir = "/definitely/not/a/real/skills/dir".into();
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        assert!(agent.get_skills_content().is_empty());
    }

    #[test]
    fn workspace_containment_blocks_escapes() {
        let root = std::env::temp_dir().join("bos-gui-caps-root");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let ok = resolve_path(&Some(root.clone()), "inside.txt").unwrap();
        assert!(ok.starts_with(&root));
        let err = resolve_path(&Some(root.clone()), "../escape.txt").unwrap_err();
        assert!(err.contains("escapes"), "unexpected error: {err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn capabilities_snapshot_lists_tools() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        let caps = capabilities_of(&agent, &settings);
        let all: Vec<&CapInfo> = caps.tools.iter().chain(caps.async_tools.iter()).collect();
        assert!(all.iter().any(|t| t.name == "bash"));
        assert!(all.iter().any(|t| t.name == "write_file"));
        assert!(all.iter().any(|t| t.name == "read_file"));
    }

    #[test]
    fn memory_attaches_with_auto_remember() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        let dir = std::env::temp_dir().join(format!(
            "bos-gui-caps-mem-{}-{}",
            std::process::id(),
            "attach"
        ));
        settings.memory_path = dir.join("memory.jsonl").to_string_lossy().into();
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        assert!(agent.memory().is_some(), "file memory must attach");
        assert!(
            agent.auto_remember(),
            "chat exchanges store themselves via the low-level agent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_disabled_leaves_agent_bare() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.memory_enabled = false;
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        assert!(agent.memory().is_none());
        assert!(!agent.auto_remember());
    }

    #[test]
    fn require_approval_gates_side_effecting_tools() {
        let settings = Settings::from_config_value(&serde_json::Value::Null);
        assert!(settings.require_approval, "approval defaults to on");
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        let reg = agent.registry().expect("registry");
        let gated = reg.async_tool_names();
        assert!(
            gated.contains(&"bash".to_string()),
            "bash must be gated: {gated:?}"
        );
        assert!(
            gated.contains(&"write_file".to_string()),
            "write_file must be gated: {gated:?}"
        );
        // Read-only tools stay ungated.
        assert!(!gated.contains(&"read_file".to_string()));
        assert!(!gated.contains(&"list_dir".to_string()));
        let sync: Vec<String> = reg.iter().map(|(name, _)| name.clone()).collect();
        assert!(sync.contains(&"read_file".to_string()));
        assert!(!sync.contains(&"bash".to_string()), "gated tools move out");
    }

    #[test]
    fn disabling_approval_keeps_tools_synchronous() {
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.require_approval = false;
        let agent = build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        );
        let reg = agent.registry().expect("registry");
        assert!(reg.async_tool_names().is_empty());
        let names = tool_names(&agent);
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"write_file".to_string()));
    }

    #[test]
    fn mcp_tools_get_plugin_category() {
        let echo = |name: &str| {
            Arc::new(agent::tools::AsyncFunctionTool::new(
                name,
                "echo",
                serde_json::json!({"type": "object"}),
                |input: &serde_json::Value| {
                    let value = input.clone();
                    Box::pin(async move { Ok(value) })
                },
            ))
        };
        // One tool registered through the MCP path (marked in the registry),
        // one plain async tool, for contrast.
        let mut agent = Agent::from_config(agent::AgentConfig::default());
        agent
            .add_mcp_tool("srv", "read", echo("srv_read"))
            .expect("mcp attach");
        agent
            .try_add_async_tool(echo("plain"))
            .expect("plain async");

        let settings = Settings::from_config_value(&serde_json::Value::Null);
        let caps = capabilities_of(&agent, &settings);
        let mcp: Vec<&CapInfo> = caps
            .async_tools
            .iter()
            .filter(|t| t.category == "mcp")
            .collect();
        assert_eq!(mcp.len(), 1, "exactly one plugin tool: {mcp:?}");
        assert_eq!(mcp[0].name, "srv_read");
        let plain = caps
            .async_tools
            .iter()
            .find(|t| t.name == "plain")
            .expect("plain tool listed");
        assert_eq!(plain.category, "async", "non-MCP async stays async");
    }
}
