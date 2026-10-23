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
    Action, EgressGate, EgressRuleSet, PolicyData, PolicyInput, PolicySet, PolicySource,
    SystemResolver, Target,
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

/// The one sandbox spec for a turn's shell and its terminal pane.
///
/// Resolution decides heel vs off once (`Effective::sandbox`); this maps that
/// decision onto the spec both spawn paths share:
///
/// - heel resolves to a policy-derived spec. A FULL-ACCESS session whose
///   PRESET asked for heel is built as Workspace Write: the permission released
///   confinement, and the preset narrowed it back, so the grants and the egress
///   proxy come from the policy exactly as they do for a confined session.
/// - off (permitted only at full access) is `bare(FullAccess)`: it spawns a
///   bare shell and the tool result reports `sandbox: "none"`.
/// - an absent field at full access keeps the permission's own full-access
///   spec, which is byte-for-byte today's behaviour.
pub(crate) fn sandbox_spec_for(
    policy: &Arc<PolicySet>,
    eff: &Effective,
    tool: &str,
    session: &str,
) -> Arc<agent::sandbox::SandboxSpec> {
    let principal = agent::sandbox::Principal::new(tool, session, eff.preset_id.clone());
    if eff.sandbox.heel {
        // A forced heel under full access must NOT derive a FullAccess spec:
        // that spec is exactly the release the preset narrowed back.
        let tier = if eff.permission.effective == FULL_ACCESS {
            WORKSPACE_WRITE
        } else {
            eff.permission.effective.as_str()
        };
        Arc::new(agent::sandbox::SandboxSpec::for_policy(
            policy, tier, principal,
        ))
    } else if eff.sandbox.declared == Some(crate::settings::global::SandboxChoice::Off) {
        Arc::new(agent::sandbox::SandboxSpec::bare(
            agent::sandbox::SandboxMode::FullAccess,
            policy.data().workspace_root.clone(),
        ))
    } else {
        // Inherit at full access: the permission's own release, unchanged.
        Arc::new(agent::sandbox::SandboxSpec::for_policy(
            policy,
            &eff.permission.effective,
            principal,
        ))
    }
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

/// The process-wide plugin bridge pool.
///
/// A bridged tool is only callable while its plugin's process is alive, and a
/// build re-reads the package on every turn, so the session cannot live in the
/// build. It lives here instead: one child per (package, root), reused across
/// turns, and torn down by the reconcile below when a package stops being asked
/// for.
///
/// This mirrors the MCP pool's shape for the same reason - a connection is the
/// expensive part and a turn must not pay it again - with one difference that
/// matters: an MCP connection is a socket, and this is a process running code the
/// reader approved, so every spawn is behind the trust gate.
static BRIDGE_POOL: std::sync::OnceLock<std::sync::Mutex<agent::extensions::bridge::BridgePool>> =
    std::sync::OnceLock::new();

/// Run `f` against the process-wide bridge pool.
fn with_bridge_pool<T>(f: impl FnOnce(&mut agent::extensions::bridge::BridgePool) -> T) -> T {
    let pool = BRIDGE_POOL
        .get_or_init(|| std::sync::Mutex::new(agent::extensions::bridge::BridgePool::new()));
    match pool.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => f(&mut poisoned.into_inner()),
    }
}

/// Stop every bridged plugin process.
///
/// Called when the shell tears down, so no child outlives the app that started
/// it. Nothing else depends on the timing: the sessions are also reconciled on
/// every build and reaped by `Drop` if the process exits first.
pub(crate) fn stop_bridge_pool() {
    with_bridge_pool(|pool| pool.stop_all());
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
        // A build that only prices the request's tools carries its own browser:
        // the tool schema must be the one a real turn sends, and nothing a
        // pricing build does may reach the reader's pane.
        Arc::new(crate::browser::BrowserHandle::new()),
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
        Arc::new(crate::browser::BrowserHandle::new()),
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
    browser: Arc<crate::browser::BrowserHandle>,
) -> BuiltAgent {
    build_agent_for_session_inner(
        settings,
        vault,
        choice,
        mcp,
        session,
        Some((parent, subagents)),
        jobs,
        browser,
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
    browser: Arc<crate::browser::BrowserHandle>,
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
    // The SHELL's confinement is its own resolved fact: a preset may force heel
    // under full access, or (only at full access) turn it off. The file tools
    // keep following the permission tier, which the preset already caps.
    let shell_confined = eff.sandbox.heel;
    let writes = allows_writes(&permission);
    // ONE capability decision for the shell family and the delegation tool.
    // Permission, the preset's tool allowlist and the Rego capability answer
    // are intersected here; registration below only consumes the result.
    let gates = tool_gates_for(&permission, preset_tools, &policy, session);

    if eff.bash_enabled && !off_bundle("shell") {
        let mut shell_tools: Vec<Arc<dyn agent::tools::Tool>> = Vec::new();
        if gates.bash && !off_tool("bash") {
            let config = agent::tools::bash::BashToolConfig {
                // The cwd is the workspace root whenever the shell is CONFINED
                // or a preset explicitly unconfined it: for those two arms the
                // spawn question is asked about this path, and the baseline's
                // path rule answers it.
                //
                // An INHERITED full-access release is the third arm, and it is
                // deliberately handed None: None is the one switch that leaves
                // BashTool's validator unset and so drops its destructive and
                // elevated-privilege guards. None also means the shell's cwd is
                // the app's launch directory rather than the workspace, so the
                // policy MUST permit the spawn by TIER - that is what the
                // baseline's `spawn-shell-full-access` arm is for. A path-only
                // policy would refuse every command here before a process
                // existed, which is exactly the bug this arm fixes.
                workspace_root: if shell_confined || eff.sandbox.off_by_preset {
                    root.as_ref().map(|r| r.to_string_lossy().to_string())
                } else {
                    None
                },
                timeout_secs: eff.shell_timeout_secs,
                ..Default::default()
            };
            // ONE sandbox spec for the shell and the terminal pane: the grants
            // are the policy's own answers, asked once per candidate, so the
            // confinement is not a profile this file wrote down. At full access
            // the DEFAULT is still an unconfined spec (today's release); a
            // preset's explicit heel is the narrowing arm, and an explicit none
            // is the widening arm the resolver already guard-checked.
            let spec = sandbox_spec_for(&policy, &eff, "bash", session);
            let mut bash = BashTool::new("bash")
                .with_config(config)
                .with_policy(policy.clone(), session, preset.clone())
                .with_jobs(jobs.clone(), session)
                .with_background(gates.background_spawn)
                .with_sandbox_spec(spec);
            if eff.sandbox.off_by_preset {
                bash = bash.with_sandbox_reason(agent::tools::bash::PRESET_SANDBOX_OFF_REASON);
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
    // The packages this build actually asked for, as (id, root). The bridge
    // reconcile below uses it to stop the process of a package that was disabled,
    // uninstalled, distrusted, or repointed: a plugin whose tools are no longer
    // offered must not keep running.
    let mut wanted: Vec<(String, PathBuf)> = Vec::new();
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
        // TRUST IS THE GATE. Only a package the reader approved at THIS path may
        // have its host entry executed; an untrusted one is parsed declaratively
        // and its entry refused by name, which is what `load_with_trust` does
        // without starting a process.
        let trust = if entry.is_trusted() {
            agent::extensions::Trust::Trusted
        } else {
            agent::extensions::Trust::Untrusted
        };
        let root = expand_path(raw);
        match agent::extensions::ExtensionRegistry::load_with_trust(&root, trust) {
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
                // A trusted entry that RAN left a live session behind. Adopt it
                // into the process-wide pool so its tools stay callable after
                // this build returns: the pool is the only owner that outlives
                // the parse.
                if let Some(session) = package.session.clone() {
                    with_bridge_pool(|pool| {
                        pool.adopt(&package_id, root.clone(), Arc::clone(&session));
                    });
                    wanted.push((package_id.clone(), root.clone()));
                }
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
    // A plugin process outlives the build that started it, so the reconcile has
    // to be here: every package the pool holds but this build did not ask for is
    // stopped now. Without it, disabling or untrusting a package would leave its
    // process running with its tools still on the agent.
    with_bridge_pool(|pool| {
        pool.retain(|id, root| {
            wanted
                .iter()
                .any(|(wanted_id, wanted_root)| wanted_id == id && wanted_root == root)
        });
    });
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

    // The built-in browser: its tools act on ONE shared pane and ask the SAME
    // policy every other tool asks. The handle is the shell's, so a call and the
    // reader's pane show the same document; a pricing build passes its own.
    crate::browser::attach(
        &mut agent,
        crate::browser::Tools {
            browser,
            policy: policy.clone(),
            session: session.to_string(),
            preset: preset.clone(),
        },
    );

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
        // The file is TRI-STATE, and the state is a fact the empty rule lists
        // cannot carry: ABSENT means nothing constrains egress (a fresh
        // workspace reaches the network), PRESENT with no rule is a deliberate
        // deny-all, PRESENT with rules is the allowlist. Only the last one is
        // described by the three rule shapes (addresses = the strong door,
        // hostnames = the weak door, ports = a constraint on both).
        match read_egress_rules(root) {
            WorkspaceEgress::Absent => {
                data = data.with_egress_unrestricted(true);
            }
            WorkspaceEgress::Empty => {
                // The baseline's deny-all floor is the right answer for a file
                // the reader created and left empty.
            }
            WorkspaceEgress::Rules(rules) => {
                data = data
                    .with_egress_allowlist(rules.addresses)
                    .with_egress_hosts(rules.hosts)
                    .with_egress_ports(rules.ports);
            }
        }
        // One parse of each scope list into its LITERAL roots (prefix
        // matching, what every shipped preset relies on) and its GLOBS (the
        // entries that carry * ? [ or {). Both shapes reach the same decision,
        // and deny is decided first so it wins even inside a granted root.
        let (read_roots, read_globs) = eff
            .preset_read
            .as_ref()
            .map(|items| preset_scope_patterns(root, items, false))
            .unwrap_or_default();
        let (write_roots, write_globs) = eff
            .preset_write
            .as_ref()
            .map(|items| preset_scope_patterns(root, items, false))
            .unwrap_or_default();
        let (deny_roots, deny_globs) = preset_scope_patterns(root, &eff.preset_deny, true);
        data = data
            .with_preset_scope(
                eff.preset_read.is_some(),
                read_roots,
                eff.preset_write.is_some(),
                write_roots,
                deny_roots,
            )
            .with_preset_globs(read_globs, write_globs, deny_globs);
        if let Some(egress) = &eff.preset_egress {
            // The preset's own entries are classified through the SAME parser
            // the workspace file uses, so a preset can name an address, a
            // hostname (with a `*.` wildcard) or a bare port and mean the same
            // thing the file means. A declared empty list still restricts: it
            // is the preset saying "no network", not an absent field, and the
            // policy then refuses every address while the derived proxy set
            // stays empty.
            data = data.with_preset_egress(true, EgressRuleSet::parse(egress));
        }
    }
    // The session tier as a policy fact, generated for the effective level.
    // A read-only session denies writes, egress and spawns here as well as in
    // the Rust registration matrix; that matrix is the floor (the tools are not
    // even offered), and this module is the second tier (defence in depth).
    // ADVISORY applies to the modules the PRESET contributes, and only to
    // them. The baseline, the session tier and the workspace policy are never
    // marked: in advisory mode the preset's own scope, egress, guard and custom
    // Rego warn instead of blocking, while the floor still refuses. That is
    // what keeps advisory from widening the session or leaving the workspace.
    let advisory = eff.policy_mode() == crate::settings::global::PolicyMode::Advisory;
    let mut sources = vec![
        agent::policy::baseline_source(),
        agent::policy::session_permission_source(&eff.permission.effective),
    ];
    let scope_active =
        eff.preset_read.is_some() || eff.preset_write.is_some() || !eff.preset_deny.is_empty();
    if scope_active {
        sources.push(agent::policy::preset_scope_source().advisory(advisory));
    }
    if eff.preset_egress.is_some() {
        sources.push(agent::policy::preset_egress_source().advisory(advisory));
    }
    // A read-only CEILING is also a policy fact of its own: the preset's module
    // narrows independently of the session tier. The built-in read-only module
    // already carries this rule; add the generated one only when the preset does
    // not name it, so every read-only preset is enforced by Rego.
    let generated_read_only = eff.preset_permission.as_deref() == Some("read-only")
        && !eff
            .preset_builtin_policy
            .iter()
            .any(|id| canonical_builtin_policy_id(id) == "builtin:read-only");
    if generated_read_only {
        sources.push(agent::policy::preset_permission_source().advisory(advisory));
    }
    for id in &eff.preset_builtin_policy {
        match builtin_policy_source(id) {
            Some(source) => sources.push(source.advisory(advisory)),
            None => {
                // An unknown built-in id must not silently disappear from the
                // set: an unparseable placeholder latches the failure mode.
                warn!("unknown preset builtin policy '{id}'");
                sources.push(PolicySource::extension(
                    "preset-builtin",
                    "# unknown preset builtin policy",
                    false,
                ));
            }
        }
    }
    // A required policy that the reader has not trusted makes the preset
    // unusable; refuse it in the set as well as in the send gate.
    if eff.preset_policy_required_untrusted() {
        sources.push(PolicySource::extension(
            "preset-policy",
            "# required preset policy is untrusted",
            false,
        ));
    } else if eff.preset_trusted {
        // A preset is an authority only after the reader trusts it. An
        // untrusted policy is ignored; the send gate refuses the required case
        // before a turn starts.
        if let Some(policy) = eff.preset_policy.as_deref() {
            sources.push(PolicySource::extension("preset", policy, true).advisory(advisory));
        }
    }
    if let Some(root) = root {
        let path = root.join(".bsh").join("policy.rego");
        match std::fs::read_to_string(&path) {
            Ok(rego) => sources.push(PolicySource::workspace("workspace", rego)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                // A policy file that exists but cannot be read must not
                // silently leave only the baseline: that would widen a gate
                // the workspace meant to narrow. An unparseable placeholder
                // latches the set into its failure mode instead.
                warn!("policy file {} could not be read: {error}", path.display());
                sources.push(PolicySource::workspace(
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

/// The stable id a preset's `builtin_policy` entry names.
///
/// The bare names (`deny-secrets`) are the vocabulary every existing preset
/// file uses, so they are accepted as aliases and canonicalised to the
/// `builtin:` id; an unknown entry is returned unchanged so the loader can
/// report it rather than silently substituting one.
pub(crate) fn canonical_builtin_policy_id(id: &str) -> &str {
    let trimmed = id.trim();
    let bare = trimmed.strip_prefix("builtin:").unwrap_or(trimmed);
    match bare {
        "deny-secrets" => "builtin:deny-secrets",
        "read-only" => "builtin:read-only",
        "no-shell" => "builtin:no-shell",
        "no-egress" => "builtin:no-egress",
        "docs-only" => "builtin:docs-only",
        "deny-git-writes" => "builtin:deny-git-writes",
        _ => trimmed,
    }
}

fn builtin_policy_source(id: &str) -> Option<PolicySource> {
    let canonical = canonical_builtin_policy_id(id);
    let rego = match canonical {
        "builtin:deny-secrets" => BUILTIN_DENY_SECRETS,
        "builtin:read-only" => BUILTIN_READ_ONLY,
        "builtin:no-shell" => BUILTIN_NO_SHELL,
        "builtin:no-egress" => BUILTIN_NO_EGRESS,
        "builtin:docs-only" => BUILTIN_DOCS_ONLY,
        "builtin:deny-git-writes" => BUILTIN_DENY_GIT_WRITES,
        _ => return None,
    };
    Some(PolicySource::builtin(canonical, rego))
}

/// Resolve a preset scope list into its LITERAL roots and GLOB patterns.
///
/// The two shapes are not alternatives: a list may mix `docs` (a literal
/// prefix, which is what a directory entry has always meant) with
/// `src/**/*.rs` (a glob). A literal entry is canonicalized when the path
/// exists so a symlinked workspace compares like the tools' resolved paths; a
/// pattern cannot be canonicalized, so it is joined to the same canonical root
/// the tools use.
///
/// Validation happens at LOAD (`Preset::scope_problem`), so an entry here has
/// already been accepted. The fallback still fails CLOSED: an entry that slips
/// through is logged and, for a deny list, becomes a match-everything pattern
/// rather than disappearing - a dropped deny entry would be a widening.
fn preset_scope_patterns(
    root: &std::path::Path,
    items: &[String],
    deny: bool,
) -> (Vec<String>, Vec<String>) {
    let mut roots: Vec<String> = Vec::new();
    let mut globs: Vec<String> = Vec::new();
    for item in items {
        let entry = item.trim();
        if let Err(why) = agent::policy::validate_scope_entry(entry) {
            warn!("preset scope entry '{entry}' is invalid ({why}); refusing to drop it silently");
            // A deny entry that cannot be parsed denies everything in the
            // workspace instead of vanishing. A read/write entry is skipped,
            // which NARROWS (the scope no longer grants it) rather than widens.
            if deny {
                globs.push("**".to_string());
            }
            continue;
        }
        let path = std::path::Path::new(entry);
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if agent::policy::is_glob(entry) {
            globs.push(joined.to_string_lossy().to_string());
        } else {
            let canonical = joined.canonicalize().unwrap_or(joined);
            roots.push(canonical.to_string_lossy().to_string());
        }
    }
    (roots, globs)
}

/// The egress rules this workspace declares.
///
/// One file, `<workspace>/.bsh/egress-allowlist`, with `#` comments and blank
/// lines ignored. Each line is one of three shapes, decided by what it can be:
///
/// - a bare integer is a destination PORT (`443`). No port lines means any
///   port; one or more means only those.
/// - something that parses as an address or as a CIDR range is an ADDRESS rule
///   (`203.0.113.7`, `10.0.0.0/8`). This is the strong door: the policy decides
///   on the resolved address, so a DNS rebind cannot ride it.
/// - anything else is a HOSTNAME rule (`api.example.com`, `*.crates.io`). This
///   is the weaker door - it survives a rebind - and it exists for a policy that
///   names a service rather than an address.
///
/// The file is TRI-STATE, and the three states are different decisions:
///
/// - [`WorkspaceEgress::Absent`] - no file at all. The workspace declared
///   nothing, so egress is UNRESTRICTED; this is the state of a fresh workspace,
///   which is why the network works out of the box.
/// - [`WorkspaceEgress::Empty`] - the file exists and parses to no rule (only
///   comments and blanks). The reader created the file and left it empty, which
///   is a deliberate deny-all.
/// - [`WorkspaceEgress::Rules`] - the file declares rules, which constrain the
///   reach exactly as before.
///
/// The classification itself is [`EgressRuleSet`], the one parser both this
/// file and a preset's own `egress` field read through, so a line cannot mean
/// one thing here and another in a preset.
///
/// An unreadable file is reported as PRESENT (a deny-all), never as absent: a
/// permission error or a directory at the path must not widen a workspace that
/// meant to constrain itself. Only `NotFound` is the absent state.
fn read_egress_rules(root: &Path) -> WorkspaceEgress {
    let path = egress_allowlist_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return WorkspaceEgress::Absent;
        }
        Err(error) => {
            warn!("the workspace egress allowlist could not be read: {error}");
            return WorkspaceEgress::Empty;
        }
    };
    let rules = EgressRuleSet::parse([text]);
    if rules.is_empty() {
        WorkspaceEgress::Empty
    } else {
        WorkspaceEgress::Rules(rules)
    }
}

/// What the workspace egress file declares, in the three states the policy
/// answers differently.
///
/// The rule lists cannot carry the distinction: an absent file and a
/// present-but-empty file both have no rule, and the first is UNRESTRICTED
/// while the second is a deliberate DENY ALL. The reader owns the file's state
/// so the policy can be told which it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceEgress {
    /// No `.bsh/egress-allowlist` file at all: nothing is constrained.
    Absent,
    /// The file exists and parses to no rule: a deliberate deny-all.
    Empty,
    /// The file exists and declares at least one rule.
    Rules(EgressRuleSet),
}

/// The one path the workspace's egress allowlist lives at.
///
/// The reader, the pane's create action and the settings reveal all derive the
/// path here, so the file the shell shows and the file the policy parses cannot
/// drift apart.
pub(crate) fn egress_allowlist_path(root: &Path) -> PathBuf {
    root.join(".bsh").join("egress-allowlist")
}

/// The starter body the Policy pane's "Create starter allowlist" offers.
///
/// This is the SINGLE source of truth for that text: the pane action writes
/// exactly these bytes, so the copy a reader sees and the file they get cannot
/// drift apart.
///
/// CREATING THE FILE IS AN OPT-IN TO CONTROL. With no file at all the workspace
/// is UNRESTRICTED - the network works out of the box - while a file that
/// exists and declares no rule DENIES ALL EGRESS. The starter therefore states
/// both facts in its own words and parses to ZERO rules: the reader who writes
/// it has deliberately chosen the deny-all floor, and uncommenting a rule is
/// how they open one destination.
///
/// The starter carries a commented DENY ALL example, a commented ALLOW ALL
/// (nearest-equivalent) example and one WORKED allow example, every rule still
/// commented so the file itself cannot change the floor when it is written.
///
/// Allow-all is NOT expressible as a single hostname rule: the parser reads a
/// hostname rule as an exact name or a leading "*." suffix, so a bare "*"
/// matches no host, and there is no "any hostname" spelling. The real, closest
/// equivalent is an address rule covering every address - `0.0.0.0/0` for IPv4
/// and `::/0` for IPv6 - which the parser classifies as an ADDRESS rule and
/// `net.cidr_contains` then matches against any resolved address. That is what
/// the commented example below documents, honestly, rather than inventing a
/// syntax the parser does not accept.
pub(crate) const EGRESS_ALLOWLIST_TEMPLATE: &str = "\
# Egress allowlist for this workspace.
#
# CREATING THIS FILE OPTS INTO CONTROL.
#
# With NO file at all the workspace is UNRESTRICTED: the network works out of
# the box. A file that EXISTS and declares no rule - only comments and blank
# lines, like this starter - DENIES ALL EGRESS. That is deliberate: the file is
# the workspace's network floor, and an empty one is a lockdown, not an
# absence. Delete the file to restore the unrestricted default, or uncomment a
# rule to allow a destination.
#
# This file can only NARROW what the baseline and the preset allow; a preset
# may narrow it further and can never add a destination this file does not
# name.
#
# One rule per line; blank lines and '#' comments are ignored. A line is one of
# three shapes, decided by what it can be:
#
#   port     443              a bare integer: a destination PORT. No port lines
#                             means any port; one or more means only those.
#   address  203.0.113.7      an address or a CIDR range: the STRONG door,
#            10.0.0.0/8       decided on the resolved IP, so a DNS rebind
#                             cannot ride it.
#   hostname api.example.com  anything else: a HOSTNAME, the WEAKER door -
#            *.crates.io      the name is what the policy saw, so a rebind
#                             survives it. A leading '*.' matches any subdomain.
#
# Every line below is a COMMENTED example, so this file declares no rule and
# egress is denied until you deliberately uncomment one.
#
# WORKED EXAMPLE - allow one IPv4 address on the HTTPS port. Uncomment both
# lines to make the HTTPS URL of 203.0.113.7 reachable and nothing else:
# 203.0.113.7
# 443
#
# Example: allow a service by name (the weaker door).
# api.example.com
# *.crates.io
#
# DENY ALL: what this file already does. Leave no uncommented rule. There is no
# deny-all keyword - a present file with no rule IS the deny.
#
# ALLOW ALL: there is no wildcard-hostname spelling for any host: a hostname
# rule matches an exact name or a leading '*.' suffix, so '*' matches no host.
# The nearest real equivalent is an address rule covering every address; the
# parser accepts it and it is the honest allow-all:
# 0.0.0.0/0
# ::/0
";

/// Write the starter allowlist whether or not one is already there.
///
/// This is the Policy pane's explicit "Create starter allowlist" action: the
/// reader asked for the template, so an existing file is replaced with exactly
/// the template bytes. Nothing seeds this file automatically: an auto-seeded
/// file would flip a fresh, unrestricted workspace into a deny-all one.
///
/// The written file declares no rule, so creating it DENIES all egress; the
/// pane's notice and the template's own prose both say so before the reader
/// clicks. The workspace-registration path never calls this.
pub(crate) fn write_egress_allowlist_template(root: &Path) -> std::io::Result<()> {
    let path = egress_allowlist_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, EGRESS_ALLOWLIST_TEMPLATE)
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
        agent_messages, allows_writes, build_agent, compactable_span, egress_allowlist_path,
        is_confined, plugin_provider_node, read_egress_rules, refresh_plugin_providers,
        sandbox_spec_for, surfaced_capabilities, tool_gates_for, with_bridge_pool, BuiltAgent,
        WorkspaceEgress, EGRESS_ALLOWLIST_TEMPLATE, FULL_ACCESS, READ_ONLY, WORKSPACE_WRITE,
    };
    use crate::settings::global::{ModelEntry, Preset};
    use crate::settings::resolve::SessionChoice;
    use crate::settings::Settings;
    use crate::vault::Vault;
    use agent::policy::{Action, EgressRuleSet, PolicyInput, Target};

    /// The browser's tools reach the request. A pane the model cannot drive is
    /// a picture; this asserts the nine names are on the agent a turn runs.
    #[test]
    fn a_build_offers_the_browser_tools() {
        let built = build_agent(
            &Settings::default(),
            &Vault::disabled(),
            &SessionChoice::default(),
            &crate::mcp::McpPool::default(),
        );
        let registry = built.agent.registry().expect("a tool registry");
        let names = registry.async_tool_names();
        for tool in [
            "browser_open",
            "browser_snapshot",
            "browser_click",
            "browser_type",
            "browser_press",
            "browser_back",
            "browser_forward",
            "browser_reload",
            "browser_close",
        ] {
            assert!(
                names.iter().any(|name| name == tool),
                "{tool} is offered to the model: {names:?}"
            );
        }
    }
    /// The egress file carries three rule shapes, and a line is classified by
    /// what it can be: a port number, an address, or a hostname. The reader also
    /// reports the file's own STATE, which the rule lists cannot carry: absent
    /// (unrestricted), present-but-empty (deny-all), or present with rules.
    #[test]
    fn egress_rules_are_classified_by_what_they_can_be() {
        let root = std::env::temp_dir().join(format!("bsh-egress-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(
            root.join(".bsh").join("egress-allowlist"),
            "# the workspace's egress rules\n\
             203.0.113.7\n\
             10.0.0.0/8   # a range\n\
             API.Example.test\n\
             *.crates.io\n\
             443\n",
        )
        .expect("the allowlist");
        let rules = match read_egress_rules(&root) {
            WorkspaceEgress::Rules(rules) => rules,
            other => panic!("a file with rules must read as Rules, got {other:?}"),
        };
        assert_eq!(rules.addresses, vec!["203.0.113.7", "10.0.0.0/8"]);
        assert_eq!(
            rules.hosts,
            vec!["api.example.test", "*.crates.io"],
            "hostnames are lower-cased, ranges stay addresses"
        );
        assert_eq!(rules.ports, vec![443]);

        // A missing file is ABSENT, not an empty rule list: the policy answers
        // it with UNRESTRICTED egress, so the two must not be conflated.
        assert_eq!(
            read_egress_rules(&root.join("nowhere")),
            WorkspaceEgress::Absent,
            "no file is its own state"
        );

        // A file that exists with only comments is PRESENT and EMPTY: the
        // deliberate deny-all, which is a different decision from absence.
        let present = root.join("present-empty");
        std::fs::create_dir_all(present.join(".bsh")).expect(".bsh");
        std::fs::write(
            present.join(".bsh").join("egress-allowlist"),
            "# nothing is allowed\n\n",
        )
        .expect("the empty allowlist");
        assert_eq!(
            read_egress_rules(&present),
            WorkspaceEgress::Empty,
            "an existing file with no rule is a deny-all"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The starter template parses to ZERO rules, so WRITING IT is the
    /// deny-all floor rather than a way to get network access; it states the
    /// tri-state in its own copy (creating the file opts into control, leaving
    /// it empty denies all, deleting it restores the unrestricted default) and
    /// documents a commented deny-all example, a commented allow-all
    /// (nearest-equivalent) example and one worked allow example.
    #[test]
    fn the_starter_allowlist_parses_to_no_rules() {
        let rules = EgressRuleSet::parse([EGRESS_ALLOWLIST_TEMPLATE]);
        assert!(
            rules.is_empty(),
            "the template must declare no rule: {rules:?}"
        );
        assert!(rules.addresses.is_empty() && rules.hosts.is_empty() && rules.ports.is_empty());
        // A commented DENY-ALL marker and a commented ALLOW-ALL / nearest
        // equivalent line are both present and both still commented.
        assert!(
            EGRESS_ALLOWLIST_TEMPLATE.contains("# DENY ALL"),
            "the template names the deny-all marker"
        );
        assert!(
            EGRESS_ALLOWLIST_TEMPLATE.contains("# ALLOW ALL"),
            "the template names the allow-all marker"
        );
        // Allow-all is not expressible as a hostname wildcard; the honest form is
        // the address rule covering every address, and it is COMMENTED.
        for line in EGRESS_ALLOWLIST_TEMPLATE.lines() {
            let bare = line.split('#').next().unwrap_or("").trim();
            assert!(
                bare.is_empty(),
                "every line of the template is blank or commented; found: {line:?}"
            );
        }
        assert!(
            EGRESS_ALLOWLIST_TEMPLATE.contains("# 0.0.0.0/0")
                && EGRESS_ALLOWLIST_TEMPLATE.contains("# ::/0"),
            "the allow-all nearest equivalent is documented and commented"
        );
        // The copy is the contract: the reader must learn that the file's
        // EXISTENCE is what opts into control, that an empty one denies, that
        // absence is unrestricted, and that a worked allow example is present.
        for phrase in [
            "CREATING THIS FILE OPTS INTO CONTROL",
            "UNRESTRICTED",
            "DENIES ALL EGRESS",
            "Delete the file to restore",
            "WORKED EXAMPLE",
        ] {
            assert!(
                EGRESS_ALLOWLIST_TEMPLATE.contains(phrase),
                "the starter states the tri-state; missing {phrase:?}"
            );
        }
    }

    /// The documented allow-all nearest equivalent really is one: the parser
    /// reads `0.0.0.0/0` and `::/0` as ADDRESS rules, which is what the template
    /// claims. A bare `*` does NOT match any host, which is why the template does
    /// not offer it.
    #[test]
    fn allow_all_is_the_address_rule_and_not_a_hostname_wildcard() {
        let rules = EgressRuleSet::parse(["0.0.0.0/0", "::/0", "*"]);
        assert_eq!(
            rules.addresses,
            vec!["0.0.0.0/0", "::/0"],
            "the covering ranges are address rules"
        );
        assert_eq!(
            rules.hosts,
            vec!["*"],
            "a bare star is a hostname rule, not any host"
        );
    }

    /// A workspace whose allowlist file EXISTS but is empty denies all egress:
    /// the very command a fresh (absent-file) workspace allows is refused, the
    /// listener sees no hit, and the tool reports `network: "deny-all"`. This is
    /// the deliberate lockdown the pane's starter action opts into.
    #[test]
    fn a_present_but_empty_allowlist_denies_all_network() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("egress-present-empty");
        // The reader created the file and left it empty. The pane's starter is
        // exactly that file, so use the same writer.
        crate::caps::write_egress_allowlist_template(&root).expect("the starter file");
        assert!(
            egress_allowlist_path(&root).exists(),
            "the premise: a PRESENT file"
        );
        let mut settings = rooted(&root);
        settings.global.default_preset = "standard".to_string();
        let built = build(&settings);
        assert!(
            !built.policy.data().egress_unrestricted,
            "a present file is not the absent state"
        );
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            WORKSPACE_WRITE,
            agent::sandbox::Principal::new("bash", "session", "standard"),
        );
        assert_eq!(
            spec.egress(),
            &agent::sandbox::Egress::DenyAll,
            "an empty file derives the kernel deny, not an open reach"
        );

        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");
        // The endpoint is hidden in variables, so the command tier cannot name
        // it: the command RUNS and only the sandbox can refuse the connection.
        let hidden = bash
            .run(&serde_json::json!({
                "command": format!(
                    "H={host}; P={port}; curl -sS --max-time 3 -o /dev/null \"http://$H:$P/\""
                )
            }))
            .expect("the command runs");
        assert_eq!(
            hidden["network"],
            serde_json::json!("deny-all"),
            "the honest state for an empty file: {hidden}"
        );
        assert!(
            !hidden["success"].as_bool().unwrap_or(true),
            "the empty file let the request out: {hidden}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0, "a listener was reached");

        // A literal endpoint is refused before a process exists and names the
        // baseline rule it violated.
        match bash.run(&serde_json::json!({
            "command": format!("curl -sS --max-time 3 -o /dev/null http://{host}:{port}/")
        })) {
            Err(agent::ToolError::Failed(message)) => assert!(
                message.contains("egress-not-allowlisted"),
                "the empty file refuses with the baseline rule: {message}"
            ),
            other => panic!("an empty allowlist must refuse the literal endpoint, got {other:?}"),
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a refused command connected"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A workspace with a PRESENT allowlist reaches only the address it names,
    /// reports `network: "allowlist"`, and still refuses an address the file
    /// does not carry.
    #[test]
    fn a_present_allowlist_reaches_only_its_endpoint() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("egress-present-rules");
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(
            egress_allowlist_path(&root),
            format!("# the one reachable address\n{host}\n"),
        )
        .expect("the allowlist");
        let mut settings = rooted(&root);
        settings.global.default_preset = "standard".to_string();
        let built = build(&settings);
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            WORKSPACE_WRITE,
            agent::sandbox::Principal::new("bash", "session", "standard"),
        );
        assert_eq!(
            spec.egress(),
            &agent::sandbox::Egress::Allow(vec![host.clone()]),
            "the file's address is the derived allowlist"
        );

        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");
        let allowed = bash
            .run(&serde_json::json!({
                "command": format!("curl -sS --max-time 3 http://{host}:{port}/")
            }))
            .expect("the command runs");
        assert_eq!(
            allowed["network"],
            serde_json::json!("allowlist"),
            "a real rule reports the allowlist: {allowed}"
        );
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "the allowed endpoint was reached"
        );

        // An address the file does not name is refused at the command tier.
        match bash.run(&serde_json::json!({
            "command": "curl -sS --max-time 3 -o /dev/null http://203.0.113.9/"
        })) {
            Err(agent::ToolError::Failed(message)) => assert!(
                message.contains("egress-not-allowlisted"),
                "the unlisted address is refused: {message}"
            ),
            other => panic!("an unlisted address must be refused, got {other:?}"),
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "the refused command did not connect"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

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
    /// A TRUSTED DSH package's host entry runs during a real build, and the tool
    /// it registers is CALLABLE from the agent the shell would actually run.
    ///
    /// This is the end-to-end claim. `an_installed_extension_reaches_the_turn`
    /// covers the DECLARATIVE path; this covers the IMPERATIVE one, which is how
    /// every shipped DSH package delivers its payload. The assertion is the CALL,
    /// and the expected value is what the fixture's own `execute` returns, so a
    /// build that registered a name without a live process behind it cannot pass.
    ///
    /// The fixture is copied to a scratch root first: the trusted half runs its
    /// JavaScript, and a test must not have a child writing into the repository.
    #[test]
    fn a_trusted_dsh_entry_reaches_the_turn_through_the_bridge() {
        if agent::extensions::bridge::find_interpreter().is_none() {
            eprintln!("skipped: no Node interpreter");
            return;
        }
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions/dsh-bridge");
        let root = std::env::temp_dir().join(format!("bsh-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        for entry in std::fs::read_dir(&source).expect("fixture") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), root.join(entry.file_name())).expect("copy");
        }

        let mut settings = bare();
        let record = crate::settings::global::InstalledExtension {
            id: "dsh-bridge".to_string(),
            path: Some(root.to_string_lossy().to_string()),
            trusted: true,
            trusted_path: Some(root.to_string_lossy().to_string()),
            ..Default::default()
        };
        assert!(record.is_trusted(), "the premise: the record is trusted");
        settings.global.extensions.push(record);

        let built = build(&settings);
        let tool = built
            .agent
            .registry()
            .expect("tool registry")
            .get("dsh_bridge_echo")
            .expect("the bridged tool reached the turn");
        let output = tool
            .run(&serde_json::json!({ "value": "turn" }))
            .expect("the bridged tool runs inside the build");
        assert_eq!(output["echoed"], "turn", "{output}");
        // The config came from the package's own bundle patch, so the manifest's
        // config reached the plugin and this is not a hardcoded default.
        assert_eq!(output["plugin"], "fixture", "{output}");

        // The process is alive AFTER the build returned, which is what makes the
        // tool usable across turns rather than for exactly one call.
        let again = tool
            .run(&serde_json::json!({ "value": "second" }))
            .expect("the session outlives the build that created it");
        assert_eq!(again["echoed"], "second", "{again}");

        // A build that no longer asks for the package offers no tool from it.
        let without = build(&bare());
        assert!(
            without
                .agent
                .registry()
                .expect("tool registry")
                .get("dsh_bridge_echo")
                .is_none(),
            "a package nobody lists contributes no tool"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An UNTRUSTED DSH package contributes no tool in a real build, and its
    /// entry never runs: the trust gate at the build boundary.
    ///
    /// The fixture records execution by writing a marker on its module's first
    /// line, so the assertion is about EXECUTION and not merely about a missing
    /// registration. It is copied to a scratch root first: a test must never let a
    /// child write into the repository, and a marker left in the fixture by an
    /// earlier run would silently invert what this test proves.
    #[test]
    fn an_untrusted_dsh_entry_contributes_no_tool_to_the_turn() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions/dsh-untrusted");
        let root = std::env::temp_dir().join(format!("bsh-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        for entry in std::fs::read_dir(&source).expect("fixture") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().ends_with(".marker"),
                "the fixture carries {name:?}: a previous run left execution evidence behind"
            );
            std::fs::copy(entry.path(), root.join(name)).expect("copy");
        }

        let mut settings = bare();
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "dsh-untrusted".to_string(),
                path: Some(root.to_string_lossy().to_string()),
                ..Default::default()
            });
        let built = build(&settings);
        assert!(
            built
                .agent
                .registry()
                .expect("tool registry")
                .get("dsh_untrusted_echo")
                .is_none(),
            "an untrusted package's entry never registers anything"
        );
        assert!(
            !root.join("executed.marker").exists(),
            "and it never ran: no marker, so no process"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The pool reconciles: a package that stops being asked for loses its child.
    ///
    /// This is the lifecycle half of the trust story. A plugin process outlives
    /// the build that spawned it, so without `retain` a reader who unticks a
    /// package - or withdraws trust - would leave its code running invisibly.
    /// The witness is the pid: `kill(pid, 0)` succeeds while pooled and fails
    /// once the pool has reconciled it away.
    #[test]
    fn the_bridge_pool_stops_a_package_the_build_no_longer_asks_for() {
        if agent::extensions::bridge::find_interpreter().is_none() {
            eprintln!("skipped: no Node interpreter");
            return;
        }
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent/tests/fixtures/extensions/dsh-bridge");
        let root = std::env::temp_dir().join(format!("bsh-pool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        for entry in std::fs::read_dir(&source).expect("fixture") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), root.join(entry.file_name())).expect("copy");
        }
        let mut settings = bare();
        settings
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: "dsh-bridge".to_string(),
                path: Some(root.to_string_lossy().to_string()),
                trusted: true,
                trusted_path: Some(root.to_string_lossy().to_string()),
                ..Default::default()
            });

        // A build that names the package starts a child and pools it.
        let built = build(&settings);
        let tool = built
            .agent
            .registry()
            .expect("tool registry")
            .get("dsh_bridge_echo")
            .expect("the bridged tool");
        let pid = with_bridge_pool(|pool| {
            pool.session("@local/dsh-bridge", &root)
                .expect("the pool holds the session")
                .lock()
                .expect("lock")
                .pid()
        });
        assert!(process_alive(pid), "the child runs while pooled");
        // Dropping the agent must not stop it: another turn may still hold it.
        drop(tool);
        drop(built);
        assert!(
            process_alive(pid),
            "the pool owns the process, not the build that adopted it"
        );

        // A build that no longer asks for the package stops it.
        let _ = build(&bare());
        assert!(
            with_bridge_pool(|pool| pool.loaded()).is_empty(),
            "the pool keeps no package this build did not ask for"
        );
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(
            !process_alive(pid),
            "reconciling the pool reaps the child rather than leaking it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Whether a process with this pid exists, without executing a new one.
    ///
    /// `kill(pid, 0)` is the POSIX existence probe: it sends no signal and
    /// reports through `errno`, so it cannot itself be mistaken for the child
    /// under test.
    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: `kill` with signal 0 performs the permission and existence
        // check and delivers nothing; the pid is one the OS issued us.
        unsafe { kill(pid as i32, 0) == 0 }
    }

    #[cfg(not(unix))]
    fn process_alive(pid: u32) -> bool {
        let _ = pid;
        false
    }

    /// The setting is the reader's half of the decision; the engine holds the
    /// other half, so this pins that the value arrives and that a chat-protocol
    /// provider is not handed a hosted tool it would reject.
    ///
    /// The preset is cleared first, on purpose: `Settings::default()` ships the
    /// STANDARD preset, which now declares `web_search = true` and therefore
    /// OVERRIDES this switch (that precedence is the resolver's own -
    /// `preset_web_search.unwrap_or(web_tools_enabled)` - and is older than this
    /// change, which only extended it to Standard). With the preset cleared the
    /// switch is the only thing deciding, which is what this test is about.
    #[test]
    fn the_web_tools_switch_reaches_the_agent() {
        let mut settings = bare();
        settings.global.default_preset = String::new();
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

    /// Standard and PTC ALLOW NETWORK AND WEB SEARCH BY DEFAULT. Proved through
    /// the build and a REAL shell, on a fresh workspace with NO `.bsh/egress-allowlist`:
    /// the command reaches a local endpoint, the result reports
    /// `network: "unrestricted"`, and the derived spec is the unrestricted state
    /// rather than an empty-list deny.
    #[test]
    fn standard_and_ptc_allow_network_and_search_by_default() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let root = scratch("preset-default-network");
        // NO allowlist file: this is the fresh-workspace state, which the
        // tri-state reads as UNRESTRICTED.
        assert!(
            !egress_allowlist_path(&root).exists(),
            "the premise: a fresh workspace has no allowlist file"
        );
        for id in ["standard", "ptc"] {
            let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
            let port = listener.local_addr().expect("its port").port();
            let hits = std::sync::Arc::new(AtomicUsize::new(0));
            serve_http(listener, std::sync::Arc::clone(&hits));

            let mut settings = rooted(&root);
            settings.global.default_preset = id.to_string();
            // The workspace switch ON, which is its shipped default: the preset
            // must not be the thing that withdraws search.
            settings.workspace.agent.web_tools_enabled = true;
            let built = build(&settings);
            assert!(
                built.agent.config().web_search,
                "{id} offers web search by default"
            );
            // Network reach: no `egress` field, so the preset inherits the
            // workspace, and the absent file leaves that reach unrestricted.
            assert!(
                !built.policy.data().preset_egress_restricted,
                "{id} narrows nothing"
            );
            assert!(
                built.policy.data().egress_unrestricted,
                "{id} builds against the absent-file state"
            );
            let spec = agent::sandbox::SandboxSpec::for_policy(
                &built.policy,
                WORKSPACE_WRITE,
                agent::sandbox::Principal::new("bash", "session", id),
            );
            assert_eq!(
                spec.egress(),
                &agent::sandbox::Egress::Unrestricted,
                "{id} must not derive an empty-list deny"
            );
            assert!(spec.egress().is_allowed() && !spec.egress().is_deny_all());
            assert!(
                spec.egress().entries().is_empty(),
                "{id} needs no allowlist entry to reach the network"
            );

            let bash = built
                .agent
                .registry()
                .expect("registry")
                .get("bash")
                .expect("bash");
            let result = bash
                .run(&serde_json::json!({
                    "command": format!("curl -sS --max-time 3 http://{host}:{port}/")
                }))
                .expect("the command runs");
            assert_eq!(
                result["network"],
                serde_json::json!("unrestricted"),
                "{id} reports the absent-file state: {result}"
            );
            assert!(
                result["success"].as_bool().unwrap_or(false),
                "{id} could not reach the network out of the box: {result}"
            );
            assert_eq!(
                hits.load(Ordering::SeqCst),
                1,
                "{id}: the allowed endpoint was reached exactly once"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Standard/PTC turn on a fresh workspace (NO allowlist file) reaches an
    /// HTTP endpoint THROUGH heel's proxy: the listener counts the request, the
    /// response comes back, and the result reports the absent-file state,
    /// `unrestricted`.
    ///
    /// The command first proves the proxy environment exists, so a success
    /// cannot be a direct connection that happened to be allowed: only
    /// localhost:<proxy port> is granted, and `curl` uses the proxy it is
    /// handed.
    #[test]
    fn standard_and_ptc_reach_http_through_the_heel_proxy() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the proxy probe needs one");
            return;
        };
        for id in ["standard", "ptc"] {
            let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
            let port = listener.local_addr().expect("its port").port();
            let hits = std::sync::Arc::new(AtomicUsize::new(0));
            serve_http(listener, std::sync::Arc::clone(&hits));

            let root = scratch(&format!("proxy-http-{id}"));
            assert!(
                !egress_allowlist_path(&root).exists(),
                "{id}: the premise is the ABSENT allowlist file"
            );
            let mut settings = rooted(&root);
            settings.global.default_preset = id.to_string();
            let built = build(&settings);
            assert!(
                built.policy.data().egress_unrestricted,
                "{id}: the absent file must derive UNRESTRICTED egress"
            );
            let bash = built
                .agent
                .registry()
                .expect("registry")
                .get("bash")
                .expect("bash");
            // NO_PROXY is cleared so the inherited environment cannot make curl
            // bypass the proxy: the only path out is heel's.
            let command = format!(
                "[ -n \"$HTTP_PROXY\" ] || {{ printf 'no proxy'; exit 97; }}\n\
                 NO_PROXY=; no_proxy=; curl -sS --max-time 5 http://{host}:{port}/"
            );
            let result = bash
                .run(&serde_json::json!({ "command": command }))
                .expect("the command runs");
            assert_eq!(
                result["network"],
                serde_json::json!("unrestricted"),
                "{id} reports the absent-file state: {result}"
            );
            assert!(
                result["success"].as_bool().unwrap_or(false),
                "{id} could not reach the endpoint through the proxy: {result}"
            );
            assert_eq!(
                result["stdout"].as_str().unwrap_or_default().trim(),
                "ok",
                "{id}: the proxy did not return the listener's body: {result}"
            );
            assert!(
                hits.load(Ordering::SeqCst) >= 1,
                "{id}: the listener was never reached: {result}"
            );
            eprintln!("{id}: HTTP listener hits = {}", hits.load(Ordering::SeqCst));
            assert!(
                !egress_allowlist_path(&root).exists(),
                "{id}: the probe created an allowlist file, changing the state it proves"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// A TCP connection to a NON-LISTED, non-host-local endpoint succeeds
    /// through the proxy's CONNECT tunnel: the destination is not on any
    /// allowlist (there is no file at all) and is not host-local (heel's proxy
    /// refuses those), yet bytes round-trip and the result still reports
    /// `unrestricted`.
    ///
    /// This is the TCP path that DOES work. It is a tunnel established by the
    /// sandbox proxy, not a direct socket: the ICMP test beside it pins that a
    /// direct socket has no path.
    #[test]
    fn standard_and_ptc_reach_a_non_listed_tcp_endpoint_through_the_connect_tunnel() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the tunnel probe needs one");
            return;
        };
        for id in ["standard", "ptc"] {
            let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
            let port = listener.local_addr().expect("its port").port();
            let hits = std::sync::Arc::new(AtomicUsize::new(0));
            serve_http(listener, std::sync::Arc::clone(&hits));

            let root = scratch(&format!("proxy-tcp-{id}"));
            assert!(
                !egress_allowlist_path(&root).exists(),
                "{id}: the premise is the ABSENT allowlist file"
            );
            let mut settings = rooted(&root);
            settings.global.default_preset = id.to_string();
            let built = build(&settings);
            let bash = built
                .agent
                .registry()
                .expect("registry")
                .get("bash")
                .expect("bash");
            // `-X connect` is the proxy CONNECT method: nc opens a TCP tunnel to
            // the destination the proxy has never been asked about at the
            // command tier, and the raw request travels inside it.
            let command = format!(
                "P=$(printf '%s' \"$HTTP_PROXY\" | cut -d/ -f3); [ -n \"$P\" ] || {{ printf 'no proxy'; exit 97; }}\n\
                 printf 'GET / HTTP/1.0\\r\\n\\r\\n' | nc -X connect -x \"$P\" -w 5 {host} {port}"
            );
            let result = bash
                .run(&serde_json::json!({ "command": command }))
                .expect("the command runs");
            assert_eq!(
                result["network"],
                serde_json::json!("unrestricted"),
                "{id} reports the absent-file state: {result}"
            );
            assert!(
                hits.load(Ordering::SeqCst) >= 1,
                "{id}: no TCP connection reached the non-listed endpoint: {result}"
            );
            eprintln!(
                "{id}: TCP tunnel listener hits = {}",
                hits.load(Ordering::SeqCst)
            );
            assert!(
                result["stdout"].as_str().unwrap_or_default().contains("ok"),
                "{id}: the tunnel did not carry the response back: {result}"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// ICMP has NO proxied path in this sandbox even when the policy allows the
    /// network: the profile denies every direct socket and grants only the
    /// localhost proxy, so the send is refused while `network` honestly says
    /// `unrestricted`. A released full-access shell does not change that: the
    /// proxy is the one way out, and it speaks TCP, not ICMP.
    ///
    /// The probe bounds ITSELF with an overall deadline (`-t` on macOS, `-w`
    /// elsewhere) on top of the per-packet wait, and a watchdog turns any
    /// residual hang into a named failure in seconds. A packet COUNT is not a
    /// deadline: on this host (macOS 26, 2026-10-24) a `ping -c 1` probe was
    /// observed still alive minutes into a full-suite run, while the `-t 2`
    /// form returns in about two seconds. The bash tool's own default bound is
    /// 300 s, far too long for this test to wait on.
    ///
    /// MEASURED OUTCOME ON THIS HOST: the kernel EPERM branch. The probe exits
    /// 2 after about 2 s with "ping: sendto: Operation not permitted" on
    /// stderr, so the result carries
    /// network_diagnostic = unproxied-direct-socket. The other branch - a send
    /// that is allowed out but gets no reply - is accepted too, but then the
    /// result must NOT claim a kernel refusal.
    ///
    /// This is the KNOWN LIMITATION, pinned on purpose. If this test starts
    /// receiving replies (success), heel grew a pass-through network mode:
    /// delete this limitation, update `docs/bsh-tool-policy.md`, and stop
    /// showing the unproxied-socket diagnostic.
    #[test]
    fn standard_and_ptc_deny_icmp_even_when_egress_is_unrestricted() {
        for id in ["standard", "ptc"] {
            let root = scratch(&format!("icmp-{id}"));
            assert!(
                !egress_allowlist_path(&root).exists(),
                "{id}: the premise is the ABSENT allowlist file"
            );
            let mut settings = rooted(&root);
            settings.global.default_preset = id.to_string();
            let built = build(&settings);
            let bash = built
                .agent
                .registry()
                .expect("registry")
                .get("bash")
                .expect("bash");
            // macOS: -W is the per-packet reply wait in MILLISECONDS and -t is
            // the overall deadline in seconds. Elsewhere: -W and -w are both
            // seconds. The command returns in about two seconds either way,
            // instead of parking the turn on the tool's 300 s default.
            let ping = if cfg!(target_os = "macos") {
                "ping -c 1 -W 1000 -t 2 1.1.1.1"
            } else {
                "ping -c 1 -W 1 -w 2 1.1.1.1"
            };
            // The configured bound is 300 s, so this test must never wait on
            // it: the call runs on its own thread and a watchdog fails it by
            // name. That makes "the probe is bounded" an assertion rather than
            // a hope about this host's ping.
            let probe = std::sync::Arc::clone(&bash);
            let (sender, receiver) = std::sync::mpsc::channel();
            let started = std::time::Instant::now();
            std::thread::spawn(move || {
                let _ = sender.send(probe.run(&serde_json::json!({ "command": ping })));
            });
            let result = match receiver.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(result) => result.expect("the command runs and reports its own failure"),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!(
                    "{id}: the bounded ICMP probe HUNG past 20 s - the command needs its own deadline"
                ),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("{id}: the probe thread died without a result")
                }
            };
            let elapsed = started.elapsed();
            eprintln!("{id}: ICMP probe returned in {elapsed:?}: {result}");
            assert_eq!(
                result["sandbox"],
                serde_json::json!("heel"),
                "{id}: the probe must run under the sandbox"
            );
            assert_eq!(
                result["network"],
                serde_json::json!("unrestricted"),
                "{id}: the policy did allow the network: {result}"
            );
            if result["success"].as_bool().unwrap_or(false) {
                panic!(
                    "{id}: ping SUCCEEDED - heel now has a pass-through network path. \
                     This known limitation is over: update docs/bsh-tool-policy.md and \
                     the unproxied-socket diagnostic, then delete this test. {result}"
                );
            }
            let text = format!(
                "{}{}",
                result["stdout"].as_str().unwrap_or_default(),
                result["stderr"].as_str().unwrap_or_default()
            )
            .to_ascii_lowercase();
            // Two honest outcomes, ONE invariant: ICMP never travels through
            // the proxy. A refused send says so with the kernel's own errno and
            // must carry the diagnostic; a send that was merely allowed out
            // gets no reply, and must not claim a kernel refusal it never saw.
            let kernel_refusal = text.contains("operation not permitted")
                || text.contains("permission denied")
                || text.contains("eperm")
                || text.contains("eacces");
            if kernel_refusal {
                assert_eq!(
                    result["network_diagnostic"],
                    serde_json::json!(agent::tools::bash::UNPROXIED_DIRECT_SOCKET),
                    "{id}: the kernel refusal is not explained in the result: {result}"
                );
            } else {
                assert!(
                    text.contains("packet loss") || text.contains("timeout"),
                    "{id}: neither a kernel refusal nor a no-reply exit: {result}"
                );
                assert_eq!(
                    result["network_diagnostic"],
                    serde_json::Value::Null,
                    "{id}: a no-reply exit must not claim a kernel refusal: {result}"
                );
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// The preset's OWN grant is what offers search: even with the workspace
    /// switch off, Standard still hands the agent the tool, because its
    /// declaration is `web_search = true` rather than an inherited default.
    #[test]
    fn the_standard_preset_states_its_own_search_grant() {
        let root = scratch("preset-standard-search");
        let mut settings = rooted(&root);
        settings.global.default_preset = "standard".to_string();
        settings.workspace.agent.web_tools_enabled = false;
        let built = build(&settings);
        assert!(
            built.agent.config().web_search,
            "Standard's own grant survives the workspace switch"
        );
        let _ = std::fs::remove_dir_all(&root);
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
            Arc::new(crate::browser::BrowserHandle::new()),
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
                policy_mode: Some(crate::settings::global::PolicyMode::Required),
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

    // ------------------------------------------------------------------
    // The preset network policy, proved end to end through a real shell.
    //
    // The observable of every case here is a SIDE EFFECT, not an error
    // string: a listener that counts its hits, a marker file the command
    // would have written, and the `network` field the bash tool reports.
    // ------------------------------------------------------------------

    /// The host's non-loopback IPv4 address, the only address a heel proxy
    /// will forward to (it refuses a host-local target), so a probe can prove
    /// a real connection was or was not made.
    fn lan_address() -> Option<String> {
        let candidates: &[(&str, &[&str])] = &[
            ("/usr/sbin/ipconfig", &["getifaddr", "en0"]),
            ("/usr/sbin/ipconfig", &["getifaddr", "en1"]),
            (
                "/bin/sh",
                &["-c", "hostname -I 2>/dev/null | awk '{print $1}'"],
            ),
        ];
        for (program, args) in candidates {
            if !std::path::Path::new(program).exists() {
                continue;
            }
            let Ok(output) = std::process::Command::new(program).args(*args).output() else {
                continue;
            };
            let host = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if host.parse::<std::net::IpAddr>().is_ok()
                && !host.starts_with("127.")
                && !host.starts_with("169.254.")
            {
                return Some(host);
            }
        }
        None
    }

    /// Serve HTTP 200 to every connection, counting the hits.
    fn serve_http(
        listener: std::net::TcpListener,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::io::{Read as _, Write as _};
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let hits = std::sync::Arc::clone(&hits);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while stream.read(&mut byte).unwrap_or(0) == 1 {
                        request.push(byte[0]);
                        if request.ends_with(b"\r\n\r\n") {
                            break;
                        }
                    }
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    );
                });
            }
        });
    }

    /// A settings document whose workspace allowlist is `entries`, with one
    /// custom preset selected for the turn.
    fn preset_build(
        root: &std::path::Path,
        entries: &str,
        preset: Preset,
    ) -> (Settings, BuiltAgent) {
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(root.join(".bsh/egress-allowlist"), entries).expect("egress list");
        let mut settings = rooted(root);
        settings.global.presets.insert("probe".to_string(), preset);
        settings.global.default_preset = "probe".to_string();
        let built = build(&settings);
        (settings, built)
    }

    /// A shell-keeping preset that declares `egress = []` must refuse a
    /// command whose literal endpoint the WORKSPACE allowlist permits: the
    /// refusal names the preset's rule, no process side effect appears, the
    /// listener sees no hit, and the derived spec has no proxy at all.
    #[test]
    fn a_deny_all_preset_refuses_a_workspace_allowed_endpoint() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("preset-egress-deny-all");
        let (settings, built) = preset_build(
            &root,
            &format!("{host}\n"),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                // A DECLARED empty list: deny all network, even here.
                egress: Some(Vec::new()),
                ..Preset::default()
            },
        );
        let url = format!("http://{host}:{port}/");
        let command = format!("printf ran > ran.txt && curl -sS --max-time 3 -o /dev/null {url}");
        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");
        match bash.run(&serde_json::json!({ "command": command })) {
            Err(agent::ToolError::Failed(message)) => assert!(
                message.contains("preset-egress"),
                "the refusal names the preset's rule: {message}"
            ),
            other => panic!("expected the preset to refuse the command, got {other:?}"),
        }
        assert!(
            !root.join("ran.txt").exists(),
            "the command's side effect happened before the network decision"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a refused command connected"
        );

        // The derived set is EMPTY, which is what tells the backend to start no
        // proxy at all: the kernel denies.
        let eff = settings.effective();
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            &eff.permission.effective,
            agent::sandbox::Principal::new("bash", "session", "probe"),
        );
        assert_eq!(
            spec.egress(),
            &agent::sandbox::Egress::DenyAll,
            "a deny-all preset must derive no proxy"
        );
        assert!(!spec.egress().is_allowed(), "no proxy is the kernel deny");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A preset whose network policy touches variables the command tier cannot
    /// see still has the KERNEL refuse it, and the tool result reports
    /// `network: "deny-all"`.
    #[test]
    fn a_deny_all_preset_reports_deny_all_and_the_kernel_holds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("preset-egress-deny-all-kernel");
        let (_settings, built) = preset_build(
            &root,
            &format!("{host}\n"),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                egress: Some(Vec::new()),
                ..Preset::default()
            },
        );
        // The endpoint is hidden in variables, so the command tier cannot name
        // it and only the kernel can refuse the connection.
        let command = format!(
            "H={host}; P={port}; printf ran > ran.txt && curl -sS --max-time 3 -o /dev/null \"http://$H:$P/\""
        );
        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");
        let result = bash
            .run(&serde_json::json!({ "command": command }))
            .expect("the command runs");
        assert_eq!(
            result["network"],
            serde_json::json!("deny-all"),
            "the honest signal for a deny-all preset: {result}"
        );
        assert!(
            !result["success"].as_bool().unwrap_or(true),
            "deny-all let the request out: {result}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "a connection reached the listener"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A preset that intersects: the workspace allows A and B, the preset
    /// allows A, so A is reachable through the proxy and B is refused with the
    /// preset's rule named.
    #[test]
    fn an_intersecting_preset_reaches_what_both_allow_and_refuses_the_rest() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host_a) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        // A second address the workspace lists but the preset does not: the
        // host's own address is enough because the preset keeps only A.
        let listener_a = std::net::TcpListener::bind((host_a.as_str(), 0)).expect("a listener");
        let port_a = listener_a.local_addr().expect("its port").port();
        let hits_a = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener_a, std::sync::Arc::clone(&hits_a));

        let root = scratch("preset-egress-intersect");
        // The workspace lists TWO entries; the preset keeps the first.
        let (_settings, built) = preset_build(
            &root,
            &format!("{host_a}\n203.0.113.9\n"),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                egress: Some(vec![host_a.clone()]),
                ..Preset::default()
            },
        );
        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");

        // A: allowed by both tiers, so the proxy forwards it and the listener
        // answers 200.
        let allowed = bash
            .run(&serde_json::json!({
                "command": format!("curl -sS --max-time 3 http://{host_a}:{port_a}/")
            }))
            .expect("the command runs");
        assert_eq!(
            allowed["network"],
            serde_json::json!("allowlist"),
            "{allowed}"
        );
        assert!(allowed["success"].as_bool().unwrap_or(false), "{allowed}");
        assert!(
            allowed["stdout"].as_str().unwrap_or("").contains("ok"),
            "{allowed}"
        );
        assert_eq!(
            hits_a.load(Ordering::SeqCst),
            1,
            "the allowed endpoint was reached"
        );

        // B: allowed by the workspace, removed by the preset. The command tier
        // refuses it and names the preset's rule.
        match bash.run(&serde_json::json!({
            "command": "curl -sS --max-time 3 -o /dev/null http://203.0.113.9/"
        })) {
            Err(agent::ToolError::Failed(message)) => assert!(
                message.contains("preset-egress"),
                "the preset rule refused the removed entry: {message}"
            ),
            other => panic!("expected the preset to refuse the removed entry, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run one shell command from a build and return the structured result.
    ///
    /// A policy refusal is a panic here: every case below hides its write or
    /// endpoint in a variable so the command tier must pass and the outcome is
    /// decided by confinement, which is what the test is about.
    fn bash_result(built: &BuiltAgent, command: &str) -> serde_json::Value {
        built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash")
            .run(&serde_json::json!({ "command": command }))
            .unwrap_or_else(|error| panic!("the command tier refused: {error}"))
    }

    /// One settings tree with a full-access session and one preset carrying an
    /// explicit sandbox choice.
    fn preset_sandbox_build(
        root: &std::path::Path,
        sandbox: crate::settings::global::SandboxChoice,
        permission: &str,
        session_permission: &str,
    ) -> (Settings, BuiltAgent) {
        let mut settings = rooted(root);
        settings.workspace.agent.permission = session_permission.to_string();
        settings.global.presets.insert(
            "probe".to_string(),
            Preset {
                permission: Some(permission.to_string()),
                sandbox: Some(sandbox),
                plugins: vec!["shell".to_string(), "files".to_string()],
                ..Preset::default()
            },
        );
        settings.global.default_preset = "probe".to_string();
        let built = build(&settings);
        (settings, built)
    }

    /// `sandbox = "none"` at full access is the widening arm working as
    /// designed: the shell runs outside the workspace and writes a file there.
    ///
    /// The path arrives through a shell variable, so the command tier cannot
    /// see it and the absence of the kernel boundary is what lets the write
    /// land. This is the cost the docs state, proved rather than implied.
    #[test]
    fn a_full_access_preset_with_sandbox_none_runs_outside_the_workspace() {
        let root = scratch("preset-sandbox-none-write");
        let outside_dir = scratch("preset-sandbox-none-outside");
        let outside = outside_dir.join(format!("escaped-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let (settings, built) = preset_sandbox_build(
            &root,
            crate::settings::global::SandboxChoice::Off,
            FULL_ACCESS,
            FULL_ACCESS,
        );
        let resolved = settings.effective();
        assert!(
            !resolved.sandbox.heel,
            "the preset asked for an unconfined shell"
        );
        assert!(resolved.sandbox.off_by_preset);
        assert!(resolved.sandbox.refusal.is_none());

        let result = bash_result(
            &built,
            &format!("T=\"{}\"; printf escaped > \"$T\"", outside.display()),
        );
        assert_eq!(result["sandbox"], serde_json::json!("none"), "{result}");
        assert_eq!(
            result["sandbox_reason"],
            serde_json::json!(agent::tools::bash::PRESET_SANDBOX_OFF_REASON),
            "the result names who turned the sandbox off: {result}"
        );
        assert!(
            result["success"].as_bool().unwrap_or(false),
            "an unsandboxed command reached its outside target: {result}"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).expect("the outside file"),
            "escaped"
        );
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The guard, in both narrower directions: a preset that turns the sandbox
    /// off under a workspace-write or read-only ceiling is REFUSED with
    /// `preset-sandbox-widens`, the effective sandbox stays heel, and the same
    /// command leaves no marker.
    #[test]
    fn a_narrow_preset_cannot_turn_the_sandbox_off() {
        for ceiling in [WORKSPACE_WRITE, READ_ONLY] {
            let root = scratch(&format!("preset-sandbox-widens-{ceiling}"));
            let outside_dir = scratch(&format!("preset-sandbox-widens-out-{ceiling}"));
            let outside = outside_dir.join(format!("marker-{}-{ceiling}.txt", std::process::id()));
            let _ = std::fs::remove_file(&outside);
            let (settings, built) = preset_sandbox_build(
                &root,
                crate::settings::global::SandboxChoice::Off,
                ceiling,
                ceiling,
            );
            let resolved = settings.effective();
            assert_eq!(
                resolved.sandbox.refusal,
                Some(crate::settings::resolve::PRESET_SANDBOX_WIDENS),
                "{ceiling}: the declaration is refused, not silently ignored"
            );
            assert!(resolved.sandbox.heel, "{ceiling}: the shell stays confined");
            assert!(!resolved.sandbox.off_by_preset);
            assert_eq!(
                settings.refusal_for(&SessionChoice::default()),
                Some("composer.presetSandboxWidens"),
                "{ceiling}: the send gate names the rule's locale key"
            );

            let registry = built.agent.registry().expect("registry");
            if ceiling == WORKSPACE_WRITE {
                let result = bash_result(
                    &built,
                    &format!("T=\"{}\"; printf escaped > \"$T\"", outside.display()),
                );
                assert_eq!(result["sandbox"], serde_json::json!("heel"), "{result}");
                assert!(
                    !result["success"].as_bool().unwrap_or(true),
                    "the heel kernel let the outside write through: {result}"
                );
            } else {
                assert!(
                    registry.get("bash").is_none(),
                    "read-only registers no shell, so the refused preset cannot run the command at all"
                );
            }
            assert!(
                !outside.exists(),
                "{ceiling}: the command's marker exists, so it ran despite the refusal"
            );
            let _ = std::fs::remove_file(&outside);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// The narrowing arm: `sandbox = "heel"` confines even a full-access
    /// session, and the result reports the backend that actually ran.
    #[test]
    fn an_explicit_heel_confines_a_full_access_session() {
        let root = scratch("preset-sandbox-heel-full");
        let outside_dir = scratch("preset-sandbox-heel-outside");
        let outside = outside_dir.join(format!("escaped-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let (settings, built) = preset_sandbox_build(
            &root,
            crate::settings::global::SandboxChoice::Heel,
            FULL_ACCESS,
            FULL_ACCESS,
        );
        let resolved = settings.effective();
        assert_eq!(
            resolved.permission.effective, FULL_ACCESS,
            "the permission itself is still full access; heel is the extra ceiling"
        );
        assert!(resolved.sandbox.heel, "the preset narrowed the shell");

        let result = bash_result(
            &built,
            &format!("T=\"{}\"; printf escaped > \"$T\"", outside.display()),
        );
        assert_eq!(result["sandbox"], serde_json::json!("heel"), "{result}");
        assert!(
            !result["success"].as_bool().unwrap_or(true),
            "the heel kernel allowed the outside write: {result}"
        );
        assert!(!outside.exists(), "the sandbox let the write escape");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The inherited FULL-ACCESS arm as a user meets it: PTC declares no sandbox
    /// field, so the shell is handed `workspace_root: None` and its cwd is the
    /// app's launch directory, NOT the workspace. The policy must permit that
    /// spawn by TIER (`spawn-shell-full-access`); a path-only policy refused
    /// every command before a process existed, which is the reported bug.
    #[test]
    fn an_inherited_full_access_shell_spawns_where_the_release_puts_it() {
        let root = scratch("full-access-inherited-spawn");
        let mut settings = rooted(&root);
        settings.workspace.agent.permission = FULL_ACCESS.to_string();
        settings.global.default_preset = "ptc".to_string();
        let resolved = settings.effective();
        assert_eq!(resolved.permission.effective, FULL_ACCESS);
        assert!(
            !resolved.sandbox.heel && !resolved.sandbox.off_by_preset,
            "the premise: PTC inherits, so the shell is released with no root"
        );
        let built = build(&settings);
        assert_eq!(built.policy.data().permission, FULL_ACCESS);

        // The path the tool asks about is the launch directory, which is what
        // BashTool falls back to when no root is configured. It is NOT the
        // workspace, which is why a path-only rule refused everything.
        let launch = std::env::current_dir().expect("a launch directory");
        assert!(
            !launch.starts_with(&root),
            "the premise: the launch directory is outside the scratch workspace"
        );
        let decision = built.policy.check(
            &PolicyInput::new("bash", Action::Spawn, Target::path(&launch))
                .with_session("session")
                .with_preset("ptc"),
        );
        assert!(
            decision.is_allowed(),
            "the released tier permits the shell: {decision:?}"
        );
        assert_eq!(decision.rule.as_deref(), Some("spawn-shell-full-access"));

        let result = bash_result(&built, "echo hi");
        assert_eq!(
            result["stdout"].as_str().unwrap_or("").trim(),
            "hi",
            "{result}"
        );
        assert_eq!(result["exit_code"], serde_json::json!(0), "{result}");
        assert!(result["success"].as_bool().unwrap_or(false), "{result}");
        assert_eq!(result["sandbox"], serde_json::json!("none"), "{result}");
        assert_eq!(
            result["sandbox_reason"],
            serde_json::json!(agent::tools::bash::SANDBOX_RELEASED_REASON),
            "the release names itself: {result}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The preset NARROWING arm under full access still asks about the workspace
    /// root: a heel preset hands the shell that root, so the spawn is the path
    /// rule's and the command runs confined.
    #[test]
    fn a_heel_preset_under_full_access_spawns_at_the_workspace_root() {
        let root = scratch("full-access-heel-spawn");
        let (settings, built) = preset_sandbox_build(
            &root,
            crate::settings::global::SandboxChoice::Heel,
            FULL_ACCESS,
            FULL_ACCESS,
        );
        let resolved = settings.effective();
        assert!(resolved.sandbox.heel, "the premise: the preset narrows");
        assert_eq!(built.policy.data().permission, FULL_ACCESS);

        let decision = built.policy.check(
            &PolicyInput::new("bash", Action::Spawn, Target::path(root.clone()))
                .with_session("session")
                .with_preset("probe"),
        );
        assert!(decision.is_allowed(), "{decision:?}");
        assert_eq!(
            decision.rule.as_deref(),
            Some("spawn-shell-in-workspace"),
            "a confined shell is the path rule's, not the released arm's"
        );

        let result = bash_result(&built, "echo hi");
        assert_eq!(
            result["stdout"].as_str().unwrap_or("").trim(),
            "hi",
            "{result}"
        );
        assert_eq!(result["sandbox"], serde_json::json!("heel"), "{result}");
        assert!(result["success"].as_bool().unwrap_or(false), "{result}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The two confined tiers keep their spawn answers, and the released arm
    /// widens nothing else: at full access the same non-workspace path is
    /// admitted only for `bash`, while another tool keeps the old rule.
    #[test]
    fn the_confined_tiers_keep_their_spawn_answers() {
        let root = scratch("tier-spawn-answers");
        let outside =
            std::env::temp_dir().join(format!("bsh-spawn-outside-{}", std::process::id()));
        let probe = |built: &BuiltAgent, tool: &str, target: &std::path::Path, preset: &str| {
            built.policy.check(
                &PolicyInput::new(tool, Action::Spawn, Target::path(target))
                    .with_session("session")
                    .with_preset(preset),
            )
        };

        // Workspace Write: the shell spawns AT the root through the path rule,
        // and a non-workspace target is still refused there.
        let write = build(&rooted(&root));
        assert_eq!(write.policy.data().permission, WORKSPACE_WRITE);
        let at_root = probe(&write, "bash", &root, "");
        assert!(at_root.is_allowed(), "{at_root:?}");
        assert_eq!(at_root.rule.as_deref(), Some("spawn-shell-in-workspace"));
        let elsewhere = probe(&write, "bash", &outside, "");
        assert!(elsewhere.is_denied(), "{elsewhere:?}");
        assert_eq!(elsewhere.rule.as_deref(), Some("spawn-not-allowed"));

        // Read Only registers no shell at all, so there is no spawn to permit.
        let mut read_only = rooted(&root);
        read_only.workspace.agent.permission = READ_ONLY.to_string();
        let built = build(&read_only);
        assert_eq!(built.policy.data().permission, READ_ONLY);
        assert!(
            built
                .agent
                .registry()
                .expect("registry")
                .get("bash")
                .is_none(),
            "read-only keeps the shell out of the request"
        );

        // Full access: the non-workspace path is the released tier's answer for
        // bash, and ONLY for bash - another process keeps the old rule.
        let mut wide = rooted(&root);
        wide.workspace.agent.permission = FULL_ACCESS.to_string();
        let built = build(&wide);
        let released = probe(&built, "bash", &outside, "");
        assert!(released.is_allowed(), "{released:?}");
        assert_eq!(released.rule.as_deref(), Some("spawn-shell-full-access"));
        let other = probe(&built, "python", &outside, "");
        assert!(other.is_denied(), "{other:?}");
        assert_eq!(other.rule.as_deref(), Some("spawn-not-allowed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The shipped presets declare no toggle, so their effective sandbox is
    /// exactly today's: heel under the default Workspace Write, and no
    /// attribution when a full-access session inherits and is released.
    #[test]
    fn the_shipped_presets_keep_their_sandbox_behaviour() {
        let mut settings = bare();
        for id in ["standard", "ptc", "creator", "read-only"] {
            let preset = settings
                .global
                .preset(id)
                .unwrap_or_else(|| panic!("the shipped {id}"));
            assert_eq!(
                preset.sandbox, None,
                "the shipped {id} must not declare a sandbox choice"
            );
            settings.global.default_preset = id.to_string();
            let resolved = settings.effective();
            assert!(
                resolved.sandbox.heel,
                "{id} stays confined under the default Workspace Write"
            );
            assert!(!resolved.sandbox.off_by_preset, "{id} turns nothing off");
            assert!(resolved.sandbox.refusal.is_none(), "{id} refuses nothing");
            let built = build(&settings);
            let spec = sandbox_spec_for(&built.policy, &resolved, "bash", "session");
            assert_eq!(
                spec.mode(),
                if id == "read-only" {
                    agent::sandbox::SandboxMode::ReadOnly
                } else {
                    agent::sandbox::SandboxMode::WorkspaceWrite
                },
                "{id}'s shell spec is today's tier"
            );
        }

        // A full-access session that inherits Standard is still released: the
        // absent field must not silently become a forced heel.
        let mut wide = bare();
        wide.workspace.agent.permission = FULL_ACCESS.to_string();
        wide.global.default_preset = "standard".to_string();
        let resolved = wide.effective();
        assert!(
            !resolved.sandbox.heel,
            "today's full-access release survives an absent sandbox field"
        );
        assert!(!resolved.sandbox.off_by_preset);
        let built = build(&wide);
        let spec = sandbox_spec_for(&built.policy, &resolved, "bash", "session");
        assert_eq!(spec.mode(), agent::sandbox::SandboxMode::FullAccess);
    }

    /// The egress consequence, stated as the tradeoff rather than hidden.
    ///
    /// With `sandbox = "none"` a command reaches a host the workspace allowlist
    /// does NOT name (the endpoint is in a variable, so the command-tier scan
    /// cannot see it either). With the same preset asking for heel, the proxy
    /// refuses the same request and the listener sees no second hit. That is
    /// the cost of turning the sandbox off: the allowlist is enforced by heel,
    /// so no heel means no egress enforcement for shell commands.
    #[test]
    fn a_preset_that_turns_the_sandbox_off_also_bypasses_the_egress_allowlist() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("preset-sandbox-none-egress");
        // The workspace allowlist names a DIFFERENT address, so this host is
        // outside it.
        std::fs::create_dir_all(root.join(".bsh")).expect(".bsh");
        std::fs::write(root.join(".bsh/egress-allowlist"), "203.0.113.9\n")
            .expect("egress allowlist");
        let command = format!("H={host}; P={port}; curl -sS --max-time 3 \"http://$H:$P/\"");
        // The listener's body, so "allowed" and "refused by the proxy" are
        // distinguishable: heel's refusal is a body, not a curl exit code.
        let reached = "ok";

        // Off: the endpoint is reached. The command tier cannot name it (a
        // variable), and there is no proxy to ask.
        let (settings, built) = preset_sandbox_build(
            &root,
            crate::settings::global::SandboxChoice::Off,
            FULL_ACCESS,
            FULL_ACCESS,
        );
        assert!(!settings.effective().sandbox.heel);
        let off = bash_result(&built, &command);
        assert_eq!(off["sandbox"], serde_json::json!("none"), "{off}");
        assert!(
            off["stdout"].as_str().unwrap_or("").contains(reached),
            "the unsandboxed request did not reach the listener: {off}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "the unsandboxed shell ignored the workspace allowlist (this is the documented cost)"
        );

        // Heel: the same preset, confined. The proxy asks the policy for the
        // resolved address and refuses it; no second connection arrives.
        let (settings, built) = preset_sandbox_build(
            &root,
            crate::settings::global::SandboxChoice::Heel,
            FULL_ACCESS,
            FULL_ACCESS,
        );
        assert!(settings.effective().sandbox.heel);
        let heel = bash_result(&built, &command);
        assert_eq!(heel["sandbox"], serde_json::json!("heel"), "{heel}");
        assert!(
            !heel["stdout"].as_str().unwrap_or("").contains(reached),
            "the heel proxy allowed an address outside the workspace allowlist: {heel}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "a confined request reached the listener, so the allowlist did not hold"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A preset with no `egress` field inherits the workspace allowlist: the
    /// derived proxy set is the whole workspace list, and the shell reports
    /// `network: "allowlist"`.
    #[test]
    fn an_absent_preset_egress_inherits_the_workspace_list() {
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let root = scratch("preset-egress-inherit");
        let (_settings, built) = preset_build(
            &root,
            &format!("{host}\n203.0.113.9\n"),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                // No egress field at all: inherit.
                ..Preset::default()
            },
        );
        assert!(
            !built.policy.data().preset_egress_restricted,
            "an absent field does not restrict"
        );
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            WORKSPACE_WRITE,
            agent::sandbox::Principal::new("bash", "session", "probe"),
        );
        match spec.egress() {
            agent::sandbox::Egress::Allow(entries) => assert!(
                entries.contains(&host) && entries.contains(&"203.0.113.9".to_string()),
                "the whole workspace list is inherited: {entries:?}"
            ),
            other => panic!("an inherited list must derive a proxy, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The shipped read-only preset reads the workspace and needs no network,
    /// so its own policy derives NO proxy - even on an UNRESTRICTED workspace
    /// (no allowlist file). This is the shipped declaration, not a hand-built
    /// one, and it proves the narrowing invariant: a preset's declared empty
    /// reach denies without the workspace having to declare anything.
    #[test]
    fn the_shipped_read_only_preset_derives_no_proxy() {
        let root = scratch("preset-read-only-network");
        // NO workspace allowlist: egress is unrestricted before the preset.
        assert!(!egress_allowlist_path(&root).exists());
        let mut settings = rooted(&root);
        settings.global.default_preset = "read-only".to_string();
        let built = build(&settings);
        assert!(
            built.policy.data().egress_unrestricted,
            "the premise: the workspace itself is open"
        );
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            READ_ONLY,
            agent::sandbox::Principal::new("bash", "session", "read-only"),
        );
        assert_eq!(
            spec.egress(),
            &agent::sandbox::Egress::DenyAll,
            "read-only needs no network, so it derives no proxy"
        );
        // And the policy refuses the address outright: the preset's declared
        // empty reach denies even though the workspace itself is open.
        let decision = built.policy.check(
            &PolicyInput::new("mcp", Action::Egress, Target::host("h", "203.0.113.7"))
                .with_preset("read-only"),
        );
        assert!(decision.is_denied(), "{decision:?}");
        // Two tiers can refuse a read-only preset's egress and the FIRST module
        // in source order reports: the generated session tier denies egress for
        // a read-only effective permission, and the preset's own module denies
        // it again. Either name is honest; what must NOT happen is the
        // workspace's allowlist winning, which is what the assertion above
        // rules out.
        let rule = decision.rule.as_deref().unwrap_or("");
        assert!(
            rule == "session-read-only-egress"
                || rule.starts_with("preset-egress")
                || rule == "no-egress",
            "an egress tier of the preset refused it, not the workspace list: {decision:?}"
        );
        // The preset's declaration itself is what restricts, and it is
        // empty: the workspace's open default does not make it reachable.
        assert!(
            built.policy.data().preset_egress_restricted,
            "declared, not absent"
        );
        assert!(
            built.policy.data().preset_egress_allowlist.is_empty()
                && built.policy.data().preset_egress_hosts.is_empty()
                && built.policy.data().preset_egress_ports.is_empty(),
            "read-only declares an empty reach"
        );
        assert!(
            built
                .policy
                .rule_ids()
                .iter()
                .any(|id| id == "preset-egress-not-allowed"),
            "the shipped read-only preset compiles the preset egress module: {:?}",
            built.policy.rule_ids()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A preset WITH a list still narrows an unrestricted workspace: the
    /// preset's entries are the whole reach, so a destination the open
    /// workspace would have allowed is refused and the local listener sees no
    /// hit. The workspace's openness is not a union with the preset.
    #[test]
    fn a_preset_with_a_list_still_narrows_an_unrestricted_workspace() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(host) = lan_address() else {
            eprintln!("no non-loopback IPv4 address on this host; the egress probe needs one");
            return;
        };
        let listener = std::net::TcpListener::bind((host.as_str(), 0)).expect("a listener");
        let port = listener.local_addr().expect("its port").port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        serve_http(listener, std::sync::Arc::clone(&hits));

        let root = scratch("preset-egress-unrestricted-narrow");
        assert!(
            !egress_allowlist_path(&root).exists(),
            "the premise: the workspace declares no file"
        );
        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "probe".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                // A list that does NOT name the local endpoint.
                egress: Some(vec!["203.0.113.9".to_string()]),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "probe".to_string();
        let built = build(&settings);
        assert!(
            built.policy.data().egress_unrestricted,
            "the workspace itself is open"
        );
        assert!(
            built.policy.data().preset_egress_restricted,
            "the preset declares a reach"
        );
        let spec = agent::sandbox::SandboxSpec::for_policy(
            &built.policy,
            WORKSPACE_WRITE,
            agent::sandbox::Principal::new("bash", "session", "probe"),
        );
        assert_eq!(
            spec.egress(),
            &agent::sandbox::Egress::Allow(vec!["203.0.113.9".to_string()]),
            "the preset's list is the reach, not the open workspace"
        );

        let bash = built
            .agent
            .registry()
            .expect("registry")
            .get("bash")
            .expect("bash");
        // The local endpoint is allowed by the OPEN workspace but not by the
        // preset, so the preset's rule refuses it before a process exists.
        match bash.run(&serde_json::json!({
            "command": format!("curl -sS --max-time 3 -o /dev/null http://{host}:{port}/")
        })) {
            Err(agent::ToolError::Failed(message)) => assert!(
                message.contains("preset-egress"),
                "the preset's list refused the local endpoint: {message}"
            ),
            other => panic!("the preset must narrow the open workspace, got {other:?}"),
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the local endpoint stayed unreachable through the preset"
        );

        // The entry the PRESET names is admitted by the policy: the list is the
        // reach, not a deny of everything.
        let listed = built.policy.check(
            &PolicyInput::new("mcp", Action::Egress, Target::host("h", "203.0.113.9"))
                .with_preset("probe"),
        );
        assert!(listed.is_allowed(), "{listed:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The port dimension narrows through a preset: a preset that lists only
    /// port 443 refuses the same address at another port, naming the port rule.
    #[test]
    fn a_preset_port_list_refuses_the_wrong_port_with_its_own_rule() {
        let root = scratch("preset-egress-ports");
        let (_settings, built) = preset_build(
            &root,
            "203.0.113.7\n",
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["shell".to_string(), "files".to_string()],
                egress: Some(vec!["203.0.113.7".to_string(), "443".to_string()]),
                ..Preset::default()
            },
        );
        assert_eq!(
            built.policy.data().preset_egress_ports,
            vec![443u16],
            "the bare integer is classified as a port"
        );
        let ask = |port: u16| {
            built.policy.check(
                &PolicyInput::new(
                    "mcp",
                    Action::Egress,
                    Target::host_on("h", "203.0.113.7", port),
                )
                .with_preset("probe"),
            )
        };
        assert!(ask(443).is_allowed(), "the declared port is allowed");
        let refused = ask(80);
        assert!(refused.is_denied(), "{refused:?}");
        assert_eq!(
            refused.rule.as_deref(),
            Some("preset-egress-port-not-allowed")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A preset that declares no read/write/deny/web_search/policy_mode resolves
    /// EXACTLY as before: the shipped defaults install no scope restriction, and
    /// the read-only preset keeps its own declared list. This is the "absent
    /// field behaves as today" contract, asserted on the policy data a build
    /// actually hands Rego.
    #[test]
    fn the_shipped_presets_resolve_the_same_scope_with_the_new_fields_absent() {
        let root = scratch("preset-absent-defaults");
        let standard = build(&rooted(&root));
        let data = standard.policy.data();
        assert!(
            !data.preset_read_restricted && !data.preset_write_restricted,
            "no declared list inherits the workspace"
        );
        assert!(data.preset_read_roots.is_empty());
        assert!(
            data.preset_read_globs.is_empty()
                && data.preset_write_globs.is_empty()
                && data.preset_deny_globs.is_empty(),
            "standard declares no globs"
        );
        assert_eq!(
            data.preset_deny_roots.len(),
            6,
            "the shipped secret list is untouched: {:?}",
            data.preset_deny_roots
        );
        assert!(
            !data.preset_egress_restricted,
            "standard inherits the workspace reach"
        );

        let mut read_only_settings = rooted(&root);
        read_only_settings.global.default_preset = "read-only".to_string();
        let read_only = build(&read_only_settings);
        let data = read_only.policy.data();
        assert!(data.preset_read_restricted, "read-only declares reads");
        assert_eq!(
            data.preset_read_roots,
            vec![root.to_string_lossy().to_string()],
            "'.' resolves to the workspace root itself"
        );
        assert!(
            data.preset_write_restricted && data.preset_write_roots.is_empty(),
            "an empty write list allows no writes"
        );
        assert!(
            data.preset_egress_restricted,
            "read-only declares egress = [], a deny-all"
        );
        assert_eq!(data.permission, "read-only");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The glob scope reaches the TOOL boundary: a deny inside a granted write
    /// scope refuses the call and names the preset rule; a write outside the
    /// granted globs refuses; a read outside a declared read glob refuses while
    /// one inside succeeds.
    #[test]
    fn a_preset_glob_scope_is_enforced_by_the_file_tools() {
        let root = scratch("preset-glob-tools");
        std::fs::create_dir_all(root.join("src")).expect("src");
        std::fs::write(root.join("src").join("lib.rs"), "pub fn x() {}").expect("a source file");
        std::fs::write(root.join("README.md"), "# readme").expect("a readme");
        std::fs::create_dir_all(root.join("secrets")).expect("secrets");

        // (a) Deny wins inside the workspace even when the tier and a broad
        // write scope would allow it.
        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "guarded".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["files".to_string()],
                write: Some(vec!["**".to_string()]),
                deny: vec!["secrets/**".to_string()],
                ..Preset::default()
            },
        );
        settings.global.default_preset = "guarded".to_string();
        let built = build(&settings);
        let secret = root.join("secrets").join("key.pem");
        let error = built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": secret.to_string_lossy(),
                    "content": "secret",
                }),
            )
            .expect_err("the deny scope refuses the write");
        assert!(
            error.to_string().contains("preset-deny-path"),
            "the refusal names the scope rule: {error}"
        );
        assert!(!secret.exists(), "the refused write created nothing");

        // (b) A write scope refuses outside its globs.
        settings.global.presets.insert(
            "docs-only".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["files".to_string()],
                write: Some(vec!["docs/**".to_string()]),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "docs-only".to_string();
        let built = build(&settings);
        let outside = root.join("src").join("new.rs");
        let error = built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": outside.to_string_lossy(),
                    "content": "fn main() {}",
                }),
            )
            .expect_err("the write scope refuses the path");
        assert!(error.to_string().contains("preset-write-scope"), "{error}");
        assert!(!outside.exists());
        // The same preset writes inside its glob.
        built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": root.join("docs").join("note.md").to_string_lossy(),
                    "content": "ok",
                }),
            )
            .expect("the declared glob allows the write");

        // (c) A read scope refuses outside its globs and allows inside.
        settings.global.presets.insert(
            "sources".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["files".to_string()],
                read: Some(vec!["src/**/*.rs".to_string()]),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "sources".to_string();
        let built = build(&settings);
        let registry = built.agent.registry().expect("registry");
        registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("src").join("lib.rs").to_string_lossy() }),
            )
            .expect("a read inside the glob is allowed");
        let error = registry
            .execute(
                "read_file",
                &serde_json::json!({ "path": root.join("README.md").to_string_lossy() }),
            )
            .expect_err("a read outside the glob is refused");
        assert!(error.to_string().contains("preset-read-scope"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `web_search = false` reaches the agent's request switch, so the hosted
    /// tool is not offered - the same removal `egress = []` performs for the
    /// network. The engine's own test pins the request's tool array.
    #[test]
    fn web_search_false_withdraws_the_hosted_tool_at_the_build() {
        let root = scratch("preset-web-search");
        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "no-search".to_string(),
            Preset {
                web_search: Some(false),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "no-search".to_string();
        let built = build(&settings);
        assert!(
            !built.agent.config().web_search,
            "the built agent asks the provider for no hosted web search"
        );

        settings.global.presets.insert(
            "search".to_string(),
            Preset {
                web_search: Some(true),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "search".to_string();
        let built = build(&settings);
        assert!(built.agent.config().web_search, "the positive case");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One preset whose own policy refuses writes, in both modes: advisory
    /// records the refusal and lets the call through; required refuses it and
    /// leaves no side effect.
    #[test]
    fn policy_mode_decides_whether_a_preset_refusal_stops_the_call() {
        let rego = "package policy.soft\n\n# rule: soft-deny-write\nverdict := {\"allow\": false, \"rule\": \"soft-deny-write\", \"reason\": \"this preset advises against writes\"} if {\n    input.action == \"write\"\n} else := {\"allow\": true}\n";
        let root = scratch("preset-policy-mode");
        let mut settings = rooted(&root);
        settings.global.presets.insert(
            "soft".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["files".to_string()],
                policy: Some(rego.to_string()),
                trusted: true,
                policy_mode: Some(crate::settings::global::PolicyMode::Advisory),
                ..Preset::default()
            },
        );
        settings.global.default_preset = "soft".to_string();
        let built = build(&settings);
        let target = root.join("notes.md");
        built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": target.to_string_lossy(),
                    "content": "advisory",
                }),
            )
            .expect("an advisory refusal lets the call proceed");
        assert!(target.exists(), "the call's effect is real");
        let advisories = built.policy.advisories();
        assert_eq!(advisories.len(), 1, "{advisories:?}");
        assert!(advisories[0].advisory);
        assert_eq!(
            advisories[0].rule.as_deref(),
            Some("soft-deny-write"),
            "the audit records the refusal verbatim"
        );
        assert_eq!(built.policy.advisories_total(), 1);

        // The floor is not advisory: a write outside the workspace still fails.
        let outside =
            std::env::temp_dir().join(format!("bsh-advisory-outside-{}.md", std::process::id()));
        let error = built
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": outside.to_string_lossy(),
                    "content": "nope",
                }),
            )
            .expect_err("advisory never widens the baseline");
        assert!(
            error.to_string().contains("write-outside-workspace")
                || error.to_string().contains("escapes the workspace"),
            "the floor still refuses outside the workspace: {error}"
        );
        assert!(!outside.exists());

        // The SAME preset in required mode refuses the in-workspace write.
        settings.global.presets.insert(
            "soft".to_string(),
            Preset {
                permission: Some(WORKSPACE_WRITE.to_string()),
                plugins: vec!["files".to_string()],
                policy: Some(rego.to_string()),
                trusted: true,
                policy_mode: Some(crate::settings::global::PolicyMode::Required),
                ..Preset::default()
            },
        );
        let required = build(&settings);
        let refused = root.join("required.md");
        let error = required
            .agent
            .registry()
            .expect("registry")
            .execute(
                "write_file",
                &serde_json::json!({
                    "path": refused.to_string_lossy(),
                    "content": "required",
                }),
            )
            .expect_err("required refuses the call");
        assert!(error.to_string().contains("soft-deny-write"), "{error}");
        assert!(!refused.exists(), "no side effect on a hard refusal");
        assert!(required.policy.advisories().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path scope that cannot be honoured is a LOAD error naming the preset
    /// and the field, never a silent default.
    #[test]
    fn an_unparsable_path_scope_names_the_preset_and_field() {
        let text = "[presets.bad]\nread = [\"docs/[unclosed\"]\n";
        let error = crate::settings::global::GlobalSettings::from_toml(text)
            .expect_err("an unparsable glob is refused");
        let message = error.to_string();
        assert!(message.contains("bad"), "names the preset: {message}");
        assert!(message.contains("read"), "names the field: {message}");
        assert!(
            message.contains("character class"),
            "says what is wrong: {message}"
        );

        let escape = "[presets.bad]\nwrite = [\"../outside\"]\n";
        let error = crate::settings::global::GlobalSettings::from_toml(escape)
            .expect_err("a parent escape is refused");
        assert!(error.to_string().contains("write"), "{error}");
        assert!(error.to_string().contains(".."), "{error}");
    }
}
