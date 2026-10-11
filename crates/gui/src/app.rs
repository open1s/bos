//! Tauri backend: IPC commands, the streaming event bridge, and app bootstrap.
//!
//! The web frontend (`ui/`) talks to this layer through `invoke` calls and
//! receives stream chunks via the `agent-event` / `stream-finished` events.
//! Session persistence, settings, and agent streaming live in the sibling
//! modules ([`crate::session`], [`crate::settings`], [`crate::runner`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use agent::Agent;
use async_channel::Receiver;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::caps::{self, Capabilities};
use crate::runner::{AgentEvent, Runner};
use crate::session::{unix_now, ChatMessage, Role, SessionRecord, SessionStore};
use crate::settings::Settings;

/// An in-flight streaming turn.
#[derive(Clone, Serialize)]
struct Streaming {
    /// Session the stream writes into.
    session_id: String,
    /// Generation of the stream; stale callbacks compare against this.
    gen: u64,
    /// Unix time the stream started (seconds).
    started_at: u64,
    /// Background job (M7) that mirrors this stream's output for `jobs_list`.
    job: String,
}

/// One cached per-session agent: built for a settings generation, carrying
/// its own approval broker so concurrent streams never cross-wire requests.
struct AgentEntry {
    /// `settings_gen` this agent was built for.
    gen: u64,
    /// The agent holding that session's conversation and working plan.
    agent: Arc<Agent>,
    /// Session-local approval broker wired into the agent's gated tools.
    broker: Arc<crate::approval::ApprovalBroker>,
}

/// Mutable GUI state shared by all commands and the streaming bridge.
struct Inner {
    store: SessionStore,
    settings: Settings,
    settings_gen: u64,
    /// Per-session agents, rebuilt lazily whenever settings change.
    agent_cache: HashMap<String, AgentEntry>,
    /// Every broker ever built, so a pending approval stays resolvable even
    /// after its session's agent entry was rebuilt or evicted.
    brokers: Vec<Arc<crate::approval::ApprovalBroker>>,
    /// Connected MCP servers' handshakes, attached to every agent build.
    mcp_ready: Vec<(String, crate::mcp::Handshake)>,
    /// Webview handle installed once during setup (used by new brokers).
    app_handle: Option<AppHandle>,
    runner: Runner,
    /// In-flight streams by session id: several sessions stream at once.
    streaming: HashMap<String, Streaming>,
    stream_gen: u64,
    /// Per-server results of the most recent MCP connect pass.
    mcp_status: Vec<crate::mcp::McpStatus>,
    /// Whether an MCP connect pass for the current `settings_gen` is running.
    mcp_connecting: bool,
    /// Sessions with a manual `/compact` summarization in flight, so a
    /// second request (or a racing send) cannot double-run or interleave.
    compacting: HashSet<String>,
    /// True while a project-level `/init` generation is running.
    init_in_flight: bool,
    /// Scoped persistent approval rules (M4), shared by every broker so a
    /// rule added in one session applies wherever it is scoped to.
    rules: Arc<crate::approval::RuleBook>,
    /// Background jobs (M7) whose bounded output the UI can page through.
    jobs: crate::runs::Jobs,
    /// Durable reminders (M11). The store is the only state; the clock comes
    /// from the caller, so a tick is reproducible.
    schedule: crate::schedule::Store,
    /// The curated knowledge base (M11). Shared with the `knowledge_search`
    /// tool registered on every agent, so the pane and the model read the same
    /// entries.
    knowledge: Arc<Mutex<crate::knowledge::Store>>,
    mcp_policy: Arc<Mutex<crate::mcp_policy::Store>>,
    /// Which skills each chat switched on. Held here rather than read from disk
    /// on every build: the file is the durable copy, this is the one the agents
    /// are built from, and a test can seed it without touching the user's home.
    skill_activation: Mutex<crate::skills::Activation>,
    /// Where that record is written.
    skill_activation_path: std::path::PathBuf,
}

impl Inner {
    fn new() -> Self {
        Self {
            store: SessionStore::default_store(),
            settings: Settings::load(),
            settings_gen: 0,
            agent_cache: HashMap::new(),
            brokers: Vec::new(),
            mcp_ready: Vec::new(),
            app_handle: None,
            runner: Runner::new(),
            streaming: HashMap::new(),
            stream_gen: 0,
            mcp_status: Vec::new(),
            mcp_connecting: false,
            compacting: HashSet::new(),
            init_in_flight: false,
            rules: Arc::new(crate::approval::RuleBook::open_default()),
            jobs: crate::runs::Jobs::new(),
            schedule: crate::schedule::Store::open(),
            knowledge: Arc::new(Mutex::new(crate::knowledge::Store::open())),
            mcp_policy: Arc::new(Mutex::new(crate::mcp_policy::Store::open())),
            skill_activation: Mutex::new(crate::skills::Activation::open()),
            skill_activation_path: crate::skills::Activation::default_path(),
        }
    }

    /// The agent for `session_id`, rebuilt when settings change.
    ///
    /// Each session owns its agent (and approval broker), so parallel streams
    /// keep transcripts, plans, and approval attribution isolated. Entries
    /// from an older settings generation are dropped as soon as a new agent
    /// is built; current-generation entries live on — the frontend re-hydrates
    /// a session's plan through [`restore_plan`] whenever it is opened.
    fn agent(&mut self, session_id: &str) -> Arc<Agent> {
        if let Some(entry) = self.agent_cache.get(session_id) {
            if entry.gen == self.settings_gen {
                return Arc::clone(&entry.agent);
            }
        }
        let broker = Arc::new(crate::approval::ApprovalBroker::default());
        // Allowances the user made permanent outlive the app, so every broker
        // adopts them and an allowed tool is never asked about again.
        broker.seed_allowed(self.settings.allowed_tools.iter().cloned());
        // Scoped rules are consulted before any dialog, and the workspace the
        // broker reports is the one its own tools run against.
        broker.set_rules(Arc::clone(&self.rules));
        let effective = self.settings.effective();
        broker.set_workspace(workspace_root(&effective.bash_workspace));
        if let Some(handle) = &self.app_handle {
            broker.set_handle(handle.clone());
        }
        // The agent is built from the active workspace's choices, not from the
        // globals: model, tools, approval and budget all come from one place.
        let (agent, _mcp_errors) = self.build_agent_for(session_id, &broker);
        let agent = Arc::new(agent);
        let settings_gen = self.settings_gen;
        self.agent_cache
            .retain(|_, entry| entry.gen == settings_gen);
        self.agent_cache.insert(
            session_id.to_string(),
            AgentEntry {
                gen: settings_gen,
                agent: Arc::clone(&agent),
                broker: Arc::clone(&broker),
            },
        );
        self.brokers.push(broker);
        agent
    }

    /// Build an agent for the active workspace, with the connected MCP tools
    /// (under the policy) and the knowledge base attached.
    ///
    /// **The one construction path.** Re-attaching used to clone a live agent
    /// and add the missing tools to it, which can only ever add: a tool the
    /// policy now denies stayed in the registry, because the clone still held
    /// the old wrapper. Building from the base agent is what makes removal
    /// possible, and sharing this function is what keeps a re-attach from
    /// disagreeing with a first build.
    fn build_agent_for(
        &self,
        session: &str,
        broker: &Arc<crate::approval::ApprovalBroker>,
    ) -> (Agent, Vec<(String, String)>) {
        let effective = self.settings.effective();
        let skill_deny = self.session_skill_deny(session);
        let base = caps::build_agent_with(&effective, Arc::clone(broker), skill_deny.as_deref());
        let mut agent = base.as_ref().clone();
        let errors = self.attach_mcp_tools(&mut agent, broker, &effective);
        // The curated knowledge base is a tool, not prompt text: memory is
        // recalled whether it helps or not, while a knowledge entry is pulled
        // only when the model asks, so the corpus can be large and precise.
        attach_knowledge(&mut agent, Arc::clone(&self.knowledge));
        (agent, errors)
    }

    /// The skills this chat's agent must skip, or `None` when it inherits the
    /// workspace.
    ///
    /// The GUI's registration API takes what to *skip*, so a chat's allow-list
    /// is inverted here — in one place, because an inverted allow-list that says
    /// nothing is a chat that receives everything, and that is the failure this
    /// shape can hide.
    fn session_skill_deny(&self, session: &str) -> Option<Vec<String>> {
        let effective = self.settings.effective();
        let dir = effective.skills_dir.trim();
        if dir.is_empty() {
            return None;
        }
        let (_, listed) =
            crate::skills::list(&crate::caps::expand_tilde(dir), &effective.disabled_skills);
        let discovered: Vec<String> = listed.iter().map(|skill| skill.name.clone()).collect();
        // A poisoned lock reads as "inherit", which is the workspace's own
        // answer: a chat losing its own list is a smaller failure than a chat
        // that cannot be opened at all.
        let record = self.skill_activation.lock().ok();
        let active = record.as_deref().and_then(|record| record.active(session));
        crate::skills::session_deny(&discovered, &effective.disabled_skills, active)
    }

    /// The approval broker serving `session_id`, if its agent was built.
    fn broker(&self, session_id: &str) -> Option<Arc<crate::approval::ApprovalBroker>> {
        self.agent_cache
            .get(session_id)
            .map(|e| Arc::clone(&e.broker))
    }

    /// Tag the session's broker so approval requests carry this stream.
    fn set_active_context(&self, session_id: &str, gen: u64) {
        if let Some(entry) = self.agent_cache.get(session_id) {
            entry.broker.set_active(session_id, gen);
        }
    }

    /// Register the connected MCP tools on `agent`, applying the workspace's
    /// gate **and** the per-tool policy. Returns `(namespace, error)` for each
    /// registration failure.
    ///
    /// The policy is consulted here, where the tools are attached, so a denied
    /// tool never reaches the registry and an `ask` rule adds a gate the
    /// workspace does not have. An unreadable policy attaches **nothing**:
    /// failing open would turn a damaged file into silently granted access.
    fn attach_mcp_tools(
        &self,
        agent: &mut Agent,
        broker: &Arc<crate::approval::ApprovalBroker>,
        settings: &Settings,
    ) -> Vec<(String, String)> {
        // Approval policy and the allowed servers are the workspace's, not the
        // globals': the handshake may exist because the server is configured,
        // while this workspace is one that switched it off.
        let workspace_gate = settings.require_approval;
        let allowed = enabled_mcp_servers(settings);
        let policy = match self.mcp_policy.lock() {
            Ok(policy) => policy,
            Err(_) => {
                return vec![(
                    "*".to_string(),
                    "the MCP policy could not be read (lock poisoned); no MCP tool was attached"
                        .to_string(),
                )];
            }
        };
        let mut errors = Vec::new();
        for (namespace, handshake) in &self.mcp_ready {
            if !allowed.iter().any(|name| name == namespace) {
                continue;
            }
            let names: Vec<String> = handshake.tools.iter().map(|d| d.name.clone()).collect();
            // The decision is made by the module the pane reads, so the two
            // cannot disagree about what a rule means.
            let attachment = crate::mcp_policy::attach_tools_with_policy(
                agent,
                namespace,
                &names,
                &policy,
                workspace_gate,
                |index, gated| {
                    crate::mcp::tool_for(
                        handshake,
                        namespace,
                        &handshake.tools[index],
                        gated.then(|| Arc::clone(broker)),
                    )
                },
            );
            if let Err(err) = attachment {
                errors.push((namespace.clone(), err));
            }
        }
        errors
    }
}

/// Wrapper managed by Tauri; commands take `State<Arc<GuiState>>`.
struct GuiState(Mutex<Inner>);

impl GuiState {
    fn new() -> Self {
        Self(Mutex::new(Inner::new()))
    }

    /// Lock the inner state, treating a poisoned lock as a hard error.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, String> {
        self.0
            .lock()
            .map_err(|_| "internal state lock poisoned".to_string())
    }
}

/// MCP connection results handed to the frontend.
#[derive(Clone, Serialize)]
struct McpStatusReport {
    /// True while the current settings generation's connect pass is running.
    connecting: bool,
    /// One entry per configured server, in settings order.
    servers: Vec<crate::mcp::McpStatus>,
}

/// Connect every enabled MCP server for settings generation `gen` in the
/// background and attach their tools to the agent.
///
/// The async handshake ([`crate::mcp::preflight`]) runs *before* any lock is
/// taken — a network await must never hold the non-`Send` state guard.
/// Servers that fail are retried with backoff (rounds at +2s and +4s) before
/// the pass reports, and only the failures are re-attempted, so recovered
/// servers keep their connection. Registration then happens on a clone of
/// every cached session agent (the registry is `Arc` copy-on-write, so a
/// stream holding the old `Arc` keeps running untouched) under one short
/// lock, and the handshakes are remembered so later agent builds attach the
/// same tools. A generation change — the user saved settings again — aborts
/// the pass at each lock and between retries so a stale connect never
/// overwrites fresher state.
fn spawn_mcp_connect(state: Arc<GuiState>, gen: u64) {
    tauri::async_runtime::spawn(async move {
        let entries = {
            let Ok(mut inner) = state.lock() else {
                return;
            };
            if inner.settings_gen != gen {
                return;
            }
            inner.mcp_connecting = true;
            inner.settings.mcp_servers.clone()
        };

        // Handshake every enabled server; one failure must not block the rest.
        // Later rounds re-preflight only the servers that failed before, so a
        // working connection is never dropped to chase someone else's retry.
        // Cold stdio spawns fail transiently often enough that two backoff
        // rounds (2s, 4s) recover real outages without any user action.
        const RETRY_DELAYS: [u64; 2] = [2, 4];
        let mut handshakes: HashMap<String, crate::mcp::Handshake> = HashMap::new();
        let mut results: HashMap<String, crate::mcp::McpStatus> = HashMap::new();
        let mut pending: Vec<&crate::settings::McpServerEntry> =
            entries.iter().filter(|e| e.enabled).collect();
        let mut attempt = 0usize;
        loop {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAYS[attempt - 1])).await;
                // A settings change while sleeping invalidates this pass.
                let Ok(inner) = state.lock() else {
                    return;
                };
                if inner.settings_gen != gen {
                    return;
                }
                drop(inner);
            }
            let mut failed: Vec<&crate::settings::McpServerEntry> = Vec::new();
            for entry in pending {
                let name = entry.name.trim().to_string();
                match crate::mcp::preflight(entry).await {
                    Ok(handshake) => {
                        results.insert(
                            name.clone(),
                            crate::mcp::McpStatus {
                                name: name.clone(),
                                transport: entry.transport.clone(),
                                enabled: true,
                                connected: true,
                                tools: handshake.tools.len(),
                                tool_names: handshake
                                    .tools
                                    .iter()
                                    .map(|d| d.name.clone())
                                    .collect(),
                                error: None,
                            },
                        );
                        handshakes.insert(name, handshake);
                    }
                    Err(err) => {
                        results.insert(
                            name.clone(),
                            crate::mcp::McpStatus {
                                name,
                                transport: entry.transport.clone(),
                                enabled: true,
                                connected: false,
                                tools: 0,
                                tool_names: Vec::new(),
                                error: Some(err),
                            },
                        );
                        failed.push(entry);
                    }
                }
            }
            attempt += 1;
            if failed.is_empty() || attempt > RETRY_DELAYS.len() {
                break;
            }
            pending = failed;
        }

        let Ok(mut inner) = state.lock() else {
            return;
        };
        if inner.settings_gen != gen {
            return;
        }
        // Settings-ordered report; disabled servers still appear so the UI
        // can show them. Handshakes are emitted in settings order too, so
        // tool registration order is deterministic across passes.
        let statuses = build_statuses(&entries, &results);
        let ready: Vec<(String, crate::mcp::Handshake)> = {
            let mut map = handshakes;
            entries
                .iter()
                .filter(|e| e.enabled)
                .filter_map(|e| {
                    let name = e.name.trim().to_string();
                    map.remove(&name).map(|handshake| (name, handshake))
                })
                .collect()
        };
        // Remember the handshakes so later agent builds attach them too.
        inner.mcp_ready = ready;
        // Install this pass's rows first, then rebuild every cached session
        // agent: the rebuild has to be able to *remove* a tool (a policy that
        // started denying one, or a server that dropped it), and re-attaching
        // onto a clone of the running agent can only ever add. A stream that is
        // mid-turn keeps its previous `Arc` untouched, because the cache entry
        // is swapped rather than mutated.
        inner.mcp_status = statuses;
        let attach_errors = reattach_cached_agents(&mut inner);
        for (namespace, err) in &attach_errors {
            if let Some(status) = inner.mcp_status.iter_mut().find(|s| s.name == *namespace) {
                status.connected = false;
                status.error = Some(err.clone());
            }
        }
        inner.mcp_connecting = false;
    });
}

/// Assemble the settings-ordered status list for a finished connect pass.
///
/// Disabled servers report as off with no error; enabled servers carry the
/// attempt's result; an enabled entry the pass never reached reports as an
/// unattempted failure so the dialog never shows a phantom "pending" row
/// once `mcp_connecting` has dropped.
fn build_statuses(
    entries: &[crate::settings::McpServerEntry],
    results: &HashMap<String, crate::mcp::McpStatus>,
) -> Vec<crate::mcp::McpStatus> {
    entries
        .iter()
        .map(|entry| {
            let name = entry.name.trim().to_string();
            if !entry.enabled {
                return crate::mcp::McpStatus {
                    name,
                    transport: entry.transport.clone(),
                    enabled: false,
                    connected: false,
                    tools: 0,
                    tool_names: Vec::new(),
                    error: None,
                };
            }
            results
                .get(&name)
                .cloned()
                .unwrap_or_else(|| crate::mcp::McpStatus {
                    name,
                    transport: entry.transport.clone(),
                    enabled: true,
                    connected: false,
                    tools: 0,
                    tool_names: Vec::new(),
                    error: Some("connect pass never reached this server".to_string()),
                })
        })
        .collect()
}

/// Sidebar row payload sent to the frontend.
#[derive(Clone, Serialize, Debug)]
struct SessionSummary {
    /// Stable identifier.
    id: String,
    /// Display title (derived from the first user message once known).
    title: String,
    /// Unix time of the last activity.
    updated_at: u64,
    /// Workspace this session belongs to; never empty in the UI, because a
    /// session written before workspaces existed is reported as the active one.
    workspace: String,
    /// Whether the sidebar floats this chat above the others.
    pinned: bool,
    /// Whether the chat is hidden in the archive rather than in the list.
    archived: bool,
}

fn summary(record: &SessionRecord, active: &str) -> SessionSummary {
    SessionSummary {
        id: record.id.clone(),
        title: record.title.clone(),
        updated_at: record.updated_at,
        workspace: if record.workspace.is_empty() {
            active.to_string()
        } else {
            record.workspace.clone()
        },
        pinned: record.pinned,
        archived: record.archived_at.is_some(),
    }
}

/// One streamed event forwarded to the frontend.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StreamKind {
    /// A chunk of assistant text.
    Text {
        /// Text to append to the last assistant message.
        text: String,
    },
    /// A chunk of reasoning/thinking text.
    Reasoning {
        /// Text to append to the reasoning trace.
        text: String,
    },
    /// A tool invocation executed during the turn.
    Tool {
        /// Tool name.
        name: String,
        /// Serialized arguments.
        args: String,
    },
    /// A tool call finished, with its output and duration.
    ToolResult {
        /// Tool name (matches the preceding `Tool` event).
        name: String,
        /// Tool output text (capped for display).
        output: String,
        /// Execution time in milliseconds.
        ms: u64,
    },
    /// Token usage reported by the provider.
    Usage {
        /// Prompt (input) tokens.
        prompt: u64,
        /// Completion (output) tokens.
        completion: u64,
    },
    /// Older messages were summarized away before the turn started.
    Compacted {
        /// Estimated prompt tokens before compaction.
        before: usize,
        /// Estimated prompt tokens after compaction.
        after: usize,
        /// Messages folded into the summary.
        dropped: usize,
    },
    /// The turn failed.
    Error {
        /// Error description.
        message: String,
    },
    /// The turn finished normally.
    Done,
}

/// Event payload emitted as `agent-event`.
#[derive(Clone, Serialize)]
struct AgentMessage {
    /// Session the chunk belongs to.
    session_id: String,
    /// Stream generation.
    gen: u64,
    /// The chunk itself.
    #[serde(flatten)]
    event: StreamKind,
}

/// Payload emitted as `stream-finished`.
#[derive(Clone, Serialize)]
struct StreamFinished {
    /// Session whose stream ended.
    session_id: String,
    /// Stream generation that ended.
    gen: u64,
    /// Prompt tokens the provider reported for the last call, when it reported any.
    prompt_tokens: Option<u64>,
    /// Completion tokens likewise.
    completion_tokens: Option<u64>,
}

/// List all sessions, most recently active first.
#[tauri::command]
fn list_sessions(state: State<'_, Arc<GuiState>>) -> Result<Vec<SessionSummary>, String> {
    let inner = state.lock()?;
    let active = inner.settings.active_workspace.clone();
    Ok(inner
        .store
        .load_all()
        .iter()
        .map(|r| summary(r, &active))
        .collect())
}

/// Create an empty session and return its summary.
#[tauri::command]
fn create_session(state: State<'_, Arc<GuiState>>) -> Result<SessionSummary, String> {
    let inner = state.lock()?;
    // Sessions own their agent (and plan) from the first build — nothing to
    // reset on a brand-new id.
    let mut record = SessionRecord::new();
    // A new session belongs to the workspace in use, recorded at creation so the
    // assignment survives a later switch away from that workspace (§24.6).
    record.workspace = inner.settings.active_workspace.clone();
    inner.store.save(&record).map_err(|e| e.to_string())?;
    Ok(summary(&record, &inner.settings.active_workspace))
}

/// Rename a chat.
///
/// Refused while a turn or a compaction is running, for the same reason a plan
/// edit is: the sidebar must not change the thing the reader is looking at
/// mid-flight. The new name is validated by the record, so the rule lives in one
/// place and a hand-written name cannot be longer than a typed one.
#[tauri::command]
fn rename_session(
    state: State<'_, Arc<GuiState>>,
    id: String,
    title: String,
) -> Result<SessionSummary, String> {
    let mut inner = state.lock()?;
    rename_session_in(&mut inner, &id, &title)
}

fn rename_session_in(inner: &mut Inner, id: &str, title: &str) -> Result<SessionSummary, String> {
    editable(inner, id)?;
    let mut record = session_record(inner, id)?;
    record.rename(title)?;
    inner.store.save(&record).map_err(|e| e.to_string())?;
    Ok(summary(&record, &inner.settings.active_workspace))
}

/// Pin or unpin a chat. A pin is presentation only, so it is allowed even while
/// a turn streams — the one session edit that does not wait for the chat.
#[tauri::command]
fn set_session_pinned(
    state: State<'_, Arc<GuiState>>,
    id: String,
    pinned: bool,
) -> Result<SessionSummary, String> {
    let mut inner = state.lock()?;
    set_session_pinned_in(&mut inner, &id, pinned)
}

fn set_session_pinned_in(
    inner: &mut Inner,
    id: &str,
    pinned: bool,
) -> Result<SessionSummary, String> {
    let mut record = session_record(inner, id)?;
    // Asking for the state a chat is already in is a no-op *on disk*: a pin
    // must not rewrite the record (and bump `updated_at`) just because a menu
    // was opened and closed.
    if record.pinned != pinned {
        record.pinned = pinned;
        inner.store.save(&record).map_err(|e| e.to_string())?;
    }
    Ok(summary(&record, &inner.settings.active_workspace))
}

/// Archive or restore a chat.
///
/// Archiving is not deleting: the file stays, the transcript stays, and
/// [`set_session_archived`] with `false` puts the chat back exactly as it was.
/// A *streaming* chat is refused, because hiding the surface a running turn is
/// writing into hides the one thing the reader is waiting for.
#[tauri::command]
fn set_session_archived(
    state: State<'_, Arc<GuiState>>,
    id: String,
    archived: bool,
) -> Result<SessionSummary, String> {
    let mut inner = state.lock()?;
    set_session_archived_in(&mut inner, &id, archived)
}

fn set_session_archived_in(
    inner: &mut Inner,
    id: &str,
    archived: bool,
) -> Result<SessionSummary, String> {
    // Hiding a chat mid-turn hides the output the turn is producing, so the
    // archive waits. Restoring one is always allowed: the chat is not on screen
    // while it is archived, so there is no turn to interrupt.
    if archived {
        editable(inner, id)?;
    }
    let mut record = session_record(inner, id)?;
    if record.archived_at.is_some() != archived {
        record.set_archived(archived);
        inner.store.save(&record).map_err(|e| e.to_string())?;
    }
    Ok(summary(&record, &inner.settings.active_workspace))
}

/// Delete a session's stored file, stopping its stream if it is running.
#[tauri::command]
fn delete_session(state: State<'_, Arc<GuiState>>, id: String) -> Result<(), String> {
    let mut inner = state.lock()?;
    if inner.streaming.remove(&id).is_some() {
        inner.runner.stop(&id);
        // Deny whatever that session was waiting on; other chats' streams
        // and approvals are untouched. The forwarder will find the record
        // gone and skip persisting.
        if let Some(broker) = inner.broker(&id) {
            broker.reject_all();
        }
        inner.agent_cache.remove(&id);
        inner.stream_gen += 1;
    }
    inner.store.delete(&id);
    Ok(())
}

/// Move a chat into another workspace (§24.6). A session's workspace decides the
/// model, tools and skills that run it, so the move drops the cached agent and the
/// next turn is built from the workspace it now belongs to. A streaming chat is
/// refused rather than moved mid-flight, because the stream's forwarder is bound
/// to the configuration it started with.
#[tauri::command]
fn move_session(
    state: State<'_, Arc<GuiState>>,
    id: String,
    workspace_id: String,
) -> Result<Vec<SessionRecord>, String> {
    let mut inner = state.lock()?;
    if inner.streaming.contains_key(&id) {
        return Err("this chat is streaming; stop it before moving it".to_string());
    }
    if !inner
        .settings
        .workspaces
        .iter()
        .any(|w| w.id == workspace_id)
    {
        return Err(format!("unknown workspace '{workspace_id}'"));
    }
    inner
        .store
        .move_to(&id, &workspace_id)
        .map_err(|e| e.to_string())?;
    inner.agent_cache.remove(&id);
    Ok(inner.store.load_all())
}

/// A remembered line has to be worth storing and has to be bounded: an empty
/// string is a mistake, and an unbounded one would push the store's own budget
/// out of the way one paste at a time.
const MAX_MEMORY_CHARS: usize = 2000;

/// Trim and check a memory before it is stored. Split out from the command so the
/// rule can be tested without a running app.
fn clean_memory_text(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("a memory needs some text".to_string());
    }
    if text.chars().count() > MAX_MEMORY_CHARS {
        return Err(format!(
            "a memory is capped at {MAX_MEMORY_CHARS} characters"
        ));
    }
    Ok(text.to_string())
}

/// The memory store behind a session's agent, cloned out so the caller can await
/// on it without holding the state lock across a suspension point.
fn memory_store(
    state: &Arc<GuiState>,
    session_id: &str,
) -> Result<Arc<dyn agent::memory::MemoryStore>, String> {
    let mut inner = state.lock()?;
    let agent = inner.agent(session_id);
    agent
        .memory()
        .cloned()
        .ok_or_else(|| "memory is switched off; turn it on in Settings before using it".to_string())
}

/// Everything the store holds, newest first: the order a person reads a memory
/// list in, and stable between identical calls.
async fn memories_of(
    store: &Arc<dyn agent::memory::MemoryStore>,
) -> Vec<agent::memory::MemoryItem> {
    let mut items = store.all().await;
    items.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    items
}

/// Where this workspace's memory really lives, and what bounds it (§20).
///
/// The pane labels the list by workspace, but an empty `memory_path` — the
/// default — puts every workspace on one shared file, so the label alone can
/// imply an isolation that is not there. This answers with the resolved file,
/// how many workspaces resolve to it, and the two caps a person cannot otherwise
/// see: the item bound (past which the oldest memory is evicted) and the
/// per-note character bound. It never fails: a status that refuses to load when
/// memory is off would hide the very fact it exists to report.
#[derive(serde::Serialize)]
pub(crate) struct MemoryStatus {
    /// Whether the active workspace builds an agent with memory at all.
    pub(crate) enabled: bool,
    /// Resolved path of the store, empty when memory is off.
    pub(crate) path: String,
    /// How many workspaces resolve to that same file.
    pub(crate) shared_workspaces: usize,
    /// Item bound of the store ([`crate::settings::MEMORY_MAX_ITEMS`]).
    pub(crate) cap: usize,
    /// Longest note the store accepts.
    pub(crate) max_chars: usize,
}

#[tauri::command]
fn memory_status(state: State<'_, Arc<GuiState>>) -> Result<MemoryStatus, String> {
    let inner = state.lock()?;
    let effective = inner.settings.effective();
    let path = crate::settings::memory_file(&effective);
    let shared = inner
        .settings
        .workspace_memory_files()
        .iter()
        .filter(|(_, f)| f == &path)
        .count();
    Ok(MemoryStatus {
        enabled: effective.memory_enabled,
        path: if effective.memory_enabled {
            path.to_string_lossy().into_owned()
        } else {
            String::new()
        },
        shared_workspaces: shared,
        cap: crate::settings::MEMORY_MAX_ITEMS,
        max_chars: MAX_MEMORY_CHARS,
    })
}

/// List what the GUI remembers (§20). Memory is one file shared by every chat in
/// this app (`caps::open_file_memory`), so any session's agent answers for all.
#[tauri::command]
async fn list_memories(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<Vec<agent::memory::MemoryItem>, String> {
    let store = memory_store(state.inner(), &session_id)?;
    Ok(memories_of(&store).await)
}

/// Add one memory by hand. The agent also remembers exchanges by itself when
/// auto-remember is on; this is the deliberate kind.
#[tauri::command]
async fn remember_memory(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    text: String,
) -> Result<Vec<agent::memory::MemoryItem>, String> {
    let text = clean_memory_text(&text)?;
    let store = memory_store(state.inner(), &session_id)?;
    store.add(text, None).await;
    Ok(memories_of(&store).await)
}

/// Forget one memory by id, and answer with the list as it now stands — the
/// caller's next question is always "what is left?".
#[tauri::command]
async fn forget_memory(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    id: String,
) -> Result<Vec<agent::memory::MemoryItem>, String> {
    let store = memory_store(state.inner(), &session_id)?;
    if !store.remove(&id).await {
        return Err("that memory is already gone".to_string());
    }
    Ok(memories_of(&store).await)
}

/// Search every stored chat — titles and message text (§17). Hits come back
/// newest chat first and are capped, because this box is for finding a chat, not
/// for reading the archive.
#[tauri::command]
/// The reminders, their deliveries and the bounds in force, as of `now_ms`.
fn schedule_list(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::schedule::Report, String> {
    let inner = state.lock()?;
    Ok(inner.schedule.report(now_ms))
}

#[tauri::command]
/// Deliver everything due at `now_ms` and return the state that follows.
fn schedule_tick(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::schedule::Report, String> {
    let mut inner = state.lock()?;
    let fired = inner.schedule.tick_due(now_ms)?;
    Ok(inner.schedule.report_with(now_ms, fired))
}

#[tauri::command]
/// Add a reminder. `every_minutes` makes it repeat; otherwise `at_ms` is the
/// one delivery. Either way the store validates before it accepts.
fn schedule_add(
    state: State<'_, Arc<GuiState>>,
    text: String,
    every_minutes: Option<u64>,
    at_ms: Option<u64>,
    now_ms: u64,
) -> Result<crate::schedule::Report, String> {
    let mut inner = state.lock()?;
    let cadence = match (every_minutes, at_ms) {
        (Some(every), _) => crate::schedule::Cadence::Every {
            every_minutes: every,
            next_ms: now_ms + every * 60_000,
        },
        (None, Some(at)) => crate::schedule::Cadence::Once { at_ms: at },
        (None, None) => {
            return Err("a reminder needs either an interval or a time".to_string());
        }
    };
    inner.schedule.add(&text, cadence, now_ms)?;
    inner.schedule.save()?;
    Ok(inner.schedule.report(now_ms))
}

#[tauri::command]
/// Remove one reminder.
fn schedule_cancel(
    state: State<'_, Arc<GuiState>>,
    id: String,
    now_ms: u64,
) -> Result<crate::schedule::Report, String> {
    let mut inner = state.lock()?;
    inner.schedule.cancel(&id)?;
    inner.schedule.save()?;
    Ok(inner.schedule.report(now_ms))
}

#[tauri::command]
/// Forget every delivery, keeping the reminders.
fn schedule_clear_history(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::schedule::Report, String> {
    let mut inner = state.lock()?;
    inner.schedule.clear_history();
    inner.schedule.save()?;
    Ok(inner.schedule.report(now_ms))
}

/// Register the curated knowledge base on `agent`.
///
/// A free function so the registration a session actually gets can be tested
/// against a real agent (see `the_agent_the_gui_builds_can_search_its_knowledge_base`).
fn attach_knowledge(agent: &mut Agent, store: Arc<Mutex<crate::knowledge::Store>>) {
    agent.add_tool(crate::knowledge::tool(store));
}

/// The knowledge store, or an error naming the lock rather than a panic.
fn knowledge_store(
    inner: &Inner,
) -> Result<std::sync::MutexGuard<'_, crate::knowledge::Store>, String> {
    inner
        .knowledge
        .lock()
        .map_err(|_| "the knowledge store lock is poisoned".to_string())
}

/// The knowledge entries a query selects (empty for all of them), best first.
#[tauri::command]
fn knowledge_list(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::knowledge::Report, String> {
    let inner = state.lock()?;
    let store = knowledge_store(&inner)?;
    Ok(store.report("", None, now_ms))
}

/// Search the knowledge base.
#[tauri::command]
fn knowledge_search(
    state: State<'_, Arc<GuiState>>,
    query: String,
    limit: Option<usize>,
    now_ms: u64,
) -> Result<crate::knowledge::Report, String> {
    let inner = state.lock()?;
    let store = knowledge_store(&inner)?;
    Ok(store.report(&query, limit, now_ms))
}

/// Add a knowledge entry. `tags` arrives as typed — comma-separated.
#[tauri::command]
fn knowledge_add(
    state: State<'_, Arc<GuiState>>,
    title: String,
    tags: String,
    body: String,
    now_ms: u64,
) -> Result<crate::knowledge::Report, String> {
    let inner = state.lock()?;
    let mut store = knowledge_store(&inner)?;
    store.add(&title, &tags, &body, now_ms)?;
    store.save()?;
    Ok(store.report("", None, now_ms))
}

/// Replace an entry's title, tags and body.
#[tauri::command]
fn knowledge_update(
    state: State<'_, Arc<GuiState>>,
    id: String,
    title: String,
    tags: String,
    body: String,
    now_ms: u64,
) -> Result<crate::knowledge::Report, String> {
    let inner = state.lock()?;
    let mut store = knowledge_store(&inner)?;
    store.update(&id, &title, &tags, &body, now_ms)?;
    store.save()?;
    Ok(store.report("", None, now_ms))
}

/// Remove one entry.
#[tauri::command]
fn knowledge_delete(
    state: State<'_, Arc<GuiState>>,
    id: String,
    now_ms: u64,
) -> Result<crate::knowledge::Report, String> {
    let inner = state.lock()?;
    let mut store = knowledge_store(&inner)?;
    store.remove(&id)?;
    store.save()?;
    Ok(store.report("", None, now_ms))
}

/// The skills the active workspace's agent would receive.
///
/// Read through the same loader that builds the agent, so this count and the
/// prompt are the same number by construction.
#[tauri::command]
fn skill_list(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::skills::Report, String> {
    let inner = state.lock()?;
    Ok(skill_report(&inner, now_ms))
}

/// Author a skill: validate it, write `SKILL.md`, then rebuild the agents that
/// already exist so the skill is available to a session that is already open.
#[tauri::command]
fn skill_author(
    state: State<'_, Arc<GuiState>>,
    name: String,
    description: String,
    body: String,
    overwrite: bool,
    now_ms: u64,
) -> Result<crate::skills::Report, String> {
    let mut inner = state.lock()?;
    let report = author_skill(&mut inner, &name, &description, &body, overwrite, now_ms)?;
    Ok(report)
}

/// The directory the active workspace reads skills from.
fn skill_dir(inner: &Inner) -> std::path::PathBuf {
    let dir = inner.settings.effective().skills_dir.trim().to_string();
    if dir.is_empty() {
        return std::path::PathBuf::new();
    }
    caps::expand_tilde(&dir)
}

/// The pane's report for the active workspace.
fn skill_report(inner: &Inner, now_ms: u64) -> crate::skills::Report {
    let effective = inner.settings.effective();
    let dir = skill_dir(inner);
    crate::skills::report(&dir, &effective.disabled_skills, now_ms)
}

/// Write a skill and make it reach the running sessions. Split out from the
/// command so a test can drive it without a Tauri `State`.
fn author_skill(
    inner: &mut Inner,
    name: &str,
    description: &str,
    body: &str,
    overwrite: bool,
    now_ms: u64,
) -> Result<crate::skills::Report, String> {
    let dir = skill_dir(inner);
    if dir.as_os_str().is_empty() {
        return Err(
            "no skills directory is configured for this workspace — set one in Settings, Tools & skills"
                .to_string(),
        );
    }
    crate::skills::save(&dir, name, description, body, overwrite)?;
    // A session's agent read the directory when it was built, so a saved skill
    // only reaches that session if the agents are rebuilt — the same rule the
    // MCP policy follows.
    let _ = reattach_cached_agents(inner);
    Ok(skill_report(inner, now_ms))
}

#[tauri::command]
fn search_sessions(
    state: State<'_, Arc<GuiState>>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<crate::session::SearchHit>, String> {
    let inner = state.lock()?;
    Ok(inner.store.search(&query, limit.unwrap_or(50).min(200)))
}

/// Load a session's messages (pure read; no shared-state side effects —
/// the frontend also loads background sessions through this).
#[tauri::command]
fn load_session(state: State<'_, Arc<GuiState>>, id: String) -> Result<Vec<ChatMessage>, String> {
    let inner = state.lock()?;
    inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.messages)
        .ok_or_else(|| "session not found".to_string())
}

/// Load session `id`'s persisted plan into its agent and return it.
///
/// The panel hydrates through this on app start and on session switch; that
/// also rebuilds an agent that was pruned after a settings change. Mid-stream
/// reads go through [`get_plan`], which sees that session's live agent.
#[tauri::command]
fn restore_plan(
    state: State<'_, Arc<GuiState>>,
    id: String,
) -> Result<Vec<agent::tools::PlanItem>, String> {
    let mut inner = state.lock()?;
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "session not found".to_string())?;
    inner.agent(&id).set_plan(record.plan.clone());
    Ok(record.plan)
}

/// The live working plan of `session_id`, maintained through `update_plan`.
#[tauri::command]
fn get_plan(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<Vec<agent::tools::PlanItem>, String> {
    let mut inner = state.lock()?;
    Ok(inner.agent(&session_id).plan_items())
}

/// Human-in-the-loop plan steering: replace the working plan from the UI.
///
/// Writes go through the same [`PlanStore`] the `update_plan` tool and the
/// prompt injection read, so the very next turn sees the edited steps. The
/// sanitized snapshot is persisted immediately — no stream end required —
/// and single-writer guards keep a mid-turn tool call from racing an edit.
///
/// [`PlanStore`]: agent::tools::PlanStore
#[tauri::command]
fn set_plan(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    items: Vec<agent::tools::PlanItem>,
) -> Result<Vec<agent::tools::PlanItem>, String> {
    let mut inner = state.lock()?;
    if inner.streaming.contains_key(&session_id) {
        return Err("a turn is in progress — wait for it to finish".to_string());
    }
    if inner.compacting.contains(&session_id) {
        return Err("a compaction is running for this chat".to_string());
    }
    let mut record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    let agent = inner.agent(&session_id);
    agent.set_plan(items);
    record.plan = agent.plan_items(); // the store's sanitized view
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist: {err}"))?;
    Ok(record.plan)
}

/// Locate one session record by id.
fn session_record(inner: &Inner, session_id: &str) -> Result<SessionRecord, String> {
    inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())
}

/// Refuse an edit that would change the record under a running turn.
fn editable(inner: &Inner, session_id: &str) -> Result<(), String> {
    if inner.streaming.contains_key(session_id) {
        return Err("a turn is in progress — wait for it to finish".to_string());
    }
    if inner.compacting.contains(session_id) {
        return Err("a compaction is running for this chat".to_string());
    }
    Ok(())
}

/// The goal `session_id` is pursuing, if it has one.
#[tauri::command]
fn get_goal(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<Option<agent::goal::Goal>, String> {
    let inner = state.lock()?;
    Ok(session_record(&inner, &session_id)?.goal)
}

/// Create or replace the goal of `session_id`.
///
/// A blank objective **clears** the goal: the absence of a goal is expressed by
/// having none, not by storing an empty one. An edit mid-turn is refused for the
/// same reason a plan edit is — the statement the model is working from must not
/// change under it.
#[tauri::command]
fn set_goal(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    objective: String,
    max_rounds: Option<u32>,
) -> Result<Option<agent::goal::Goal>, String> {
    let inner = state.lock()?;
    editable(&inner, &session_id)?;
    let mut record = session_record(&inner, &session_id)?;
    record.goal = agent::goal::Goal::new(objective, max_rounds);
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist: {err}"))?;
    Ok(record.goal)
}

/// Drive the goal's state machine: `advance`, `pause`, `resume`, `complete`, or
/// `block` (which requires a `reason`).
///
/// The transitions live in [`agent::goal::Goal`], so the host cannot invent one:
/// an action that does not apply is an error that says why, and nothing is
/// written.
#[tauri::command]
fn goal_action(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    action: String,
    reason: Option<String>,
) -> Result<agent::goal::Goal, String> {
    let inner = state.lock()?;
    editable(&inner, &session_id)?;
    let mut record = session_record(&inner, &session_id)?;
    let goal = record
        .goal
        .as_mut()
        .ok_or_else(|| "this chat has no goal".to_string())?;
    let changed = match action.as_str() {
        "advance" => goal.advance(),
        "pause" => goal.pause(),
        "resume" => goal.resume(),
        "complete" => goal.complete(),
        "block" => goal.block(reason.unwrap_or_default()),
        other => return Err(format!("unknown goal action '{other}'")),
    };
    if !changed {
        return Err(format!(
            "'{action}' does not apply while the goal is {:?}",
            goal.state
        ));
    }
    let updated = goal.clone();
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist: {err}"))?;
    Ok(updated)
}

/// The `[llm.<name>]` profiles discovered in the config file.
///
/// A profile is selected by typing its name into the model field, which is not
/// discoverable without this: the names live in the config file and the settings
/// payload never serializes them.
#[tauri::command]
fn list_profiles(
    state: State<'_, Arc<GuiState>>,
) -> Result<Vec<crate::settings::ProfileInfo>, String> {
    let inner = state.lock()?;
    Ok(inner.settings.profile_infos())
}

/// Export a session as portable Markdown under `~/.bos/gui/exports/`,
/// returning the absolute file path (frontend shows it in a toast).
#[tauri::command]
fn export_session(state: State<'_, Arc<GuiState>>, id: String) -> Result<String, String> {
    let inner = state.lock()?;
    let record = inner
        .store
        .load(&id)
        .ok_or_else(|| format!("session {id} not found"))?;
    let path = inner.store.export(&record).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Every stream currently in flight (used to restore UI state).
#[tauri::command]
fn current_stream(state: State<'_, Arc<GuiState>>) -> Result<Vec<Streaming>, String> {
    let mut streams: Vec<Streaming> = state.lock()?.streaming.values().cloned().collect();
    streams.sort_by_key(|s| s.started_at);
    Ok(streams)
}

/// Start a fresh stream for `text` (with optional attached `images`) given the
/// prior history `transcript`.
///
/// The caller must hold the state lock; the forwarder is attached by the
/// command afterwards. Other sessions may be streaming concurrently — each
/// session gets its own agent, runner slot, and approval context.
fn begin_stream(
    inner: &mut Inner,
    session_id: &str,
    text: String,
    images: Vec<String>,
    transcript: Vec<ChatMessage>,
) -> (Receiver<AgentEvent>, Streaming) {
    inner.stream_gen += 1;
    let job = inner
        .jobs
        .start(format!("session {session_id}"), "stream")
        .id;
    let streaming = Streaming {
        session_id: session_id.to_string(),
        gen: inner.stream_gen,
        started_at: unix_now(),
        job,
    };
    let budget = inner.settings.context_budget;
    // Expand `@path` mentions on the outgoing copy only: the transcript in
    // the store keeps the raw text, so re-sends stay lean and the byte/file
    // caps re-apply fresh on every turn (including retries).
    let text = {
        let root = std::path::Path::new(inner.settings.bash_workspace.trim());
        crate::mentions::expand(&text, root)
    };
    let agent = inner.agent(session_id);
    // Approval requests raised during this turn carry this context.
    inner.set_active_context(session_id, streaming.gen);
    let rx = inner.runner.spawn(
        session_id,
        agent,
        crate::runner::turn_content(text, &images),
        transcript,
        budget,
    );
    inner
        .streaming
        .insert(session_id.to_string(), streaming.clone());
    (rx, streaming)
}

/// Persist the user turn and start streaming an assistant reply.
///
/// `truncate_from` supports edit-and-resend: when set, every message from
/// that index on (including the one being edited) is dropped before the new
/// pair is appended, and the agent re-seeds from the surviving prefix only.
///
/// `images` carries attached screenshots/photos as `data:image/…;base64,…`
/// URLs; the stored message keeps them (the transcript re-renders them) and
/// the outgoing turn re-derives binary parts from them. A turn may be
/// image-only: empty text with at least one valid image is a question in
/// pixels, and the model answers it.
#[tauri::command]
fn send_message(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    text: String,
    images: Option<Vec<String>>,
    truncate_from: Option<usize>,
) -> Result<Streaming, String> {
    let images = crate::session::sanitize_images(images.as_deref().unwrap_or(&[]))?;
    if text.trim().is_empty() && images.is_empty() {
        return Err("message is empty".to_string());
    }

    let (rx, streaming) = {
        let mut inner = state.lock()?;
        let mut record = inner
            .store
            .load_all()
            .into_iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| "session not found".to_string())?;

        if let Some(idx) = truncate_from {
            record.truncate_from(idx)?;
        }
        // Seed the transcript from the stored history so the agent sees it.
        let transcript = record.messages.clone();
        // Restore this session's plan into its own agent so `update_plan`
        // edits these steps (plans are per-session, never shared).
        inner.agent(&session_id).set_plan(record.plan.clone());
        record
            .messages
            .push(ChatMessage::user_with_images(text.clone(), images.clone()));
        record.messages.push(ChatMessage::assistant());
        record.touch();
        inner.store.save(&record).map_err(|e| e.to_string())?;
        begin_stream(&mut inner, &session_id, text, images, transcript)
    };

    spawn_forwarder(
        state.inner().clone(),
        app,
        rx,
        session_id,
        streaming.gen,
        streaming.job.clone(),
    );
    Ok(streaming)
}

/// Drop every message from `index` onward — the transcript half of the
/// message actions ("delete from here"). No stream starts; the surviving
/// prefix is what the next turn re-seeds from.
#[tauri::command]
fn truncate_session(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    index: usize,
) -> Result<usize, String> {
    let inner = state.lock()?;
    if inner.streaming.contains_key(&session_id) {
        return Err("a turn is in progress — wait for it to finish".to_string());
    }
    if inner.compacting.contains(&session_id) {
        return Err("a compaction is running for this chat".to_string());
    }
    let mut record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    record.truncate_from(index)?;
    record.touch();
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist: {err}"))?;
    Ok(record.messages.len())
}

/// How many turns this chat's last compaction folded away.
///
/// Zero means there is nothing to restore, which is what the UI checks before
/// offering the affordance at all.
#[tauri::command]
fn compacted_archive(state: State<'_, Arc<GuiState>>, session_id: String) -> Result<usize, String> {
    let inner = state.lock()?;
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    Ok(record.archived.len())
}

/// The turns this chat's last compaction folded, for reading before restoring.
///
/// Read-only: the UI shows them in the document panel so the reader can see
/// what a summary replaced, and restoring stays a separate, deliberate act.
#[tauri::command]
fn archived_messages(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<Vec<ChatMessage>, String> {
    let inner = state.lock()?;
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    Ok(record.archived)
}

/// Counts, size and turn outline for one chat, computed from its stored record.
///
/// Read-only, like the other inspection commands: it answers "what is in here"
/// without touching the conversation.
#[tauri::command]
fn session_overview(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::session::SessionOverview, String> {
    let inner = state.lock()?;
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    Ok(record.overview())
}

/// Keep one chat as a ZIP beside the session store, returning where it landed.
///
/// The archive is written by the core so the shell only reports the path: an
/// export is a file the user owns, not a view the shell renders.
#[tauri::command]
fn export_chat(state: State<'_, Arc<GuiState>>, session_id: String) -> Result<String, String> {
    let inner = state.lock()?;
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    let path = crate::export::export_chat(inner.store.dir(), &record)?;
    Ok(path.to_string_lossy().into_owned())
}

/// A chat in the trash, as the settings panel needs to show it.
#[derive(Clone, Serialize)]
pub(crate) struct TrashedChat {
    /// Stable identifier, used to bring the chat back.
    pub(crate) id: String,
    /// Title, so a person can recognise which chat it was.
    pub(crate) title: String,
    /// Unix seconds when it was last touched before being deleted.
    pub(crate) updated_at: u64,
}

/// The chats that were deleted and can still be brought back, newest first.
#[tauri::command]
fn trashed_chats(state: State<'_, Arc<GuiState>>) -> Result<Vec<TrashedChat>, String> {
    let inner = state.lock()?;
    Ok(inner
        .store
        .trashed()
        .into_iter()
        .map(|record| TrashedChat {
            id: record.id,
            title: record.title,
            updated_at: record.updated_at,
        })
        .collect())
}

/// Bring a deleted chat back, returning its title.
///
/// The record's workspace field was never changed on the way out, so a restored
/// chat rejoins the workspace it belonged to.
#[tauri::command]
fn restore_session(state: State<'_, Arc<GuiState>>, id: String) -> Result<String, String> {
    let inner = state.lock()?;
    inner.store.restore(&id)
}

/// Empty the trash for good, returning how many chats were purged.
#[tauri::command]
fn purge_trash(state: State<'_, Arc<GuiState>>) -> Result<usize, String> {
    let inner = state.lock()?;
    Ok(inner.store.purge_trash())
}

/// Put a compacted chat's folded turns back, in front of everything since.
#[tauri::command]
fn restore_compacted(state: State<'_, Arc<GuiState>>, session_id: String) -> Result<usize, String> {
    let inner = state.lock()?;
    if inner.streaming.contains_key(&session_id) {
        return Err("a turn is in progress — wait for it to finish".to_string());
    }
    if inner.compacting.contains(&session_id) {
        return Err("a compaction is running for this chat".to_string());
    }
    let mut record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    let restored = crate::compact::restore(&mut record)?;
    record.touch();
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist the restored history: {err}"))?;
    Ok(restored)
}

/// Clone the transcript prefix (messages before `index`) into a brand-new
/// session — the non-destructive counterpart to [`truncate_session`]. The
/// fork (including the plan snapshot) is persisted immediately so the
/// sidebar can list and open it; the source record is never modified.
#[tauri::command]
fn fork_session(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    index: usize,
) -> Result<SessionRecord, String> {
    let inner = state.lock()?;
    if inner.streaming.contains_key(&session_id) {
        return Err("a turn is in progress — wait for it to finish".to_string());
    }
    if inner.compacting.contains(&session_id) {
        return Err("a compaction is running for this chat".to_string());
    }
    let record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    let fork = record.fork_from(index)?;
    inner
        .store
        .save(&fork)
        .map_err(|err| format!("failed to persist: {err}"))?;
    Ok(fork)
}

/// Peel the trailing exchange (assistant reply + its user prompt) off a
/// transcript and re-seed the placeholders for a fresh reply.
///
/// Returns the user text so the caller can re-run it. The record is only
/// modified once the exchange is known to be peelable, so a failed peel
/// leaves the transcript untouched.
fn peel_exchange(record: &mut SessionRecord) -> Result<(String, Vec<String>), String> {
    let n = record.messages.len();
    if n < 2 {
        return Err("nothing to regenerate".to_string());
    }
    let (user, last) = (&record.messages[n - 2], &record.messages[n - 1]);
    if user.role != Role::User || last.role != Role::Assistant {
        return Err("nothing to regenerate".to_string());
    }
    let text = user.text.clone();
    let images = user.images.clone();
    if text.trim().is_empty() && images.is_empty() {
        return Err("message is empty".to_string());
    }
    record.messages.truncate(n - 2);
    record
        .messages
        .push(ChatMessage::user_with_images(text.clone(), images.clone()));
    record.messages.push(ChatMessage::assistant());
    Ok((text, images))
}

/// Drop the last exchange and re-run its prompt (regenerate the reply).
#[tauri::command]
fn retry_last(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<Streaming, String> {
    let (rx, streaming) = {
        let mut inner = state.lock()?;
        if inner.streaming.contains_key(&session_id) {
            return Err("this chat is already streaming".to_string());
        }
        let mut record = inner
            .store
            .load_all()
            .into_iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| "session not found".to_string())?;

        // Transcript for the re-run excludes the dropped exchange.
        let (text, images) = peel_exchange(&mut record)?;
        let transcript = record.messages.clone();
        // Same plan restore as `send_message`: the re-run belongs to this
        // session, so its own agent holds the persisted plan.
        inner.agent(&session_id).set_plan(record.plan.clone());
        record.touch();
        inner.store.save(&record).map_err(|e| e.to_string())?;
        begin_stream(&mut inner, &session_id, text, images, transcript)
    };

    spawn_forwarder(
        state.inner().clone(),
        app,
        rx,
        session_id,
        streaming.gen,
        streaming.job.clone(),
    );
    Ok(streaming)
}

/// Open an http(s) link in the system browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("only http(s) URLs can be opened".to_string());
    }
    std::process::Command::new("open")
        .arg(trimmed)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Fuzzy-search workspace files for the composer's `@`-mention picker.
///
/// Ranks workspace-relative paths with `mentions::fuzzy_score` (boundary-
/// and run-aware subsequence scoring) and returns up to `limit` matches,
/// clamped to 1..=50. An empty or missing `bash_workspace` yields an empty
/// list — the picker simply stays closed rather than erroring.
#[tauri::command]
fn search_files(
    state: State<'_, Arc<GuiState>>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let inner = state.lock()?;
    let root = std::path::Path::new(inner.settings.bash_workspace.trim());
    if root.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(8).clamp(1, 50);
    Ok(crate::mentions::search_files(root, query.trim(), limit))
}

/// Largest file the document panel reads in one go (2 MiB).
///
/// The panel is for reading, not for loading whole trees into memory, so the
/// cap stays explicit and the read is bounded rather than post-truncated.
const VIEW_MAX_BYTES: usize = 2 * 1024 * 1024;

/// One bounded, workspace-confined file read for the document panel.
#[derive(serde::Serialize)]
struct FileView {
    /// Workspace-relative path, `/`-separated.
    path: String,
    /// File text; a prefix of the file when `truncated` is set.
    text: String,
    /// Size on disk in bytes.
    bytes: u64,
    /// Whether `text` is shorter than the file.
    truncated: bool,
}

/// Resolve a workspace-relative path to a readable file.
///
/// Escapes are refused rather than sanitised: `..`, absolute paths and symlinks
/// that point outside the workspace all fail, as do directories and missing
/// files. Returns the canonical path and its size on disk.
fn resolve_workspace_file(
    root: &std::path::Path,
    rel: &str,
) -> Result<(std::path::PathBuf, u64), String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("no path given".into());
    }
    if root.as_os_str().is_empty() {
        return Err("no workspace configured".into());
    }
    if rel.split(['/', '\\']).any(|part| part == "..") {
        return Err("path escapes the workspace".into());
    }
    let canon_root = root
        .canonicalize()
        .map_err(|err| format!("workspace unavailable: {err}"))?;
    let canon = canon_root
        .join(rel)
        .canonicalize()
        .map_err(|_| format!("not found: {rel}"))?;
    if !canon.starts_with(&canon_root) {
        return Err("path escapes the workspace".into());
    }
    let meta = std::fs::metadata(&canon).map_err(|err| format!("not readable: {err}"))?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    Ok((canon, meta.len()))
}

/// Read a workspace file for the right-hand document panel.
///
/// The path is workspace-relative and confined to the configured workspace:
/// escapes, directories, missing files, binary payloads and non-UTF-8 data are
/// refused with a message the UI can show. At most [`VIEW_MAX_BYTES`] are
/// returned, with an explicit `truncated` flag — an omission the UI states
/// rather than hides. The panel renders the text inertly.
#[tauri::command]
fn read_file(state: State<'_, Arc<GuiState>>, path: String) -> Result<FileView, String> {
    let root = {
        let inner = state.lock()?;
        std::path::PathBuf::from(inner.settings.bash_workspace.trim())
    };
    let (canon, bytes) = resolve_workspace_file(&root, &path)?;
    let shown = {
        let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        canon
            .strip_prefix(&canon_root)
            .unwrap_or(&canon)
            .to_string_lossy()
            .replace('\\', "/")
    };
    use std::io::Read as _;
    let file = std::fs::File::open(&canon).map_err(|err| format!("read failed: {err}"))?;
    let mut buf = Vec::with_capacity(64 * 1024);
    file.take(VIEW_MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|err| format!("read failed: {err}"))?;
    if buf.iter().take(8192).any(|b| *b == 0) {
        return Err(format!("{shown} looks binary; the panel shows text only"));
    }
    let truncated = buf.len() > VIEW_MAX_BYTES;
    buf.truncate(VIEW_MAX_BYTES);
    let text = match String::from_utf8(buf) {
        Ok(text) => text,
        Err(err) => {
            // Keep the valid prefix: a multi-byte character cut by the cap must
            // not turn the whole file into an encoding error.
            let valid = err.utf8_error().valid_up_to();
            if valid == 0 {
                return Err(format!("{shown} is not UTF-8 text"));
            }
            let mut bytes = err.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }
    };
    Ok(FileView {
        path: shown,
        text,
        bytes,
        truncated,
    })
}

/// One expansion of the file sidebar: a single directory, sorted and sized.
///
/// Same confinement as `read_file` (an escape is refused, not sanitised) and
/// the same display policy as the `@`-mention walker, because both now live
/// in [`crate::filetree`] — what the sidebar shows and what `@` can mention
/// are one list, so the two cannot drift apart.
#[tauri::command]
fn list_files(
    state: State<'_, Arc<GuiState>>,
    path: String,
) -> Result<crate::filetree::FileList, String> {
    let root = {
        let inner = state.lock()?;
        std::path::PathBuf::from(inner.settings.bash_workspace.trim())
    };
    crate::filetree::list_dir(&root, &path)
}

/// Result payload of the `run-line` event: one line of one run.
#[derive(Clone, Serialize)]
struct RunLineEvent {
    /// The run the line belongs to, so two runs never mix in the panel.
    run_id: String,
    /// Which stream the line came from: `"stdout"` or `"stderr"`.
    stream: String,
    /// The line text, already clamped by the runner.
    text: String,
}

/// Result payload of the `run-finished` event: the outcome of one run.
#[derive(Clone, Serialize)]
struct RunFinishedEvent {
    /// The run this summarizes.
    run_id: String,
    /// The process exit code, when the child exited on its own.
    exit_code: Option<i32>,
    /// Wall-clock milliseconds from spawn to the last line.
    duration_ms: u64,
    /// How many lines were shown.
    lines: usize,
    /// Whether the run hit a budget (line cap or deadline) and was stopped.
    truncated: bool,
    /// Set when the child could not be started or did not exit cleanly.
    error: Option<String>,
}

/// Run a shell command in the workspace and stream its output to the panel.
///
/// Deliberately **not** a terminal: the command runs against pipes, so there
/// is no interactivity and no ANSI colour. That is the point — this is the
/// bounded path for a one-shot command, with output held by
/// [`crate::runs`]'s line budget, per-line clamp and deadline, and the panel
/// keeps what it showed across a reload. Interactive shells live in the
/// panel's terminal mode behind [`pty_start`].
#[tauri::command]
fn run_command(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    run_id: String,
    command: String,
) -> Result<(), String> {
    let root = {
        let inner = state.lock()?;
        std::path::PathBuf::from(inner.settings.bash_workspace.trim())
    };
    if root.as_os_str().is_empty() {
        return Err("no workspace configured".to_string());
    }
    let command = crate::runs::validate(&command)?;
    let run_id = run_id.trim().to_string();
    if run_id.is_empty() || run_id.len() > 128 {
        return Err("bad run id".to_string());
    }
    std::thread::spawn(move || {
        let line_app = app.clone();
        let id = run_id.clone();
        let summary = crate::runs::run_streaming(
            &command,
            &root,
            Default::default(),
            crate::runs::RUN_TIMEOUT,
            move |line| {
                let payload = RunLineEvent {
                    run_id: id.clone(),
                    stream: line.stream.to_string(),
                    text: line.text,
                };
                let _ = line_app.emit("run-line", payload);
            },
        );
        let payload = RunFinishedEvent {
            run_id,
            exit_code: summary.exit_code,
            duration_ms: summary.duration_ms,
            lines: summary.lines,
            truncated: summary.truncated,
            error: summary.error,
        };
        let _ = app.emit("run-finished", payload);
    });
    Ok(())
}

/// Result payload of the `pty-output` event: one read from a session,
/// escape sequences already stripped by the backend.
#[derive(Clone, Serialize)]
struct PtyOutputEvent {
    /// The session the text came from, so two sessions never mix.
    session_id: String,
    /// The text of this read; printable bytes only.
    text: String,
}

/// Result payload of the `pty-exit` event: a session's process ended.
#[derive(Clone, Serialize)]
struct PtyExitEvent {
    /// The session that ended.
    session_id: String,
    /// Its exit code; `None` when it died on a signal or could not be reaped.
    exit_code: Option<i32>,
}

/// What `pty_start` reports; the panel uses it to replay a re-attach.
#[derive(Clone, Serialize)]
struct PtyStatus {
    /// True when this call spawned a new shell (false = re-attached).
    fresh: bool,
    /// The shell program the session runs.
    shell: String,
    /// Recent output, oldest first — everything since the last reload.
    tail: Vec<String>,
}

/// Start a PTY session for the workspace, or re-attach to the running one.
///
/// The session id is chosen by the panel (one per workspace root) and the
/// process lives on the Rust side, so a webview reload re-attaches with the
/// tail replayed instead of losing the shell. Output arrives as `pty-output`
/// events with escape sequences already stripped by [`crate::pty`]; the end
/// arrives as `pty-exit`.
#[tauri::command]
fn pty_start(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    hub: State<'_, crate::pty::PtyRegistry>,
    session_id: String,
    shell: Option<String>,
) -> Result<PtyStatus, String> {
    let root = {
        let inner = state.lock()?;
        std::path::PathBuf::from(inner.settings.bash_workspace.trim())
    };
    if root.as_os_str().is_empty() {
        return Err("no workspace configured".to_string());
    }
    let session_id = session_id.trim().to_string();
    if session_id.is_empty() || session_id.len() > 128 {
        return Err("bad session id".to_string());
    }
    let sink_app = app.clone();
    let sink = move |event: crate::pty::PtyEvent| match event {
        crate::pty::PtyEvent::Output { session_id, text } => {
            let _ = sink_app.emit("pty-output", PtyOutputEvent { session_id, text });
        }
        crate::pty::PtyEvent::Exit { session_id, code } => {
            let _ = sink_app.emit(
                "pty-exit",
                PtyExitEvent {
                    session_id,
                    exit_code: code,
                },
            );
        }
    };
    let report = hub.start(
        &session_id,
        shell.as_deref(),
        &root,
        crate::pty::DEFAULT_COLS,
        crate::pty::DEFAULT_ROWS,
        Arc::new(sink),
    )?;
    Ok(PtyStatus {
        fresh: report.fresh,
        shell: report.shell,
        tail: report.tail,
    })
}

/// Send raw input to a PTY session (the line the panel typed, Enter included).
#[tauri::command]
fn pty_write(
    hub: State<'_, crate::pty::PtyRegistry>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    hub.write(session_id.trim(), &data)
}

/// Resize a session's window; the backend clamps to a sane range.
#[tauri::command]
fn pty_resize(
    hub: State<'_, crate::pty::PtyRegistry>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    hub.resize(session_id.trim(), cols, rows)
}

/// Kill a PTY session's process; its printed tail stays until restart.
#[tauri::command]
fn pty_kill(hub: State<'_, crate::pty::PtyRegistry>, session_id: String) -> Result<(), String> {
    hub.kill(session_id.trim())
}

#[cfg(test)]
mod file_view_tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("bos-gui-view-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn reads_a_workspace_file_and_reports_its_size() {
        let root = temp_root("read");
        std::fs::write(root.join("note.md"), "hello\nworld\n").unwrap();
        let (canon, bytes) = resolve_workspace_file(&root, "note.md").expect("resolves");
        assert!(canon.ends_with("note.md"));
        assert_eq!(bytes, 12);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_escapes_absolute_paths_and_directories() {
        let root = temp_root("escape");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/inner.txt"), "x").unwrap();
        assert!(resolve_workspace_file(&root, "../etc/passwd").is_err());
        assert!(resolve_workspace_file(&root, "sub/../../outside").is_err());
        assert!(resolve_workspace_file(&root, "/etc/passwd").is_err());
        assert!(resolve_workspace_file(&root, "sub").is_err());
        assert!(resolve_workspace_file(&root, "").is_err());
        // A legal nested relative path still works.
        assert!(resolve_workspace_file(&root, "sub/inner.txt").is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_missing_files_and_a_missing_workspace() {
        let root = temp_root("missing");
        assert!(resolve_workspace_file(&root, "nope.txt").is_err());
        assert!(resolve_workspace_file(std::path::Path::new(""), "nope.txt").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlink_out_of_the_workspace_is_refused() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        #[cfg(unix)]
        assert!(resolve_workspace_file(&root, "link.txt").is_err());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}

/// Abort in-flight streams; each forwarder persists its own partial turn.
///
/// With `session_id` only that chat stops and only *its* pending approvals
/// are denied; `None` stops every stream. Streaming entries stay in the map
/// until their forwarder finalizes — removing them early would let an aborted
/// turn race a re-send's fresh placeholder.
#[tauri::command]
fn stop_streaming(
    state: State<'_, Arc<GuiState>>,
    session_id: Option<String>,
) -> Result<(), String> {
    let mut inner = state.lock()?;
    match session_id {
        Some(id) => {
            if let Some(broker) = inner.broker(&id) {
                broker.reject_all();
            }
            inner.runner.stop(&id);
            // The stream's job settles as interrupted, matching what the user
            // asked for, rather than lingering as "running" forever.
            let job = inner.streaming.get(&id).map(|stream| stream.job.clone());
            if let Some(job) = job {
                let _ = inner.jobs.interrupt(&job);
            }
        }
        None => {
            for broker in &inner.brokers {
                broker.reject_all();
            }
            inner.runner.stop_all();
            let jobs: Vec<String> = inner
                .streaming
                .values()
                .map(|stream| stream.job.clone())
                .collect();
            for job in jobs {
                let _ = inner.jobs.interrupt(&job);
            }
        }
    }
    Ok(())
}

/// Result payload of the `compact-finished` event.
#[derive(Clone, Serialize)]
struct CompactFinished {
    /// Session that was compacted.
    session_id: String,
    /// Whether the history was rewritten; on `false` the history is intact
    /// and `error` explains what went wrong.
    ok: bool,
    /// Messages in the history before the attempt.
    before: usize,
    /// Messages after (equals `before` when `ok` is false).
    after: usize,
    /// Failure description when `ok` is false.
    error: Option<String>,
}

/// Fold a session's history into an LLM-written summary (`/compact`).
///
/// Returns as soon as the history snapshot is taken; the summarization turn
/// (tools, skills, memory, MCP, and project instructions all disabled) runs
/// on a background thread and reports through the `compact-finished` event.
/// Refuses while the session streams or while a previous compaction for the
/// same chat is still in flight, and the rewrite re-validates under the lock
/// so a racing turn can never be clobbered — worst case the summary is
/// discarded and the event reports why.
#[tauri::command]
fn compact_session(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<(), String> {
    begin_compaction(app, state.inner(), &session_id)
}

/// Validate and start one compaction, reporting through `compact-finished`.
///
/// Shared by `/compact` ([`compact_session`]) and the M6 `compact_now`
/// command, so the two can never disagree about what a startable compaction
/// looks like.
fn begin_compaction(app: AppHandle, state: &Arc<GuiState>, session_id: &str) -> Result<(), String> {
    let (settings, messages, before) = {
        let mut inner = state.lock()?;
        if inner.streaming.contains_key(session_id) {
            return Err("a turn is in progress — wait for it to finish".to_string());
        }
        if !inner.compacting.insert(session_id.to_string()) {
            return Err("a compaction is already running for this chat".to_string());
        }
        let found = inner
            .store
            .load_all()
            .into_iter()
            .find(|s| s.id == session_id);
        let Some(record) = found else {
            inner.compacting.remove(session_id);
            return Err("session not found".to_string());
        };
        if let Some(reason) = crate::compact::refusal(&record.messages) {
            inner.compacting.remove(session_id);
            return Err(reason);
        }
        let before = record.messages.len();
        (inner.settings.clone(), record.messages, before)
    };

    let state = Arc::clone(state);
    let session_id = session_id.to_string();
    std::thread::spawn(move || {
        let outcome = run_compaction(&state, &session_id, &settings, &messages);
        // Release the in-flight marker before announcing, so an immediate
        // retry from the event handler is not rejected as a duplicate.
        if let Ok(mut inner) = state.lock() {
            inner.compacting.remove(&session_id);
        }
        let payload = match outcome {
            Ok((new_before, new_after)) => CompactFinished {
                session_id: session_id.clone(),
                ok: true,
                before: new_before,
                after: new_after,
                error: None,
            },
            Err(error) => CompactFinished {
                session_id: session_id.clone(),
                ok: false,
                before,
                after: before,
                error: Some(error),
            },
        };
        let _ = app.emit("compact-finished", payload);
    });
    Ok(())
}

/// Background body of [`compact_session`]: summarize, re-validate, rewrite.
///
/// Runs on the spawning thread (never the Tauri main thread), holds the
/// state lock only for the final read-modify-write, and returns
/// `(before, after)` message counts for the success event.
fn run_compaction(
    state: &Arc<GuiState>,
    session_id: &str,
    settings: &Settings,
    messages: &[ChatMessage],
) -> Result<(usize, usize), String> {
    let summary = crate::compact::summarize(settings, messages)?;
    let inner = state.lock()?;
    if inner.streaming.contains_key(session_id) {
        return Err("a new turn started during compaction — summary discarded".to_string());
    }
    let mut record = inner
        .store
        .load_all()
        .into_iter()
        .find(|s| s.id == session_id)
        .ok_or_else(|| "session not found".to_string())?;
    let counts = crate::compact::apply(&mut record, summary)?;
    inner
        .store
        .save(&record)
        .map_err(|err| format!("failed to persist the compacted history: {err}"))?;
    Ok(counts)
}

/// Result payload of the `init-finished` event.
#[derive(Clone, Serialize)]
struct InitFinished {
    /// Whether `AGENTS.md` was written; on `false` `error` explains why.
    ok: bool,
    /// Absolute path of the written file (present on success).
    path: Option<String>,
    /// Failure description when `ok` is false.
    error: Option<String>,
}

/// Generate `AGENTS.md` for the bash workspace (`/init`).
///
/// Returns as soon as the in-flight guard is taken; the tool-less probe
/// turn runs on a background thread and reports through the `init-finished`
/// event. Refuses while a previous generation is still running, while the
/// workspace is not a directory, and whenever `AGENTS.md` already exists
/// unless `force` is set — an existing instruction file is never clobbered.
#[tauri::command]
fn init_agents(app: AppHandle, state: State<'_, Arc<GuiState>>, force: bool) -> Result<(), String> {
    let (settings, root) = {
        let mut inner = state.lock()?;
        if inner.init_in_flight {
            return Err("an AGENTS.md generation is already running".to_string());
        }
        let ws = inner.settings.bash_workspace.trim();
        if ws.is_empty() {
            return Err("no bash workspace set — pick one in Settings first".to_string());
        }
        // Same expansion the instruction loader uses, so the generated file
        // lands exactly where `agent_config` will look for it.
        let root = std::path::PathBuf::from(shellexpand::tilde(ws).into_owned());
        if let Some(reason) = crate::init::refusal(&root, force) {
            return Err(reason);
        }
        inner.init_in_flight = true;
        (inner.settings.clone(), root)
    };
    let state = Arc::clone(state.inner());
    std::thread::spawn(move || {
        let outcome = crate::init::generate(&settings, &root)
            .and_then(|body| crate::init::write(&root, &body));
        // Release the guard before announcing so an immediate retry from the
        // event handler is never rejected as a duplicate.
        if let Ok(mut inner) = state.lock() {
            inner.init_in_flight = false;
        }
        let payload = match outcome {
            Ok(path) => InitFinished {
                ok: true,
                path: Some(path.display().to_string()),
                error: None,
            },
            Err(error) => InitFinished {
                ok: false,
                path: None,
                error: Some(error),
            },
        };
        let _ = app.emit("init-finished", payload);
    });
    Ok(())
}

/// Answer a pending approval request (`approval-request` event payload id).
#[tauri::command]
/// Record an approval allowance for a tool. `scope` is `"session"` (until the
/// session changes) or `"always"` (persisted). Returns the persistent list, so
/// the caller can show it without a second round trip.
fn allow_tool(
    state: State<'_, Arc<GuiState>>,
    tool: String,
    scope: String,
) -> Result<Vec<String>, String> {
    let mut inner = state.lock()?;
    let tool = tool.trim().to_string();
    if tool.is_empty() {
        return Err("no tool name given".to_string());
    }
    // Every broker ever built, so an allowance reaches cached sessions too.
    for broker in &inner.brokers {
        if scope == "always" {
            broker.allow_always(&tool);
        } else {
            broker.allow_for_session(&tool);
        }
    }
    if scope == "always" && !inner.settings.allowed_tools.contains(&tool) {
        inner.settings.allowed_tools.push(tool);
        inner.settings.allowed_tools.sort();
        inner.settings.save().map_err(|e| e.to_string())?;
    }
    Ok(inner.settings.allowed_tools.clone())
}

#[tauri::command]
/// Take a tool off the permanent allow-list, so it asks again.
fn forget_allowed_tool(
    state: State<'_, Arc<GuiState>>,
    tool: String,
) -> Result<Vec<String>, String> {
    let mut inner = state.lock()?;
    inner.settings.allowed_tools.retain(|name| name != &tool);
    for broker in &inner.brokers {
        broker.disallow(&tool);
    }
    inner.settings.save().map_err(|e| e.to_string())?;
    Ok(inner.settings.allowed_tools.clone())
}

#[tauri::command]
fn respond_approval(
    state: State<'_, Arc<GuiState>>,
    id: String,
    approved: bool,
) -> Result<(), String> {
    let inner = state.lock()?;
    // Try every broker ever built: the request may belong to a session whose
    // entry was rebuilt since. A stale id simply means the stream already
    // resolved or timed out.
    for broker in &inner.brokers {
        if broker.resolve(&id, approved) {
            break;
        }
    }
    Ok(())
}

/// Read the current settings for the settings dialog.
#[tauri::command]
fn get_settings(state: State<'_, Arc<GuiState>>) -> Result<Settings, String> {
    // The dialog reads what will actually run, so it cannot show a value the
    // active workspace overrides (§24.4).
    Ok(state.lock()?.settings.effective())
}

/// Validate a workspace root the way file reads are validated: it must exist and
/// be a directory, and what gets stored is canonical, so `~`, `..` and symlinks
/// cannot turn one folder into two workspaces (§24.7). An empty root means the
/// process working directory and is always acceptable.
fn normalize_workspace_root(root: &str) -> Result<String, String> {
    let root = root.trim();
    if root.is_empty() {
        return Ok(String::new());
    }
    let path = std::path::PathBuf::from(shellexpand::tilde(root).into_owned());
    let meta = std::fs::metadata(&path).map_err(|e| format!("{root}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("{root} is not a directory"));
    }
    let canon = path.canonicalize().map_err(|e| format!("{root}: {e}"))?;
    Ok(canon.to_string_lossy().into_owned())
}

/// Create a workspace, with everything the "Add a workspace" dialog collected.
///
/// A workspace starts from the current defaults, so a new one behaves like
/// today's installation until it is configured otherwise — and `config` is how
/// it is configured *at* creation, in one step, rather than being created wrong
/// and edited right. A draft naming something that does not exist is refused
/// before the workspace is added, so a failed creation leaves nothing behind —
/// and the folder is given its own `.bos` configuration directory before the
/// record exists anywhere (§24.15), so a folder that cannot take one is refused
/// with the system's reason rather than becoming a workspace without a home.
#[tauri::command]
fn create_workspace(
    state: State<'_, Arc<GuiState>>,
    name: String,
    root: String,
    config: Option<crate::settings::WorkspaceDraft>,
) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    create_workspace_in(&mut inner, &name, &root, config.as_ref())
}

/// [`create_workspace`]'s body, separated so a test can drive it without a
/// Tauri `State` — and so a refused draft can be shown to add nothing.
fn create_workspace_in(
    inner: &mut Inner,
    name: &str,
    root: &str,
    config: Option<&crate::settings::WorkspaceDraft>,
) -> Result<Settings, String> {
    create_workspace_record(inner, name, root, config)?;
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
}

/// Everything a creation does except write the settings file: build the record,
/// prepare the folder it is going to live in, and put it in place.
///
/// Separated for the same reason as `drafted_workspace` — the settings write is
/// the one step a test must not reach, because it would rewrite the user's
/// registry — and because the order is the promise: the folder's `.bos` is
/// written **before** the record is pushed, so a root that will not take one is
/// refused with nothing added, not after the fact.
fn create_workspace_record(
    inner: &mut Inner,
    name: &str,
    root: &str,
    config: Option<&crate::settings::WorkspaceDraft>,
) -> Result<(), String> {
    let w = drafted_workspace(&inner.settings, name, root, config)?;
    crate::workspace_dir::write_for(&w)?;
    inner.settings.active_workspace = w.id.clone();
    inner.settings.workspaces.push(w);
    Ok(())
}

/// Build the workspace a creation asks for, without adding it anywhere.
///
/// Separated from [`create_workspace_in`] so the two promises can be checked
/// without a settings file: a valid draft lands whole, and a draft naming an
/// MCP server this installation does not have is refused *before* anything is
/// added — the caller pushes only what this returns.
fn drafted_workspace(
    settings: &crate::settings::Settings,
    name: &str,
    root: &str,
    config: Option<&crate::settings::WorkspaceDraft>,
) -> Result<crate::settings::Workspace, String> {
    let root = normalize_workspace_root(root)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::session::unix_now();
    let mut w = crate::settings::Workspace::adopted_from(settings, &id, now);
    w.name = if name.trim().is_empty() {
        if root.is_empty() {
            "Default".to_string()
        } else {
            root.clone()
        }
    } else {
        name.trim().to_string()
    };
    w.root = root;
    if let Some(draft) = config {
        w.apply_draft(draft)?;
    }
    Ok(w)
}

/// List the sub-folders of `path`, for the folder chooser behind
/// "Add a workspace".
///
/// Deliberately reads outside the current workspace: choosing a root is what
/// happens *before* a workspace exists, so there is nothing to be contained in
/// yet. It returns directory names and resolved paths only, never a file's
/// contents, and refuses anything that is not an existing directory.
#[tauri::command]
fn list_folder(path: Option<String>) -> Result<crate::folders::FolderListing, String> {
    crate::folders::list(path.as_deref())
}

/// Ask the platform for a folder — the system's own panel, not ours.
///
/// Returns the chosen folder, or `null` when the user dismissed the panel (a
/// dismissal is a decision, not an error: the field must keep whatever it had).
/// An error means this system has no panel to ask, and the caller is expected to
/// fall back to the in-app browser rather than leave the user stuck.
///
/// The path the panel returns is **resolved here**, through the same
/// `folders::resolve` the typed and browsed paths go through, so no caller can
/// receive a root that is not a folder or that points at a different folder than
/// it names. `spawn_blocking` is not decoration: the panel is modal and the
/// command must not hold a worker of the async runtime while a human decides.
#[tauri::command]
async fn choose_folder(
    title: Option<String>,
    start: Option<String>,
) -> Result<Option<String>, String> {
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Choose a folder for this workspace".to_string());
    let start = start
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let picked = tauri::async_runtime::spawn_blocking(move || {
        crate::picker::choose(&title, start.as_deref())
    })
    .await
    .map_err(|e| format!("the folder dialog task did not finish: {e}"))??;
    crate::folders::resolve_picked(picked)
}

/// Rename a workspace or move its root. Sessions stay put: a chat recorded in the
/// old folder keeps saying so, because its approvals and tool calls happened there.
/// The workspace's own `.bos/workspace.toml` is rewritten from the result — the
/// folder's copy is the one a clone would carry — and a write that fails puts the
/// record back, so the app and the file cannot disagree about what was saved.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn update_workspace(
    state: State<'_, Arc<GuiState>>,
    id: String,
    name: String,
    root: String,
    disabled_tools: Vec<String>,
    disabled_skills: Vec<String>,
    mcp_servers: Vec<crate::settings::McpServerEntry>,
    model: String,
    base_url: String,
    api_key: String,
    system_prompt: String,
    temperature: Option<f32>,
    reasoning_effort: String,
    memory_path: String,
    skills_dir: String,
    memory_enabled: Option<bool>,
    bash_enabled: Option<bool>,
    file_tools_enabled: Option<bool>,
    require_approval: Option<bool>,
    context_budget: Option<usize>,
    project_instructions: Option<bool>,
) -> Result<Settings, String> {
    // The panel round-trips the entries the server gave it, changing only
    // `enabled`, so revalidating them costs nothing and keeps the invariant that
    // a stored server is a valid one.
    crate::settings::validate_mcp_servers(&mcp_servers)?;
    let mut inner = state.lock()?;
    let root = normalize_workspace_root(&root)?;
    let runtime = crate::settings::WorkspaceRuntime {
        bash_enabled,
        file_tools_enabled,
        require_approval,
        context_budget,
        project_instructions,
        memory_enabled,
        model: model.to_string(),
        base_url: base_url.to_string(),
        api_key: api_key.to_string(),
        system_prompt: system_prompt.to_string(),
        temperature,
        reasoning_effort: reasoning_effort.to_string(),
        memory_path: memory_path.to_string(),
        skills_dir: skills_dir.to_string(),
    };
    rewrite_workspace(&mut inner, &id, &runtime, |w| {
        if !name.trim().is_empty() {
            w.name = name.trim().to_string();
        }
        w.root = root;
        // The deny-lists are the workspace's own selection (§24.8), and the names
        // are the ones the model sees, so the panel hands back exactly what it
        // showed.
        w.disabled_tools = disabled_tools;
        w.disabled_skills = disabled_skills;
        // Which MCP servers this workspace may use. A handshake that exists does
        // not attach unless its server is enabled here (§24.8, enforced at attach
        // time).
        w.mcp_servers = mcp_servers;
    })?;
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
}

/// Change one stored workspace and the configuration in its own folder, or
/// change neither.
///
/// The record is cloned before the first edit and put back when the folder
/// cannot take the write, because the file is where this workspace's
/// configuration lives: an app holding settings the folder never took is the
/// state §24.15 exists to prevent. The settings file is deliberately **not**
/// written here, so a test can drive the whole rule without rewriting the user's
/// registry — the command saves after this returns.
fn rewrite_workspace(
    inner: &mut Inner,
    id: &str,
    runtime: &crate::settings::WorkspaceRuntime,
    edit: impl FnOnce(&mut crate::settings::Workspace),
) -> Result<(), String> {
    let Some(backup) = inner
        .settings
        .workspaces
        .iter()
        .find(|w| w.id == id)
        .cloned()
    else {
        return Err(format!("unknown workspace '{id}'"));
    };
    inner.settings.set_workspace_runtime(id, runtime)?;
    let Some(w) = inner.settings.workspaces.iter_mut().find(|w| w.id == id) else {
        return Err(format!("unknown workspace '{id}'"));
    };
    edit(w);
    w.updated_at = crate::session::unix_now();
    let written = crate::workspace_dir::write_for(w);
    if let Err(e) = written {
        if let Some(slot) = inner.settings.workspaces.iter_mut().find(|x| x.id == id) {
            *slot = backup;
        }
        return Err(e);
    }
    Ok(())
}

/// Remove a workspace. The last one cannot go — the GUI always needs one to work
/// in — and its sessions are either deleted with it or re-homed to the workspace
/// that takes over, which the caller chooses explicitly (§24.6).
#[tauri::command]
fn remove_workspace(
    state: State<'_, Arc<GuiState>>,
    id: String,
    delete_sessions: bool,
) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    if !inner.settings.workspaces.iter().any(|w| w.id == id) {
        return Err(format!("unknown workspace '{id}'"));
    }
    if inner.settings.workspaces.len() <= 1 {
        return Err("the last workspace cannot be removed".to_string());
    }
    inner.settings.workspaces.retain(|w| w.id != id);
    inner.settings.active_workspace = inner.settings.workspaces[0].id.clone();
    let fallback = inner.settings.active_workspace.clone();
    for mut record in inner.store.load_all() {
        if record.workspace != id {
            continue;
        }
        if delete_sessions {
            inner.store.delete(&record.id);
        } else {
            record.workspace = fallback.clone();
            let _ = inner.store.save(&record);
        }
    }
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
}

/// Save a workspace as a reusable preset, for offering when a workspace is
/// created. It copies the configuration, so the preset and the workspace it came
/// from drift apart from that moment on (§24.9).
#[tauri::command]
fn save_preset(
    state: State<'_, Arc<GuiState>>,
    workspace_id: String,
    name: String,
) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    inner.settings.save_preset(&workspace_id, &name)?;
    inner.settings.save().map_err(|e| e.to_string())?;
    // No generation bump: presets are a catalogue, not a capability of the agent
    // currently running, so no agent needs rebuilding.
    Ok(inner.settings.effective())
}

/// Create a workspace from a preset and switch to it. The result is a copy, so
/// editing it never rewrites the preset or any other workspace made from it.
#[tauri::command]
fn apply_preset(
    state: State<'_, Arc<GuiState>>,
    id: String,
    name: String,
) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    inner.settings.apply_preset(&id, &name)?;
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
}

/// Switch the workspace the GUI is using. Sessions, the LLM binding, tools,
/// skills, MCP servers and memory all follow it, because the agents are rebuilt
/// from the effective settings of the newly active workspace (§24).
#[tauri::command]
fn set_active_workspace(state: State<'_, Arc<GuiState>>, id: String) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    let id = id.trim().to_string();
    if !inner.settings.workspaces.iter().any(|w| w.id == id) {
        return Err(format!("unknown workspace '{id}'"));
    }
    if inner.settings.active_workspace == id {
        return Ok(inner.settings.clone());
    }
    inner.settings.active_workspace = id;
    inner.settings.save().map_err(|e| e.to_string())?;
    // Approvals belong to the agents of the old workspace: deny them before the
    // rebuild, exactly as a settings save does.
    for broker in &inner.brokers {
        broker.reject_all();
    }
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.clone())
}

/// Copy the globals the settings dialog owns onto the workspace in use, and put
/// the same answer in that workspace's own `.bos` (§24.15).
///
/// The globals are refreshed as well, because they are what a *new* workspace
/// starts from; these values are what this workspace actually runs with. The folder
/// write is not decoration: the file is the source of truth for the fields it
/// carries, so a registry that moved on while the folder kept the old answer would
/// be silently reverted at the next load — and a folder that refuses the write is a
/// save that did not happen, reported with the folder's own reason.
///
/// The tool and skill deny-lists are deliberately absent: they are the workspace's
/// own selection, and overwriting them here would silently re-enable what the user
/// switched off.
fn push_globals_to_active_workspace(inner: &mut Inner) -> Result<(), String> {
    let Some(idx) = inner
        .settings
        .workspaces
        .iter()
        .position(|w| w.id == inner.settings.active_workspace)
    else {
        return Ok(());
    };
    let g = inner.settings.clone();
    let now = crate::session::unix_now();
    let w = &mut inner.settings.workspaces[idx];
    // A workspace named after its folder follows the folder when it moves.
    let follows_root = w.name == w.root;
    w.model = g.model.clone();
    w.base_url = g.base_url.clone();
    w.api_key = g.api_key.clone();
    w.system_prompt = g.system_prompt.clone();
    w.temperature = Some(g.temperature);
    w.reasoning_effort = g.reasoning_effort.clone();
    w.project_instructions = g.project_instructions;
    w.bash_enabled = g.bash_enabled;
    w.file_tools_enabled = g.file_tools_enabled;
    w.root = g.bash_workspace.clone();
    w.skills_dir = g.skills_dir.clone();
    w.memory_enabled = g.memory_enabled;
    w.memory_path = g.memory_path.clone();
    w.require_approval = g.require_approval;
    w.context_budget = g.context_budget;
    w.mcp_servers = g.mcp_servers.clone();
    w.providers = g.providers.clone();
    if follows_root {
        w.name = w.root.clone();
    }
    w.updated_at = now;
    crate::workspace_dir::write_for(&inner.settings.workspaces[idx])?;
    Ok(())
}

/// Apply and persist settings; the agent cache is invalidated and any MCP
/// servers reconnect in the background. The workspace in use is written along with
/// the globals it was filled from — see
/// [`push_globals_to_active_workspace`] — so this dialog cannot move the registry
/// on while the workspace's own folder keeps the older answer.
///
/// [`push_globals_to_active_workspace`]: push_globals_to_active_workspace
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn save_settings(
    state: State<'_, Arc<GuiState>>,
    model: String,
    base_url: String,
    api_key: String,
    system_prompt: String,
    project_instructions: bool,
    temperature: f32,
    reasoning_effort: Option<String>,
    bash_enabled: bool,
    file_tools_enabled: bool,
    bash_workspace: String,
    skills_dir: String,
    memory_enabled: bool,
    memory_path: String,
    require_approval: bool,
    context_budget: usize,
    web_tools_enabled: bool,
    search_api_url: String,
    search_api_key: String,
    mcp_servers: Vec<crate::settings::McpServerEntry>,
    providers: Vec<crate::settings::ProviderEntry>,
) -> Result<Settings, String> {
    // Reject a bad server or provider list before touching any state.
    crate::settings::validate_mcp_servers(&mcp_servers)?;
    crate::settings::validate_providers(&providers)?;
    let connect = mcp_servers.iter().any(|s| s.enabled);
    let mut inner = state.lock()?;
    if !model.trim().is_empty() {
        inner.settings.model = model.trim().to_string();
    }
    if !base_url.trim().is_empty() {
        inner.settings.base_url = base_url.trim().to_string();
    }
    inner.settings.api_key = api_key.trim().to_string();
    inner.settings.system_prompt = system_prompt;
    inner.settings.project_instructions = project_instructions;
    inner.settings.temperature = temperature.clamp(0.0, 2.0);
    let effort = reasoning_effort.unwrap_or_default().trim().to_lowercase();
    inner.settings.reasoning_effort = if effort.is_empty() {
        None
    } else {
        Some(effort)
    };
    inner.settings.bash_enabled = bash_enabled;
    inner.settings.file_tools_enabled = file_tools_enabled;
    inner.settings.bash_workspace = bash_workspace.trim().to_string();
    inner.settings.skills_dir = skills_dir.trim().to_string();
    inner.settings.memory_enabled = memory_enabled;
    inner.settings.memory_path = memory_path.trim().to_string();
    inner.settings.require_approval = require_approval;
    inner.settings.context_budget = context_budget;
    inner.settings.web_tools_enabled = web_tools_enabled;
    inner.settings.search_api_url = search_api_url.trim().to_string();
    inner.settings.search_api_key = search_api_key.trim().to_string();
    inner.settings.mcp_servers = mcp_servers;
    inner.settings.providers = providers;
    push_globals_to_active_workspace(&mut inner)?;
    inner.settings.save().map_err(|e| e.to_string())?;
    // Pending approvals belong to the old agents: deny them before rebuild.
    for broker in &inner.brokers {
        broker.reject_all();
    }
    // Persist the raw model string (a profile name survives reload and is
    // re-resolved there), but hand the resolved endpoint to the agent now.
    inner.settings.resolve_profile();
    inner.settings_gen += 1;
    inner.agent_cache.clear();
    // The MCP set may have changed: the next connect pass re-learns it, and
    // in the meantime fresh agents must not attach the old handshakes.
    inner.mcp_ready.clear();
    let gen = inner.settings_gen;
    // Show every configured server as pending until the connect pass reports.
    inner.mcp_connecting = connect;
    inner.mcp_status = inner
        .settings
        .mcp_servers
        .iter()
        .map(|s| crate::mcp::McpStatus {
            name: s.name.trim().to_string(),
            transport: s.transport.clone(),
            enabled: s.enabled,
            connected: false,
            tools: 0,
            tool_names: Vec::new(),
            error: None,
        })
        .collect();
    let saved = inner.settings.clone();
    drop(inner);
    if connect {
        spawn_mcp_connect(state.inner().clone(), gen);
    }
    Ok(saved)
}

/// Names of the MCP servers a workspace allows. A server is added by hand, so it
/// is opt-in per workspace: a handshake that exists globally does not ride into a
/// workspace that switched it off (§24.8).
fn enabled_mcp_servers(settings: &Settings) -> Vec<String> {
    settings
        .mcp_servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.clone())
        .collect()
}

/// List the registered tools, skills, and plugins for the current settings.
///
/// The panel is settings-scoped, so this reports from a fresh throwaway
/// build (MCP handshakes attached) rather than any session's cached agent.
#[tauri::command]
fn list_capabilities(
    state: State<'_, Arc<GuiState>>,
    session: Option<String>,
) -> Result<Capabilities, String> {
    let inner = state.lock()?;
    let broker = Arc::new(crate::approval::ApprovalBroker::default());
    let effective = inner.settings.effective();
    // With a chat open this describes *that chat's* agent, so a skill the chat
    // switched off is absent from the panel and from the palette that reads it.
    // A chat's own list is a scope, and a scope the inventory does not follow is
    // one the inventory lies about.
    let skill_deny = session
        .as_deref()
        .filter(|id| !id.is_empty())
        .and_then(|id| inner.session_skill_deny(id));
    let base = caps::build_agent_with(&effective, Arc::clone(&broker), skill_deny.as_deref());
    let mut agent = base.as_ref().clone();
    inner.attach_mcp_tools(&mut agent, &broker, &effective);
    // Describe the effective set, so the panel reports what this workspace will
    // actually run with.
    Ok(caps::capabilities_of(&Arc::new(agent), &effective))
}

/// What the policy says about every tool the connected servers advertise.
///
/// The pane reads this instead of deriving decisions from the rules, so what it
/// prints is what the last attach act did.
fn policy_report(inner: &Inner, now_ms: u64) -> Result<crate::mcp_policy::Report, String> {
    let store = inner
        .mcp_policy
        .lock()
        .map_err(|_| "the MCP policy lock is poisoned".to_string())?;
    let mut servers: Vec<(String, Vec<String>)> = inner
        .mcp_ready
        .iter()
        .map(|(name, handshake)| {
            (
                name.clone(),
                handshake.tools.iter().map(|d| d.name.clone()).collect(),
            )
        })
        .collect();
    servers.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(store.report(&servers, now_ms))
}

/// Rebuild every cached session agent from the current settings and policy,
/// keeping the derived MCP counts true.
///
/// Two calls need this, for the same reason: a change that is only honored on
/// the *next* build would leave the running session disagreeing with what the
/// user just did. An MCP policy change must remove a denied tool immediately
/// rather than at the next open; a newly authored skill must be available to a
/// session that is already open rather than after a restart. Both go through
/// [`Inner::build_agent_for`], so a re-attach cannot drift from a first build —
/// and a clone of the running agent could only ever add, never remove.
fn reattach_cached_agents(inner: &mut Inner) -> Vec<(String, String)> {
    let jobs: Vec<(String, Arc<crate::approval::ApprovalBroker>)> = inner
        .agent_cache
        .iter()
        .map(|(id, e)| (id.clone(), Arc::clone(&e.broker)))
        .collect();
    let mut errors = Vec::new();
    for (id, broker) in jobs {
        // A fresh build, not a clone of the running agent: removing a tool is
        // the whole point of re-attaching.
        let (fresh, mut mcp_errors) = inner.build_agent_for(&id, &broker);
        let fresh = Arc::new(fresh);
        errors.append(&mut mcp_errors);
        if let Some(reg) = fresh.registry() {
            let names = reg.async_tool_names();
            for status in inner.mcp_status.iter_mut() {
                let prefix = format!("{}_", status.name);
                status.tools = names.iter().filter(|n| n.starts_with(&prefix)).count();
            }
        }
        if let Some(entry) = inner.agent_cache.get_mut(&id) {
            entry.agent = fresh;
        }
    }
    errors
}

/// Rebuild **one** cached session agent, for a change that belongs to that
/// chat alone.
///
/// A skill activation is the first such change: switching a skill off in one
/// chat must not rebuild the other chats, because a rebuild swaps the `Arc` a
/// running stream is holding and there is no reason to disturb a chat whose
/// owner asked for nothing. Returns whether an agent was there to rebuild — a
/// chat nobody has opened yet needs no work, since its first build reads the
/// record.
fn rebuild_session_agent(inner: &mut Inner, session: &str) -> bool {
    let Some(broker) = inner
        .agent_cache
        .get(session)
        .map(|e| Arc::clone(&e.broker))
    else {
        return false;
    };
    // The same construction path as a first build and as a whole-cache
    // re-attach: a clone of the running agent could only ever add a skill back.
    let (fresh, _mcp_errors) = inner.build_agent_for(session, &broker);
    if let Some(entry) = inner.agent_cache.get_mut(session) {
        entry.agent = Arc::new(fresh);
    }
    true
}

/// The skills directory in force, expanded, and the workspace's denials.
fn skills_dir_and_denials(inner: &Inner) -> (std::path::PathBuf, Vec<String>) {
    let effective = inner.settings.effective();
    let dir = effective.skills_dir.trim();
    (
        crate::caps::expand_tilde(if dir.is_empty() { "." } else { dir }),
        effective.disabled_skills,
    )
}

/// The pane's answer for one chat, from the directory and the record.
fn activation_state(
    inner: &Inner,
    session: &str,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let session = session.trim();
    if session.is_empty() {
        return Err("no chat is open to switch a skill on for".to_string());
    }
    let (dir, denied) = skills_dir_and_denials(inner);
    let record = inner
        .skill_activation
        .lock()
        .map_err(|_| "the skill activation lock is poisoned".to_string())?;
    Ok(crate::skills::activation_report(
        &dir,
        &denied,
        session,
        &record,
        &inner.skill_activation_path,
        now_ms,
    ))
}

/// Switch one skill on or off for one chat, then make it take effect.
///
/// The record is written before the agent is rebuilt, and the rebuild happens
/// only when something actually changed: enabling a skill the chat already
/// carries is a no-op that must not freeze the chat's set, and rebuilding on it
/// would swap an `Arc` for nothing.
fn set_skill_active(
    inner: &mut Inner,
    session: &str,
    name: &str,
    on: bool,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let session = session.trim();
    if session.is_empty() {
        return Err("no chat is open to switch a skill on for".to_string());
    }
    let (dir, denied) = skills_dir_and_denials(inner);
    let (_, listed) = crate::skills::list(&dir, &denied);
    let discovered: Vec<String> = listed.iter().map(|skill| skill.name.clone()).collect();
    let available = crate::skills::effective(&discovered, &denied, None);
    if !available.iter().any(|held| held == name) {
        // Name which of the two reasons it is: a skill the workspace switched
        // off is not the same answer as a name nothing on this machine holds,
        // and the pane's own listing may show a row for one of them.
        return Err(if discovered.iter().any(|held| held == name) {
            format!("this workspace has switched {name:?} off")
        } else {
            format!("this workspace has no skill named {name:?}")
        });
    }
    let changed = {
        let mut record = inner
            .skill_activation
            .lock()
            .map_err(|_| "the skill activation lock is poisoned".to_string())?;
        let changed = record.set(session, name, on, &available)?;
        if changed {
            record.save(&inner.skill_activation_path)?;
        }
        changed
    };
    if changed {
        rebuild_session_agent(inner, session);
    }
    activation_state(inner, session, now_ms)
}

/// Put a chat back on the workspace's skills.
fn inherit_skills(
    inner: &mut Inner,
    session: &str,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let session = session.trim();
    if session.is_empty() {
        return Err("no chat is open to switch a skill on for".to_string());
    }
    let changed = {
        let mut record = inner
            .skill_activation
            .lock()
            .map_err(|_| "the skill activation lock is poisoned".to_string())?;
        let changed = record.clear(session);
        if changed {
            record.save(&inner.skill_activation_path)?;
        }
        changed
    };
    if changed {
        rebuild_session_agent(inner, session);
    }
    activation_state(inner, session, now_ms)
}

/// Change the policy and make it take effect, then report the result.
fn edit_policy<T>(
    inner: &mut Inner,
    now_ms: u64,
    edit: T,
) -> Result<crate::mcp_policy::Report, String>
where
    T: FnOnce(&mut crate::mcp_policy::Store) -> Result<(), String>,
{
    {
        let mut store = inner
            .mcp_policy
            .lock()
            .map_err(|_| "the MCP policy lock is poisoned".to_string())?;
        edit(&mut store)?;
        store.save()?;
    }
    reattach_cached_agents(inner);
    policy_report(inner, now_ms)
}

/// The per-tool MCP policy, and what it does to the connected servers.
#[tauri::command]
fn mcp_policy_status(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::mcp_policy::Report, String> {
    let inner = state.lock()?;
    policy_report(&inner, now_ms)
}

/// Set one `(server, tool)` pair to allow, ask or deny.
#[tauri::command]
fn mcp_policy_set(
    state: State<'_, Arc<GuiState>>,
    server: String,
    tool: String,
    effect: String,
    now_ms: u64,
) -> Result<crate::mcp_policy::Report, String> {
    let effect = crate::mcp_policy::Effect::parse(&effect)
        .ok_or_else(|| format!("unknown effect {effect:?}; expected allow, ask or deny"))?;
    let mut inner = state.lock()?;
    edit_policy(&mut inner, now_ms, |store| {
        store.set(&server, &tool, effect)
    })
}

/// Drop the rule stored for a pair, so it follows the default again.
#[tauri::command]
fn mcp_policy_clear(
    state: State<'_, Arc<GuiState>>,
    server: String,
    tool: String,
    now_ms: u64,
) -> Result<crate::mcp_policy::Report, String> {
    let mut inner = state.lock()?;
    edit_policy(&mut inner, now_ms, |store| {
        store.clear(&server, &tool);
        Ok(())
    })
}

/// Set what a pair with no rule gets.
#[tauri::command]
fn mcp_policy_default(
    state: State<'_, Arc<GuiState>>,
    effect: String,
    now_ms: u64,
) -> Result<crate::mcp_policy::Report, String> {
    let effect = crate::mcp_policy::Effect::parse(&effect)
        .ok_or_else(|| format!("unknown effect {effect:?}; expected allow, ask or deny"))?;
    let mut inner = state.lock()?;
    edit_policy(&mut inner, now_ms, |store| {
        store.set_default(effect);
        Ok(())
    })
}

/// The servers a catalog pass visits, in settings order, each with the live
/// client when this pass has one.
///
/// Configured-but-unreachable servers are included so the pane can say so
/// rather than showing an empty list, and only a server with a client is asked
/// anything.
fn catalog_plan(inner: &Inner) -> Vec<(String, Option<Arc<agent::mcp::McpClient>>)> {
    inner
        .mcp_status
        .iter()
        .map(|status| {
            let client = inner
                .mcp_ready
                .iter()
                .find(|(name, _)| name == &status.name)
                .map(|(_, handshake)| handshake.client.clone());
            (status.name.clone(), client)
        })
        .collect()
}

/// The live client for one server, or the refusal the pane shows.
fn read_target(inner: &Inner, server: &str) -> Result<Arc<agent::mcp::McpClient>, String> {
    inner
        .mcp_ready
        .iter()
        .find(|(name, _)| name == server)
        .map(|(_, handshake)| handshake.client.clone())
        .ok_or_else(|| format!("no connected MCP server is named {server:?}"))
}

/// Validate a URI a read was asked for, before any server sees it.
fn read_uri(uri: &str) -> Result<String, String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Err("a resource read needs a URI".to_string());
    }
    let chars = trimmed.chars().count();
    if chars > crate::mcp_view::MAX_URI_CHARS {
        return Err(format!(
            "that URI is {chars} characters; the limit is {}",
            crate::mcp_view::MAX_URI_CHARS
        ));
    }
    Ok(trimmed.to_string())
}

/// One server's resources and prompts.
///
/// A capability the server never declared is not asked about, and a listing
/// that fails or does not answer lands in this server's own `error` field: the
/// catalog is a summary of several servers and one of them must not be able to
/// blank the rest.
async fn fetch_catalog(
    server: &str,
    client: Option<&agent::mcp::McpClient>,
) -> crate::mcp_view::ServerView {
    let Some(client) = client else {
        return crate::mcp_view::server_view(server, false, None, Ok(Vec::new()), Vec::new());
    };
    let capabilities = client.get_capabilities().await;
    let caps = capabilities.as_ref();
    let offered = |cap: &serde_json::Value| crate::mcp_view::supported(cap);
    let resources_offered = caps.map(|c| offered(&c.resources)).unwrap_or(false);
    let prompts_offered = caps.map(|c| offered(&c.prompts)).unwrap_or(false);
    let resources = if resources_offered {
        match tokio::time::timeout(crate::mcp::LIST_TIMEOUT, client.list_resources()).await {
            Ok(Ok(listed)) => Ok(listed),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!(
                "resources/list did not answer within {:?}",
                crate::mcp::LIST_TIMEOUT
            )),
        }
    } else {
        Ok(Vec::new())
    };
    // `list_prompts` swallows its own errors inside the client, so a timeout is
    // the only failure this side can observe: it reads as "no prompts", which
    // is why the wait is bounded rather than left to the server.
    let prompts = if prompts_offered {
        tokio::time::timeout(crate::mcp::LIST_TIMEOUT, client.list_prompts())
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    crate::mcp_view::server_view(server, true, caps, resources, prompts)
}

/// What the connected MCP servers publish besides tools: their resources and
/// prompts, read on demand rather than carried in every prompt.
#[tauri::command]
async fn mcp_catalog(
    state: State<'_, Arc<GuiState>>,
    now_ms: u64,
) -> Result<crate::mcp_view::Catalog, String> {
    let plan = {
        let inner = state.lock()?;
        catalog_plan(&inner)
    };
    let mut views = Vec::with_capacity(plan.len());
    for (server, client) in plan {
        views.push(fetch_catalog(&server, client.as_deref()).await);
    }
    Ok(crate::mcp_view::catalog(now_ms, views))
}

/// Read one resource from one connected server. The text is bounded on the way
/// out and is never injected anywhere: reading is a gesture, not a side effect
/// of a server being connected.
#[tauri::command]
async fn mcp_read_resource(
    state: State<'_, Arc<GuiState>>,
    server: String,
    uri: String,
    now_ms: u64,
) -> Result<crate::mcp_view::ReadView, String> {
    let uri = read_uri(&uri)?;
    let server = server.trim().to_string();
    let client = {
        let inner = state.lock()?;
        read_target(&inner, &server)?
    };
    let result =
        match tokio::time::timeout(crate::mcp::LIST_TIMEOUT, client.read_resource(&uri)).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => return Err(error.to_string()),
            Err(_) => {
                return Err(format!(
                    "resources/read did not answer within {:?}",
                    crate::mcp::LIST_TIMEOUT
                ))
            }
        };
    Ok(crate::mcp_view::read_view(&server, &uri, &result, now_ms))
}

/// Fetch one prompt from one connected server. The text is bounded on the way
/// out and is never injected anywhere: fetching is a gesture, like reading a
/// resource — the pane shows it, nothing else consumes it.
#[tauri::command]
async fn mcp_get_prompt(
    state: State<'_, Arc<GuiState>>,
    server: String,
    name: String,
    now_ms: u64,
) -> Result<crate::mcp_view::PromptView, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("a prompt fetch needs a name".to_string());
    }
    let server = server.trim().to_string();
    let client = {
        let inner = state.lock()?;
        read_target(&inner, &server)?
    };
    let result = match tokio::time::timeout(
        crate::mcp::LIST_TIMEOUT,
        client.get_prompt(&name, None),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => {
            return Err(format!(
                "prompts/get did not answer within {:?}",
                crate::mcp::LIST_TIMEOUT
            ))
        }
    };
    Ok(crate::mcp_view::prompt_view(
        &server, &name, &result, now_ms,
    ))
}

/// Which skills the open chat carries, what it could carry, and what the
/// workspace keeps from it.
#[tauri::command]
fn skill_activation(
    state: State<'_, Arc<GuiState>>,
    session: String,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let inner = state.lock()?;
    activation_state(&inner, &session, now_ms)
}

/// Switch one skill on or off for one chat.
///
/// The chat's agent is rebuilt when the record actually changed, so the skill
/// the chat just dropped is gone from what the model is told — not at the next
/// restart, and not in the other chats.
#[tauri::command]
fn skill_set_active(
    state: State<'_, Arc<GuiState>>,
    session: String,
    name: String,
    active: bool,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let mut inner = state.lock()?;
    set_skill_active(&mut inner, &session, name.trim(), active, now_ms)
}

/// Forget a chat's own list: it follows the workspace again.
#[tauri::command]
fn skill_inherit(
    state: State<'_, Arc<GuiState>>,
    session: String,
    now_ms: u64,
) -> Result<crate::skills::ActivationReport, String> {
    let mut inner = state.lock()?;
    inherit_skills(&mut inner, &session, now_ms)
}

/// Report MCP connection progress for the settings dialog and status bar.
#[tauri::command]
fn mcp_status(state: State<'_, Arc<GuiState>>) -> Result<McpStatusReport, String> {
    let inner = state.lock()?;
    Ok(McpStatusReport {
        connecting: inner.mcp_connecting,
        servers: inner.mcp_status.clone(),
    })
}

/// Reconnect every enabled server without a settings change.
///
/// The agent cache and remembered handshakes are dropped first so the fresh
/// pass rebuilds from the base configuration — otherwise tools from the
/// previous pass would collide on registration. A pass that is already
/// running is left alone.
#[tauri::command]
fn reconnect_mcp(state: State<'_, Arc<GuiState>>) -> Result<(), String> {
    let (gen, connect) = {
        let mut inner = state.lock()?;
        if inner.mcp_connecting {
            return Ok(());
        }
        inner.agent_cache.clear();
        inner.mcp_ready.clear();
        let connect = inner.settings.mcp_servers.iter().any(|s| s.enabled);
        inner.mcp_connecting = connect;
        inner.mcp_status = inner
            .settings
            .mcp_servers
            .iter()
            .map(|s| crate::mcp::McpStatus {
                name: s.name.trim().to_string(),
                transport: s.transport.clone(),
                enabled: s.enabled,
                connected: false,
                tools: 0,
                tool_names: Vec::new(),
                error: None,
            })
            .collect();
        (inner.settings_gen, connect)
    };
    if connect {
        spawn_mcp_connect(state.inner().clone(), gen);
    }
    Ok(())
}

/// Forward runner events to the frontend and finalize the session on end.
fn spawn_forwarder(
    state: Arc<GuiState>,
    app: AppHandle,
    rx: Receiver<AgentEvent>,
    session_id: String,
    gen: u64,
    job: String,
) {
    tauri::async_runtime::spawn(async move {
        // Buffered this turn's output; the record's blank assistant
        // placeholder is filled from it once the stream settles.
        let mut text_buf = String::new();
        let mut reasoning_buf = String::new();
        let mut tools_buf = Vec::new();
        let mut error_buf: Option<String> = None;
        // What the provider said the turn cost, carried to `stream-finished`.
        let mut usage: Option<(u64, u64)> = None;
        loop {
            let Ok(event) = rx.recv().await else {
                break;
            };
            let terminal = matches!(event, AgentEvent::Done | AgentEvent::Error(_));
            let payload = AgentMessage {
                session_id: session_id.clone(),
                gen,
                event: match event {
                    AgentEvent::Text(text) => {
                        text_buf.push_str(&text);
                        StreamKind::Text { text }
                    }
                    AgentEvent::Reasoning(text) => {
                        reasoning_buf.push_str(&text);
                        StreamKind::Reasoning { text }
                    }
                    AgentEvent::ToolCall(tool) => {
                        tools_buf.push(tool.clone());
                        StreamKind::Tool {
                            name: tool.name,
                            args: tool.args,
                        }
                    }
                    AgentEvent::ToolResult { name, output, ms } => {
                        // Attach the result to the matching call in this
                        // turn's buffer so the persisted transcript keeps it.
                        if let Some(tool) = tools_buf
                            .iter_mut()
                            .rev()
                            .find(|t| t.name == name && t.output.is_none())
                        {
                            tool.output = Some(output.clone());
                            tool.ms = Some(ms);
                        }
                        StreamKind::ToolResult { name, output, ms }
                    }
                    AgentEvent::Usage { prompt, completion } => {
                        StreamKind::Usage { prompt, completion }
                    }
                    AgentEvent::Compacted {
                        before,
                        after,
                        dropped,
                    } => StreamKind::Compacted {
                        before,
                        after,
                        dropped,
                    },
                    AgentEvent::Done => StreamKind::Done,
                    AgentEvent::Error(message) => {
                        error_buf = Some(message.clone());
                        StreamKind::Error { message }
                    }
                },
            };
            let _ = app.emit("agent-event", &payload);
            // Mirror the low-volume milestones into the session's job, so the
            // jobs panel shows what a running turn is doing. Text deltas are
            // pushed once at the end instead, to avoid locking per token.
            let milestone = match &payload.event {
                StreamKind::Tool { name, .. } => Some(format!("[tool] {name}\n")),
                StreamKind::ToolResult { name, output, ms } => {
                    Some(format!("[result] {name} ({ms} ms): {output}\n"))
                }
                StreamKind::Usage { prompt, completion } => Some(format!(
                    "[usage] prompt {prompt} tokens, completion {completion} tokens\n"
                )),
                StreamKind::Compacted {
                    before,
                    after,
                    dropped,
                } => Some(format!(
                    "[compacted] {before} -> {after} messages ({dropped} folded)\n"
                )),
                StreamKind::Error { message } => Some(format!("[error] {message}\n")),
                _ => None,
            };
            if let Some(line) = milestone {
                if let Ok(inner) = state.lock() {
                    let _ = inner.jobs.push(&job, &line);
                }
            }
            if terminal {
                break;
            }
        }

        // Mirror the turn's text into its job and settle the job, before the
        // record is finalized and `error_buf` is consumed.
        if let Ok(inner) = state.lock() {
            if !text_buf.is_empty() {
                let _ = inner.jobs.push(&job, &text_buf);
            }
            let (kind, error) = match &error_buf {
                Some(message) => (crate::runs::JobState::Failed, Some(message.clone())),
                None => (crate::runs::JobState::Finished, None),
            };
            inner.jobs.finish(&job, kind, error);
        }

        // Finalize: fill the placeholder, drop it if still blank, derive a
        // title, snapshot the plan, persist.
        {
            let mut inner = match state.lock() {
                Ok(inner) => inner,
                Err(_) => return,
            };
            // A newer turn for THIS session (re-send supersession) owns the
            // placeholder; other sessions' entries are irrelevant.
            let newer = inner
                .streaming
                .get(&session_id)
                .is_some_and(|s| s.gen != gen);
            if !newer {
                if let Some(mut record) = inner
                    .store
                    .load_all()
                    .into_iter()
                    .find(|s| s.id == session_id)
                {
                    // Oversized tool outputs leave the transcript before it is
                    // persisted: the full text moves to `<store>/spill/`, and
                    // the turn keeps a short marker naming the file.
                    crate::context::spill_oversized_tools(inner.store.dir(), &mut tools_buf);
                    // Write what streamed into the assistant placeholder, so
                    // reloading the session (or restarting) keeps the reply.
                    if let Some(last) = record.messages.last_mut() {
                        if last.role == Role::Assistant && last.is_blank() {
                            last.text = std::mem::take(&mut text_buf);
                            last.reasoning = std::mem::take(&mut reasoning_buf);
                            last.tools = std::mem::take(&mut tools_buf);
                            last.error = error_buf.take();
                        }
                    }
                    let pop_placeholder = record.messages.len() > 1
                        && record.messages.last().is_some_and(|m| m.is_blank());
                    if pop_placeholder {
                        record.messages.pop();
                    }
                    record.derive_title();
                    record.touch();
                    // Persist this session's working plan from its own agent.
                    let agent = inner.agent(&session_id);
                    record.plan = agent.plan_items();
                    // What the provider said this turn cost. It is the only honest
                    // source: a local estimate would drift from the provider's own
                    // count exactly when the context is close to its limit.
                    usage = agent.last_token_usage();
                    let _ = inner.store.save(&record);
                }
                if inner
                    .streaming
                    .get(&session_id)
                    .is_some_and(|s| s.gen == gen)
                {
                    inner.streaming.remove(&session_id);
                }
            }
        }

        let (prompt_tokens, completion_tokens) = match usage {
            Some((prompt, completion)) => (Some(prompt), Some(completion)),
            None => (None, None),
        };
        let _ = app.emit(
            "stream-finished",
            StreamFinished {
                session_id,
                gen,
                prompt_tokens,
                completion_tokens,
            },
        );
    });
}

/// Expand a configured workspace root; `None` when none is set.
fn workspace_root(raw: &str) -> Option<std::path::PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(std::path::PathBuf::from(
        shellexpand::tilde(raw).into_owned(),
    ))
}

/// Load one session by id, or a clean error.
fn load_record(inner: &Inner, session_id: &str) -> Result<SessionRecord, String> {
    inner
        .store
        .load_all()
        .into_iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| format!("session {session_id} not found"))
}

/* ---------------- M4: scoped approval rules ---------------- */

/// The rules in force, plus anything the rules file got wrong.
#[derive(Clone, Serialize)]
pub(crate) struct ApprovalRulesReport {
    /// Every stored rule, in insertion order.
    rules: Vec<crate::approval::ApprovalRule>,
    /// Problems found while loading the file (bad entries, bad JSON).
    warnings: Vec<String>,
    /// How many decisions the audit ring can hold.
    audit_cap: usize,
}

/// Build the report the rules commands return.
fn rules_report(book: &crate::approval::RuleBook) -> ApprovalRulesReport {
    ApprovalRulesReport {
        rules: book.rules(),
        warnings: book.warnings(),
        audit_cap: crate::approval::AUDIT_CAP,
    }
}

/// Parse a rule scope from its wire name.
fn parse_rule_scope(raw: &str) -> Result<crate::approval::RuleScope, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "global" => Ok(crate::approval::RuleScope::Global),
        "workspace" => Ok(crate::approval::RuleScope::Workspace),
        "session" => Ok(crate::approval::RuleScope::Session),
        other => Err(format!(
            "unknown scope '{other}' (expected global, workspace, or session)"
        )),
    }
}

/// Parse a rule effect from its wire name.
fn parse_rule_effect(raw: &str) -> Result<crate::approval::RuleEffect, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok(crate::approval::RuleEffect::Allow),
        "deny" => Ok(crate::approval::RuleEffect::Deny),
        other => Err(format!("unknown effect '{other}' (expected allow or deny)")),
    }
}

/// Parse an argument predicate kind from its wire name.
fn parse_match_kind(raw: &str) -> Result<crate::approval::MatchKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "exact" => Ok(crate::approval::MatchKind::Exact),
        "prefix" => Ok(crate::approval::MatchKind::Prefix),
        "contains" => Ok(crate::approval::MatchKind::Contains),
        other => Err(format!(
            "unknown argument match '{other}' (expected exact, prefix, or contains)"
        )),
    }
}

/// List the scoped approval rules and any load warnings.
#[tauri::command]
fn approval_rules_list(state: State<'_, Arc<GuiState>>) -> Result<ApprovalRulesReport, String> {
    let inner = state.lock()?;
    Ok(rules_report(&inner.rules))
}

/// Add a scoped allow/deny rule and return the book as it now stands.
///
/// A `workspace`-scoped rule defaults its workspace to the active one, and a
/// `session`-scoped rule requires an explicit `session_id`; the argument
/// predicate must be given whole (key, match kind, and value) or not at all.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn approval_rule_add(
    state: State<'_, Arc<GuiState>>,
    tool: String,
    scope: String,
    effect: String,
    arg_key: Option<String>,
    arg_match: Option<String>,
    arg_value: Option<String>,
    workspace: Option<String>,
    session_id: Option<String>,
) -> Result<ApprovalRulesReport, String> {
    let inner = state.lock()?;
    let scope = parse_rule_scope(&scope)?;
    let effect = parse_rule_effect(&effect)?;
    let arg = match (arg_key, arg_match, arg_value) {
        (None, None, None) => None,
        (Some(key), Some(kind), Some(value)) => Some(crate::approval::ArgPredicate {
            key,
            match_kind: parse_match_kind(&kind)?,
            value,
        }),
        _ => {
            return Err(
                "an argument predicate needs arg_key, arg_match, and arg_value together"
                    .to_string(),
            )
        }
    };
    let workspace = match scope {
        crate::approval::RuleScope::Workspace => workspace
            .filter(|root| !root.trim().is_empty())
            .or_else(|| {
                workspace_root(&inner.settings.effective().bash_workspace)
                    .map(|root| root.to_string_lossy().into_owned())
            }),
        _ => None,
    };
    let session = match scope {
        crate::approval::RuleScope::Session => session_id.filter(|id| !id.trim().is_empty()),
        _ => None,
    };
    if scope == crate::approval::RuleScope::Workspace && workspace.is_none() {
        return Err("a workspace-scoped rule needs an active workspace".to_string());
    }
    if scope == crate::approval::RuleScope::Session && session.is_none() {
        return Err("a session-scoped rule needs a session id".to_string());
    }
    inner.rules.add(crate::approval::NewRule {
        tool,
        arg,
        scope,
        effect,
        workspace,
        session,
    })?;
    Ok(rules_report(&inner.rules))
}

/// Revoke a rule by id and return the book as it now stands.
#[tauri::command]
fn approval_rule_revoke(
    state: State<'_, Arc<GuiState>>,
    id: String,
) -> Result<ApprovalRulesReport, String> {
    let inner = state.lock()?;
    if !inner.rules.revoke(&id)? {
        return Err(format!("no rule with id '{id}'"));
    }
    Ok(rules_report(&inner.rules))
}

/// The most recent approval decisions, newest first (bounded ring).
#[tauri::command]
fn approval_audit(
    state: State<'_, Arc<GuiState>>,
    limit: Option<usize>,
) -> Result<Vec<crate::approval::AuditEntry>, String> {
    let inner = state.lock()?;
    Ok(inner
        .rules
        .audit(limit.unwrap_or(50).min(crate::approval::AUDIT_CAP)))
}

/* ---------------- M6: context, spill, compaction ---------------- */

/// Estimate a session's token cost and reconcile it with provider usage.
#[tauri::command]
fn token_meter(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    provider_tokens: Option<usize>,
) -> Result<crate::context::TokenMeterReport, String> {
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    Ok(crate::context::meter(&record, provider_tokens))
}

/// List the tool outputs that were spilled to disk for this store.
#[tauri::command]
fn spill_list(state: State<'_, Arc<GuiState>>) -> Result<Vec<crate::context::SpillEntry>, String> {
    let inner = state.lock()?;
    Ok(crate::context::spill_list(inner.store.dir()))
}

/// Read one spilled tool output, bounded by `max_bytes`.
#[tauri::command]
fn spill_read(
    state: State<'_, Arc<GuiState>>,
    id: String,
    max_bytes: Option<usize>,
) -> Result<crate::context::SpillRead, String> {
    let inner = state.lock()?;
    crate::context::spill_read(inner.store.dir(), &id, max_bytes)
}

/// Report what `/compact` would do for a session right now.
#[tauri::command]
fn compaction_status(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::context::CompactionStatus, String> {
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    let budget = inner.settings.effective().context_budget;
    let compacting = inner.compacting.contains(&session_id);
    Ok(crate::context::compaction_status(
        &record, budget, compacting,
    ))
}

/// Start a compaction now and report the status as it begins.
///
/// The summarization turn runs on a background thread and reports through the
/// same `compact-finished` event as `/compact`; the returned status is the
/// snapshot taken once the run has been accepted.
#[tauri::command]
fn compact_now(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::context::CompactionStatus, String> {
    begin_compaction(app, state.inner(), &session_id)?;
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    let budget = inner.settings.effective().context_budget;
    Ok(crate::context::compaction_status(&record, budget, true))
}

/* ---------------- M7: background jobs ---------------- */

/// Every background job, newest first (bounded output bodies).
#[tauri::command]
fn jobs_list(state: State<'_, Arc<GuiState>>) -> Result<Vec<crate::runs::JobRecord>, String> {
    let inner = state.lock()?;
    Ok(inner.jobs.list())
}

/// Read the tail of a job's output, bounded by `max_bytes`.
#[tauri::command]
fn job_output(
    state: State<'_, Arc<GuiState>>,
    id: String,
    max_bytes: Option<usize>,
) -> Result<crate::runs::JobOutput, String> {
    let inner = state.lock()?;
    inner.jobs.output(&id, max_bytes)
}

/// Interrupt a job by id, returning its updated record.
#[tauri::command]
fn job_interrupt(
    state: State<'_, Arc<GuiState>>,
    id: String,
) -> Result<crate::runs::JobRecord, String> {
    let inner = state.lock()?;
    inner.jobs.interrupt(&id)
}

/* ---------------- M10: deterministic replay ---------------- */

/// Replay a stored session into an ordered transcript and deterministic metrics.
#[tauri::command]
fn replay_session(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::replay::ReplayReport, String> {
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    Ok(crate::replay::replay(&record))
}

/* ---------------- M11: AGI surfaces ---------------- */

/// Read the agent's own composition: tools, skills, plugins, MCP, memories, goal.
#[tauri::command]
async fn self_inspect(
    state: State<'_, Arc<GuiState>>,
    session_id: Option<String>,
) -> Result<crate::agi::SelfInspection, String> {
    // Snapshot everything that needs the lock, then await memory outside it:
    // the state guard is not `Send` and must never cross an await point.
    let (capabilities, settings, goal, mcp_servers, rules) = {
        let inner = state.lock()?;
        let effective = inner.settings.effective();
        let broker = Arc::new(crate::approval::ApprovalBroker::default());
        let base = caps::build_agent(&effective, Arc::clone(&broker));
        let mut agent = base.as_ref().clone();
        inner.attach_mcp_tools(&mut agent, &broker, &effective);
        let capabilities = caps::capabilities_of(&Arc::new(agent), &effective);
        let goal = match &session_id {
            Some(id) => inner.store.load(id).and_then(|record| record.goal),
            None => None,
        };
        (
            capabilities,
            effective,
            goal,
            inner.mcp_ready.len(),
            inner.rules.rules().len(),
        )
    };
    let memories = match &session_id {
        Some(id) => {
            let store = {
                let mut inner = state.lock()?;
                inner.agent(id).memory().cloned()
            };
            match store {
                Some(store) => store.all().await.len(),
                None => 0,
            }
        }
        None => 0,
    };
    Ok(crate::agi::inspect(
        &capabilities,
        memories,
        goal.as_ref(),
        &settings,
        mcp_servers,
        rules,
    ))
}

/// Score a session transcript with explicit reasons (deterministic critic).
#[tauri::command]
fn evaluate_transcript(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::agi::TranscriptScore, String> {
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    Ok(crate::agi::evaluate(&record))
}

/* ---------------- M14: packaging ---------------- */

/// Bundle a chat as a ZIP (transcript + metrics + manifest) and return its path.
#[tauri::command]
fn export_bundle(
    state: State<'_, Arc<GuiState>>,
    session_id: String,
) -> Result<crate::export::BundleFileReport, String> {
    let inner = state.lock()?;
    let record = load_record(&inner, &session_id)?;
    let exported = crate::export::export_bundle(inner.store.dir(), &record)?;
    Ok(crate::export::BundleFileReport {
        path: exported.path.to_string_lossy().into_owned(),
        bytes: exported.bytes,
        schema_version: exported.manifest.schema_version,
        entries: exported.manifest.entries,
        files: exported.manifest.files,
    })
}

/// Build the Tauri app and run the event loop (blocks until quit).
pub(crate) fn run() -> anyhow::Result<()> {
    tauri::Builder::default()
        .manage(Arc::new(GuiState::new()))
        .manage(crate::pty::PtyRegistry::default())
        .setup(|app| {
            // Brokers emit `approval-request` from inside the agent: install
            // the handle once here; brokers built later pick it up too.
            let state = app.state::<Arc<GuiState>>();
            let (gen, connect) = {
                let mut inner = state.lock().map_err(Box::<dyn std::error::Error>::from)?;
                inner.app_handle = Some(app.handle().clone());
                let connect = inner.settings.mcp_servers.iter().any(|s| s.enabled);
                (inner.settings_gen, connect)
            };
            // Reconnect persisted MCP servers without blocking window startup.
            if connect {
                spawn_mcp_connect(state.inner().clone(), gen);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            create_session,
            delete_session,
            rename_session,
            set_session_pinned,
            set_session_archived,
            load_session,
            restore_plan,
            get_plan,
            set_plan,
            get_goal,
            set_goal,
            goal_action,
            export_session,
            current_stream,
            send_message,
            retry_last,
            truncate_session,
            compacted_archive,
            archived_messages,
            session_overview,
            export_chat,
            trashed_chats,
            restore_session,
            purge_trash,
            restore_compacted,
            fork_session,
            open_url,
            search_files,
            read_file,
            list_files,
            run_command,
            pty_start,
            pty_write,
            pty_resize,
            pty_kill,
            stop_streaming,
            respond_approval,
            compact_session,
            init_agents,
            get_settings,
            save_settings,
            set_active_workspace,
            create_workspace,
            list_folder,
            choose_folder,
            apply_preset,
            save_preset,
            update_workspace,
            remove_workspace,
            list_capabilities,
            mcp_status,
            reconnect_mcp,
            mcp_policy_status,
            mcp_policy_set,
            mcp_policy_clear,
            mcp_policy_default,
            move_session,
            search_sessions,
            memory_status,
            list_memories,
            schedule_list,
            schedule_tick,
            schedule_add,
            schedule_cancel,
            schedule_clear_history,
            knowledge_list,
            knowledge_search,
            knowledge_add,
            knowledge_update,
            knowledge_delete,
            skill_activation,
            skill_set_active,
            skill_inherit,
            mcp_catalog,
            mcp_read_resource,
            mcp_get_prompt,
            skill_list,
            skill_author,
            remember_memory,
            forget_memory,
            allow_tool,
            forget_allowed_tool,
            list_profiles,
            approval_rules_list,
            approval_rule_add,
            approval_rule_revoke,
            approval_audit,
            token_meter,
            spill_list,
            spill_read,
            compaction_status,
            compact_now,
            jobs_list,
            job_output,
            job_interrupt,
            replay_session,
            self_inspect,
            evaluate_transcript,
            export_bundle,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    Ok(())
}

#[cfg(test)]
mod tests {

    /// The knowledge tool has to be reachable the way the model reaches it: in
    /// the registry of a real agent, counted by the same composition the Insight
    /// pane reads, and callable by name through the registry's own dispatch. A
    /// registration assertion alone would not prove a call works.
    #[test]
    fn the_agent_the_gui_builds_can_search_its_knowledge_base() {
        let dir = std::env::temp_dir().join(format!("bos-kb-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = crate::knowledge::Store::load(&dir.join("knowledge.json"));
        store
            .add("Release ritual", "ops", "tag, then push main", 7)
            .unwrap();
        let settings = crate::settings::Settings::default();

        let mut plain = (*crate::caps::build_agent(
            &settings,
            Arc::new(crate::approval::ApprovalBroker::default()),
        ))
        .clone();
        let without = crate::caps::capabilities_of(&Arc::new(plain.clone()), &settings);
        attach_knowledge(&mut plain, Arc::new(Mutex::new(store)));
        let with = crate::caps::capabilities_of(&Arc::new(plain.clone()), &settings);
        assert_eq!(
            with.tools.len(),
            without.tools.len() + 1,
            "the composition the Insight pane reads must count the knowledge tool"
        );

        let registry = plain.registry().expect("a registry");
        let names = registry.list();
        assert!(
            names.iter().any(|name| name == crate::knowledge::TOOL_NAME),
            "{names:?}"
        );
        // `execute` is the model's dispatch path, and it validates the arguments
        // against the tool's own schema first — so this also pins the schema.
        let out = registry
            .execute(
                crate::knowledge::TOOL_NAME,
                &serde_json::json!({ "query": "ritual" }),
            )
            .expect("the knowledge tool answers");
        assert_eq!(out["matches"][0]["title"], "Release ritual");
        assert_eq!(out["matches"][0]["body"], "tag, then push main");
        assert_eq!(out["total"], 1);
        assert!(out["note"].as_str().unwrap().contains("1 of 1"));

        // And the path a session actually takes: `Inner::agent` must attach it,
        // not just `attach_knowledge` when called by hand.
        let mut inner = Inner::new();
        let session_agent = inner.agent("knowledge-check");
        let names = session_agent.registry().expect("a registry").list();
        assert!(
            names.iter().any(|name| name == crate::knowledge::TOOL_NAME),
            "a session's agent must carry the knowledge tool: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The policy is consulted **where tools are attached**, so a denied tool is
    /// absent from the agent a session actually runs on — not merely from the
    /// report. The handshake is injected because the decision, not the network,
    /// is what is under test.
    #[test]
    fn a_policy_denied_mcp_tool_never_reaches_a_session_agent() {
        let mut inner = Inner::new();
        let handshake = crate::mcp::Handshake {
            // `connect_http` builds a transport and nothing more; registration
            // never calls the client, so no server is needed.
            client: Arc::new(agent::mcp::McpClient::connect_http(
                "http://127.0.0.1:1/mcp",
            )),
            tools: vec![
                agent::mcp::ToolDefinition {
                    name: "read".to_string(),
                    description: "read a file".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                agent::mcp::ToolDefinition {
                    name: "write".to_string(),
                    description: "write a file".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
        };
        inner.mcp_ready.push(("srv".to_string(), handshake));
        // Which servers a session may use is the **workspace's** list, so the
        // test has to enable it there; the globals are set as well so the test
        // holds on an installation with no workspaces at all.
        let entry = crate::settings::McpServerEntry {
            name: "srv".to_string(),
            transport: "http".to_string(),
            url: "http://127.0.0.1:1/mcp".to_string(),
            enabled: true,
            ..crate::settings::McpServerEntry::default()
        };
        inner.settings.mcp_servers = vec![entry.clone()];
        let active = inner.settings.active_workspace.clone();
        for workspace in inner.settings.workspaces.iter_mut() {
            if workspace.id == active {
                workspace.mcp_servers = vec![entry.clone()];
            }
        }

        let names = |agent: &Arc<Agent>| agent.registry().expect("registry").async_tool_names();
        let agent = inner.agent("s");
        assert!(names(&agent).contains(&"srv_read".to_string()));
        assert!(names(&agent).contains(&"srv_write".to_string()));

        inner
            .mcp_policy
            .lock()
            .expect("policy")
            .set("srv", "write", crate::mcp_policy::Effect::Deny)
            .expect("rule");
        // A rebuild is what a settings change does; the point is that the next
        // agent of this session carries one tool instead of two.
        inner.settings_gen += 1;
        let agent = inner.agent("s");
        let after = names(&agent);
        assert!(after.contains(&"srv_read".to_string()), "{after:?}");
        assert!(
            !after.contains(&"srv_write".to_string()),
            "a denied tool must not exist on the session's agent: {after:?}"
        );

        // And the report the pane reads agrees with what was attached.
        let report = policy_report(&inner, 7).expect("report");
        assert_eq!(report.servers.len(), 1);
        assert_eq!(report.servers[0].name, "srv");
        assert_eq!(report.servers[0].attached, 1);
        assert_eq!(report.servers[0].denied, 1);
        assert_eq!(report.servers[0].tools[1].by, "srv/write");
        assert_eq!(report.default, crate::mcp_policy::Effect::Allow);
        // The tool left behind is not an `ask` rule's business: nothing in the
        // report claims a gate the policy did not ask for.
        assert_eq!(
            report.servers[0].tools[0].effect,
            crate::mcp_policy::Effect::Allow
        );
        assert_eq!(report.servers[0].tools[0].by, "default");
        assert_eq!(report.asked, 0);
    }

    /// A policy change reaches the agent a session is **already running**, not
    /// just the next build: otherwise a denied tool stays callable for the rest
    /// of the session, which is the promise the pane is making.
    #[test]
    fn a_policy_change_re_attaches_the_agents_that_already_exist() {
        let mut inner = Inner::new();
        let handshake = crate::mcp::Handshake {
            client: Arc::new(agent::mcp::McpClient::connect_http(
                "http://127.0.0.1:1/mcp",
            )),
            tools: vec![
                agent::mcp::ToolDefinition {
                    name: "read".to_string(),
                    description: "read a file".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                agent::mcp::ToolDefinition {
                    name: "write".to_string(),
                    description: "write a file".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ],
        };
        inner.mcp_ready.push(("srv".to_string(), handshake));
        let entry = crate::settings::McpServerEntry {
            name: "srv".to_string(),
            transport: "http".to_string(),
            url: "http://127.0.0.1:1/mcp".to_string(),
            enabled: true,
            ..crate::settings::McpServerEntry::default()
        };
        inner.settings.mcp_servers = vec![entry.clone()];
        let active = inner.settings.active_workspace.clone();
        for workspace in inner.settings.workspaces.iter_mut() {
            if workspace.id == active {
                workspace.mcp_servers = vec![entry.clone()];
            }
        }
        let cached_names = |inner: &Inner| {
            inner
                .agent_cache
                .get("s")
                .expect("cached")
                .agent
                .registry()
                .expect("registry")
                .async_tool_names()
        };
        inner.agent("s");
        assert!(cached_names(&inner).contains(&"srv_write".to_string()));
        // The connect pass's row, so the re-attach has a count to correct.
        inner.mcp_status = vec![crate::mcp::McpStatus {
            name: "srv".to_string(),
            transport: "http".to_string(),
            enabled: true,
            connected: true,
            tools: 2,
            tool_names: vec!["read".to_string(), "write".to_string()],
            error: None,
        }];

        inner
            .mcp_policy
            .lock()
            .expect("policy")
            .set("srv", "write", crate::mcp_policy::Effect::Deny)
            .expect("rule");
        // No settings change, no rebuild: the same session's cached agent has
        // to lose the tool.
        reattach_cached_agents(&mut inner);
        let after = cached_names(&inner);
        assert!(after.contains(&"srv_read".to_string()), "{after:?}");
        assert!(
            !after.contains(&"srv_write".to_string()),
            "a denied tool must leave the running session: {after:?}"
        );
        // And the reported count follows the re-attach, not the handshake.
        let filesystem = inner
            .mcp_status
            .iter()
            .find(|s| s.name == "srv")
            .expect("status row");
        assert_eq!(filesystem.tools, 1, "the status counts what is attached");
        assert_eq!(
            filesystem.tool_names.len(),
            2,
            "and still knows what was offered"
        );
    }

    /// A rule that asks gates its tool even where the workspace asks for
    /// nothing — that is the whole difference between `ask` and the global gate.
    #[test]
    fn an_ask_rule_gates_one_tool_without_a_workspace_gate() {
        let path = std::env::temp_dir().join("bos-mcp-policy-gate-test.json");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::mcp_policy::Store::load(&path);
        store
            .set("srv", "write", crate::mcp_policy::Effect::Ask)
            .expect("rule");
        // With no workspace gate, only the asked tool takes one.
        assert!(crate::mcp_policy::gate_for(
            false,
            &store.decide("srv", "write")
        ));
        assert!(!crate::mcp_policy::gate_for(
            false,
            &store.decide("srv", "read")
        ));
        // With the workspace gate on, everything takes one — including a tool
        // the policy explicitly allows, which must not weaken the gate.
        store
            .set("srv", "read", crate::mcp_policy::Effect::Allow)
            .expect("rule");
        assert!(crate::mcp_policy::gate_for(
            true,
            &store.decide("srv", "read")
        ));
    }

    #[test]
    fn a_memory_has_to_say_something_and_fit() {
        assert_eq!(clean_memory_text("  hello  ").unwrap(), "hello");
        assert!(clean_memory_text("   ").is_err());
        assert!(clean_memory_text("").is_err());
        // The cap counts characters, not bytes: a CJK memory may hold a full
        // 2000 of them, and a byte cap would reject most of it.
        let cjk = "记".repeat(MAX_MEMORY_CHARS);
        assert_eq!(
            clean_memory_text(&cjk).unwrap().chars().count(),
            MAX_MEMORY_CHARS
        );
        assert!(clean_memory_text(&"记".repeat(MAX_MEMORY_CHARS + 1)).is_err());
    }

    /// The fast paths must not change what is found, so this attacks the two
    /// cases they are not allowed to handle: a cased non-ASCII needle, and an
    /// ASCII needle against non-ASCII text where Unicode folding can match.
    #[test]
    fn a_case_insensitive_search_survives_the_fast_paths() {
        let dir = std::env::temp_dir().join(format!("bos-fold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::session::SessionStore::new(&dir);
        let mut record = crate::session::SessionRecord::new();
        record.title = "folding".to_string();
        record.workspace = "w1".to_string();
        record
            .messages
            .push(crate::session::ChatMessage::user("Grüße aus Köln, ÄÖÜ"));
        record
            .messages
            .push(crate::session::ChatMessage::user("中文检索应当命中"));
        store.save(&record).expect("saved");

        // Cased non-ASCII needle: the allocating path must still be used.
        assert_eq!(store.search("grüße", 10).len(), 1, "non-ASCII case folding");
        assert_eq!(store.search("köln", 10).len(), 1, "non-ASCII case folding");
        // An uncased needle takes the byte path, and must still find CJK.
        assert_eq!(store.search("中文检索", 10).len(), 1, "uncased byte path");
        // And plain ASCII, the case the fast path is for.
        assert_eq!(store.search("aus", 10).len(), 1, "ascii ignore-case");
        assert!(
            store.search("gruse", 10).is_empty(),
            "no false match from folding"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_search_cache_notices_a_rewritten_session() {
        let dir = std::env::temp_dir().join(format!("bos-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::session::SessionStore::new(&dir);
        let mut record = crate::session::SessionRecord::new();
        record.title = "first".to_string();
        record.workspace = "w1".to_string();
        record
            .messages
            .push(crate::session::ChatMessage::user("alpha only"));
        store.save(&record).expect("saved");
        assert_eq!(store.search("alpha", 10).len(), 1, "this warms the cache");

        // Rewrite the same session. Any rewrite changes the file's length, so
        // the stamp catches it even when both writes land in the same tick.
        record.title = "second".to_string();
        record.messages = vec![crate::session::ChatMessage::user("beta only, and longer")];
        store.save(&record).expect("saved");
        assert!(
            store.search("alpha", 10).is_empty(),
            "a rewritten session must not be served from the cache"
        );
        assert_eq!(store.search("beta", 10).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Prints what one keystroke costs with and without the parse cache.
    ///
    /// Ignored rather than asserted: a timing is evidence for a person, not a
    /// gate, because a busy machine would make it flaky. Run it with
    /// `cargo test -p bsh search_benchmark -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn search_benchmark_prints_cold_and_warm_costs() {
        let dir = std::env::temp_dir().join(format!("bos-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::session::SessionStore::new(&dir);
        const SESSIONS: usize = 200;
        const MESSAGES: usize = 30;
        for n in 0..SESSIONS {
            let mut record = crate::session::SessionRecord::new();
            record.title = format!("session {n}");
            record.workspace = "w1".to_string();
            for m in 0..MESSAGES {
                record
                    .messages
                    .push(crate::session::ChatMessage::user(format!(
                        "message {m} of {n} with some words to scan over"
                    )));
            }
            if n == SESSIONS - 1 {
                record
                    .messages
                    .push(crate::session::ChatMessage::user("消息 检索 命中"));
            }
            store.save(&record).expect("saved");
        }
        // A needle that matches one message, so every query scans everything.
        // The CJK needle is here because the ASCII fast path cannot serve it:
        // it is the case that proves the cached lowercase is what pays.
        println!("[bench] {SESSIONS} sessions x {MESSAGES} messages, 50-hit cap");
        for needle in ["message 29 of 199", "消息"] {
            let first = std::time::Instant::now();
            let expected = store.search(needle, 50).len();
            let first = first.elapsed();
            let rest = std::time::Instant::now();
            for _ in 0..20 {
                assert_eq!(store.search(needle, 50).len(), expected);
            }
            let rest = rest.elapsed() / 20;
            println!(
                "[bench] {needle:?}: {expected} hit(s); first {first:?}; following {rest:?} ({:.1}x)",
                first.as_secs_f64() / rest.as_secs_f64().max(f64::EPSILON)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_finds_a_phrase_across_sessions() {
        let dir = std::env::temp_dir().join(format!("bos-search-{}", std::process::id()));
        let store = crate::session::SessionStore::new(&dir);
        let mut first = crate::session::SessionRecord::new();
        first.title = "about foxes".to_string();
        first.workspace = "w1".to_string();
        first
            .messages
            .push(crate::session::ChatMessage::user("The Quick brown fox"));
        store.save(&first).expect("saved");
        let mut second = crate::session::SessionRecord::new();
        second.title = "unrelated".to_string();
        second.workspace = "w2".to_string();
        second
            .messages
            .push(crate::session::ChatMessage::user("nothing to see"));
        store.save(&second).expect("saved");

        // Case-insensitive, and the snippet quotes the original casing.
        let hits = store.search("quick", 10);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].session, first.id);
        assert_eq!(hits[0].workspace, "w1");
        assert_eq!(hits[0].index, 0);
        assert!(hits[0].snippet.contains("Quick"), "{}", hits[0].snippet);

        // A title is findable from the same box.
        let by_title = store.search("foxes", 10);
        assert_eq!(by_title.len(), 1);
        assert!(by_title[0].snippet.contains("foxes"));

        // Nothing, and the cap and the empty query all behave.
        assert!(store.search("platypus", 10).is_empty());
        assert!(store.search("", 10).is_empty());
        assert!(store.search("nothing", 0).is_empty());
        assert_eq!(store.search("e", 1).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_can_be_moved_between_workspaces() {
        let dir = std::env::temp_dir().join(format!("bos-move-{}", std::process::id()));
        let store = crate::session::SessionStore::new(&dir);
        let mut record = crate::session::SessionRecord::new();
        record
            .messages
            .push(crate::session::ChatMessage::user("hello"));
        store.save(&record).expect("saved");
        let id = record.id.clone();

        let moved = store.move_to(&id, "workspace-b").expect("moved");
        assert_eq!(moved.workspace, "workspace-b");
        // A move changes who runs the chat, not what was said in it.
        assert_eq!(moved.messages.len(), 1);
        let reloaded = store.load(&id).expect("reloaded");
        assert_eq!(reloaded.workspace, "workspace-b");
        assert_eq!(reloaded.messages.len(), 1);

        assert!(store.move_to("missing-session", "workspace-b").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_can_be_pointed_at_its_own_model() {
        let mut s = crate::settings::Settings::from_config_value(&serde_json::Value::Null);
        s.migrate();
        let id = s.workspaces[0].id.clone();
        s.set_workspace_runtime(
            &id,
            &crate::settings::WorkspaceRuntime {
                model: "big/model".to_string(),
                base_url: "https://x/v1".to_string(),
                api_key: "k".to_string(),
                system_prompt: "be terse".to_string(),
                temperature: Some(0.0),
                reasoning_effort: "high".to_string(),
                ..Default::default()
            },
        )
        .expect("configured");
        let w = s.workspaces.iter().find(|w| w.id == id).expect("workspace");
        assert_eq!(w.model, "big/model");
        assert_eq!(w.base_url, "https://x/v1");
        assert_eq!(w.system_prompt, "be terse");
        // 0.0 is a setting, not an absence: `Option` is what keeps that true.
        assert_eq!(w.temperature, Some(0.0));
        assert_eq!(w.reasoning_effort.as_deref(), Some("high"));
        // Blanks fall back to the globals, and a blank effort is an absence again.
        s.set_workspace_runtime(
            &id,
            &crate::settings::WorkspaceRuntime {
                reasoning_effort: "  ".to_string(),
                ..Default::default()
            },
        )
        .expect("configured");
        let w = s.workspaces.iter().find(|w| w.id == id).expect("workspace");
        assert_eq!(w.model, "");
        assert_eq!(w.temperature, None);
        assert_eq!(w.reasoning_effort, None);
        // And the overlay is what the agent would see.
        let eff = s.effective();
        assert_eq!(
            eff.model,
            crate::settings::Settings::from_config_value(&serde_json::Value::Null).model
        );
        assert!(s
            .set_workspace_runtime("nope", &crate::settings::WorkspaceRuntime::default())
            .is_err());
    }

    #[test]
    fn saving_a_preset_copies_the_workspace_and_refuses_a_duplicate_name() {
        let mut s = crate::settings::Settings::from_config_value(&serde_json::Value::Null);
        s.migrate();
        s.workspaces[0].model = "m/x".to_string();
        let ws_id = s.workspaces[0].id.clone();
        let id = s.save_preset(&ws_id, "Strict").expect("saved");
        assert_eq!(s.presets.len(), 1);
        assert_eq!(s.presets[0].name, "Strict");
        assert_eq!(s.presets[0].workspace.model, "m/x");
        assert_eq!(s.presets[0].id, id);
        assert_ne!(
            s.presets[0].workspace.id, ws_id,
            "a copy, not the workspace"
        );
        // Editing the workspace afterwards must not reach the preset.
        s.workspaces[0].model = "changed".to_string();
        assert_eq!(s.presets[0].workspace.model, "m/x");
        // Names are how a preset is picked: a duplicate would be ambiguous.
        assert!(s.save_preset(&ws_id, "Strict").is_err());
        assert!(s.save_preset("nope", "x").is_err());
    }

    #[test]
    fn applying_a_preset_creates_a_divergent_workspace() {
        let mut s = crate::settings::Settings::from_config_value(&serde_json::Value::Null);
        s.migrate();
        let mut w = s.workspaces[0].clone();
        w.model = "preset/model".to_string();
        w.root = "/preset".to_string();
        s.presets.push(crate::settings::Preset {
            id: "p1".to_string(),
            name: "Strict".to_string(),
            workspace: w,
        });
        let before = s.workspaces.len();
        let id = s.apply_preset("p1", "").expect("preset applies");
        assert_eq!(s.workspaces.len(), before + 1);
        assert_eq!(s.active_workspace, id);
        let applied = s.workspaces.iter().find(|w| w.id == id).expect("applied");
        assert_eq!(applied.name, "Strict");
        assert_eq!(applied.model, "preset/model");
        assert_ne!(
            applied.id, s.presets[0].workspace.id,
            "a copy, not the preset"
        );
        // Divergence: editing what we made must leave the preset untouched.
        s.workspaces
            .iter_mut()
            .find(|w| w.id == id)
            .expect("applied")
            .model = "changed".to_string();
        assert_eq!(s.presets[0].workspace.model, "preset/model");
        assert!(s.apply_preset("nope", "").is_err());
    }

    #[test]
    fn a_workspace_root_must_be_a_directory_and_is_stored_canonical() {
        let dir = std::env::temp_dir();
        let rooted = normalize_workspace_root(&dir.to_string_lossy()).expect("temp dir");
        // Canonical, so `~`, symlinks and `..` cannot make two workspaces of one
        // folder (on macOS /tmp is itself a symlink, which this asserts).
        assert_eq!(rooted, dir.canonicalize().unwrap().to_string_lossy());
        assert!(normalize_workspace_root("")
            .expect("empty means cwd")
            .is_empty());
        assert!(normalize_workspace_root("/nope/not/here-2971").is_err());

        let file = dir.join("bos-workspace-root-probe.txt");
        std::fs::write(&file, "x").expect("probe file");
        assert!(
            normalize_workspace_root(&file.to_string_lossy()).is_err(),
            "a file is not a root"
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_workspace_only_attaches_the_servers_it_switched_on() {
        let mut settings = crate::settings::Settings::from_config_value(&serde_json::Value::Null);
        settings.mcp_servers = vec![
            crate::settings::McpServerEntry {
                name: "on".to_string(),
                enabled: true,
                ..crate::settings::McpServerEntry::default()
            },
            crate::settings::McpServerEntry {
                name: "off".to_string(),
                enabled: false,
                ..crate::settings::McpServerEntry::default()
            },
        ];
        assert_eq!(
            enabled_mcp_servers(&settings),
            vec!["on".to_string()],
            "a switched-off server does not attach"
        );
        // Nothing configured means nothing attaches, rather than everything.
        settings.mcp_servers.clear();
        assert!(enabled_mcp_servers(&settings).is_empty());
    }
    use super::*;

    fn record_with(messages: Vec<ChatMessage>) -> SessionRecord {
        let mut record = SessionRecord::new();
        record.messages = messages;
        record
    }

    #[test]
    fn peel_drops_last_exchange_and_returns_prompt() {
        let mut rec = record_with(vec![
            ChatMessage::user("first"),
            ChatMessage::assistant(),
            ChatMessage::user("second"),
            ChatMessage::assistant(),
        ]);
        let (text, images) = peel_exchange(&mut rec).expect("peel");
        assert_eq!(text, "second");
        assert!(images.is_empty());
        // Re-seeded: first exchange kept, trailing pair replaced with fresh
        // user + empty assistant placeholders.
        assert_eq!(rec.messages.len(), 4);
        assert_eq!(rec.messages[2].text, "second");
        assert_eq!(rec.messages[2].role, Role::User);
        assert_eq!(rec.messages[3].role, Role::Assistant);
        assert!(rec.messages[3].is_blank());
    }

    #[test]
    fn peel_rejects_lonely_messages_without_mutating() {
        let mut empty = record_with(vec![]);
        assert!(peel_exchange(&mut empty).is_err());
        assert!(empty.messages.is_empty());

        let mut one = record_with(vec![ChatMessage::user("hi")]);
        assert!(peel_exchange(&mut one).is_err());
        assert_eq!(one.messages.len(), 1);
        assert_eq!(one.messages[0].text, "hi");
    }

    #[test]
    fn peel_rejects_non_exchange_shapes() {
        // Assistant reply must follow a user prompt, not another reply.
        let mut rec = record_with(vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant(),
            ChatMessage::assistant(),
        ]);
        assert!(peel_exchange(&mut rec).is_err());
        assert_eq!(rec.messages.len(), 3);
    }

    #[test]
    fn peel_rejects_blank_prompt() {
        let mut rec = record_with(vec![ChatMessage::user("   "), ChatMessage::assistant()]);
        assert!(peel_exchange(&mut rec).is_err());
        assert_eq!(rec.messages.len(), 2);
    }

    #[test]
    fn peel_keeps_the_attachments_of_the_regenerated_turn() {
        let url = "data:image/png;base64,aGVsbG8=";
        let mut rec = record_with(vec![
            ChatMessage::user("first"),
            ChatMessage::assistant(),
            ChatMessage::user_with_images("look", vec![url.into()]),
            ChatMessage::assistant(),
        ]);
        let (text, images) = peel_exchange(&mut rec).expect("peel");
        assert_eq!(text, "look");
        assert_eq!(images, vec![url]);
        assert_eq!(rec.messages[2].images, vec![url]);
    }

    #[test]
    fn a_regenerated_image_only_turn_does_not_need_text() {
        let url = "data:image/png;base64,aGVsbG8=";
        let mut rec = record_with(vec![
            ChatMessage::user_with_images("", vec![url.into()]),
            ChatMessage::assistant(),
        ]);
        let (text, images) = peel_exchange(&mut rec).expect("peel");
        assert!(text.is_empty());
        assert_eq!(images, vec![url]);
    }

    /// The composer's prompt-history state machine is plain JS — run its
    /// node suite from the Rust gate so it can never silently rot. Skips
    /// (without failing) when node is not installed on the machine.
    #[test]
    fn ui_prompt_nav_node_suite_passes() {
        let suite = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("ui")
            .join("prompt-nav.test.js");
        let output = match std::process::Command::new("node").arg(&suite).output() {
            Ok(out) => out,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => panic!("node failed to start: {err}"),
        };
        assert!(
            output.status.success(),
            "prompt-nav.test.js failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The finished connect pass must report in settings order, keep
    /// disabled servers as explicit "off" rows, and mark an enabled entry
    /// that never got attempted as failed (never as a phantom pending).
    #[test]
    fn mcp_status_report_follows_settings_order() {
        let entry = |name: &str, enabled: bool| crate::settings::McpServerEntry {
            name: name.to_string(),
            transport: "stdio".to_string(),
            command: "run".to_string(),
            args: Vec::new(),
            url: String::new(),
            enabled,
        };
        let entries = vec![entry("a", true), entry("b", false), entry("c", true)];
        let mut results = HashMap::new();
        results.insert(
            "a".to_string(),
            crate::mcp::McpStatus {
                name: "a".to_string(),
                transport: "stdio".to_string(),
                enabled: true,
                connected: true,
                tools: 3,
                tool_names: vec!["read".to_string(), "write".to_string(), "list".to_string()],
                error: None,
            },
        );
        let statuses = build_statuses(&entries, &results);
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(statuses[0].connected);
        assert_eq!(statuses[0].tools, 3);
        assert_eq!(statuses[0].tool_names.len(), 3, "names ride with the count");
        assert!(!statuses[1].enabled);
        assert!(statuses[1].error.is_none());
        assert!(!statuses[2].connected);
        assert!(statuses[2].error.is_some());
    }

    fn mcp_status_row(name: &str, connected: bool) -> crate::mcp::McpStatus {
        crate::mcp::McpStatus {
            name: name.to_string(),
            transport: "stdio".to_string(),
            enabled: true,
            connected,
            tools: 0,
            tool_names: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn a_catalog_pass_lists_every_configured_server_and_asks_only_the_live_ones() {
        let mut inner = Inner::new();
        inner.mcp_status = vec![
            mcp_status_row("reachable", true),
            mcp_status_row("gone", false),
        ];
        let client = Arc::new(agent::mcp::McpClient::connect_http("http://127.0.0.1:9"));
        inner.mcp_ready = vec![(
            "reachable".to_string(),
            crate::mcp::Handshake {
                client: client.clone(),
                tools: Vec::new(),
            },
        )];

        let plan = catalog_plan(&inner);
        let names: Vec<&str> = plan.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["reachable", "gone"],
            "settings order, nothing dropped"
        );
        assert!(
            plan[0].1.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)),
            "the live client is the one this pass holds"
        );
        assert!(
            plan[1].1.is_none(),
            "an unreachable server has nothing to ask"
        );
    }

    #[test]
    fn reading_from_a_server_that_is_not_connected_is_refused_by_name() {
        let mut inner = Inner::new();
        inner.mcp_status = vec![mcp_status_row("files", true), mcp_status_row("gone", false)];
        // Two live clients, so "the one named" and "the first one there" are
        // different answers: with a single client the check cannot tell them
        // apart, which is how the first version of this test stayed green while
        // `read_target` ignored the name it was handed.
        let named = Arc::new(agent::mcp::McpClient::connect_http("http://127.0.0.1:9"));
        let other = Arc::new(agent::mcp::McpClient::connect_http("http://127.0.0.1:9"));
        inner.mcp_ready = vec![
            (
                "files".to_string(),
                crate::mcp::Handshake {
                    client: named.clone(),
                    tools: Vec::new(),
                },
            ),
            (
                "other".to_string(),
                crate::mcp::Handshake {
                    client: other.clone(),
                    tools: Vec::new(),
                },
            ),
        ];
        // Not `expect`/`expect_err`: `McpClient` is not `Debug`, so both sides
        // are matched out by hand.
        let client = match read_target(&inner, "files") {
            Ok(client) => client,
            Err(err) => panic!("files has a live client: {err}"),
        };
        assert!(Arc::ptr_eq(&client, &named), "the server that was named");
        let client = match read_target(&inner, "other") {
            Ok(client) => client,
            Err(err) => panic!("other has a live client: {err}"),
        };
        assert!(
            Arc::ptr_eq(&client, &other),
            "the second server, not the first one on the list"
        );
        let err = match read_target(&inner, "gone") {
            Ok(_) => panic!("a server with no live client must be refused"),
            Err(err) => err,
        };
        assert!(err.contains("gone"), "{err}");
        assert!(err.contains("no connected MCP server"), "{err}");

        // The URI is checked before any server is consulted, so a blank or
        // oversized one never becomes a round trip.
        let blank = read_uri("   ").expect_err("refused");
        assert!(blank.contains("needs a URI"), "{blank}");
        assert_eq!(read_uri("  file:///a  ").expect("ok"), "file:///a");
        let long = "u".repeat(crate::mcp_view::MAX_URI_CHARS + 1);
        let err = read_uri(&long).expect_err("refused");
        assert!(
            err.contains(&format!("{}", crate::mcp_view::MAX_URI_CHARS)),
            "{err}"
        );
    }

    /// A skills directory of this test's own: removed first, so a previous run
    /// cannot answer for this one.
    fn skill_temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-skill-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Point the whole configuration at `dir`, workspace and global alike, so
    /// the test cannot read the machine's real skills.
    fn use_skills_dir(inner: &mut Inner, dir: &str) {
        inner.settings.skills_dir = dir.to_string();
        let active = inner.settings.active_workspace.clone();
        for workspace in inner.settings.workspaces.iter_mut() {
            if workspace.id == active {
                workspace.skills_dir = dir.to_string();
            }
        }
    }

    /// A record file of this test's own, with an empty record in memory: an
    /// assertion about activations must not read — or write — the file the
    /// user's own chats are in.
    fn use_activation(inner: &mut Inner, tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bos-gui-activation-{tag}-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        *inner.skill_activation.lock().expect("lock") = crate::skills::Activation::default();
        inner.skill_activation_path = path.clone();
        path
    }

    /// The skills one chat's agent actually carries, sorted.
    ///
    /// Sorted because the agent's order is the loader's directory read order,
    /// which is not a promise: every assertion here is about which skills a
    /// chat has, not the order they would be listed in.
    fn skill_names(inner: &Inner, session: &str) -> Vec<String> {
        let mut names: Vec<String> = inner
            .agent_cache
            .get(session)
            .expect("cached")
            .agent
            .get_skills_content()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();
        names.sort();
        names
    }

    /// A store the test owns: the commands under test write records, and
    /// `Inner::new()` points at the user's real `~/.bos/gui/sessions`.
    fn private_store(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bos-gui-sessions-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// One stored chat with a message and a plan, so an edit that damages the
    /// record can be told apart from one that only renames it.
    fn stored_chat(inner: &mut Inner, title: &str, text: &str) -> String {
        let mut record = SessionRecord::new();
        record.title = title.to_string();
        record.workspace = inner.settings.active_workspace.clone();
        record
            .messages
            .push(crate::session::ChatMessage::user(text));
        inner.store.save(&record).expect("stored");
        record.id
    }

    /// The mtime of the one file holding `id`, which is what tells "the record
    /// was rewritten" apart from "the answer was the same". Comparing
    /// `updated_at` cannot: a save that does not `touch()` leaves that field
    /// identical, so the check would pass whether or not the write happened.
    fn record_mtime(dir: &std::path::Path, id: &str) -> std::time::SystemTime {
        let entry = std::fs::read_dir(dir)
            .expect("store dir")
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains(id))
            .expect("the record file");
        entry
            .metadata()
            .expect("metadata")
            .modified()
            .expect("mtime")
    }

    fn reload(inner: &Inner, id: &str) -> SessionRecord {
        inner
            .store
            .load_all()
            .into_iter()
            .find(|r| r.id == id)
            .expect("the record is still on disk")
    }

    #[test]
    fn a_new_workspace_can_be_born_configured_and_a_bad_draft_adds_nothing() {
        let mut settings = crate::settings::Settings::from_config_value(&serde_json::Value::Null);
        settings.mcp_servers = vec![crate::settings::McpServerEntry {
            name: "files".to_string(),
            enabled: true,
            ..Default::default()
        }];
        // The valid draft lands whole: model, plugins, and exactly the MCP
        // servers that were ticked.
        let draft = crate::settings::WorkspaceDraft {
            model: "glm-4.7".to_string(),
            disabled_plugins: vec!["audit".to_string()],
            mcp_enabled: Some(Vec::new()),
            ..Default::default()
        };
        let dir = private_store("draft-ws");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let root = dir.to_string_lossy().to_string();
        let w = drafted_workspace(&settings, "trino", &root, Some(&draft)).expect("built");
        assert_eq!(w.name, "trino");
        assert_eq!(w.model, "glm-4.7");
        assert_eq!(w.disabled_plugins, vec!["audit".to_string()]);
        assert!(!w.mcp_servers[0].enabled, "nothing ticked means nothing on");
        // The bad one is refused, and the caller has nothing to add.
        let bad = crate::settings::WorkspaceDraft {
            mcp_enabled: Some(vec!["ghost".to_string()]),
            ..Default::default()
        };
        let err =
            drafted_workspace(&settings, "trino", &root, Some(&bad)).expect_err("unknown server");
        assert!(err.contains("ghost"), "{err}");
        // A root that is not a folder is refused the same way — the chooser
        // hands over a real one, and the backend does not take a path on trust.
        let missing = dir.join("nowhere");
        let err = drafted_workspace(&settings, "trino", &missing.to_string_lossy(), None)
            .expect_err("a missing root is refused");
        assert!(err.contains("nowhere"), "{err}");
        // A path that *exists* and is a file is refused by the directory rule,
        // and this half is not redundant: a falsification that weakened the
        // lookup to fall back to another directory left the missing-path check
        // green, because `canonicalize` refuses a missing path on its own. Two
        // refusals guard the missing case, so only the file case can tell the
        // directory check apart from the lookup check.
        let file = dir.join("notes.txt");
        std::fs::write(&file, b"x").expect("write file");
        let err = drafted_workspace(&settings, "trino", &file.to_string_lossy(), None)
            .expect_err("a file is not a root");
        assert!(err.contains("is not a directory"), "{err}");
        // And the payload an older caller sends (none) still creates a workspace
        // that behaves exactly as before.
        let plain = drafted_workspace(&settings, "", &root, None).expect("built");
        assert_eq!(
            plain.name, w.root,
            "an unnamed workspace takes the folder's resolved path as its name"
        );
        assert_eq!(plain.model, settings.model);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A root this test owns and has created. `private_store` is named for the
    /// session store it isolates, so a workspace root gets its own name — and is
    /// left behind for the next run to clear, like every other fixture here.
    fn workspace_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-wsroot-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_creation_prepares_the_folders_own_configuration_before_it_records_it() {
        let mut inner = Inner::new();
        let dir = workspace_root("create");
        let before = inner.settings.workspaces.len();
        create_workspace_record(&mut inner, "trino", &dir.to_string_lossy(), None)
            .expect("created");

        let path = dir.join(".bos").join("workspace.toml");
        assert!(
            path.is_file(),
            "the folder the user picked carries its own configuration"
        );
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(
            text.contains("bash_enabled"),
            "and it is the configuration: {text}"
        );
        assert_eq!(inner.settings.workspaces.len(), before + 1);
        let added = inner.settings.workspaces.last().expect("added");
        assert_eq!(
            inner.settings.active_workspace, added.id,
            "the new one is the shown one"
        );
        assert_eq!(
            added.root,
            dir.canonicalize().expect("canonical").to_string_lossy(),
            "and the stored root is the resolved one the file was written from"
        );

        // A root that cannot take a `.bos` is refused **with nothing added**: the
        // write happens before the record is pushed, which is the whole reason it
        // is a separate function.
        let blocked = workspace_root("create-blocked");
        std::fs::write(blocked.join(".bos"), b"not a directory").expect("the blocking file");
        let list = inner.settings.workspaces.clone();
        let active = inner.settings.active_workspace.clone();
        let err = create_workspace_record(&mut inner, "blocked", &blocked.to_string_lossy(), None)
            .expect_err("refused");
        assert!(err.contains(".bos"), "{err}");
        assert_eq!(
            inner.settings.workspaces, list,
            "a refused creation adds nothing"
        );
        assert_eq!(
            inner.settings.active_workspace, active,
            "and switches nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&blocked);
    }

    #[test]
    fn a_settings_save_puts_the_same_answer_in_the_workspaces_own_folder() {
        let mut inner = Inner::new();
        let dir = workspace_root("globals");
        create_workspace_record(&mut inner, "trino", &dir.to_string_lossy(), None)
            .expect("created");
        let id = inner.settings.active_workspace.clone();
        // The settings dialog owns the globals; the workspace in use is written from
        // them, and the global root is what this workspace's root follows.
        inner.settings.model = "gemini-3".to_string();
        inner.settings.bash_enabled = false;
        inner.settings.bash_workspace = dir.to_string_lossy().into_owned();
        push_globals_to_active_workspace(&mut inner).expect("pushed");

        let stored = inner
            .settings
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .expect("the workspace in use");
        assert_eq!(
            stored.model, "gemini-3",
            "the workspace follows the globals"
        );
        assert!(!stored.bash_enabled);
        // And the folder follows the workspace — without this write the next load
        // would fold the older file back over the save, which is the one way the two
        // can silently disagree.
        let text = std::fs::read_to_string(dir.join(".bos").join("workspace.toml"))
            .expect("the workspace folder is readable");
        assert!(
            text.contains("gemini-3"),
            "the folder carries the new model: {text}"
        );
        assert!(
            text.contains("bash_enabled = false"),
            "and the switch: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_rewrites_the_folders_file_and_a_refused_write_puts_the_record_back() {
        let mut inner = Inner::new();
        let dir = workspace_root("edit");
        create_workspace_record(&mut inner, "trino", &dir.to_string_lossy(), None)
            .expect("created");
        let id = inner.settings.active_workspace.clone();
        let path = dir.join(".bos").join("workspace.toml");

        let runtime = crate::settings::WorkspaceRuntime {
            model: "glm-4.8".to_string(),
            ..Default::default()
        };
        rewrite_workspace(&mut inner, &id, &runtime, |w| {
            w.disabled_tools = vec!["bash".to_string()];
        })
        .expect("rewritten");
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(
            text.contains("glm-4.8"),
            "the folder's copy carries the edit: {text}"
        );
        assert!(text.contains("\"bash\""), "including the deny-list: {text}");

        // Now make the folder refuse. The whole edit must be undone — the model
        // included, because it is set before the write and the backup is taken
        // before that: a restore that ran after the runtime was applied would
        // leave the app holding a model the folder never took.
        std::fs::remove_dir_all(dir.join(".bos")).expect("remove the folder");
        std::fs::write(dir.join(".bos"), b"not a directory").expect("block it");
        let before = inner.settings.workspaces.clone();
        let runtime = crate::settings::WorkspaceRuntime {
            model: "glm-4.9".to_string(),
            ..Default::default()
        };
        let err = rewrite_workspace(&mut inner, &id, &runtime, |w| {
            w.name = "renamed".to_string();
        })
        .expect_err("refused");
        assert!(err.contains(".bos"), "{err}");
        assert_eq!(
            inner.settings.workspaces, before,
            "a refused write leaves the record exactly as it was"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_a_chat_rewrites_the_name_and_leaves_the_conversation_alone() {
        let mut inner = Inner::new();
        inner.store = crate::session::SessionStore::new(private_store("rename"));
        let id = stored_chat(&mut inner, "old name", "the only prompt");

        let after = rename_session_in(&mut inner, &id, "  Deploy notes  ").expect("renamed");
        assert_eq!(after.title, "Deploy notes", "the name is trimmed");
        assert!(!after.pinned && !after.archived);

        // Durable, not just returned: the sidebar re-reads the list on the next
        // turn, so a rename that only reached the response would vanish.
        let on_disk = reload(&inner, &id);
        assert_eq!(on_disk.title, "Deploy notes");
        assert_eq!(on_disk.messages.len(), 1);
        assert_eq!(on_disk.messages[0].text, "the only prompt");
    }

    #[test]
    fn a_name_that_is_blank_or_too_long_is_refused_without_touching_the_file() {
        let mut inner = Inner::new();
        inner.store = crate::session::SessionStore::new(private_store("rename-refuse"));
        let id = stored_chat(&mut inner, "keep me", "prompt");

        let blank = rename_session_in(&mut inner, &id, "   ").expect_err("blank refused");
        assert!(blank.contains("needs a name"), "{blank}");
        let too_long = rename_session_in(&mut inner, &id, &"x".repeat(81)).expect_err("long");
        assert!(too_long.contains("at most 80"), "{too_long}");
        // 80 is the boundary and must be accepted, or the rule documented in
        // the message would be one character stricter than the one enforced.
        assert_eq!(
            rename_session_in(&mut inner, &id, &"y".repeat(80))
                .expect("80 characters is a name")
                .title
                .chars()
                .count(),
            80
        );

        assert_eq!(reload(&inner, &id).title, "y".repeat(80));
    }

    #[test]
    fn pinning_is_allowed_mid_turn_and_rewriting_the_same_pin_is_not_a_write() {
        let mut inner = Inner::new();
        let dir = private_store("pin");
        inner.store = crate::session::SessionStore::new(&dir);
        let id = stored_chat(&mut inner, "busy", "prompt");
        let before = reload(&inner, &id).updated_at;
        inner.streaming.insert(
            id.clone(),
            Streaming {
                session_id: id.clone(),
                gen: 1,
                started_at: 1,
                job: String::new(),
            },
        );

        assert!(
            set_session_pinned_in(&mut inner, &id, true)
                .expect("pinned mid-turn")
                .pinned
        );
        assert!(
            inner.streaming.contains_key(&id),
            "the turn was not disturbed"
        );
        assert!(reload(&inner, &id).updated_at >= before);

        // The same pin again is answered, not written: a menu opened and closed
        // must not keep rewriting the record.
        let pinned_at = record_mtime(&dir, &id);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            set_session_pinned_in(&mut inner, &id, true)
                .expect("idempotent")
                .pinned
        );
        assert_eq!(
            record_mtime(&dir, &id),
            pinned_at,
            "pinning an already-pinned chat rewrote the file"
        );

        // Renaming and archiving are refused while the turn runs, so hiding a
        // chat cannot hide the output it is producing.
        assert!(rename_session_in(&mut inner, &id, "later").is_err());
        assert!(set_session_archived_in(&mut inner, &id, true).is_err());
        assert!(!reload(&inner, &id).archived_at.is_some());
    }

    #[test]
    fn archiving_hides_a_chat_and_restoring_brings_it_back_whole() {
        let mut inner = Inner::new();
        inner.store = crate::session::SessionStore::new(private_store("archive"));
        let id = stored_chat(&mut inner, "on the record", "the only prompt");
        let pinned = set_session_pinned_in(&mut inner, &id, true).expect("pinned");
        assert!(pinned.pinned);

        let archived = set_session_archived_in(&mut inner, &id, true).expect("archived");
        assert!(
            archived.archived && archived.pinned,
            "a pin survives archiving"
        );
        let on_disk = reload(&inner, &id);
        assert!(on_disk.archived_at.is_some());
        assert_eq!(on_disk.messages.len(), 1, "archiving destroys nothing");

        let back = set_session_archived_in(&mut inner, &id, false).expect("restored");
        assert!(!back.archived);
        assert_eq!(reload(&inner, &id).messages[0].text, "the only prompt");
    }

    /// A workspace denial, set the way the settings pane sets it.
    ///
    /// `effective()` reads the *workspace's* list, not the global one, so a test
    /// that only writes `settings.disabled_skills` writes to a field the agent
    /// build never reads — and passes while proving nothing.
    fn use_disabled_skills(inner: &mut Inner, denied: &[&str]) {
        let denied: Vec<String> = denied.iter().map(|name| name.to_string()).collect();
        inner.settings.disabled_skills = denied.clone();
        let active = inner.settings.active_workspace.clone();
        for workspace in inner.settings.workspaces.iter_mut() {
            if workspace.id == active {
                workspace.disabled_skills = denied.clone();
            }
        }
    }

    fn cached_skills(inner: &Inner) -> Vec<(String, String)> {
        inner
            .agent_cache
            .get("s")
            .expect("cached")
            .agent
            .get_skills_content()
            .into_iter()
            .map(|(name, instructions)| (name.to_string(), instructions.to_string()))
            .collect()
    }

    /// Two chats, two skills, one activation.
    ///
    /// The whole promise of this feature is a *scope*: the chat that switched a
    /// skill off loses it now, and the chat that asked for nothing keeps both
    /// its skill and the agent it was already running.
    fn two_chats(tag: &str) -> (Inner, std::path::PathBuf) {
        let mut inner = Inner::new();
        let dir = skill_temp_dir(tag);
        use_skills_dir(&mut inner, &dir.display().to_string());
        let record = use_activation(&mut inner, tag);
        author_skill(
            &mut inner,
            "pr-review",
            "Review a pull request",
            "Read the diff.",
            false,
            1,
        )
        .expect("authored");
        author_skill(
            &mut inner,
            "docs",
            "Write the docs",
            "Explain it.",
            false,
            1,
        )
        .expect("authored");
        inner.agent("a");
        inner.agent("b");
        (inner, record)
    }

    #[test]
    fn switching_a_skill_off_in_one_chat_rebuilds_that_chat_and_no_other() {
        let (mut inner, record_path) = two_chats("activation");
        assert_eq!(skill_names(&inner, "a"), vec!["docs", "pr-review"]);
        assert_eq!(skill_names(&inner, "b"), vec!["docs", "pr-review"]);
        let untouched = Arc::clone(&inner.agent_cache.get("b").expect("cached").agent);

        let report =
            set_skill_active(&mut inner, "a", "pr-review", false, 7).expect("switched off");
        assert_eq!(report.session, "a");
        assert_eq!(report.now_ms, 7);
        assert!(report.explicit, "the chat now holds a list of its own");
        assert_eq!(report.active, vec!["docs"]);
        assert_eq!(report.available, vec!["docs", "pr-review"]);
        assert!(report.denied.is_empty());
        assert_eq!(report.path, record_path.display().to_string());
        assert_eq!(report.sessions_known, 1);
        assert_eq!(report.max_sessions, crate::skills::MAX_SESSIONS);

        // Now, not at the next open: the running chat's agent is what changed.
        assert_eq!(skill_names(&inner, "a"), vec!["docs"]);
        assert_eq!(
            skill_names(&inner, "b"),
            vec!["docs", "pr-review"],
            "the other chat asked for nothing"
        );
        assert!(
            Arc::ptr_eq(
                &untouched,
                &inner.agent_cache.get("b").expect("cached").agent
            ),
            "an activation in one chat does not rebuild another"
        );

        // The list is durable, so a chat opened later reads the same one.
        let stored = crate::skills::Activation::load(&record_path);
        assert_eq!(stored.active("a"), Some(&["docs".to_string()][..]));
        assert_eq!(stored.active("b"), None, "b inherits");

        // Inheriting is one gesture, and it puts the workspace back.
        let back = inherit_skills(&mut inner, "a", 8).expect("inherited");
        assert!(!back.explicit);
        assert_eq!(back.active, vec!["docs", "pr-review"]);
        assert_eq!(report_path(&back), record_path.display().to_string());
        assert_eq!(skill_names(&inner, "a"), vec!["docs", "pr-review"]);
        assert_eq!(
            crate::skills::Activation::load(&record_path).record_count(),
            0
        );

        let _ = std::fs::remove_file(&record_path);
    }

    fn report_path(report: &crate::skills::ActivationReport) -> String {
        report.path.clone()
    }

    #[test]
    fn a_chat_that_is_not_open_and_a_skill_the_workspace_switched_off_are_refused() {
        let (mut inner, record_path) = two_chats("activation-refuse");

        let err = set_skill_active(&mut inner, "  ", "docs", false, 1).expect_err("no chat");
        assert!(err.contains("no chat is open"), "{err}");
        let err = set_skill_active(&mut inner, "a", "nope", true, 1).expect_err("not offered");
        assert!(
            err.contains("no skill named") && err.contains("nope"),
            "{err}"
        );
        assert!(activation_state(&inner, "", 1).is_err());
        assert!(inherit_skills(&mut inner, "", 1).is_err());
        // A refusal leaves no record and no rebuild behind.
        assert!(!inner
            .skill_activation
            .lock()
            .expect("lock")
            .is_explicit("a"));

        // The workspace's switch-off is a floor, even for a chat whose own
        // record names the skill — the file is not trusted over the workspace.
        use_disabled_skills(&mut inner, &["pr-review"]);
        std::fs::write(
            &record_path,
            r#"{"version": 1, "sessions": {"a": ["pr-review", "docs"]}}"#,
        )
        .expect("wrote");
        *inner.skill_activation.lock().expect("lock") =
            crate::skills::Activation::load(&record_path);
        assert!(rebuild_session_agent(&mut inner, "a"));
        assert_eq!(
            skill_names(&inner, "a"),
            vec!["docs"],
            "the workspace keeps it off"
        );
        let report = activation_state(&inner, "a", 2).expect("report");
        assert_eq!(report.available, vec!["docs"]);
        assert_eq!(report.denied, vec!["pr-review"]);
        assert_eq!(report.active, vec!["docs"]);
        let err = set_skill_active(&mut inner, "a", "pr-review", true, 3).expect_err("denied");
        assert!(
            err.contains("switched \"pr-review\" off"),
            "a workspace denial says so: {err}"
        );

        // A chat nobody opened needs no rebuild: its first build reads the
        // record, so there is nothing to swap.
        assert!(!rebuild_session_agent(&mut inner, "never-opened"));

        let _ = std::fs::remove_file(&record_path);
    }

    #[test]
    fn an_authored_skill_reaches_the_session_that_already_exists() {
        let mut inner = Inner::new();
        let dir = skill_temp_dir("author");
        use_skills_dir(&mut inner, &dir.display().to_string());
        inner.agent("s");
        assert!(
            !cached_skills(&inner)
                .iter()
                .any(|(name, _)| name == "pr-review"),
            "the machine already has a skill by this name"
        );

        let body = "Read the diff, then list findings with file and line.";
        let report = author_skill(
            &mut inner,
            "pr-review",
            "Review a pull request",
            body,
            false,
            11,
        )
        .expect("authored");

        assert_eq!(report.now_ms, 11);
        assert!(report.exists);
        assert_eq!(report.skills.len(), 1, "{:?}", report.skills);
        assert_eq!(report.skills[0].name, "pr-review");
        assert_eq!(report.skills[0].instructions_chars, body.chars().count());
        assert!(!report.skills[0].denied);
        assert!(dir.join("pr-review").join("SKILL.md").is_file());

        // The session that was already open carries it, without reopening.
        let skills = cached_skills(&inner);
        assert_eq!(
            skills
                .iter()
                .find(|(name, _)| name == "pr-review")
                .map(|(_, instructions)| instructions.as_str()),
            Some(body),
            "the authored skill did not reach the running session: {skills:?}"
        );

        // A second save neither replaces the file nor the running agent.
        let err = author_skill(&mut inner, "pr-review", "Other", "Other body.", false, 12)
            .expect_err("refused");
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(
            cached_skills(&inner)
                .iter()
                .find(|(name, _)| name == "pr-review")
                .map(|(_, instructions)| instructions.as_str()),
            Some(body),
            "a refused save changed the running session"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn authoring_without_a_skills_directory_is_refused() {
        let mut inner = Inner::new();
        use_skills_dir(&mut inner, "");
        // The name carries the process id: this asserts about a path in the
        // working directory, so it has to own that path rather than inherit a
        // directory a previous run (or a deliberate removal of the guard) left.
        let stray = format!("nowhere-{}", std::process::id());
        let path = std::path::Path::new(&stray);
        let _ = std::fs::remove_dir_all(path);
        let err = author_skill(&mut inner, &stray, "d", "b", false, 0).expect_err("refused");
        assert!(err.contains("no skills directory is configured"), "{err}");
        // Nothing was written into the working directory.
        assert!(!path.exists(), "wrote {}", path.display());
        let _ = std::fs::remove_dir_all(path);
    }
}
