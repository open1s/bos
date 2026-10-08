//! Tauri backend: IPC commands, the streaming event bridge, and app bootstrap.
//!
//! The web frontend (`ui/`) talks to this layer through `invoke` calls and
//! receives stream chunks via the `agent-event` / `stream-finished` events.
//! Session persistence, settings, and agent streaming live in the sibling
//! modules ([`crate::session`], [`crate::settings`], [`crate::runner`]).

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

/// Mutable GUI state shared by all commands and the streaming bridge.
struct Inner {
    store: SessionStore,
    settings: Settings,
    settings_gen: u64,
    agent_cache: Option<(u64, Arc<Agent>)>,
    runner: Runner,
    streaming: Option<Streaming>,
    stream_gen: u64,
    approvals: Arc<crate::approval::ApprovalBroker>,
    /// Per-server results of the most recent MCP connect pass.
    mcp_status: Vec<crate::mcp::McpStatus>,
    /// Whether an MCP connect pass for the current `settings_gen` is running.
    mcp_connecting: bool,
}

impl Inner {
    fn new() -> Self {
        Self {
            store: SessionStore::default_store(),
            settings: Settings::load(),
            settings_gen: 0,
            agent_cache: None,
            runner: Runner::new(),
            streaming: None,
            stream_gen: 0,
            approvals: Arc::new(crate::approval::ApprovalBroker::default()),
            mcp_status: Vec::new(),
            mcp_connecting: false,
        }
    }

    /// The agent for the current settings, rebuilt when settings change.
    fn agent(&mut self) -> Arc<Agent> {
        if let Some((gen, agent)) = &self.agent_cache {
            if *gen == self.settings_gen {
                return Arc::clone(agent);
            }
        }
        let agent = caps::build_agent(&self.settings, Arc::clone(&self.approvals));
        self.agent_cache = Some((self.settings_gen, Arc::clone(&agent)));
        agent
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
/// Registration then happens on a clone of the cached agent (the registry is
/// `Arc` copy-on-write, so a stream holding the old `Arc` keeps running
/// untouched) under one short lock. A generation change — the user saved
/// settings again — aborts the pass at each lock so a stale connect never
/// overwrites fresher state.
fn spawn_mcp_connect(state: Arc<GuiState>, gen: u64) {
    tauri::async_runtime::spawn(async move {
        let (entries, gate) = {
            let Ok(mut inner) = state.lock() else {
                return;
            };
            if inner.settings_gen != gen {
                return;
            }
            inner.mcp_connecting = true;
            (
                inner.settings.mcp_servers.clone(),
                inner
                    .settings
                    .require_approval
                    .then(|| Arc::clone(&inner.approvals)),
            )
        };

        // Handshake every enabled server; one failure must not block the rest.
        let mut statuses = Vec::new();
        let mut ready: Vec<(String, crate::mcp::Handshake)> = Vec::new();
        for entry in entries.iter().filter(|e| e.enabled) {
            let name = entry.name.trim().to_string();
            match crate::mcp::preflight(entry).await {
                Ok(handshake) => {
                    statuses.push(crate::mcp::McpStatus {
                        name: name.clone(),
                        transport: entry.transport.clone(),
                        enabled: true,
                        connected: true,
                        tools: handshake.tools.len(),
                        error: None,
                    });
                    ready.push((name, handshake));
                }
                Err(err) => statuses.push(crate::mcp::McpStatus {
                    name,
                    transport: entry.transport.clone(),
                    enabled: true,
                    connected: false,
                    tools: 0,
                    error: Some(err),
                }),
            }
        }
        // Disabled servers still appear in the report so the UI can show them.
        for entry in entries.iter().filter(|e| !e.enabled) {
            statuses.push(crate::mcp::McpStatus {
                name: entry.name.trim().to_string(),
                transport: entry.transport.clone(),
                enabled: false,
                connected: false,
                tools: 0,
                error: None,
            });
        }

        let Ok(mut inner) = state.lock() else {
            return;
        };
        if inner.settings_gen != gen {
            return;
        }
        let mut agent = inner.agent().as_ref().clone();
        for (namespace, handshake) in &ready {
            for def in &handshake.tools {
                let tool = crate::mcp::tool_for(handshake, namespace, def, gate.clone());
                if let Err(err) = agent.add_mcp_tool(namespace, &def.name, tool) {
                    if let Some(status) = statuses.iter_mut().find(|s| s.name == *namespace) {
                        status.connected = false;
                        status.error = Some(err.to_string());
                    }
                    break;
                }
            }
        }
        // Recount from the registry so the report matches what is attached.
        if let Some(reg) = agent.registry() {
            let names = reg.async_tool_names();
            for status in statuses.iter_mut().filter(|s| s.connected) {
                let prefix = format!("{}_", status.name);
                status.tools = names.iter().filter(|n| n.starts_with(&prefix)).count();
            }
        }
        inner.agent_cache = Some((gen, Arc::new(agent)));
        inner.mcp_status = statuses;
        inner.mcp_connecting = false;
    });
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
    /// Token usage reported by the provider.
    Usage {
        /// Prompt (input) tokens.
        prompt: u64,
        /// Completion (output) tokens.
        completion: u64,
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
    let record = SessionRecord::new();
    inner.store.save(&record).map_err(|e| e.to_string())?;
    Ok(summary(&record))
}

/// Delete a session's stored file, stopping its stream if it is running.
#[tauri::command]
fn delete_session(state: State<'_, Arc<GuiState>>, id: String) -> Result<(), String> {
    let mut inner = state.lock()?;
    if inner.streaming.as_ref().is_some_and(|s| s.session_id == id) {
        inner.runner.stop();
        inner.streaming = None;
        inner.stream_gen += 1;
    }
    inner.store.delete(&id);
    Ok(())
}

/// Load a session's messages.
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

/// The currently active stream, if any (used to restore UI state).
#[tauri::command]
fn current_stream(state: State<'_, Arc<GuiState>>) -> Result<Option<Streaming>, String> {
    Ok(state.lock()?.streaming.clone())
}

/// Start a fresh stream for `text` given the prior history `transcript`.
///
/// The caller must hold the state lock; the forwarder is attached by the
/// command afterwards.
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
    // Approval requests raised during this turn carry this context.
    inner.approvals.set_active(session_id, streaming.gen);
    let agent = inner.agent();
    let rx = inner.runner.spawn(agent, text, transcript);
    inner.streaming = Some(streaming.clone());
    (rx, streaming)
}

/// Persist the user turn and start streaming an assistant reply.
#[tauri::command]
fn send_message(
    app: AppHandle,
    state: State<'_, Arc<GuiState>>,
    session_id: String,
    text: String,
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

        // Seed the transcript from the stored history so the agent sees it.
        let transcript = record.messages.clone();
        record.messages.push(ChatMessage::user(text.clone()));
        record.messages.push(ChatMessage::assistant());
        record.touch();
        inner.store.save(&record).map_err(|e| e.to_string())?;
        begin_stream(&mut inner, &session_id, text, transcript)
    };

    spawn_forwarder(state.inner().clone(), app, rx, session_id, streaming.gen);
    Ok(streaming)
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
        if inner.streaming.is_some() {
            return Err("a reply is already streaming".to_string());
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

/// Abort the in-flight stream; the forwarder persists the partial turn.
///
/// Pending approval requests are denied first so no gated tool stays
/// suspended waiting for an answer that will never come.
#[tauri::command]
fn stop_streaming(state: State<'_, Arc<GuiState>>) -> Result<(), String> {
    let mut inner = state.lock()?;
    inner.approvals.reject_all();
    inner.runner.stop();
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
    // A stale id simply means the stream already resolved or timed out.
    inner.approvals.resolve(&id, approved);
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
    temperature: f32,
    reasoning_effort: Option<String>,
    bash_enabled: bool,
    file_tools_enabled: bool,
    bash_workspace: String,
    skills_dir: String,
    require_approval: bool,
    mcp_servers: Vec<crate::settings::McpServerEntry>,
) -> Result<Settings, String> {
    // Reject a bad server list before touching any state.
    crate::settings::validate_mcp_servers(&mcp_servers)?;
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
    inner.settings.require_approval = require_approval;
    inner.settings.mcp_servers = mcp_servers;
    inner.settings.save().map_err(|e| e.to_string())?;
    // Pending approvals belong to the old agent: deny them before rebuild.
    inner.approvals.reject_all();
    // Persist the raw model string (a profile name survives reload and is
    // re-resolved there), but hand the resolved endpoint to the agent now.
    inner.settings.resolve_profile();
    inner.settings_gen += 1;
    inner.agent_cache = None;
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

/// List the agent's registered tools, skills, and plugins.
#[tauri::command]
fn list_capabilities(state: State<'_, Arc<GuiState>>) -> Result<Capabilities, String> {
    let mut inner = state.lock()?;
    let agent = inner.agent();
    Ok(caps::capabilities_of(&agent, &inner.settings))
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
/// The agent cache is dropped first so the fresh pass rebuilds from the base
/// configuration — otherwise tools from the previous pass would collide on
/// registration. A pass that is already running is left alone.
#[tauri::command]
fn reconnect_mcp(state: State<'_, Arc<GuiState>>) -> Result<(), String> {
    let (gen, connect) = {
        let mut inner = state.lock()?;
        if inner.mcp_connecting {
            return Ok(());
        }
        inner.agent_cache = None;
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
        loop {
            let Ok(event) = rx.recv().await else {
                break;
            };
            let terminal = matches!(event, AgentEvent::Done | AgentEvent::Error(_));
            let payload = AgentMessage {
                session_id: session_id.clone(),
                gen,
                event: match event {
                    AgentEvent::Text(text) => StreamKind::Text { text },
                    AgentEvent::Reasoning(text) => StreamKind::Reasoning { text },
                    AgentEvent::ToolCall(tool) => StreamKind::Tool {
                        name: tool.name,
                        args: tool.args,
                    },
                    AgentEvent::Usage { prompt, completion } => {
                        StreamKind::Usage { prompt, completion }
                    }
                    AgentEvent::Done => StreamKind::Done,
                    AgentEvent::Error(message) => StreamKind::Error { message },
                },
            };
            let _ = app.emit("agent-event", &payload);
            if terminal {
                break;
            }
        }

        // Finalize: drop the empty placeholder, derive a title, persist.
        {
            let mut inner = match state.lock() {
                Ok(inner) => inner,
                Err(_) => return,
            };
            let newer = inner.streaming.as_ref().is_some_and(|s| s.gen != gen);
            if !newer {
                if let Some(mut record) = inner
                    .store
                    .load_all()
                    .into_iter()
                    .find(|s| s.id == session_id)
                {
                    let pop_placeholder = record.messages.len() > 1
                        && record
                            .messages
                            .last()
                            .is_some_and(|m| m.is_blank() && m.error.is_none());
                    if pop_placeholder {
                        record.messages.pop();
                    }
                    record.derive_title();
                    record.touch();
                    let _ = inner.store.save(&record);
                }
                if inner.streaming.as_ref().is_some_and(|s| s.gen == gen) {
                    inner.streaming = None;
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
            // The broker emits `approval-request` events from inside the agent.
            let state = app.state::<Arc<GuiState>>();
            let (gen, connect) = {
                let inner = state.lock().map_err(Box::<dyn std::error::Error>::from)?;
                inner.approvals.set_handle(app.handle().clone());
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
            current_stream,
            send_message,
            retry_last,
            open_url,
            stop_streaming,
            respond_approval,
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
}
