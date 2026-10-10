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
        if let Some(handle) = &self.app_handle {
            broker.set_handle(handle.clone());
        }
        // The agent is built from the active workspace's choices, not from the
        // globals: model, tools, approval and budget all come from one place.
        let effective = self.settings.effective();
        let base = caps::build_agent(&effective, Arc::clone(&broker));
        let mut agent = base.as_ref().clone();
        // Connected MCP tools ride along with fresh builds (and the connect
        // pass re-attaches them to entries that already exist).
        self.attach_mcp_tools(&mut agent, &broker, &effective);
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

    /// Register the connected MCP tools on `agent`, gated by approvals when
    /// required. Returns `(namespace, error)` for each registration failure.
    fn attach_mcp_tools(
        &self,
        agent: &mut Agent,
        broker: &Arc<crate::approval::ApprovalBroker>,
        settings: &Settings,
    ) -> Vec<(String, String)> {
        // Approval policy and the allowed servers are the workspace's, not the
        // globals': the handshake may exist because the server is configured,
        // while this workspace is one that switched it off.
        let gate = settings.require_approval.then(|| Arc::clone(broker));
        let allowed = enabled_mcp_servers(settings);
        let mut errors = Vec::new();
        for (namespace, handshake) in &self.mcp_ready {
            if !allowed.iter().any(|name| name == namespace) {
                continue;
            }
            for def in &handshake.tools {
                let tool = crate::mcp::tool_for(handshake, namespace, def, gate.clone());
                if let Err(err) = agent.add_mcp_tool(namespace, &def.name, tool) {
                    errors.push((namespace.clone(), err.to_string()));
                    break;
                }
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
        let mut statuses = build_statuses(&entries, &results);
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
        // Re-attach on a clone of every cached session agent; a stream that
        // is mid-turn keeps its previous `Arc` untouched.
        let jobs: Vec<(String, Arc<Agent>, Arc<crate::approval::ApprovalBroker>)> = inner
            .agent_cache
            .iter()
            .map(|(id, e)| (id.clone(), Arc::clone(&e.agent), Arc::clone(&e.broker)))
            .collect();
        let effective = inner.settings.effective();
        for (id, agent, broker) in jobs {
            let mut fresh = agent.as_ref().clone();
            let errors = inner.attach_mcp_tools(&mut fresh, &broker, &effective);
            for (namespace, err) in &errors {
                if let Some(status) = statuses.iter_mut().find(|s| s.name == *namespace) {
                    status.connected = false;
                    status.error = Some(err.clone());
                }
            }
            // Recount from the registry so the report matches what is attached.
            if let Some(reg) = fresh.registry() {
                let names = reg.async_tool_names();
                for status in statuses.iter_mut().filter(|s| s.connected) {
                    let prefix = format!("{}_", status.name);
                    status.tools = names.iter().filter(|n| n.starts_with(&prefix)).count();
                }
            }
            if let Some(entry) = inner.agent_cache.get_mut(&id) {
                entry.agent = Arc::new(fresh);
            }
        }
        inner.mcp_status = statuses;
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
                    error: Some("connect pass never reached this server".to_string()),
                })
        })
        .collect()
}

/// Sidebar row payload sent to the frontend.
#[derive(Clone, Serialize)]
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

/// Start a fresh stream for `text` given the prior history `transcript`.
///
/// The caller must hold the state lock; the forwarder is attached by the
/// command afterwards. Other sessions may be streaming concurrently — each
/// session gets its own agent, runner slot, and approval context.
fn begin_stream(
    inner: &mut Inner,
    session_id: &str,
    text: String,
    transcript: Vec<ChatMessage>,
) -> (Receiver<AgentEvent>, Streaming) {
    inner.stream_gen += 1;
    let streaming = Streaming {
        session_id: session_id.to_string(),
        gen: inner.stream_gen,
        started_at: unix_now(),
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
    let rx = inner
        .runner
        .spawn(session_id, agent, text, transcript, budget);
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
#[tauri::command]
fn send_message(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    text: String,
    truncate_from: Option<usize>,
) -> Result<Streaming, String> {
    if text.trim().is_empty() {
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
        record.messages.push(ChatMessage::user(text.clone()));
        record.messages.push(ChatMessage::assistant());
        record.touch();
        inner.store.save(&record).map_err(|e| e.to_string())?;
        begin_stream(&mut inner, &session_id, text, transcript)
    };

    spawn_forwarder(state.inner().clone(), app, rx, session_id, streaming.gen);
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
fn peel_exchange(record: &mut SessionRecord) -> Result<String, String> {
    let n = record.messages.len();
    if n < 2 {
        return Err("nothing to regenerate".to_string());
    }
    let (user, last) = (&record.messages[n - 2], &record.messages[n - 1]);
    if user.role != Role::User || last.role != Role::Assistant {
        return Err("nothing to regenerate".to_string());
    }
    let text = user.text.clone();
    if text.trim().is_empty() {
        return Err("message is empty".to_string());
    }
    record.messages.truncate(n - 2);
    record.messages.push(ChatMessage::user(text.clone()));
    record.messages.push(ChatMessage::assistant());
    Ok(text)
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
        let text = peel_exchange(&mut record)?;
        let transcript = record.messages.clone();
        // Same plan restore as `send_message`: the re-run belongs to this
        // session, so its own agent holds the persisted plan.
        inner.agent(&session_id).set_plan(record.plan.clone());
        record.touch();
        inner.store.save(&record).map_err(|e| e.to_string())?;
        begin_stream(&mut inner, &session_id, text, transcript)
    };

    spawn_forwarder(state.inner().clone(), app, rx, session_id, streaming.gen);
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
        }
        None => {
            for broker in &inner.brokers {
                broker.reject_all();
            }
            inner.runner.stop_all();
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
    let (settings, messages, before) = {
        let mut inner = state.lock()?;
        if inner.streaming.contains_key(&session_id) {
            return Err("a turn is in progress — wait for it to finish".to_string());
        }
        if !inner.compacting.insert(session_id.clone()) {
            return Err("a compaction is already running for this chat".to_string());
        }
        let found = inner
            .store
            .load_all()
            .into_iter()
            .find(|s| s.id == session_id);
        let Some(record) = found else {
            inner.compacting.remove(&session_id);
            return Err("session not found".to_string());
        };
        if let Some(reason) = crate::compact::refusal(&record.messages) {
            inner.compacting.remove(&session_id);
            return Err(reason);
        }
        let before = record.messages.len();
        (inner.settings.clone(), record.messages, before)
    };

    let state = Arc::clone(state.inner());
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

/// Create a workspace. It starts from the current defaults, so a new workspace
/// behaves like today's installation until it is configured otherwise.
#[tauri::command]
fn create_workspace(
    state: State<'_, Arc<GuiState>>,
    name: String,
    root: String,
) -> Result<Settings, String> {
    let mut inner = state.lock()?;
    let root = normalize_workspace_root(&root)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::session::unix_now();
    let mut w = crate::settings::Workspace::adopted_from(&inner.settings, &id, now);
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
    inner.settings.workspaces.push(w);
    inner.settings.active_workspace = id;
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
}

/// Rename a workspace or move its root. Sessions stay put: a chat recorded in the
/// old folder keeps saying so, because its approvals and tool calls happened there.
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
    inner.settings.set_workspace_runtime(
        &id,
        &crate::settings::WorkspaceRuntime {
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
        },
    )?;
    let now = crate::session::unix_now();
    let Some(w) = inner.settings.workspaces.iter_mut().find(|w| w.id == id) else {
        return Err(format!("unknown workspace '{id}'"));
    };
    if !name.trim().is_empty() {
        w.name = name.trim().to_string();
    }
    w.root = root;
    // The deny-lists are the workspace's own selection (§24.8), and the names are
    // the ones the model sees, so the panel hands back exactly what it showed.
    w.disabled_tools = disabled_tools;
    w.disabled_skills = disabled_skills;
    // Which MCP servers this workspace may use. A handshake that exists does not
    // attach unless its server is enabled here (§24.8, enforced at attach time).
    w.mcp_servers = mcp_servers;
    w.updated_at = now;
    inner.settings.save().map_err(|e| e.to_string())?;
    inner.settings_gen = inner.settings_gen.wrapping_add(1);
    inner.agent_cache.clear();
    Ok(inner.settings.effective())
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

/// Apply and persist settings; the agent cache is invalidated and any MCP
/// servers reconnect in the background.
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
    inner.settings.mcp_servers = mcp_servers;
    inner.settings.providers = providers;
    // The dialog edits the workspace in use. The globals above are refreshed too,
    // because they are what a *new* workspace starts from; these values are what
    // this workspace actually runs with.
    if let Some(idx) = inner
        .settings
        .workspaces
        .iter()
        .position(|w| w.id == inner.settings.active_workspace)
    {
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
        // The tool and skill deny-lists are not in this dialog: they are the
        // workspace's own selection, and overwriting them here would silently
        // re-enable what the user switched off.
        if follows_root {
            w.name = w.root.clone();
        }
        w.updated_at = now;
    }
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
fn list_capabilities(state: State<'_, Arc<GuiState>>) -> Result<Capabilities, String> {
    let inner = state.lock()?;
    let broker = Arc::new(crate::approval::ApprovalBroker::default());
    let effective = inner.settings.effective();
    let base = caps::build_agent(&effective, Arc::clone(&broker));
    let mut agent = base.as_ref().clone();
    inner.attach_mcp_tools(&mut agent, &broker, &effective);
    // Describe the effective set, so the panel reports what this workspace will
    // actually run with.
    Ok(caps::capabilities_of(&Arc::new(agent), &effective))
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
            if terminal {
                break;
            }
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
            apply_preset,
            save_preset,
            update_workspace,
            remove_workspace,
            list_capabilities,
            mcp_status,
            reconnect_mcp,
            move_session,
            search_sessions,
            list_memories,
            remember_memory,
            forget_memory,
            allow_tool,
            forget_allowed_tool,
            list_profiles,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    Ok(())
}

#[cfg(test)]
mod tests {

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
        let text = peel_exchange(&mut rec).expect("peel");
        assert_eq!(text, "second");
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
                error: None,
            },
        );
        let statuses = build_statuses(&entries, &results);
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(statuses[0].connected);
        assert_eq!(statuses[0].tools, 3);
        assert!(!statuses[1].enabled);
        assert!(statuses[1].error.is_none());
        assert!(!statuses[2].connected);
        assert!(statuses[2].error.is_some());
    }
}
