//! Building the chat agent from settings.
//!
//! The agent is rebuilt on every turn from the persisted settings, so a
//! settings change reaches the very next send without an app restart. How much
//! the agent may do is the permission mode, and every arm of it lands on a real
//! seam: Read Only withholds the side-effecting tools from the request, Workspace
//! Write confines the shell and the file tools to the active workspace, and Full
//! access releases both.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::memory::{FileMemory, MemoryStore};
use agent::policy::{
    Action, EgressGate, PolicyData, PolicyInput, PolicySet, PolicySource, SystemResolver, Target,
};
use agent::tools::{BashTool, FunctionTool, PlanTool};
use agent::{Agent, AgentConfig, ToolError};
use futures::StreamExt;
use log::warn;

use crate::plugins::{self, PluginInfo};
use crate::settings::{Effective, Settings};
use crate::vault::{self, Vault, VaultError};

/// The capabilities this shell can actually put in front of the reader.
///
/// A package that loads is not the same as a package that is usable here. The
/// registry accepts every contribution kind, but bsh draws four of them: a
/// package's tools become model-facing tools, its hooks become agent plugins,
/// its skills become skill manifests, and its providers become entries in the
/// model picker.
///
/// The other three land in the host registry and stop there. A command is
/// registered but bsh's composer palette is a fixed list, so a reader never
/// sees it; a panel is registered but nothing renders a registry panel; a
/// prompt has no consumer at all. Listing them beside the four that work
/// would put a capability chip on a control the reader cannot reach, which is
/// the same lie as declaring a capability that never loaded.
///
/// The filter is a pure function of the loaded set, so it cannot drift from
/// what the registry holds.
pub(crate) fn surfaced_capabilities(
    loaded: &[agent::extensions::Capability],
) -> Vec<agent::extensions::Capability> {
    use agent::extensions::Capability;
    loaded
        .iter()
        .copied()
        .filter(|capability| {
            matches!(
                capability,
                Capability::Tools | Capability::Hooks | Capability::Skills | Capability::Models
            )
        })
        .collect()
}

/// Lower one extension package's declared model providers onto this shell's
/// provider node.
///
/// A `ProviderContribution` is the declarative form of what a DSH package
/// registers imperatively with `ctx.llm.registerAdapter([id], adapter)`: an id,
/// a base URL, a protocol, and the models it serves. This is the whole of the
/// translation, and it is deliberately a pure function of the declaration - no
/// package file is executed and no network call is made, so a package cannot
/// make the shell do work merely by being installed.
///
/// A model nickname and a model id are the same string here. The reference
/// picker shows the upstream id, and the wire form the engine dispatches on is
/// `<provider>/<model>`, which reads back the same way, so introducing a
/// separate display nickname would be a second name for one fact.
pub(crate) fn plugin_provider_node(
    provider: &agent::extensions::ProviderContribution,
) -> crate::settings::global::Provider {
    use crate::settings::global::{ModelEntry, Provider};
    let mut models = std::collections::BTreeMap::new();
    for model in &provider.models {
        let id = model.id.trim();
        if id.is_empty() {
            continue;
        }
        models.insert(
            id.to_string(),
            ModelEntry {
                model: id.to_string(),
                name: model.name.clone(),
                max_tokens: model.max_tokens,
                api_mode: provider.api_mode.clone(),
                reasoning_effort: model.reasoning_effort.clone(),
                input_types: model.input_types.clone(),
            },
        );
    }
    Provider {
        name: provider.name.clone().or_else(|| Some(provider.id.clone())),
        base_url: provider.base_url.trim().to_string(),
        // A contributed provider NEVER carries a credential: the secret is the
        // reader's, and it reaches the endpoint through the conventional
        // `provider:<id>:api_key` vault entry at build time.
        api_key_vault: None,
        legacy_api_key: None,
        // Not the reader's declaration, so the pane must not offer to delete it
        // as one; the package is the owner.
        declared: false,
        api_mode: provider.api_mode.clone(),
        models,
    }
}

/// Rebuild the contributed model providers from the enabled extension
/// inventory.
///
/// This is the one place the three contributed maps are written, and it runs
/// per build rather than per frame: a package directory is read only when its
/// `(path, mtime)` stamp moved, so a steady state costs a stat per package.
///
/// The gates are the same ones the extension loader applies to tools, so a
/// package switched off in a workspace contributes no provider there, and a
/// preset that allows only some extensions cannot leak an endpoint through the
/// model layer.
pub(crate) fn refresh_plugin_providers(settings: &mut Settings) {
    use std::collections::BTreeMap;

    let mut providers: BTreeMap<String, crate::settings::global::Provider> = BTreeMap::new();
    let mut keys: BTreeMap<String, bool> = BTreeMap::new();
    let mut owners: BTreeMap<String, String> = BTreeMap::new();

    // The inventory is read from the persisted global document, not from a
    // draft: a build describes what is installed, and the pane's unsaved edits
    // are not yet facts.
    let inventory = settings.global.extensions.clone();
    for entry in &inventory {
        let Some(raw) = entry
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        let root = expand_path(raw);
        let Ok(package) = agent::extensions::ExtensionRegistry::load(&root) else {
            continue;
        };
        let extension = package.extension;
        for contribution in &extension.contributions {
            let agent::extensions::Contribution::Provider(provider) = contribution else {
                continue;
            };
            // The reader's own declaration wins an id collision, and so does an
            // earlier package: the tree is keyed by id, and replacing a node
            // another owner already placed would make the inventory order
            // silently decide which endpoint a session runs against.
            if settings.global.providers.contains_key(&provider.id)
                || providers.contains_key(&provider.id)
            {
                continue;
            }
            providers.insert(provider.id.clone(), plugin_provider_node(provider));
            keys.insert(provider.id.clone(), provider.requires_key);
            owners.insert(provider.id.clone(), extension.id.clone());
        }
    }

    settings
        .global
        .set_plugin_providers(providers, keys, owners);
}

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

/// Which tools a turn may offer, after permission, preset and policy agree.
///
/// One predicate owns this decision: registration reads these fields and
/// nothing else decides whether a gated tool exists. `bash` and the two job
/// tools are the shell family; `subagent` is the delegation tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolGates {
    /// Whether the `bash` tool may be registered at all.
    pub(crate) bash: bool,
    /// Whether that shell may expose `run_in_background`.
    pub(crate) background_spawn: bool,
    /// Whether `job_output` may read a job's captured output.
    pub(crate) output: bool,
    /// Whether `job_kill` may terminate a job.
    pub(crate) kill: bool,
    /// Whether the `subagent` tool may be attached.
    pub(crate) subagent: bool,
}

/// The single tool-capability predicate. A tool is offered only when **all**
/// three gates allow it:
///
/// 1. the permission matrix grants side effects (`read-only` grants none of
///    `bash`, `run_in_background`, `job_output`, `job_kill` or
///    `subagent`; the two writable modes grant all five);
/// 2. the preset's `tools` allowlist either is absent (inherit) or names the
///    tool - an absent list inherits, an explicit list intersects; and
/// 3. the Rego policy answers the capability question for the tool name with
///    `allow`. `run_in_background` is the shell's own capability, so its
///    preset entry is `bash`.
///
/// The permission gate is applied first and short-circuits: a read-only turn
/// asks the policy nothing, so the audit ring is not filled with questions its
/// own matrix already refused. The Rego baseline carries the same rule for a
/// caller that asks it directly.
pub(crate) fn tool_gates_for(
    permission: &str,
    preset_tools: Option<&[String]>,
    policy: &PolicySet,
    session: &str,
) -> ToolGates {
    let allowed = allows_writes(permission);
    let in_preset =
        |name: &str| preset_tools.is_none_or(|tools| tools.iter().any(|tool| tool == name));
    // The capability question: tool name plus the turn's principal. The target
    // is the workspace root (capability asks about a tool, not a path).
    let capability = |tool: &str| {
        let input = PolicyInput::new(
            tool,
            Action::Capability,
            Target::path(policy.data().workspace_root.clone()),
        )
        .with_session(session)
        .with_preset(policy.data().preset.clone());
        policy.check(&input).is_allowed()
    };
    let bash = in_preset("bash");
    ToolGates {
        bash: allowed && bash,
        background_spawn: allowed && bash && capability("run_in_background"),
        output: allowed && in_preset("job_output") && capability("job_output"),
        kill: allowed && in_preset("job_kill") && capability("job_kill"),
        subagent: allowed && in_preset("subagent") && capability("subagent"),
    }
}

/// Ask the policy whether `tool` may spawn a process or child in `root`.
///
/// This is the call-time half of the capability decision: registration narrows
/// what a turn carries, and this check refuses a call that a policy narrows
/// later (the workspace policy is read on every build, so both halves see the
/// same source). The error is the policy's own refusal, which names the rule.
pub(crate) fn check_spawn(
    policy: &PolicySet,
    tool: &str,
    session: &str,
    preset: &str,
    root: &str,
) -> Result<(), String> {
    let input = PolicyInput::new(tool, Action::Spawn, Target::path(root))
        .with_session(session.to_string())
        .with_preset(preset.to_string());
    let decision = policy.check(&input);
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(policy.refusal(&decision))
    }
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
    /// The tool-boundary policy this turn's tools ask before they act.
    ///
    /// It is compiled once here, not per tool call, and every file/shell tool
    /// holds a clone. The denial ring and allow counters are readable through
    /// it for the UI a later slice adds; until that pane exists, the tests are
    /// the reader, so the field is deliberately kept without a production
    /// caller.
    #[allow(dead_code)]
    pub(crate) policy: Arc<PolicySet>,
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
    build_agent_for_session(settings, vault, choice, mcp, "")
}

/// Build the agent for one turn, naming the session the turn belongs to.
///
/// The session id is half of a policy principal (session + preset + tool) and
/// is what a denial row records. A build that only prices the tool schemas may
/// pass an empty id.
pub(crate) fn build_agent_for_session(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
    mcp: &crate::mcp::McpPool,
    session: &str,
) -> BuiltAgent {
    build_agent_for_session_inner(
        settings,
        vault,
        choice,
        mcp,
        session,
        None,
        Arc::new(agent::jobs::JobRegistry::new()),
    )
}

/// Build a turn that can delegate to child agents. The parent id and the shared
/// registry are attached before the tool schema is priced and before the agent
/// is frozen into an Arc.
#[allow(dead_code)]
pub(crate) fn build_agent_for_session_with_subagents(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
    mcp: &crate::mcp::McpPool,
    session: &str,
    parent: &str,
    subagents: Arc<crate::subagent::SubagentRegistry>,
) -> BuiltAgent {
    build_agent_for_session_with_subagents_and_jobs(
        settings,
        vault,
        choice,
        mcp,
        session,
        parent,
        subagents,
        Arc::new(agent::jobs::JobRegistry::new()),
    )
}

/// Build a turn that both delegates and shares the shell's live job roster.
///
/// The roster is created once per app and handed to every build, so a job a
/// previous turn started stays readable and killable by the next one. The
/// tools and the header draw the same registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_agent_for_session_with_subagents_and_jobs(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
    mcp: &crate::mcp::McpPool,
    session: &str,
    parent: &str,
    subagents: Arc<crate::subagent::SubagentRegistry>,
    jobs: Arc<agent::jobs::JobRegistry>,
) -> BuiltAgent {
    build_agent_for_session_inner(
        settings,
        vault,
        choice,
        mcp,
        session,
        Some((parent, subagents)),
        jobs,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_agent_for_session_inner(
    settings: &Settings,
    vault: &Vault,
    choice: &crate::settings::resolve::SessionChoice,
    mcp: &crate::mcp::McpPool,
    session: &str,
    subagents: Option<(&str, Arc<crate::subagent::SubagentRegistry>)>,
    jobs: Arc<agent::jobs::JobRegistry>,
) -> BuiltAgent {
    let eff = settings.effective_for(choice);
    // A contributed provider may declare that it needs no credential - an open
    // or anonymous lane. Its package said so, so the build does not invent a
    // missing-key refusal for an endpoint that was never going to be sent one.
    let (api_key, credential_error) =
        if eff.provider.trim().is_empty() || settings.global.provider_requires_key(&eff.provider) {
            provider_api_key(&eff, vault)
        } else {
            (String::new(), None)
        };
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
        system_prompt: eff.llm_system_prompt().to_string(),
        temperature: eff.temperature,
        // The composer's "Off" level is stored as the literal `off` so it can
        // beat the model's declared effort; on the wire it means NO effort field
        // rather than a string a provider would reject.
        reasoning_effort: eff
            .reasoning_effort
            .clone()
            .filter(|level| !level.eq_ignore_ascii_case("off")),
        max_tokens: eff.max_tokens,
        // The summarizer's request is bounded against the same window the
        // session is configured to send into, less the checkpoint's own output
        // reservation. Without this, a history larger than the window reached
        // the provider whole and came back as an error the UI called a useless
        // summary.
        compaction_input_tokens: compaction_input_tokens(&eff),
        // A zero-step loop would return nothing, so the floor is one.
        max_steps: eff.max_steps.max(1),
        parallel_tool_calls: eff
            .preset_parallel_tool_calls
            .unwrap_or(eff.parallel_tool_calls),
        ..AgentConfig::default()
    };
    // BSH does not read the ambient BOS config: the settings files are the
    // whole truth for this shell. The model's declared protocol arrives through
    // the settings instead, so the pane and the wire agree by construction.
    config.api_mode = eff.api_mode.clone();
    // The hosted web-search tool. The engine still requires the Responses
    // protocol before it offers the tool, because a chat or anthropic endpoint
    // would reject the object; the setting decides whether the reader wants it.
    config.web_search = eff.preset_web_search.unwrap_or(eff.web_tools_enabled);

    let mut agent = Agent::from_config(config);
    // Canonicalise once: `resolve_path` canonicalises every target, so the
    // policy data and the shell cwd must carry the same spelling of the root or
    // a symlinked workspace would look outside itself.
    let root = workspace_root(&eff).map(|path| path.canonicalize().unwrap_or(path));
    // The tool-boundary policy is compiled ONCE for this turn. Every file and
    // shell tool below holds a clone of this Arc, so a check costs an engine
    // eval and nothing else; there is no parse, no disk read and no data
    // install per call. It is rebaked whenever the turn is rebuilt, which is
    // the preset or workspace change the design calls for.
    let policy = policy_for_effective(settings, &eff);
    let preset = eff.preset_id.clone();
    // A preset names the BUNDLES it wants, not their tools: the shipped
    // presets say `shell`, `files` and `plan`, while the bundle that carries the
    // shell is named `shell` and its own tool `bash`. Comparing a tool name
    // against a bundle name would switch every tool off, which is exactly the
    // wrong-model symptom of an agent that answers with no tools at all. So the
    // preset answers by bundle, and the tool-level switch answers by tool.
    // A selected preset's plugin list is authoritative even when it is EMPTY:
    // Minimal mounts nothing, and an empty list must not read as "no preset".
    let preset_bundles = (!eff.preset_id.is_empty()).then(|| eff.preset_plugins.clone());
    let off_bundle = |name: &str| {
        if let Some(allowed) = &preset_bundles {
            if !allowed.iter().any(|bundle| bundle == name) {
                return true;
            }
        }
        false
    };
    let preset_tools = eff.preset_tools.as_deref();
    let off_tool = |name: &str| {
        eff.disabled_tools.iter().any(|d| d == name)
            || preset_tools.is_some_and(|tools| !tools.iter().any(|tool| tool == name))
    };
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
    // The preset's permission is a ceiling, never a grant: a read-only preset
    // keeps a Full-access session read-only, while a full-access preset cannot
    // lift a read-only session.
    // The ONE resolved tier: the session's request capped by the preset's
    // ceiling. The Rust registration matrix below is the first tier and stays
    // in place; the policy engine gets the same effective value as data, so a
    // tool that is reached another way is still refused (defence in depth).
    let permission = eff.permission.effective.clone();
    let confined = is_confined(&permission);
    let writes = allows_writes(&permission);
    // ONE capability decision for the shell family and the delegation tool.
    // Permission, the preset's tool allowlist and the Rego capability answer
    // are intersected here; registration below only consumes the result.
    let gates = tool_gates_for(&permission, preset_tools, &policy, session);

    if eff.bash_enabled && !off_bundle("shell") {
        let mut shell_tools: Vec<Arc<dyn agent::tools::Tool>> = Vec::new();
        if gates.bash && !off_tool("bash") {
            let config = agent::tools::bash::BashToolConfig {
                workspace_root: if confined {
                    root.as_ref().map(|r| r.to_string_lossy().to_string())
                } else {
                    None
                },
                timeout_secs: eff.shell_timeout_secs,
                ..Default::default()
            };
            let mut bash = BashTool::new("bash")
                .with_config(config)
                .with_policy(policy.clone(), session, preset.clone())
                .with_jobs(jobs.clone(), session)
                .with_background(gates.background_spawn);
            // Workspace Write confines shell writes per syscall through Seatbelt
            // when the host has it; Full access deliberately does not.
            if confined {
                if let Some(root) = &root {
                    bash = bash.with_sandbox(root.clone());
                }
            }
            shell_tools.push(Arc::new(bash));
        }
        // The job tools share the shell bundle but each has its own gate: a
        // preset that names only `job_output` gets exactly that tool, and a
        // policy that denies `job_kill` leaves the reader intact.
        if gates.output && !off_tool("job_output") {
            shell_tools.push(Arc::new(
                agent::tools::JobTool::reader("job_output")
                    .with_policy(policy.clone(), session, preset.clone())
                    .with_jobs(jobs.clone(), session),
            ));
        }
        if gates.kill && !off_tool("job_kill") {
            shell_tools.push(Arc::new(
                agent::tools::JobTool::stopper("job_kill")
                    .with_policy(policy.clone(), session, preset.clone())
                    .with_jobs(jobs.clone(), session),
            ));
        }
        if !shell_tools.is_empty() {
            bundles.push(plugins::Bundle::new("shell", shell_tools));
        }
    }
    if eff.file_tools_enabled && !off_bundle("files") {
        let files_root = if confined { root.clone() } else { None };
        let mut files: Vec<Arc<dyn agent::tools::Tool>> = Vec::new();
        if !off_tool("read_file") {
            files.push(Arc::new(read_file_tool(
                files_root.clone(),
                policy.clone(),
                session.to_string(),
                preset.clone(),
            )));
        }
        if writes && !off_tool("write_file") {
            files.push(Arc::new(write_file_tool(
                files_root.clone(),
                policy.clone(),
                session.to_string(),
                preset.clone(),
            )));
        }
        if writes && !off_tool("edit_file") {
            files.push(Arc::new(edit_file_tool(
                files_root.clone(),
                policy.clone(),
                session.to_string(),
                preset.clone(),
            )));
        }
        if !off_tool("list_dir") {
            files.push(Arc::new(list_dir_tool(
                files_root.clone(),
                policy.clone(),
                session.to_string(),
                preset.clone(),
            )));
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
    let (mut registry, mut plugins) =
        plugins::install(bundles, &settings.workspace.disabled.extensions);
    for name in registry.tool_names() {
        if let Some(tool) = registry.tool(&name) {
            agent.add_tool(tool);
        }
    }

    // The MCP HTTP path is the tool boundary's network egress. The pool shares
    // one slot with every live transport, so installing this turn's gate here
    // updates connections that were opened before the turn without respawning
    // them. ureq resolves through the gate, so a denied host opens no socket.
    mcp.set_egress_gate(Arc::new(EgressGate::new(
        policy.clone(),
        Arc::new(SystemResolver),
        session,
        preset.clone(),
        "mcp",
    )));

    // MCP tools come from the live pool rather than from the document: the
    // connection is the expensive part and it is shared across turns. A name
    // the registry already holds is skipped by the registration itself.
    for tool in mcp.tools() {
        // A preset names the MCP server namespaces it wants. None means every
        // connected server; an empty list means none.
        if eff
            .preset_mcps
            .as_ref()
            .is_some_and(|servers| !servers.iter().any(|server| server == &tool.namespace))
        {
            continue;
        }
        if eff.preset_mcp_tools.as_ref().is_some_and(|tools| {
            let full = format!("{}_{}", tool.namespace, tool.name);
            !tools.iter().any(|allowed| allowed == &full)
        }) {
            continue;
        }
        if let Err(error) = agent.add_mcp_tool(&tool.namespace, &tool.name, tool.adapter.clone()) {
            log::warn!("MCP tool '{}' was not registered: {error}", tool.name);
        }
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

    // Installed extension packages named by a path in the global inventory.
    // Both standards lower onto one registry: their tools and hooks go onto the
    // same agent this turn runs, while their commands and panels land in the
    // host plugin registry. A refusal (a DSH JavaScript entry, a Codex app
    // connector) is reported with the package id rather than silently dropped;
    // declared MCP servers are handled by the MCP pool below.
    let mut extensions = agent::extensions::ExtensionRegistry::new();
    let mut installed_extensions: Vec<(String, String)> = Vec::new();
    for entry in &settings.global.extensions {
        if eff
            .preset_extensions
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|id| id == &entry.id))
        {
            continue;
        }
        if settings
            .workspace
            .disabled
            .extensions
            .iter()
            .any(|disabled| disabled == &entry.id)
        {
            continue;
        }
        let Some(raw) = entry
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        match agent::extensions::ExtensionRegistry::load(&expand_path(raw)) {
            Ok(package) => {
                for refusal in &package.refusals {
                    // The shim declines to own MCP processes; the MCP pool
                    // starts them from the package's config instead.
                    if refusal.feature == agent::extensions::RefusedFeature::McpServer {
                        continue;
                    }
                    warn!("extension refused a part: {refusal}");
                }
                let package_id = package.extension.id.clone();
                if let Err(error) = extensions.install(package) {
                    warn!("extension '{}' was not installed: {error}", entry.id);
                } else {
                    // The inventory row keeps the settings id the disable list
                    // and the pane toggle use; the package id is the registry
                    // key and the source of the contributed tool names.
                    installed_extensions.push((entry.id.clone(), package_id));
                }
            }
            Err(error) => warn!("extension '{}' could not be loaded: {error}", entry.id),
        }
    }
    if !extensions.is_empty() {
        if let Err(error) = extensions.apply_to_agent(&mut agent) {
            warn!("extensions could not be applied to the agent: {error}");
        }
        for (settings_id, package_id) in &installed_extensions {
            plugins.push(PluginInfo {
                name: settings_id.clone(),
                tools: extensions.tool_names_of(package_id),
                enabled: true,
            });
        }
        if let Err(error) = extensions.install_host_items(&mut registry) {
            warn!("extension commands and panels could not be installed: {error}");
        }
    }

    if eff.memory_enabled {
        if let Some(memory) = open_memory(settings) {
            agent = agent.with_memory(memory);
        }
    }

    if let Some((parent, registry)) = subagents {
        // Delegation is a capability, not a free attachment: the same single
        // predicate that gates the shell family decides whether the tool and
        // its standing guidance exist at all.
        if gates.subagent {
            let context = SubagentContext::new(
                &agent,
                parent,
                parent,
                0,
                registry,
                policy.clone(),
                preset.clone(),
                root.as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            attach_subagent_tool(&mut agent, context);
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
        policy,
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
/// The input-token budget one summarizer request may use.
///
/// The configured context budget is the request ceiling bsh already prices
/// against, so the summarizer must fit inside it: the checkpoint's own output
/// is reserved first (it is capped by the agent at
/// `COMPACTION_MAX_OUTPUT_TOKENS`), then a small margin for the instruction and
/// framing. A zero context budget means the setting disables pressure-based
/// compaction, but a manual `/compact` still needs a sane ceiling, so the
/// agent's default budget is kept rather than unbounded.
fn compaction_input_tokens(eff: &Effective) -> usize {
    compaction_input_tokens_for(eff.context_budget)
}

/// The pure budget rule, split out so a test can pin the arithmetic.
fn compaction_input_tokens_for(context_budget: usize) -> usize {
    if context_budget == 0 {
        return agent::agent::AgentConfig::default().compaction_input_tokens;
    }
    let reserved_output = agent::agent::context::COMPACTION_MAX_OUTPUT_TOKENS as usize;
    context_budget
        .saturating_sub(reserved_output + 1_024)
        .max(4_096)
}

/// The older span a compaction would fold out of one session, if any.
///
/// This is the gate the Compact row reads before it builds an agent, so a
/// session with nothing safely compactable costs no request. It asks the SAME
/// selection the summarizer will: the agent-side rule protects a leading system
/// prompt and never cuts between an assistant tool call and its result, and the
/// bridge below feeds it the transcript the request would carry.
///
/// `retain_tokens` is the recent tail kept verbatim; zero means "everything
/// safe", which is what the manual command asks for.
pub(crate) fn compactable_span(
    transcript: &[crate::model::ChatMessage],
    retain_tokens: usize,
) -> Option<usize> {
    let messages = agent_messages(transcript);
    let mut probe = agent::agent::AgentSession::new();
    probe.restore_messages(messages);
    probe
        .split_for_compaction_at(retain_tokens)
        .map(|(removed, _)| removed.len())
}

/// The agent-side message log for one transcript window.
///
/// The bridge is the same shape `Runner::seed_session` builds, minus the empty
/// rows: a blank assistant placeholder is a streaming artifact, not history, and
/// an empty user row carries nothing to summarize.
///
/// Two limits are deliberate, not oversights. Tool calls and their results live
/// on the row for DISPLAY (`ChatMessage::tools`) and are not bridged, so the
/// compaction contract's tool-pairing boundary has no tool units to protect in
/// bsh; a fold can only land between user/assistant text rows. Images live on
/// the row too and are not bridged here either, so the summarizer sees the text
/// of a span but not its pictures, and an image-only row has no text to count.
/// Both need a richer agent-history bridge (and an image-offload seam) before
/// the reference's behaviour can exist here.
pub(crate) fn agent_messages(
    transcript: &[crate::model::ChatMessage],
) -> Vec<agent::agent::Message> {
    let mut messages = Vec::with_capacity(transcript.len());
    for message in transcript {
        match message.role {
            crate::model::Role::User if !message.text.is_empty() => {
                messages.push(agent::agent::Message::user(message.text.clone()));
            }
            crate::model::Role::Assistant if !message.text.is_empty() => {
                messages.push(agent::agent::Message::assistant(message.text.clone()));
            }
            _ => {}
        }
    }
    messages
}

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
pub(crate) fn tools_schema_chars(registry: &agent::tools::ToolRegistry) -> usize {
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

/// Compile a policy for a resolution that already exists.
///
/// The turn build and the MCP pool both need the same policy; going through one
/// function is what keeps the turn's principal and the network gate's data
/// document from drifting apart.
pub(crate) fn policy_for_effective(settings: &Settings, eff: &Effective) -> Arc<PolicySet> {
    // The same spelling the tools and `resolve_path` use, so a symlinked
    // workspace is not mistaken for a path outside itself.
    let root = workspace_root(eff).map(|path| path.canonicalize().unwrap_or(path));
    Arc::new(build_policy(settings, eff, root.as_deref()))
}
/// The capability guidance appended to a turn that carries the subagent tool.
///
/// The tool description is per-call; this is the standing rule that makes a
/// model split a large decomposable job instead of doing every part serially in
/// one context.
const SUBAGENT_GUIDANCE: &str = "## Delegating with subagents

A big job that can be split into independent parts should be split. When a request
spans several files or modules, several questions, or independent review and
verification passes, use the subagent tool for each self-contained part instead
of doing every part serially in this context.

Use subagents when:
- the work can be divided into two or more independent workstreams;
- a part needs a focused deep pass and can return a concise result;
- the parts can be investigated or implemented in parallel.

Do NOT use them for one trivial step, or for steps that must share tight
sequential state with this turn.

Pattern:
1. State a short plan naming the independent parts.
2. Call subagent once per part with a complete prompt: goal, relevant paths and
   context, constraints, expected output format, and what to report.
3. Run at most three children at once; put additional parts in later waves.
4. Read each result, reconcile conflicts, and do the final verification yourself.

A child has its own context and the same tools; it does not see this conversation
unless the prompt contains the needed context.";

/// The factory one parent turn hands to its subagent tool.
///
/// It snapshots the parent's LLM config and tool registry at build time, so a
/// child runs with the same model and the same tools but a fresh context. A
/// child at depth 0 or 1 may attach the same tool again, producing the bounded
/// DSH child tree; the depth check stops the recursion.
pub(crate) struct SubagentContext {
    session: String,
    parent: String,
    depth: usize,
    registry: Arc<crate::subagent::SubagentRegistry>,
    config: AgentConfig,
    sync_tools: Vec<Arc<dyn agent::tools::Tool>>,
    async_tools: Vec<Arc<dyn react::tool::registry::AsyncTool>>,
    /// The policy a call re-asks before a child exists.
    policy: Arc<PolicySet>,
    /// The preset half of the principal.
    preset: String,
    /// The workspace root the child runs in.
    root: String,
}

impl SubagentContext {
    /// Capture the parent agent's config and tools.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent: &Agent,
        session: impl Into<String>,
        parent: impl Into<String>,
        depth: usize,
        registry: Arc<crate::subagent::SubagentRegistry>,
        policy: Arc<PolicySet>,
        preset: impl Into<String>,
        root: impl Into<String>,
    ) -> Self {
        let (sync_tools, async_tools) = match agent.registry() {
            Some(registry) => {
                let sync_tools: Vec<Arc<dyn agent::tools::Tool>> =
                    registry.iter().map(|(_, tool)| tool.clone()).collect();
                let async_tools: Vec<Arc<dyn react::tool::registry::AsyncTool>> = registry
                    .async_tool_names()
                    .into_iter()
                    .filter_map(|name| registry.get_async(&name))
                    .collect();
                (sync_tools, async_tools)
            }
            None => (Vec::new(), Vec::new()),
        };
        Self {
            session: session.into(),
            parent: parent.into(),
            depth,
            registry,
            config: agent.config().clone(),
            sync_tools,
            async_tools,
            policy,
            preset: preset.into(),
            root: root.into(),
        }
    }

    /// Re-ask the policy before a child is spawned.
    ///
    /// A denial returns the policy's own refusal and leaves the registry
    /// untouched: no run row exists, so no child session is ever persisted.
    fn authorize_spawn(&self) -> Result<(), ToolError> {
        match check_spawn(
            &self.policy,
            "subagent",
            &self.session,
            &self.preset,
            &self.root,
        ) {
            Ok(()) => Ok(()),
            Err(refusal) => Err(ToolError::Failed(refusal)),
        }
    }

    /// Build a child and, below the depth cap, give it the subagent tool too.
    fn child(&self, child_id: &str) -> Agent {
        let mut config = self.config.clone();
        config.name = "subagent".to_string();
        config.max_steps = config.max_steps.min(12);
        // A leaf child has no subagent tool, so it must not inherit the parent's
        // standing instruction to delegate. A child that gets the tool has it
        // appended by attach_subagent_tool below.
        if let Some(pos) = config.system_prompt.find("## Delegating with subagents") {
            config.system_prompt.truncate(pos);
            config.system_prompt = config.system_prompt.trim_end().to_string();
        }
        config.system_prompt = format!(
            "You are a focused subagent. Complete the task and return a concise result.\n\n{}",
            config.system_prompt
        );
        let mut child = Agent::from_config(config);
        for tool in &self.sync_tools {
            child.add_tool(tool.clone());
        }
        for tool in &self.async_tools {
            if let Err(error) = child.try_add_async_tool(tool.clone()) {
                warn!(
                    "subagent tool '{}' was not registered: {error}",
                    tool.name()
                );
            }
        }
        if self.depth < 1 {
            let nested = SubagentContext::new(
                &child,
                self.session.clone(),
                child_id.to_string(),
                self.depth + 1,
                self.registry.clone(),
                self.policy.clone(),
                self.preset.clone(),
                self.root.clone(),
            );
            attach_subagent_tool(&mut child, nested);
        }
        child
    }
}

/// Flatten a child's session into the rows the read-only view draws.
pub(crate) fn child_transcript(child: &Agent) -> Vec<crate::subagent::SubagentMessage> {
    let session = child.session();
    let mut rows: Vec<crate::subagent::SubagentMessage> = Vec::new();
    for message in session.messages() {
        let row = match message {
            react::llm::LlmMessage::System { .. } => None,
            react::llm::LlmMessage::User { content } => {
                Some(("user", content.as_text().unwrap_or("").to_string(), None))
            }
            react::llm::LlmMessage::Assistant { content } => {
                Some(("assistant", content.clone(), None))
            }
            react::llm::LlmMessage::AssistantToolCall { name, args, .. } => {
                Some(("tool", format!("{name} {args}"), Some(name.clone())))
            }
            react::llm::LlmMessage::ToolResult { content, .. } => {
                Some(("tool", content.clone(), None))
            }
        };
        if let Some((role, content, tool)) = row {
            rows.push(crate::subagent::SubagentMessage {
                role: role.to_string(),
                content,
                tool,
            });
        }
    }
    let mut rows: Vec<crate::subagent::SubagentMessage> = rows;
    // Some vendor branches historically recorded the same final assistant row
    // twice; adjacent identical assistant rows are never meaningful.
    rows.dedup_by(|a, b| a.role == "assistant" && b.role == "assistant" && a.content == b.content);
    rows
}

/// Stream one child turn, updating the live registry as tokens and tools land.
///
/// The final child Session is still the authority; the live rows make the
/// transcript and token counter move while the child is incomplete.
pub(crate) async fn stream_child(
    child: &Agent,
    prompt: String,
    registry: Arc<crate::subagent::SubagentRegistry>,
    id: &str,
) -> Result<(String, (u64, u64)), String> {
    let mut stream = child.stream(prompt);
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(react::llm::StreamToken::Text(chunk)) => {
                text.push_str(&chunk);
                registry.append_text(id, &chunk);
            }
            Ok(react::llm::StreamToken::ReasoningContent(_)) => {}
            Ok(react::llm::StreamToken::ToolCall { name, args, .. }) => {
                registry.append_tool_call(id, &name, &args.to_string());
            }
            Ok(react::llm::StreamToken::ToolResult { output, .. }) => {
                registry.append_tool_result(id, &output);
            }
            Ok(react::llm::StreamToken::Usage(usage)) => {
                registry.set_usage(
                    id,
                    u64::from(usage.prompt_tokens),
                    u64::from(usage.completion_tokens),
                );
            }
            Ok(react::llm::StreamToken::Done | react::llm::StreamToken::Stopped) => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    let usage = child.last_token_usage().unwrap_or((0, 0));
    Ok((text, usage))
}

/// Install the subagent tool on a parent turn.
pub(crate) fn attach_subagent_tool(agent: &mut Agent, context: SubagentContext) {
    let context = Arc::new(context);
    let tool = agent::tools::AsyncFunctionTool::from_fn(
        "subagent",
        "Delegate ONE self-contained part of a large job to a focused child agent with its own context. Use this whenever the work can be split into independent parts (multi-file changes, parallel research, independent reviews); write a complete prompt and run up to three children at once. Returns the child's final result. Do not use it for a trivial single step or tightly coupled sequential work.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "Short 3-8 word title for the child run; shown in the subagent menu." },
                "prompt": { "type": "string", "description": "Complete self-contained task: goal, relevant paths and context, constraints, expected output format, and what to report. The child cannot see this conversation." }
            },
            "required": ["description", "prompt"]
        }),
        move |args| {
            let context = context.clone();
            async move {
                let title = args
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let prompt = args
                    .get("prompt")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if prompt.is_empty() {
                    return Err(ToolError::InvalidInput(
                        "prompt is required and must not be empty".to_string(),
                    ));
                }
                if context.registry.running_count() >= 3 {
                    return Err(ToolError::Failed(
                        "too many subagents are already running; wait for one to finish"
                            .to_string(),
                    ));
                }
                // Policy before the child exists: a denied spawn creates no run
                // row and no child session.
                context.authorize_spawn()?;
                let title = if title.is_empty() {
                    prompt
                        .lines()
                        .next()
                        .unwrap_or("Subagent")
                        .chars()
                        .take(60)
                        .collect()
                } else {
                    title
                };
                // A live estimate for the header before the child reports
                // usage: prompt bytes plus a small allowance for tools/config.
                let estimated_tokens = (prompt.chars().count() / 4) as u64 + 2_000;
                let id = context.registry.begin(
                    &context.session,
                    &context.parent,
                    context.depth + 1,
                    estimated_tokens,
                    &title,
                    &prompt,
                );
                let child = context.child(&id);
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(600),
                    stream_child(
                        &child,
                        prompt,
                        context.registry.clone(),
                        &id,
                    ),
                )
                .await
                .map_err(|_| "subagent timed out".to_string())
                .and_then(|result| result);
                let usage = child.last_token_usage().unwrap_or((0, 0));
                let transcript = child_transcript(&child);
                context.registry.set_agent(
                    &id,
                    Arc::new(tokio::sync::Mutex::new(child)),
                );
                match result {
                    Ok((text, usage)) => {
                        context
                            .registry
                            .finish(&id, text.clone(), usage, transcript);
                        let result: String = text.chars().take(16_000).collect();
                        Ok(serde_json::json!({
                            "subagent_id": id,
                            "result": result,
                            "prompt_tokens": usage.0,
                            "completion_tokens": usage.1,
                        }))
                    }
                    Err(message) => {
                        context
                            .registry
                            .fail(&id, message.clone(), Some(usage), transcript);
                        Err(ToolError::Failed(message))
                    }
                }
            }
        },
    );
    if !agent
        .config()
        .system_prompt
        .contains("## Delegating with subagents")
    {
        agent.append_system_prompt(SUBAGENT_GUIDANCE);
    }
    if let Err(error) = agent.try_add_async_tool(Arc::new(tool)) {
        warn!("subagent tool was not registered: {error}");
    }
}

/// Compile this turn's policy: the workspace file when present, else the
/// shipped baseline, with one data document built from the turn's facts.
///
/// A policy file that exists and does not parse does not make the build fail:
/// [`PolicySet::compile`] records the failure and every check then applies the
/// documented asymmetry (deny writes and egress, allow in-workspace reads).
fn build_policy(settings: &Settings, eff: &Effective, root: Option<&Path>) -> PolicySet {
    let mut data = PolicyData::new(root.map(Path::to_path_buf).unwrap_or_default())
        .with_preset(eff.preset_id.clone())
        .with_preset_permission(eff.preset_permission.clone().unwrap_or_default())
        // The capability query and every action rule read the EFFECTIVE
        // permission (session mode capped by the preset ceiling), not the
        // preset's ceiling alone and never the requested mode.
        .with_permission(eff.permission.effective.clone());
    if let Some(root) = root {
        data = data.with_egress_allowlist(read_egress_allowlist(root));
        let read_roots = eff
            .preset_read
            .as_ref()
            .map(|items| preset_scope_roots(root, items))
            .unwrap_or_default();
        let write_roots = eff
            .preset_write
            .as_ref()
            .map(|items| preset_scope_roots(root, items))
            .unwrap_or_default();
        let deny_roots = preset_scope_roots(root, &eff.preset_deny);
        data = data.with_preset_scope(
            eff.preset_read.is_some(),
            read_roots,
            eff.preset_write.is_some(),
            write_roots,
            deny_roots,
        );
        if let Some(egress) = &eff.preset_egress {
            data = data.with_preset_egress(true, egress.clone());
        }
    }
    // The session tier as a policy fact, generated for the effective level.
    // A read-only session denies writes, egress and spawns here as well as in
    // the Rust registration matrix; that matrix is the floor (the tools are not
    // even offered), and this module is the second tier (defence in depth).
    let mut sources = vec![
        agent::policy::baseline_source(),
        agent::policy::session_permission_source(&eff.permission.effective),
    ];
    let scope_active =
        eff.preset_read.is_some() || eff.preset_write.is_some() || !eff.preset_deny.is_empty();
    if scope_active {
        sources.push(agent::policy::preset_scope_source());
    }
    if eff.preset_egress.is_some() {
        sources.push(agent::policy::preset_egress_source());
    }
    // A read-only CEILING is also a policy fact of its own: the preset's module
    // narrows independently of the session tier. The built-in read-only module
    // already carries this rule; add the generated one only when the preset does
    // not name it, so every read-only preset is enforced by Rego.
    let generated_read_only = eff.preset_permission.as_deref() == Some("read-only")
        && !eff.preset_builtin_policy.iter().any(|id| id == "read-only");
    if generated_read_only {
        sources.push(agent::policy::preset_permission_source());
    }
    for id in &eff.preset_builtin_policy {
        match builtin_policy_source(id) {
            Some(source) => sources.push(source),
            None => {
                // An unknown built-in id must not silently disappear from the
                // set: an unparseable placeholder latches the failure mode.
                warn!("unknown preset builtin policy '{id}'");
                sources.push(PolicySource::new(
                    "preset-builtin",
                    "# unknown preset builtin policy",
                ));
            }
        }
    }
    // A required policy that the reader has not trusted makes the preset
    // unusable; refuse it in the set as well as in the send gate.
    if eff.preset_policy_required_untrusted() {
        sources.push(PolicySource::new(
            "preset-policy",
            "# required preset policy is untrusted",
        ));
    } else if eff.preset_trusted {
        // A preset is an authority only after the reader trusts it. An
        // untrusted advisory policy is ignored; the send gate refuses the
        // required case before a turn starts.
        if let Some(policy) = eff.preset_policy.as_deref() {
            sources.push(PolicySource::new("preset", policy));
        }
    }
    if let Some(root) = root {
        let path = root.join(".bsh").join("policy.rego");
        match std::fs::read_to_string(&path) {
            Ok(rego) => sources.push(PolicySource::new("workspace", rego)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                // A policy file that exists but cannot be read must not
                // silently leave only the baseline: that would widen a gate
                // the workspace meant to narrow. An unparseable placeholder
                // latches the set into its failure mode instead.
                warn!("policy file {} could not be read: {error}", path.display());
                sources.push(PolicySource::new(
                    "workspace",
                    "# the workspace policy exists but could not be read",
                ));
            }
        }
    }
    PolicySet::compile(sources, data).with_lang(settings.global.general.lang.clone())
}

/// Binary-shipped Rego modules a preset can name by id.
const BUILTIN_DENY_SECRETS: &str = r#"package policy.builtin.deny_secrets

# rule: deny-secrets
verdict := {"allow": false, "rule": "deny-secrets", "reason": "the path is a secret or credential store"} if {
    input.target.kind == "path"
    secret_path(input.target.path)
} else := {"allow": true}

secret_path(path) if {
    contains(concat("", [path, "/"]), "/.env/")
}

secret_path(path) if {
    contains(concat("", [path, "/"]), "/.ssh/")
}

secret_path(path) if {
    contains(concat("", [path, "/"]), "/.aws/")
}

secret_path(path) if {
    contains(concat("", [path, "/"]), "/secrets/")
}

secret_path(path) if {
    endswith(path, ".pem")
}

secret_path(path) if {
    endswith(path, "/id_rsa")
}
"#;

const BUILTIN_READ_ONLY: &str = r#"package policy.builtin.read_only

# rule: read-only
verdict := {"allow": false, "rule": "read-only", "reason": "this preset is read-only"} if {
    input.action == "write"
} else := {"allow": true}
"#;

const BUILTIN_NO_SHELL: &str = r#"package policy.builtin.no_shell

# rule: no-shell
verdict := {"allow": false, "rule": "no-shell", "reason": "this preset does not allow a shell"} if {
    input.action == "spawn"
    input.tool == "bash"
} else := {"allow": true}
"#;

const BUILTIN_NO_EGRESS: &str = r#"package policy.builtin.no_egress

# rule: no-egress
verdict := {"allow": false, "rule": "no-egress", "reason": "this preset does not allow network egress"} if {
    input.action == "egress"
} else := {"allow": true}
"#;

const BUILTIN_DOCS_ONLY: &str = r#"package policy.builtin.docs_only

# rule: docs-only
verdict := {"allow": false, "rule": "docs-only", "reason": "this preset writes documentation only"} if {
    input.action == "write"
    not docs_path(input.target.path)
} else := {"allow": true}

docs_path(path) if {
    contains(concat("", [path, "/"]), "/docs/")
}

docs_path(path) if {
    endswith(path, "/README.md")
}
"#;

const BUILTIN_DENY_GIT_WRITES: &str = r#"package policy.builtin.deny_git_writes

# rule: deny-git-writes
verdict := {"allow": false, "rule": "deny-git-writes", "reason": "writes into Git internals are forbidden"} if {
    input.action == "write"
    contains(concat("", [input.target.path, "/"]), "/.git/")
} else := {"allow": true}
"#;

fn builtin_policy_source(id: &str) -> Option<PolicySource> {
    let (label, rego) = match id {
        "deny-secrets" => ("builtin-deny-secrets", BUILTIN_DENY_SECRETS),
        "read-only" => ("builtin-read-only", BUILTIN_READ_ONLY),
        "no-shell" => ("builtin-no-shell", BUILTIN_NO_SHELL),
        "no-egress" => ("builtin-no-egress", BUILTIN_NO_EGRESS),
        "docs-only" => ("builtin-docs-only", BUILTIN_DOCS_ONLY),
        "deny-git-writes" => ("builtin-deny-git-writes", BUILTIN_DENY_GIT_WRITES),
        _ => return None,
    };
    Some(PolicySource::new(label, rego))
}

/// Resolve a preset's workspace-relative scope entries to canonical absolute
/// roots. Entries that escape the workspace are ignored: an invalid narrowing
/// is safer than a silent widening.
fn preset_scope_roots(root: &std::path::Path, items: &[String]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            let rel = std::path::Path::new(item);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                warn!("preset scope entry '{item}' is not workspace-relative");
                return None;
            }
            let joined = root.join(rel);
            let canonical = joined.canonicalize().unwrap_or(joined);
            if !canonical.starts_with(root) {
                warn!("preset scope entry '{item}' resolves outside the workspace");
                return None;
            }
            Some(canonical.to_string_lossy().to_string())
        })
        .collect()
}

/// The addresses this workspace allows egress to, one per line.
///
/// The file is `<workspace>/.bsh/egress-allowlist`: an exact address or a
/// CIDR range, with `#` comments and blank lines ignored. A missing file means
/// an empty list, which denies all egress - the baseline's documented default.
fn read_egress_allowlist(root: &Path) -> Vec<String> {
    let path = root.join(".bsh").join("egress-allowlist");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Ask the policy about one concrete path before a tool touches it.
fn check_path(
    policy: &PolicySet,
    session: &str,
    preset: &str,
    tool: &str,
    action: Action,
    path: &Path,
) -> Result<(), ToolError> {
    let input = PolicyInput::new(tool, action, Target::path(path))
        .with_session(session)
        .with_preset(preset);
    let decision = policy.check(&input);
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(ToolError::Failed(policy.refusal(&decision)))
    }
}

fn arg_str(args: &serde_json::Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidInput(format!("{key} is required")))
}

/// read_file - read a UTF-8 file inside the workspace.
fn read_file_tool(
    root: Option<PathBuf>,
    policy: Arc<PolicySet>,
    session: String,
    preset: String,
) -> FunctionTool {
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
            check_path(
                &policy,
                &session,
                &preset,
                "read_file",
                Action::Read,
                &target,
            )?;
            let content = std::fs::read_to_string(&target)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            Ok(serde_json::json!({ "path": target.display().to_string(), "content": content }))
        },
    )
}

/// write_file - write a UTF-8 file inside the workspace.
fn write_file_tool(
    root: Option<PathBuf>,
    policy: Arc<PolicySet>,
    session: String,
    preset: String,
) -> FunctionTool {
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
            // The check is BEFORE the parent directory is created: a denied
            // write must not leave the parent's mtime moved as a side effect.
            check_path(
                &policy,
                &session,
                &preset,
                "write_file",
                Action::Write,
                &target,
            )?;
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

/// edit_file - replace an exact string in a UTF-8 file inside the workspace.
///
/// The policy is asked for a Write on the resolved path before the file is even
/// read, so a denied edit cannot observe or modify its content. An empty
/// `old_string` is refused (it would insert at every position), a missing
/// `old_string` is an error rather than a no-op, and an ambiguous match is
/// refused unless `replace_all` says every occurrence is intended.
fn edit_file_tool(
    root: Option<PathBuf>,
    policy: Arc<PolicySet>,
    session: String,
    preset: String,
) -> FunctionTool {
    FunctionTool::new(
        "edit_file",
        "Replace an exact string in a UTF-8 text file inside the workspace. By default old_string must occur exactly once; pass replace_all to replace every occurrence.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean" }
            },
            "required": ["path", "old_string", "new_string"]
        }),
        move |args| {
            let raw = arg_str(args, "path")?;
            let old = arg_str(args, "old_string")?;
            let new = arg_str(args, "new_string")?;
            if old.is_empty() {
                return Err(ToolError::InvalidInput(
                    "old_string must not be empty".to_string(),
                ));
            }
            let replace_all = args
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let target = resolve_path(&root, &raw).map_err(ToolError::InvalidInput)?;
            // Before the read: a denied edit is refused before its content is
            // even observed.
            check_path(
                &policy,
                &session,
                &preset,
                "edit_file",
                Action::Write,
                &target,
            )?;
            let content = std::fs::read_to_string(&target)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            let occurrences = content.matches(&old).count();
            if occurrences == 0 {
                return Err(ToolError::Failed(format!(
                    "old_string not found in {}",
                    target.display()
                )));
            }
            if occurrences > 1 && !replace_all {
                return Err(ToolError::InvalidInput(format!(
                    "old_string occurs {occurrences} times in {}; pass replace_all=true to replace every occurrence",
                    target.display()
                )));
            }
            let updated = if replace_all {
                content.replace(&old, &new)
            } else {
                content.replacen(&old, &new, 1)
            };
            std::fs::write(&target, &updated)
                .map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
            Ok(serde_json::json!({
                "path": target.display().to_string(),
                "replacements": if replace_all { occurrences } else { 1 },
            }))
        },
    )
}

/// list_dir - list a directory's entries.
fn list_dir_tool(
    root: Option<PathBuf>,
    policy: Arc<PolicySet>,
    session: String,
    preset: String,
) -> FunctionTool {
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
            check_path(
                &policy,
                &session,
                &preset,
                "list_dir",
                Action::Read,
                &target,
            )?;
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
    // A test that builds a bare Settings has no redirect. Attaching nothing is
    // the only safe answer: memory_file would otherwise fall back to the
    // reader's own ~/.bos/bsh/memory.jsonl, which no test owns and several
    // tests inside one process would share. A settings document opened from a
    // scratch root (the normal test path) still gets its own scratch file.
    if cfg!(test) && settings.save_root.is_none() {
        return None;
    }
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
        agent_messages, allows_writes, build_agent, compactable_span, is_confined,
        plugin_provider_node, refresh_plugin_providers, surfaced_capabilities, tool_gates_for,
        BuiltAgent, FULL_ACCESS, READ_ONLY, WORKSPACE_WRITE,
    };
    use crate::settings::global::{ModelEntry, Preset};
    use crate::settings::resolve::SessionChoice;
    use crate::settings::Settings;
    use crate::vault::Vault;
    use agent::policy::{Action, PolicyInput, Target};

    /// A transcript with `turns` exchanges, so the gate has history to fold.
    fn turns(count: usize) -> Vec<crate::model::ChatMessage> {
        let mut messages = Vec::new();
        for i in 0..count {
            messages.push(crate::model::ChatMessage::user(format!("u{i}")));
            let mut reply = crate::model::ChatMessage::assistant();
            reply.text = format!("a{i}");
            messages.push(reply);
        }
        messages
    }

    /// The summarizer's input budget leaves the checkpoint's own output and the
    /// instruction inside the configured window: the field log's successful
    /// request already carried a 16710-token prompt while reserving 12800 for
    /// the summary, leaving only ~3.2k of a 32k window for the next chunk.
    #[test]
    fn the_summarizer_budget_leaves_room_for_its_output() {
        use super::compaction_input_tokens_for;
        let max_output = agent::agent::context::COMPACTION_MAX_OUTPUT_TOKENS as usize;
        assert_eq!(compaction_input_tokens_for(32_768), 27_648);
        assert!(
            compaction_input_tokens_for(32_768) + max_output <= 32_768,
            "input + the capped summary output fits the window"
        );
        // A zero context budget still gives a manual /compact a sane ceiling,
        // never the unbounded whole-span request.
        assert_eq!(compaction_input_tokens_for(0), 24_000);
        // A tiny window keeps a floor rather than a zero request.
        assert_eq!(compaction_input_tokens_for(1_000), 4_096);
    }

    /// The Compact gate answers with the same selection the summarizer uses: a
    /// trailing pair of messages is not enough to fold, more history is.
    #[test]
    fn the_compact_gate_agrees_with_the_selection() {
        let mut session = crate::model::Session::new("ws", "t");
        // A zero budget folds everything a balanced cut can, so even one
        // exchange gives up its first message - the contract holds nothing
        // back beyond the cut itself.
        session.messages = turns(1);
        assert_eq!(
            compactable_span(&session.messages, 0),
            Some(1),
            "one exchange folds its first message, keeping the last"
        );
        session.messages = turns(6);
        let span = compactable_span(&session.messages, 0).expect("six exchanges are foldable");
        assert_eq!(span, session.messages.len() - 1, "all but the last message");
        // A retain budget above the whole history leaves nothing to fold.
        assert!(compactable_span(&session.messages, 1_000_000).is_none());
    }

    /// A Pairing rule holds across the bridge: the span ends on a balanced cut,
    /// and a transcript whose tail is one exchange still folds its older half.
    #[test]
    fn the_bridge_spans_a_whole_number_of_messages() {
        let messages = turns(5);
        let bridged = agent_messages(&messages);
        assert_eq!(bridged.len(), messages.len(), "every non-empty row crosses");
        // Blank rows are streaming artifacts and must not cross.
        let mut with_blank = messages.clone();
        with_blank.push(crate::model::ChatMessage::assistant());
        assert_eq!(agent_messages(&with_blank).len(), messages.len());
    }

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

    /// An installed extension named by a path reaches the very agent the turn
    /// runs: its tool is in the tool registry, its skill is in the prompt set,
    /// and the settings inventory lists the package.
    #[test]
    fn an_installed_extension_reaches_the_turn() {
        let mut settings = bare();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions/dsh-echo");
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "dsh-echo".to_string(),
                path: Some(fixture.to_string_lossy().to_string()),
                ..Default::default()
            });

        let built = build(&settings);
        let tool = built
            .agent
            .registry()
            .expect("tool registry")
            .get("dsh_echo")
            .expect("the extension tool");
        let output = tool
            .run(&serde_json::json!({ "value": "hi" }))
            .expect("the tool runs");
        assert_eq!(output["value"], "hi");
        assert!(
            built
                .agent
                .get_skills_content()
                .iter()
                .any(|(name, _)| *name == "greet"),
            "the extension skill is registered on the agent"
        );
        assert!(
            built.plugins.iter().any(|plugin| plugin.name == "dsh-echo"),
            "the settings inventory lists the extension under the id its toggle writes"
        );
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
                policy: None,
                trusted: false,
                ..Default::default()
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
        assert_eq!(
            names("shell"),
            vec![
                "bash".to_string(),
                "job_kill".to_string(),
                "job_output".to_string(),
            ]
        );
        assert_eq!(
            names("files"),
            vec![
                "edit_file".to_string(),
                "list_dir".to_string(),
                "write_file".to_string(),
            ]
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
        off_tree.workspace.disabled.extensions = vec!["shell".to_string()];
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
        // `resolve_path` canonicalises the workspace root, so a test that hands
        // it a path containing `../..` would compare two spellings of the same
        // directory. Return the canonical spelling.
        dir.canonicalize().unwrap_or(dir)
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

    /// A settings tree whose only skill source is `root`, named `First`.
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
    ///
    /// The assertion is on the agent's skill CONTENT, not only on its name
    /// list: content is what reaches the model's prompt, so a gate that kept a
    /// disabled name out of the map while leaking its instructions would pass a
    /// name-only test. The sibling's body is asserted too, so a build that
    /// registered nothing at all cannot read as a pass.
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
        let content = built.agent.get_skills_content();
        assert!(
            content.iter().all(|(name, _)| *name != "alpha"),
            "the disabled skill's content is absent from the agent: {content:?}"
        );
        assert!(
            content
                .iter()
                .any(|(name, body)| *name == "beta" && body.contains("Body of beta.")),
            "the enabled sibling's instructions are present: {content:?}"
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
                "edit_file".to_string(),
                "job_kill".to_string(),
                "job_output".to_string(),
                "list_dir".to_string(),
                "read_file".to_string(),
                "update_plan".to_string(),
                "write_file".to_string(),
            ],
            "Workspace Write carries the whole shell family, including the \
             job tools its own read-write turn may start"
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

    /// The five gated capabilities, in display order.
    type Gates = (bool, bool, bool, bool, bool);
    /// One row of the registration table.
    type GateRow<'a> = (&'a str, Option<&'a [&'a str]>, Gates);

    /// The gated tools a built parent turn actually carries.
    ///
    /// The tuple is `(bash, run_in_background, job_output, job_kill,
    /// subagent)`. `run_in_background` is a shell capability rather than a
    /// tool, so it is read from the schema the agent advertises.
    fn gated_tools(settings: &Settings) -> Gates {
        use std::sync::Arc;
        let built = crate::caps::build_agent_for_session_with_subagents(
            settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "session",
            "session",
            Arc::new(crate::subagent::SubagentRegistry::default()),
        );
        let registry = built.agent.registry().expect("registry");
        let bash = registry.get("bash");
        let background = bash.as_ref().is_some_and(|bash| {
            bash.json_schema()["properties"]
                .get("run_in_background")
                .is_some()
        });
        (
            bash.is_some(),
            background,
            registry.get("job_output").is_some(),
            registry.get("job_kill").is_some(),
            registry.get_async("subagent").is_some(),
        )
    }

    /// One settings tree with an optional preset `tools` allowlist.
    fn with_tools(permission: &str, tools: Option<&[&str]>) -> Settings {
        let mut settings = bare();
        settings.workspace.agent.permission = permission.to_string();
        if let Some(tools) = tools {
            settings.global.presets.insert(
                "gated".to_string(),
                Preset {
                    name: Some("gated".to_string()),
                    plugins: vec!["shell".to_string(), "files".to_string(), "plan".to_string()],
                    tools: Some(tools.iter().map(|tool| tool.to_string()).collect()),
                    ..Preset::default()
                },
            );
            settings.global.default_preset = "gated".to_string();
        }
        settings
    }

    /// The registration table: permission x preset allowlist, row by row.
    ///
    /// This is the consequence the change is for. Read Only removes every
    /// gated tool; an allowlist that omits `subagent` removes it while the
    /// shell family stays; and a read-only preset naming `subagent` still does
    /// not get it, because permission and allowlist intersect rather than
    /// union. A preset without `tools` inherits, which the writable rows pin.
    #[test]
    fn the_tool_gate_matrix_follows_permission_and_preset() {
        let table: &[GateRow] = &[
            (READ_ONLY, None, (false, false, false, false, false)),
            (
                READ_ONLY,
                Some(&[
                    "bash",
                    "run_in_background",
                    "job_output",
                    "job_kill",
                    "subagent",
                ]),
                (false, false, false, false, false),
            ),
            (
                READ_ONLY,
                Some(&["subagent"]),
                (false, false, false, false, false),
            ),
            (WORKSPACE_WRITE, None, (true, true, true, true, true)),
            (
                WORKSPACE_WRITE,
                Some(&["bash", "job_output", "job_kill"]),
                (true, true, true, true, false),
            ),
            (
                WORKSPACE_WRITE,
                Some(&["subagent"]),
                (false, false, false, false, true),
            ),
            (FULL_ACCESS, None, (true, true, true, true, true)),
        ];
        for (permission, tools, expected) in table {
            let settings = with_tools(permission, *tools);
            assert_eq!(
                gated_tools(&settings),
                *expected,
                "registered (bash, run_in_background, job_output, job_kill, subagent) \
                 for permission={permission} tools={tools:?}"
            );
        }
    }

    /// The session tier is a Rego fact, and the data it reads is the EFFECTIVE
    /// permission: a Full-access request under a read-only preset hands the
    /// engine `read-only`, and the generated module denies write, egress and
    /// spawn while the Rust registration floor withholds the tools too.
    #[test]
    fn the_session_tier_reaches_rego_as_the_effective_permission() {
        let root = scratch("policy-effective-permission");
        let mut settings = rooted(&root);
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        settings.global.presets.insert(
            "ro".to_string(),
            Preset {
                permission: Some(READ_ONLY.to_string()),
                plugins: vec!["files".to_string()],
                ..Preset::default()
            },
        );
        settings.global.default_preset = "ro".to_string();
        // Allowlist the address so the BASELINE permits the egress and the
        // generated session tier is the module that refuses it.
        std::fs::create_dir_all(root.join(".bsh")).expect("bsh dir");
        std::fs::write(root.join(".bsh/egress-allowlist"), "1.2.3.4\n").expect("egress allowlist");
        let built = build(&settings);
        assert_eq!(
            settings.effective().permission.requested,
            FULL_ACCESS,
            "the session still asks for Full access"
        );
        assert_eq!(
            built.policy.data().permission,
            READ_ONLY,
            "the policy data hands Rego the EFFECTIVE mode (effective != requested)"
        );
        assert!(
            built
                .policy
                .rule_ids()
                .iter()
                .any(|rule| rule == "session-read-only-write"),
            "the generated session module is compiled into the set: {:?}",
            built.policy.rule_ids()
        );
        let ask = |tool: &str, action: Action, target: Target| {
            PolicyInput::new(tool, action, target)
                .with_session("session")
                .with_preset("ro")
        };
        for (tool, action, target) in [
            (
                "write_file",
                Action::Write,
                Target::path(root.join("notes.txt")),
            ),
            (
                "mcp",
                Action::Egress,
                Target::host("example.test", "1.2.3.4"),
            ),
            ("bash", Action::Spawn, Target::path(root.clone())),
        ] {
            let decision = built.policy.check(&ask(tool, action, target));
            assert!(
                !decision.is_allowed(),
                "{tool} must be denied by the read-only effective permission: {decision:?}"
            );
            assert!(
                decision
                    .rule
                    .as_deref()
                    .is_some_and(|rule| rule.starts_with("session-read-only")),
                "{tool}'s denial must name the session tier, got {decision:?}"
            );
            assert_eq!(
                built
                    .policy
                    .denials()
                    .last()
                    .map(|entry| entry.permission.as_str()),
                Some(READ_ONLY),
                "the audit row records the effective permission"
            );
        }
        // Paired: with no ceiling the same write inside the workspace is
        // allowed, so the assertions above cannot pass by denying everything.
        settings.global.default_preset.clear();
        let built = build(&settings);
        assert_eq!(built.policy.data().permission, FULL_ACCESS);
        assert!(
            built
                .policy
                .check(&ask(
                    "write_file",
                    Action::Write,
                    Target::path(root.join("notes.txt"))
                ))
                .is_allowed(),
            "Full access without a ceiling keeps the in-workspace write"
        );
    }

    /// The single predicate and the registry agree, and the predicate is the
    /// only reader of the three gates.
    #[test]
    fn the_tool_gate_predicate_matches_registration() {
        for (permission, tools) in [
            (READ_ONLY, None),
            (WORKSPACE_WRITE, None),
            (WORKSPACE_WRITE, Some(&["bash", "job_kill"][..])),
            (FULL_ACCESS, None),
        ] {
            let settings = with_tools(permission, tools);
            let built = build(&settings);
            let preset_tools = settings.effective().preset_tools.clone();
            let gates = tool_gates_for(
                permission,
                preset_tools.as_deref(),
                &built.policy,
                "session",
            );
            let registered = gated_tools(&settings);
            assert_eq!(
                (
                    gates.bash,
                    gates.background_spawn,
                    gates.output,
                    gates.kill,
                    gates.subagent,
                ),
                registered,
                "predicate and registry disagree for {permission} {tools:?}"
            );
        }
    }

    /// Write one workspace policy file under `root`.
    fn write_policy(root: &std::path::Path, rego: &str) {
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(root.join(".bsh").join("policy.rego"), rego).expect("the policy file");
    }

    /// A workspace policy that denies `action` for one `tool`, and allows
    /// everything else. The rule id is the one a refusal must name.
    fn deny_rule(rule: &str, action: &str, tool: &str) -> String {
        format!(
            r#"package policy.workspace

# rule: {rule}
verdict := {{"allow": false, "rule": "{rule}", "reason": "refused by the workspace policy"}} if {{
    input.action == "{action}"
    input.tool == "{tool}"
}} else := {{"allow": true, "rule": "workspace-allow"}}
"#
        )
    }

    /// A Rego capability denial removes the named tool and nothing else: bash
    /// and its siblings stay registered.
    #[test]
    fn a_rego_capability_denial_removes_only_the_named_tool() {
        let root = scratch("policy-capability-job-output");
        write_policy(
            &root,
            &deny_rule("no-job-output", "capability", "job_output"),
        );
        let settings = rooted(&root);
        let (bash, background, output, kill, subagent) = gated_tools(&settings);
        assert!(bash, "bash is not part of the job_output decision");
        assert!(background, "the background flag is not job_output either");
        assert!(!output, "the denied capability removed job_output");
        assert!(kill, "an unrelated tool stayed");
        assert!(subagent, "an unrelated tool stayed");
    }

    /// Denying `run_in_background` keeps the shell tool but withdraws the
    /// flag: the schema no longer advertises it, so the capability is not
    /// merely refused after the fact.
    #[test]
    fn a_rego_capability_denial_withdraws_the_background_flag() {
        let root = scratch("policy-capability-background");
        write_policy(
            &root,
            &deny_rule("no-background", "capability", "run_in_background"),
        );
        let settings = rooted(&root);
        let (bash, background, output, kill, _subagent) = gated_tools(&settings);
        assert!(bash, "the shell itself stayed");
        assert!(!background, "the denied capability withdrew the flag");
        assert!(output && kill, "the job tools are independent");
    }

    /// A policy that denies the subagent spawn creates no child at all: the
    /// registry has no row and the store has no child session to persist.
    #[test]
    fn a_rego_subagent_spawn_denial_creates_no_child() {
        use std::sync::Arc;
        let root = scratch("policy-subagent-denied");
        write_policy(&root, &deny_rule("no-subagent-spawn", "spawn", "subagent"));
        let settings = rooted(&root);
        let runs = Arc::new(crate::subagent::SubagentRegistry::default());
        let built = crate::caps::build_agent_for_session_with_subagents(
            &settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "parent",
            "parent",
            runs.clone(),
        );
        let registry = built.agent.registry().expect("registry");
        assert!(
            registry.get_async("subagent").is_some(),
            "the capability query allows the tool; the spawn check is the one that refuses"
        );
        let error = registry
            .execute(
                "subagent",
                &serde_json::json!({
                    "description": "probe",
                    "prompt": "This must never run."
                }),
            )
            .expect_err("the spawn is denied by the workspace policy");
        assert!(error.to_string().contains("no-subagent-spawn"), "{error}");
        assert!(
            runs.snapshot().is_empty(),
            "a denied spawn must not create a run row"
        );
        // Child sessions are persisted from registry rows only; proving the
        // store holds none is the end-to-end consequence of the empty row set.
        let store = crate::store::Store::open_at(root.join("store"));
        assert!(
            store.subagent_sessions().is_empty(),
            "a denied spawn must not leave a child session file"
        );
    }

    /// A policy that denies the job_kill spawn leaves the process running: the
    /// refusal names the rule, the roster still reads Running, and the pid is
    /// still alive. The command's marker is never written.
    #[test]
    fn a_rego_job_kill_spawn_denial_leaves_the_process_running() {
        use std::sync::Arc;
        let root = scratch("policy-job-kill-denied");
        write_policy(&root, &deny_rule("no-job-kill", "spawn", "job_kill"));
        let settings = rooted(&root);
        let jobs = Arc::new(agent::jobs::JobRegistry::new());
        let built = crate::caps::build_agent_for_session_with_subagents_and_jobs(
            &settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "session",
            "session",
            Arc::new(crate::subagent::SubagentRegistry::default()),
            jobs.clone(),
        );
        let registry = built.agent.registry().expect("registry");
        let marker = root.join("marker.txt");
        let started = registry
            .execute(
                "bash",
                &serde_json::json!({
                    "command": format!("sleep 30; printf made > {}", marker.display()),
                    "run_in_background": true
                }),
            )
            .expect("the spawn itself is allowed");
        let id = started["job_id"].as_str().expect("a job id").to_string();
        let pid = jobs
            .job(&id)
            .and_then(|job| job.pid)
            .expect("the roster recorded a pid");
        assert!(agent::jobs::pid_exists(pid), "the premise: it is running");

        let error = registry
            .execute("job_kill", &serde_json::json!({ "id": id }))
            .expect_err("the workspace policy denies the kill");
        assert!(error.to_string().contains("no-job-kill"), "{error}");
        assert_eq!(
            jobs.job(&id).expect("the job").status,
            agent::jobs::JobStatus::Running,
            "the denied kill changed no status"
        );
        assert!(
            agent::jobs::pid_exists(pid),
            "a denied kill must leave the process alive (pid {pid})"
        );
        assert!(
            !marker.exists(),
            "the command never reached its marker, so it did not finish on its own"
        );

        // Reap the child the test started, outside the tool boundary.
        assert!(jobs.kill(&id), "the test reaps its own child");
        jobs.wait_for(&id, std::time::Duration::from_secs(3));
        for _ in 0..40 {
            if !agent::jobs::pid_exists(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
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

    /// A settings tree whose active workspace is `root`.
    fn rooted(root: &std::path::Path) -> Settings {
        let mut settings = bare();
        let id = settings.note_workspace("scratch", &root.to_string_lossy());
        settings.global.general.active_workspace = id;
        settings
    }

    /// A full-access build, so the old root confinement is gone and the policy
    /// is the only thing that can refuse the call.
    fn full_access(root: &std::path::Path) -> Settings {
        let mut settings = rooted(root);
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        settings
    }

    /// A denied write is proved by the filesystem, not by the error string: the
    /// file does not exist and the parent directory's mtime never moved.
    #[test]
    fn a_denied_write_never_reaches_the_filesystem() {
        let root = scratch("policy-denied-write");
        std::fs::create_dir_all(root.join("src")).expect("inside directory");
        let outside_dir = scratch("policy-denied-write-outside");
        let outside = outside_dir.join("escaped-policy-write.txt");
        let parent = outside_dir.as_path();
        let before = std::fs::metadata(parent)
            .expect("parent metadata")
            .modified()
            .expect("mtime");

        let built = build(&full_access(&root));
        let error = built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": outside.to_string_lossy(),
                    "content": "escaped",
                }),
            )
            .expect_err("the policy refuses a write outside the workspace");
        let message = error.to_string();
        assert!(message.contains("write-outside-workspace"), "{message}");
        assert!(!outside.exists(), "the denied write created the file");
        let after = std::fs::metadata(parent)
            .expect("parent metadata")
            .modified()
            .expect("mtime");
        assert_eq!(
            before, after,
            "the denied write touched the parent directory"
        );
    }

    /// A write inside the workspace is allowed (so the test above is not a
    /// policy that refuses everything), and .git/.bsh internals are refused
    /// with the file never created.
    #[test]
    fn a_write_inside_is_allowed_and_git_internals_are_not() {
        let root = scratch("policy-git-write");
        std::fs::create_dir_all(root.join(".git").join("hooks")).expect("git hooks");
        let built = build(&rooted(&root));
        let registry = built.agent.registry().expect("registry");

        registry
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": root.join("src").join("main.rs").to_string_lossy(),
                    "content": "fn main() {}",
                }),
            )
            .expect("an in-workspace write is allowed");
        assert!(root.join("src").join("main.rs").exists());

        let hook = root.join(".git").join("hooks").join("pre-commit");
        let before = std::fs::metadata(root.join(".git").join("hooks"))
            .expect("metadata")
            .modified()
            .expect("mtime");
        let error = registry
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": hook.to_string_lossy(),
                    "content": "#!/bin/sh\n",
                }),
            )
            .expect_err("git internals are refused");
        assert!(
            error.to_string().contains("write-forbidden-directory"),
            "{error}"
        );
        assert!(!hook.exists(), "the denied write created the hook");
        let after = std::fs::metadata(root.join(".git").join("hooks"))
            .expect("metadata")
            .modified()
            .expect("mtime");
        assert_eq!(before, after);
    }

    /// A read outside the workspace is refused and its content never reaches
    /// the result; a read inside is allowed. Both are proved by the probe file
    /// the test owns.
    #[test]
    fn a_read_outside_is_refused_while_a_read_inside_is_allowed() {
        let root = scratch("policy-read");
        std::fs::write(root.join("inside.txt"), "inside-content").expect("inside");
        let outside_dir = scratch("policy-read-outside");
        let outside = outside_dir.join("outside-policy-read.txt");
        std::fs::write(&outside, "outside-secret").expect("outside");

        let built = build(&full_access(&root));
        let registry = built.agent.registry().expect("registry");

        let inside = registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("inside.txt").to_string_lossy() }),
            )
            .expect("an in-workspace read is allowed");
        assert_eq!(inside["content"], "inside-content");

        let error = registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": outside.to_string_lossy() }),
            )
            .expect_err("a read outside the workspace is refused");
        let message = error.to_string();
        assert!(message.contains("read-outside-workspace"), "{message}");
        assert!(
            !message.contains("outside-secret"),
            "the file was read: {message}"
        );
    }

    /// The workspace policy file is the workspace half of the intersection: it
    /// can deny a path the baseline allows.
    #[test]
    fn a_workspace_policy_file_narrows_the_baseline() {
        let root = scratch("policy-workspace-file");
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(
            root.join(".bsh").join("policy.rego"),
            r#"package policy.workspace

# rule: no-secret-reads
verdict := {"allow": false, "rule": "no-secret-reads", "reason": "secrets are not readable"} if {
    contains(input.target.path, "secret")
} else := {"allow": true, "rule": "workspace-allow"}
"#,
        )
        .expect("policy file");
        std::fs::write(root.join("secret.txt"), "shh").expect("secret");
        std::fs::write(root.join("notes.txt"), "notes").expect("notes");

        let built = build(&full_access(&root));
        assert!(built.policy.error().is_none(), "{:?}", built.policy.error());
        let registry = built.agent.registry().expect("registry");

        registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("notes.txt").to_string_lossy() }),
            )
            .expect("an ordinary read passes both policies");
        let error = registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("secret.txt").to_string_lossy() }),
            )
            .expect_err("the workspace rule narrows the baseline");
        assert!(error.to_string().contains("no-secret-reads"), "{error}");
    }

    /// edit_file asks for a Write before it reads or writes: an edit inside
    /// succeeds, an edit outside is refused with the file untouched, and an
    /// ambiguous match is refused unless replace_all says otherwise.
    #[test]
    fn an_edit_is_gated_before_the_file_is_touched() {
        let root = scratch("policy-edit");
        std::fs::write(root.join("inside.txt"), "alpha beta alpha").expect("inside");
        std::fs::write(root.join("ambiguous.txt"), "x x").expect("ambiguous");
        let outside_dir = scratch("policy-edit-outside");
        let outside = outside_dir.join("outside.txt");
        std::fs::write(&outside, "outside-original").expect("outside");

        let built = build(&full_access(&root));
        let registry = built.agent.registry().expect("registry");

        let result = registry
            .execute(
                "edit_file",
                &serde_json::json!({
                    "path": root.join("inside.txt").to_string_lossy(),
                    "old_string": "beta",
                    "new_string": "gamma",
                }),
            )
            .expect("an in-workspace edit is allowed");
        assert_eq!(result["replacements"], 1);
        assert_eq!(
            std::fs::read_to_string(root.join("inside.txt")).expect("read"),
            "alpha gamma alpha"
        );

        let error = registry
            .execute(
                "edit_file",
                &serde_json::json!({
                    "path": outside.to_string_lossy(),
                    "old_string": "outside",
                    "new_string": "hacked",
                }),
            )
            .expect_err("an edit outside the workspace is refused");
        assert!(
            error.to_string().contains("write-outside-workspace"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).expect("read"),
            "outside-original",
            "the denied edit changed the file"
        );

        let ambiguous = registry.execute(
            "edit_file",
            &serde_json::json!({
                "path": root.join("ambiguous.txt").to_string_lossy(),
                "old_string": "x",
                "new_string": "y",
            }),
        );
        assert!(
            matches!(&ambiguous, Err(agent::ToolError::InvalidInput(message)) if message.contains("occurs 2 times")),
            "{ambiguous:?}"
        );

        registry
            .execute(
                "edit_file",
                &serde_json::json!({
                    "path": root.join("ambiguous.txt").to_string_lossy(),
                    "old_string": "x",
                    "new_string": "y",
                    "replace_all": true,
                }),
            )
            .expect("replace_all covers every occurrence");
        assert_eq!(
            std::fs::read_to_string(root.join("ambiguous.txt")).expect("read"),
            "y y"
        );
    }

    /// .git internals are refused for edits as they are for writes, and the
    /// file is left exactly as it was.
    #[test]
    fn an_edit_into_git_internals_is_denied() {
        let root = scratch("policy-edit-git");
        std::fs::create_dir_all(root.join(".git")).expect(".git");
        std::fs::write(root.join(".git").join("config"), "original").expect("config");
        let built = build(&rooted(&root));
        let error = built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "edit_file",
                &serde_json::json!({
                    "path": root.join(".git").join("config").to_string_lossy(),
                    "old_string": "original",
                    "new_string": "hacked",
                }),
            )
            .expect_err("git internals are refused");
        assert!(
            error.to_string().contains("write-forbidden-directory"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".git").join("config")).expect("read"),
            "original"
        );
    }

    /// A workspace policy file that does not parse reaches the tools as the
    /// documented failure asymmetry: writes are refused, an in-workspace read
    /// is still allowed, and the refusal names the failure.
    #[test]
    fn a_broken_workspace_policy_denies_writes_and_allows_reads() {
        let root = scratch("policy-broken");
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(root.join(".bsh").join("policy.rego"), "this is not rego")
            .expect("broken policy");
        std::fs::write(root.join("notes.txt"), "notes").expect("notes");

        let built = build(&full_access(&root));
        assert!(
            built.policy.error().is_some(),
            "the set is in its failure mode"
        );
        let registry = built.agent.registry().expect("registry");

        registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("notes.txt").to_string_lossy() }),
            )
            .expect("an in-workspace read survives a broken policy");
        let error = registry
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": root.join("blocked.txt").to_string_lossy(),
                    "content": "blocked",
                }),
            )
            .expect_err("a broken policy denies writes");
        assert!(
            error.to_string().contains("policy-evaluator-failure"),
            "{error}"
        );
        assert!(!root.join("blocked.txt").exists());
    }

    /// A preset can name the tools, skills, MCP servers, extensions and LLM
    /// persona it applies; selecting it for a session applies all of them.
    #[test]
    fn a_preset_applies_tools_skills_mcps_extensions_and_persona() {
        use std::sync::Arc;

        let root = scratch("preset-config");
        write_skill(&root, "alpha", "one");
        write_skill(&root, "beta", "two");
        let mut settings = with_one_source(&root);
        settings.workspace.agent.file_tools_enabled = true;
        settings.global.presets.insert(
            "scoped".to_string(),
            Preset {
                name: Some("scoped".to_string()),
                plugins: vec!["files".to_string()],
                tools: Some(vec!["read_file".to_string()]),
                skills: Some(vec!["alpha".to_string()]),
                mcps: Some(vec!["other".to_string()]),
                extensions: Some(Vec::new()),
                system_prompt: Some("You are terse.".to_string()),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "scoped".to_string();

        let fake: Arc<dyn react::tool::registry::AsyncTool> =
            Arc::new(agent::tools::AsyncFunctionTool::from_fn(
                "test_echo",
                "Echo",
                serde_json::json!({ "type": "object", "properties": {} }),
                |_args| async move { Ok(serde_json::json!({ "ok": true })) },
            ));
        let mcp = crate::mcp::McpPool::with_tools(vec![crate::mcp::McpTool {
            namespace: "test".to_string(),
            name: "echo".to_string(),
            adapter: fake,
        }]);
        let built = build_agent(
            &settings,
            &Vault::disabled(),
            &crate::settings::resolve::SessionChoice::default(),
            &mcp,
        );

        let mut tools: Vec<String> = built
            .plugins
            .iter()
            .flat_map(|plugin| plugin.tools.clone())
            .collect();
        tools.sort();
        assert_eq!(
            tools,
            vec!["read_file".to_string()],
            "the tool allowlist wins"
        );
        assert_eq!(
            built.agent.config().system_prompt,
            "You are terse.".to_string(),
            "the preset persona reaches the model"
        );
        assert_eq!(
            skill_names(&built),
            vec!["alpha".to_string()],
            "the skill allowlist narrows the catalogue"
        );
        assert!(
            built
                .agent
                .registry()
                .expect("registry")
                .get("test_echo")
                .is_none(),
            "the MCP namespace allowlist excludes other servers"
        );

        // The same preset with the right MCP namespace and extension id sees
        // both again, so this is a filter and not a permanently empty build.
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions/dsh-echo");
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "dsh-echo".to_string(),
                path: Some(fixture.to_string_lossy().to_string()),
                ..Default::default()
            });
        settings.global.presets.get_mut("scoped").unwrap().mcps = Some(vec!["test".to_string()]);
        settings
            .global
            .presets
            .get_mut("scoped")
            .unwrap()
            .extensions = Some(vec!["dsh-echo".to_string()]);
        // The same preset selected by the SESSION applies the whole config:
        // the global default is empty here, so only Session.agent_preset names it.
        settings.global.default_preset = String::new();
        let session_choice = crate::settings::resolve::SessionChoice {
            model: None,
            preset: Some("scoped".to_string()),
        };
        let built = build_agent(&settings, &Vault::disabled(), &session_choice, &mcp);
        assert_eq!(
            built.agent.config().system_prompt,
            "You are terse.".to_string(),
            "a session's preset also carries the persona"
        );
        let mut session_tools: Vec<String> = built
            .plugins
            .iter()
            .flat_map(|plugin| plugin.tools.clone())
            .collect();
        session_tools.sort();
        assert!(
            session_tools.contains(&"read_file".to_string())
                && !session_tools.contains(&"write_file".to_string()),
            "the preset's tool allowlist still narrows the built-in tools: {session_tools:?}"
        );
        assert!(
            built
                .agent
                .registry()
                .expect("registry")
                .get_async("test_echo")
                .is_some(),
            "the selected MCP namespace is registered"
        );
        assert!(
            built
                .agent
                .registry()
                .expect("registry")
                .get("dsh_echo")
                .is_some(),
            "the selected extension is loaded"
        );
    }

    /// The shipped authorization presets enforce their permission, path and
    /// built-in policy together, and a Full-access session cannot lift a
    /// read-only preset.
    #[test]
    fn built_in_authorization_presets_enforce_their_scopes() {
        use agent::policy::{Action, PolicyInput, Target};

        let root = scratch("preset-authorization");
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::create_dir_all(root.join("docs")).expect("docs");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        std::fs::write(root.join(".env"), "SECRET=1\n").expect("env");

        let decide = |built: &BuiltAgent, action: Action, path: &std::path::Path| {
            built
                .policy
                .check(&PolicyInput::new("probe", action, Target::path(path)))
                .is_allowed()
        };

        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "scoped-docs".to_string(),
            Preset {
                permission: Some("workspace-write".to_string()),
                read: Some(vec![".".to_string()]),
                write: Some(vec!["docs".to_string(), "README.md".to_string()]),
                deny: vec![
                    ".git".to_string(),
                    ".bsh".to_string(),
                    ".env".to_string(),
                    "secrets".to_string(),
                ],
                plugins: vec!["files".to_string()],
                tools: Some(vec![
                    "read_file".to_string(),
                    "write_file".to_string(),
                    "edit_file".to_string(),
                    "list_dir".to_string(),
                ]),
                builtin_policy: vec![
                    "deny-secrets".to_string(),
                    "docs-only".to_string(),
                    "no-shell".to_string(),
                ],
                system_prompt: Some(
                    "You edit documentation only. Preserve frontmatter and code fences, and do not change code or configuration.".to_string(),
                ),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "scoped-docs".to_string();
        let built = build(&settings);
        assert!(
            decide(&built, Action::Read, &root.join("src/main.rs")),
            "docs may read the workspace"
        );
        assert!(
            !decide(&built, Action::Read, &root.join(".env")),
            "docs must not read secrets"
        );
        assert!(
            decide(&built, Action::Write, &root.join("docs/guide.md")),
            "docs may write its docs scope"
        );
        assert!(
            !decide(&built, Action::Write, &root.join("src/main.rs")),
            "docs must not write source"
        );
        assert!(
            !decide(&built, Action::Write, &root.join(".git/config")),
            "docs must not write Git internals"
        );
        assert_eq!(
            built.agent.config().system_prompt,
            "You edit documentation only. Preserve frontmatter and code fences, and do not change code or configuration."
        );

        // The session may be Full access; read-only still caps it at read-only
        // and registers no writer.
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        settings.global.default_preset = "read-only".to_string();
        let built = build(&settings);
        let mut tools: Vec<String> = built
            .plugins
            .iter()
            .flat_map(|plugin| plugin.tools.clone())
            .collect();
        tools.sort();
        assert_eq!(
            tools,
            vec![
                "list_dir".to_string(),
                "read_file".to_string(),
                "update_plan".to_string()
            ],
            "read-only mounts only the read-side tools"
        );
        assert!(
            !decide(&built, Action::Write, &root.join("docs/guide.md")),
            "a Full-access session cannot write through read-only"
        );
        assert!(
            built.agent.config().web_search,
            "read-only allows web search"
        );

        // A read-only ceiling is a Rego source even when the preset declares no
        // write scope or built-in read-only module.
        let mut generated = rooted(&root);
        generated.global.presets.insert(
            "generated-ro".to_string(),
            Preset {
                permission: Some("read-only".to_string()),
                plugins: vec!["files".to_string()],
                ..Preset::default()
            },
        );
        generated.global.default_preset = "generated-ro".to_string();
        let built = build(&generated);
        assert!(
            !decide(&built, Action::Write, &root.join("docs/guide.md")),
            "the generated preset permission policy denies writes"
        );

        // PTC is the batch-tool mode: parallel calls plus its instruction.
        settings.global.default_preset = "ptc".to_string();
        let built = build(&settings);
        assert!(
            built.agent.config().parallel_tool_calls,
            "PTC enables parallel calls"
        );
        assert!(built.agent.config().web_search, "PTC keeps web search");
        assert!(
            built
                .agent
                .config()
                .system_prompt
                .contains("programmatic tool calling"),
            "PTC carries its batch-call persona"
        );

        // Creator is the extension-authoring mode.
        settings.global.default_preset = "creator".to_string();
        let built = build(&settings);
        assert!(
            built.agent.config().system_prompt.contains("extend bsh"),
            "Creator carries its extension-authoring persona"
        );

        // A preset egress list is intersected with the workspace allowlist;
        // it can only remove addresses, never add one.
        //
        // NEW SEMANTICS: a read-only effective permission now denies egress in
        // the generated session tier, so the intersection is proved under a
        // writable ceiling. The read-only egress denial has its own test below.
        std::fs::create_dir_all(root.join(".bsh")).expect("bsh dir");
        std::fs::write(
            root.join(".bsh").join("egress-allowlist"),
            "1.2.3.4
5.6.7.8
",
        )
        .expect("egress list");
        let mut egress = rooted(&root);
        egress.global.presets.insert(
            "scoped-egress".to_string(),
            Preset {
                permission: Some("workspace-write".to_string()),
                egress: Some(vec!["1.2.3.4".to_string()]),
                ..Preset::default()
            },
        );
        egress.global.default_preset = "scoped-egress".to_string();
        let built = build(&egress);
        let egress_ok = |ip: &str| {
            built
                .policy
                .check(&PolicyInput::new(
                    "mcp",
                    Action::Egress,
                    Target::host("example.test", ip),
                ))
                .is_allowed()
        };
        assert!(egress_ok("1.2.3.4"), "the preset allows its address");
        assert!(
            !egress_ok("5.6.7.8"),
            "the preset cannot widen the workspace list and removes an address"
        );

        // A required policy that is not trusted is refused at the send gate,
        // before a turn can build without it.
        egress.global.presets.insert(
            "strict".to_string(),
            Preset {
                policy: Some("package policy.strict\nverdict := {\"allow\": true}".to_string()),
                policy_mode: Some("required".to_string()),
                ..Preset::default()
            },
        );
        egress.global.default_preset = "strict".to_string();
        let choice = crate::settings::resolve::SessionChoice {
            model: None,
            preset: Some("strict".to_string()),
        };
        assert_eq!(
            egress.refusal_for(&choice),
            Some("composer.presetPolicyUntrusted")
        );
    }

    /// The parent turn gets the subagent tool; an ordinary build (the one the
    /// child uses) does not, so children cannot recurse.
    #[test]
    fn a_parent_turn_carries_the_subagent_tool() {
        use std::sync::Arc;
        let settings = bare();
        let registry = Arc::new(crate::subagent::SubagentRegistry::default());
        let built = crate::caps::build_agent_for_session_with_subagents(
            &settings,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "parent",
            "parent",
            registry,
        );
        let tool = built
            .agent
            .registry()
            .expect("registry")
            .get_async("subagent")
            .expect("the parent turn carries the subagent tool");
        assert!(
            tool.description().contains("split into independent parts"),
            "the tool description tells the model to split a large job"
        );
        let prompt = &built.agent.config().system_prompt;
        assert!(
            prompt.contains("## Delegating with subagents"),
            "the standing guidance reaches the model"
        );
        assert!(
            prompt.contains("at most three children"),
            "the guidance names the concurrency wave"
        );
        let plain = build(&settings);
        assert!(
            plain
                .agent
                .registry()
                .expect("registry")
                .get_async("subagent")
                .is_none(),
            "a plain build does not carry the tool"
        );
        assert!(
            !plain
                .agent
                .config()
                .system_prompt
                .contains("## Delegating with subagents"),
            "guidance only travels with the tool"
        );

        // The same consequence when a parent build is gated OFF: read-only
        // removes the tool, so the standing delegation guidance must not
        // reach the model either.
        let mut read_only = bare();
        read_only.workspace.agent.permission = READ_ONLY.to_string();
        let denied = crate::caps::build_agent_for_session_with_subagents(
            &read_only,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "parent",
            "parent",
            Arc::new(crate::subagent::SubagentRegistry::default()),
        );
        assert!(
            denied
                .agent
                .registry()
                .expect("registry")
                .get_async("subagent")
                .is_none(),
            "read-only withholds the subagent tool"
        );
        assert!(
            !denied
                .agent
                .config()
                .system_prompt
                .contains("## Delegating with subagents"),
            "guidance is absent when the tool is not registered"
        );

        // An allowlist that omits the name is the other way the tool goes
        // away, and it must take the guidance with it.
        let narrowed = with_tools(WORKSPACE_WRITE, Some(&["bash"]));
        let narrowed = crate::caps::build_agent_for_session_with_subagents(
            &narrowed,
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
            "parent",
            "parent",
            Arc::new(crate::subagent::SubagentRegistry::default()),
        );
        assert!(
            narrowed
                .agent
                .registry()
                .expect("registry")
                .get_async("subagent")
                .is_none(),
            "a preset allowlist excluding subagent removes it"
        );
        assert!(
            !narrowed
                .agent
                .config()
                .system_prompt
                .contains("## Delegating with subagents"),
            "guidance is absent when an allowlist removes the tool"
        );
    }

    /// A preset's Rego module applies only when the reader trusted it. A
    /// trusted preset narrows the baseline; an untrusted one is ignored rather
    /// than failing, so the baseline and workspace policy still hold.
    #[test]
    fn a_trusted_preset_policy_narrows_and_an_untrusted_one_is_ignored() {
        let root = scratch("preset-policy");
        std::fs::write(root.join("secret.txt"), "shh").expect("secret");
        std::fs::write(root.join("notes.txt"), "notes").expect("notes");
        let rego = r#"package policy.preset

# rule: no-secret-reads
verdict := {"allow": false, "rule": "no-secret-reads", "reason": "secrets are not readable"} if {
    input.action == "read"
    contains(input.target.path, "secret")
} else := {"allow": true, "rule": "preset-allow"}
"#;
        let mut settings = full_access(&root);
        settings.global.presets.insert(
            "strict".to_string(),
            Preset {
                plugins: vec!["files".to_string()],
                policy: Some(rego.to_string()),
                trusted: true,
                ..Preset::default()
            },
        );
        settings.global.default_preset = "strict".to_string();

        let trusted = build(&settings);
        let registry = trusted.agent.registry().expect("registry");
        let error = registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("secret.txt").to_string_lossy() }),
            )
            .expect_err("the trusted preset's policy narrows the read");
        assert!(error.to_string().contains("no-secret-reads"), "{error}");
        registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("notes.txt").to_string_lossy() }),
            )
            .expect("an ordinary read still passes");

        settings.global.presets.get_mut("strict").unwrap().trusted = false;
        let untrusted = build(&settings);
        untrusted
            .agent
            .registry()
            .expect("registry")
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("secret.txt").to_string_lossy() }),
            )
            .expect("an untrusted preset's policy is ignored");
    }

    /// The policy input already carries the preset id, before the preset UI
    /// exists: the data document is where the authority half of a principal
    /// lives.
    #[test]
    fn the_policy_carries_the_preset_id() {
        let mut settings = bare();
        settings.global.presets.insert(
            "tight".to_string(),
            Preset {
                name: Some("tight".to_string()),
                plugins: vec!["shell".to_string()],
                description: None,
                order: None,
                policy: None,
                trusted: false,
                ..Default::default()
            },
        );
        settings.global.default_preset = "tight".to_string();
        let built = build(&settings);
        assert_eq!(built.policy.data().preset, "tight");
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

    /// A contributed provider's models resolve through the same lookup a
    /// hand-declared one does, and the reader's own tree still wins an id
    /// collision.
    ///
    /// This is the seam that makes a DSH package's models usable: the model
    /// layer reads ONE tree, so the composer menu, the send gate and the build
    /// cannot disagree about whether a plugin's endpoint exists.
    #[test]
    fn a_contributed_provider_resolves_and_never_shadows_the_readers_own() {
        use crate::settings::global::{GlobalSettings, ModelRef, Provider};
        use std::collections::BTreeMap;

        let mut global = GlobalSettings::default();
        let mut node = BTreeMap::new();
        node.insert(
            "deepseek-v4.1-flash-free".to_string(),
            ModelEntry {
                model: "deepseek-v4.1-flash-free".to_string(),
                api_mode: Some("chat".to_string()),
                ..ModelEntry::default()
            },
        );
        let mut providers = BTreeMap::new();
        providers.insert(
            "zen".to_string(),
            Provider {
                name: Some("OpenCode Zen".to_string()),
                base_url: "https://opencode.ai/zen/v1".to_string(),
                api_mode: Some("chat".to_string()),
                models: node,
                ..Provider::default()
            },
        );
        let mut keys = BTreeMap::new();
        keys.insert("zen".to_string(), false);
        let mut owners = BTreeMap::new();
        owners.insert("zen".to_string(), "@opencode2dsh/dsh-plugin".to_string());
        global.set_plugin_providers(providers, keys, owners);

        let reference = ModelRef {
            provider: "zen".to_string(),
            model: "deepseek-v4.1-flash-free".to_string(),
        };
        let (provider, leaf) = global.lookup(&reference).expect("a plugin model resolves");
        assert_eq!(provider.base_url, "https://opencode.ai/zen/v1");
        assert_eq!(leaf.model, "deepseek-v4.1-flash-free");
        assert!(global.is_plugin_provider("zen"));
        assert_eq!(
            global.plugin_provider_owner("zen"),
            Some("@opencode2dsh/dsh-plugin")
        );
        assert!(
            !global.provider_requires_key("zen"),
            "the package declared an anonymous lane"
        );
        assert!(global.provider_ids().contains(&"zen".to_string()));

        // The reader declaring the same id takes it back, and the plugin's
        // endpoint is no longer what a session resolves against.
        global.providers.insert(
            "zen".to_string(),
            Provider {
                base_url: "https://mine.example/v1".to_string(),
                models: BTreeMap::from([(
                    "deepseek-v4.1-flash-free".to_string(),
                    ModelEntry {
                        model: "deepseek-v4.1-flash-free".to_string(),
                        ..ModelEntry::default()
                    },
                )]),
                ..Provider::default()
            },
        );
        let (provider, _) = global.lookup(&reference).expect("still resolves");
        assert_eq!(
            provider.base_url, "https://mine.example/v1",
            "a hand-declared provider is never shadowed by a package"
        );
        assert!(!global.is_plugin_provider("zen"));
        assert!(
            global.provider_requires_key("zen"),
            "a provider the reader declared is key-bearing again"
        );
    }

    /// The lowering is a pure function of the declaration: a provider node
    /// carries no credential, and every declared model lands as its own id.
    #[test]
    fn a_contributed_provider_lowers_without_a_credential() {
        use agent::extensions::{ProviderContribution, ProviderModel};

        let contribution = ProviderContribution::new("zen", "https://opencode.ai/zen/v1")
            .with_name("OpenCode Zen")
            .with_api_mode("chat")
            .with_model(ProviderModel {
                id: "glm-5.3-flash-free".to_string(),
                name: Some("GLM 5.3 Flash".to_string()),
                max_tokens: Some(8192),
                input_types: Some(vec!["text".to_string(), "image".to_string()]),
                reasoning_effort: None,
            });
        let node = plugin_provider_node(&contribution);
        assert_eq!(node.base_url, "https://opencode.ai/zen/v1");
        assert!(node.api_key_vault.is_none(), "a package ships no secret");
        assert!(node.legacy_api_key.is_none());
        assert!(
            !node.declared,
            "the pane must not offer to delete a package's provider as the reader's"
        );
        let leaf = node.models.get("glm-5.3-flash-free").expect("the model");
        assert_eq!(leaf.model, "glm-5.3-flash-free");
        assert_eq!(leaf.max_tokens, Some(8192));
        assert_eq!(
            leaf.input_types.as_deref(),
            Some(["text".to_string(), "image".to_string()].as_slice())
        );
    }

    /// A provider declared by an installed package reaches the settings the
    /// build reads, and a package switched off contributes nothing.
    ///
    /// This is the whole seam end to end: a package directory on disk, the
    /// inventory entry that names it, and the provider set the composer, the
    /// send gate and the build all read.
    #[test]
    fn an_installed_package_contributes_its_providers_and_an_uninstalled_one_does_not() {
        use crate::settings::global::{GlobalSettings, InstalledExtension, InstalledStandard};

        let scratch =
            std::env::temp_dir().join(format!("bsh-plugin-provider-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let package = scratch.join("zen");
        std::fs::create_dir_all(package.join("providers")).expect("the descriptor directory");
        std::fs::write(
            package.join("package.json"),
            serde_json::json!({
                "name": "@local/zen",
                "version": "0.3.14",
                "dsh": { "manifestVersion": 1 },
            })
            .to_string(),
        )
        .expect("the manifest");
        std::fs::write(
            package.join("providers/zen.json"),
            serde_json::json!({
                "id": "zen",
                "name": "OpenCode Zen",
                "base_url": "https://opencode.ai/zen/v1",
                "apiMode": "chat",
                "requiresKey": false,
                "models": [
                    { "id": "deepseek-v4.1-flash-free" },
                    { "id": "glm-5.3-flash-free" },
                ],
            })
            .to_string(),
        )
        .expect("the descriptor");

        let mut settings = crate::settings::Settings {
            global: GlobalSettings::default(),
            ..crate::settings::Settings::default()
        };
        settings.global.extensions.push(InstalledExtension {
            id: "@local/zen".to_string(),
            name: "@local/zen".to_string(),
            display_name: "Zen Models".to_string(),
            description: String::new(),
            version: "0.3.14".to_string(),
            standard: InstalledStandard::Dsh,
            source_label: "test".to_string(),
            install_target: "@local/zen@0.3.14".to_string(),
            path: Some(package.to_string_lossy().to_string()),
            managed: true,
            installed_at: 1_700_000_000,
            trusted: false,
            trusted_path: None,
        });

        refresh_plugin_providers(&mut settings);
        assert!(
            settings.global.is_plugin_provider("zen"),
            "the installed package's provider is live"
        );
        assert_eq!(
            settings.global.plugin_provider_owner("zen"),
            Some("@local/zen")
        );
        assert!(
            !settings.global.provider_requires_key("zen"),
            "the package declared an anonymous lane"
        );
        let reference = crate::settings::global::ModelRef {
            provider: "zen".to_string(),
            model: "glm-5.3-flash-free".to_string(),
        };
        let (provider, leaf) = settings
            .global
            .lookup(&reference)
            .expect("a plugin model resolves for the build");
        assert_eq!(provider.base_url, "https://opencode.ai/zen/v1");
        assert_eq!(leaf.model, "glm-5.3-flash-free");
        // The composer's own catalogue reads the same set.
        assert!(
            settings
                .catalogs_by_provider("zen")
                .iter()
                .any(|(id, models)| id == "zen" && models.len() == 2),
            "the model picker offers the plugin's models"
        );

        // Removing the package clears every trace: the maps are rebuilt, never
        // merged, so no stale endpoint survives an uninstall.
        settings.global.extensions.clear();
        refresh_plugin_providers(&mut settings);
        assert!(
            !settings.global.is_plugin_provider("zen"),
            "no inventory entry means no contributed provider"
        );
        assert!(
            settings.global.lookup(&reference).is_none(),
            "and the model no longer resolves"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }
    /// A capability chip has to name something the reader can reach.
    ///
    /// The registry accepts commands, panels and prompts, but bsh draws
    /// none of them: the composer palette is a fixed list, nothing renders a
    /// registry panel, and a prompt has no consumer. Listing them would put a
    /// chip on a control that does not exist.
    #[test]
    fn only_the_capabilities_this_shell_draws_are_surfaced() {
        use agent::extensions::Capability;

        let loaded = vec![
            Capability::Tools,
            Capability::Hooks,
            Capability::Skills,
            Capability::Models,
            Capability::Commands,
            Capability::Panels,
            Capability::Prompts,
        ];
        assert_eq!(
            surfaced_capabilities(&loaded),
            vec![
                Capability::Tools,
                Capability::Hooks,
                Capability::Skills,
                Capability::Models
            ],
            "the four kinds bsh draws survive, and the three it does not are dropped"
        );

        // A package whose whole payload was refused surfaces nothing, which is
        // what the row beside its refusal disclosure has to say.
        assert!(
            surfaced_capabilities(&[]).is_empty(),
            "nothing loaded means nothing to claim"
        );
    }
}
