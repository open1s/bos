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
        if let Some(handle) = &self.app_handle {
            broker.set_handle(handle.clone());
        }
        let base = caps::build_agent(&self.settings, Arc::clone(&broker));
        let mut agent = base.as_ref().clone();
        // Connected MCP tools ride along with fresh builds (and the connect
        // pass re-attaches them to entries that already exist).
        self.attach_mcp_tools(&mut agent, &broker);
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
    ) -> Vec<(String, String)> {
        let gate = self.settings.require_approval.then(|| Arc::clone(broker));
        let mut errors = Vec::new();
        for (namespace, handshake) in &self.mcp_ready {
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
        for (id, agent, broker) in jobs {
            let mut fresh = agent.as_ref().clone();
            let errors = inner.attach_mcp_tools(&mut fresh, &broker);
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
}

fn summary(record: &SessionRecord) -> SessionSummary {
    SessionSummary {
        id: record.id.clone(),
        title: record.title.clone(),
        updated_at: record.updated_at,
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
}

/// List all sessions, most recently active first.
#[tauri::command]
fn list_sessions(state: State<'_, Arc<GuiState>>) -> Result<Vec<SessionSummary>, String> {
    let inner = state.lock()?;
    Ok(inner.store.load_all().iter().map(summary).collect())
}

/// Create an empty session and return its summary.
#[tauri::command]
fn create_session(state: State<'_, Arc<GuiState>>) -> Result<SessionSummary, String> {
    let inner = state.lock()?;
    // Sessions own their agent (and plan) from the first build — nothing to
    // reset on a brand-new id.
    let record = SessionRecord::new();
    inner.store.save(&record).map_err(|e| e.to_string())?;
    Ok(summary(&record))
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
    Ok(state.lock()?.settings.clone())
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

/// List the registered tools, skills, and plugins for the current settings.
///
/// The panel is settings-scoped, so this reports from a fresh throwaway
/// build (MCP handshakes attached) rather than any session's cached agent.
#[tauri::command]
fn list_capabilities(state: State<'_, Arc<GuiState>>) -> Result<Capabilities, String> {
    let inner = state.lock()?;
    let broker = Arc::new(crate::approval::ApprovalBroker::default());
    let base = caps::build_agent(&inner.settings, Arc::clone(&broker));
    let mut agent = base.as_ref().clone();
    inner.attach_mcp_tools(&mut agent, &broker);
    Ok(caps::capabilities_of(&Arc::new(agent), &inner.settings))
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

        let _ = app.emit("stream-finished", StreamFinished { session_id, gen });
    });
}

/// Build the Tauri app and run the event loop (blocks until quit).
pub(crate) fn run() -> anyhow::Result<()> {
    tauri::Builder::default()
        .manage(Arc::new(GuiState::new()))
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
            export_session,
            current_stream,
            send_message,
            retry_last,
            truncate_session,
            fork_session,
            open_url,
            search_files,
            read_file,
            stop_streaming,
            respond_approval,
            compact_session,
            init_agents,
            get_settings,
            save_settings,
            list_capabilities,
            mcp_status,
            reconnect_mcp
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
    Ok(())
}

#[cfg(test)]
mod tests {
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
