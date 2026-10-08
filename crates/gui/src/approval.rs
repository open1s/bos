//! User-approval gate for side-effecting tools.
//!
//! When `require_approval` is enabled, tools that can change the machine
//! (`bash`, `write_file`) are wrapped in [`ApprovalGate`]. The gate pauses the
//! agent's tool call, emits an `approval-request` event to the webview, and
//! waits for [`ApprovalBroker::resolve`] from the UI. Denied or expired
//! requests never execute the inner tool.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent::tools::AsyncTool;
use agent::{Agent, Tool, ToolError};
use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;
use uuid::Uuid;

/// How long an approval request waits before it is auto-denied.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Payload of the `approval-request` event sent to the webview.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApprovalRequest {
    /// Correlation id passed back to `respond_approval`.
    pub(crate) id: String,
    /// Session the tool call belongs to (may be empty when unknown).
    pub(crate) session_id: String,
    /// Generation of the running stream.
    pub(crate) gen: u64,
    /// Name of the tool that wants to run.
    pub(crate) tool: String,
    /// Pretty-printed JSON input shown to the user.
    pub(crate) args: String,
}

/// A pending approval waiting on the user.
struct Pending {
    /// Payload as emitted; kept for tests and future queue inspection.
    #[cfg_attr(not(test), allow(dead_code))]
    request: ApprovalRequest,
    tx: tokio::sync::oneshot::Sender<bool>,
}

/// Shared between the running agent (awaiting answers) and the GUI commands.
pub(crate) struct ApprovalBroker {
    /// Pending requests by correlation id.
    pending: Mutex<HashMap<String, Pending>>,
    /// Session/generation that owns the current stream.
    active: Mutex<(String, u64)>,
    /// Webview handle, installed once during app setup.
    handle: OnceLock<AppHandle>,
    /// Per-request wait budget (overridden in tests).
    timeout: Mutex<Duration>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new((String::new(), 0)),
            handle: OnceLock::new(),
            // NOTE: `Duration::default()` is zero, which would expire instantly.
            timeout: Mutex::new(APPROVAL_TIMEOUT),
        }
    }
}

impl ApprovalBroker {
    /// Install the webview handle used to emit `approval-request` events.
    pub(crate) fn set_handle(&self, handle: AppHandle) {
        let _ = self.handle.set(handle);
    }

    /// Tag subsequent requests with the session/stream that is running.
    pub(crate) fn set_active(&self, session_id: &str, gen: u64) {
        *self.active.lock().unwrap() = (session_id.to_string(), gen);
    }

    /// (session, gen) of the current stream.
    fn active_context(&self) -> (String, u64) {
        self.active.lock().unwrap().clone()
    }

    /// Register a pending request, notify the webview, and return its id and
    /// the receiver the gate awaits on.
    fn request(&self, tool: &str, args: &Value) -> (String, tokio::sync::oneshot::Receiver<bool>) {
        let (session_id, gen) = self.active_context();
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let request = ApprovalRequest {
            id: id.clone(),
            session_id,
            gen,
            tool: tool.to_string(),
            args: format_args_preview(args),
        };
        self.pending.lock().unwrap().insert(
            id.clone(),
            Pending {
                request: request.clone(),
                tx,
            },
        );
        if let Some(handle) = self.handle.get() {
            use tauri::Emitter;
            let _ = handle.emit("approval-request", request);
        }
        (id, rx)
    }

    /// Answer a pending request from the UI. Returns whether the id matched a
    /// live request (a stale id means the stream already moved on).
    pub(crate) fn resolve(&self, id: &str, approved: bool) -> bool {
        match self.pending.lock().unwrap().remove(id) {
            Some(pending) => pending.tx.send(approved).is_ok(),
            None => false,
        }
    }

    /// Deny every pending request (stream stopped or session reset), so no
    /// gate task waits forever on an answer that will never come.
    pub(crate) fn reject_all(&self) {
        let drained: Vec<Pending> = self
            .pending
            .lock()
            .unwrap()
            .drain()
            .map(|(_, p)| p)
            .collect();
        for pending in drained {
            let _ = pending.tx.send(false);
        }
    }

    /// Override the wait budget (tests only).
    #[cfg(test)]
    fn set_timeout(&self, timeout: Duration) {
        *self.timeout.lock().unwrap() = timeout;
    }

    fn timeout(&self) -> Duration {
        *self.timeout.lock().unwrap()
    }
}

/// Short preview of a tool's JSON input for the approval dialog.
fn format_args_preview(args: &Value) -> String {
    let text = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
    const MAX: usize = 4000;
    if text.len() > MAX {
        let mut cut = MAX;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}\n… (truncated)", &text[..cut])
    } else {
        text
    }
}

/// Wraps a side-effecting tool so it only runs after the user approves.
pub(crate) struct ApprovalGate {
    inner: Arc<dyn Tool>,
    broker: Arc<ApprovalBroker>,
}

impl ApprovalGate {
    /// Gate `inner` behind `broker` approval requests.
    pub(crate) fn new(inner: Arc<dyn Tool>, broker: Arc<ApprovalBroker>) -> Self {
        Self { inner, broker }
    }
}

#[async_trait::async_trait]
impl AsyncTool for ApprovalGate {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let (id, rx) = self.broker.request(self.inner.name(), input);
        let approved = match tokio::time::timeout(self.broker.timeout(), rx).await {
            Ok(Ok(approved)) => approved,
            // The broker dropped the sender (stream stopped): deny.
            Ok(Err(_)) => false,
            // Nobody answered in time: clean up and deny.
            Err(_) => {
                self.broker.resolve(&id, false);
                return Err(ToolError::Failed(format!(
                    "{}: approval timed out",
                    self.inner.name()
                )));
            }
        };
        if !approved {
            return Err(ToolError::Failed(format!(
                "{}: denied by user",
                self.inner.name()
            )));
        }
        // The inner tool may block (long-running bash): keep the executor free.
        let name = self.inner.name().to_string();
        let tool = self.inner.clone();
        let input = input.clone();
        tokio::task::spawn_blocking(move || tool.run(&input))
            .await
            .map_err(|e| ToolError::Failed(format!("{name}: task failed: {e}")))?
    }
}

/// The async counterpart to [`ApprovalGate`] for tools that are async already.
///
/// MCP tools talk to external servers over the async seam, so they cannot be
/// wrapped in the sync [`ApprovalGate`]. This wrapper runs the same
/// request → wait → deny-on-timeout flow and then awaits the inner tool
/// directly. Every trait method delegates to `inner`, including `name()`,
/// which the tool registry keys on (so a gated MCP tool keeps its
/// `{namespace}_{tool}` identity).
pub(crate) struct GatedAsyncTool {
    inner: Arc<dyn AsyncTool>,
    broker: Arc<ApprovalBroker>,
}

impl GatedAsyncTool {
    /// Gate `inner` behind `broker` approval requests.
    pub(crate) fn new(inner: Arc<dyn AsyncTool>, broker: Arc<ApprovalBroker>) -> Self {
        Self { inner, broker }
    }
}

#[async_trait::async_trait]
impl AsyncTool for GatedAsyncTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let (id, rx) = self.broker.request(self.inner.name(), input);
        let approved = match tokio::time::timeout(self.broker.timeout(), rx).await {
            Ok(Ok(approved)) => approved,
            // The broker dropped the sender (stream stopped): deny.
            Ok(Err(_)) => false,
            // Nobody answered in time: clean up and deny.
            Err(_) => {
                self.broker.resolve(&id, false);
                return Err(ToolError::Failed(format!(
                    "{}: approval timed out",
                    self.inner.name()
                )));
            }
        };
        if !approved {
            return Err(ToolError::Failed(format!(
                "{}: denied by user",
                self.inner.name()
            )));
        }
        self.inner.run(input).await
    }
}

/// Register `tool` on `agent`, gated behind approval when `required`.
pub(crate) fn add_gated_tool(
    agent: &mut Agent,
    tool: Arc<dyn Tool>,
    required: bool,
    broker: Arc<ApprovalBroker>,
) {
    if required {
        let _ = agent.try_add_async_tool(Arc::new(ApprovalGate::new(tool, broker)));
    } else {
        agent.add_tool(tool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_tool() -> Arc<dyn Tool> {
        Arc::new(agent::tools::FunctionTool::new(
            "echo",
            "echo",
            serde_json::json!({"type": "object"}),
            |input: &Value| Ok(input.clone()),
        ))
    }

    /// The gate must not run the inner tool before the user approves.
    #[tokio::test]
    async fn gate_waits_and_runs_after_approval() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = ApprovalGate::new(echo_tool(), broker.clone());

        let task = tokio::spawn(async move { gate.run(&serde_json::json!({"msg": "hi"})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            broker.pending.lock().unwrap().len(),
            1,
            "must be pending before approval"
        );

        let id = broker
            .pending
            .lock()
            .unwrap()
            .keys()
            .next()
            .cloned()
            .unwrap();
        assert!(broker.resolve(&id, true), "pending id must match");
        let out = task.await.unwrap().expect("approved run succeeds");
        assert_eq!(out["msg"], "hi");
        assert!(broker.pending.lock().unwrap().is_empty(), "map drained");
    }

    /// A denial must surface as a tool error and never execute the tool.
    #[tokio::test]
    async fn gate_denies_when_user_declines() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = ApprovalGate::new(echo_tool(), broker.clone());
        let task = tokio::spawn(async move { gate.run(&serde_json::json!({})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let id = broker
            .pending
            .lock()
            .unwrap()
            .keys()
            .next()
            .cloned()
            .unwrap();
        assert!(broker.resolve(&id, false));
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("denied"), "unexpected: {err}");
    }

    /// Stopping the stream rejects pending approvals so no gate hangs.
    #[tokio::test]
    async fn reject_all_denies_pending_requests() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = ApprovalGate::new(echo_tool(), broker.clone());
        let task = tokio::spawn(async move { gate.run(&serde_json::json!({})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        broker.reject_all();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("denied"), "unexpected: {err}");
        assert!(broker.pending.lock().unwrap().is_empty());
    }

    /// An unanswered request expires instead of hanging the agent forever.
    #[tokio::test]
    async fn unanswered_request_times_out() {
        let broker = Arc::new(ApprovalBroker::default());
        broker.set_timeout(Duration::from_millis(50));
        let gate = ApprovalGate::new(echo_tool(), broker.clone());
        let err = gate.run(&serde_json::json!({})).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "unexpected: {err}");
        assert!(
            broker.pending.lock().unwrap().is_empty(),
            "expired entry removed"
        );
    }

    /// The gate preserves the wrapped tool's schema and name for the LLM.
    #[test]
    fn gate_delegates_metadata() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = ApprovalGate::new(echo_tool(), broker);
        assert_eq!(gate.name(), "echo");
        assert_eq!(gate.json_schema()["type"], "object");
        assert!(!gate.description().is_empty());
    }

    /// Approval preview truncates huge inputs instead of flooding the dialog.
    #[test]
    fn preview_truncates_long_input() {
        let big = Value::String("x".repeat(10_000));
        let preview = format_args_preview(&big);
        assert!(preview.len() < 5_000, "preview must be bounded");
        assert!(preview.contains("truncated"));
    }

    /// Requests are tagged with the session/stream they belong to.
    #[tokio::test]
    async fn requests_carry_active_stream_context() {
        let broker = Arc::new(ApprovalBroker::default());
        broker.set_active("sess-1", 7);
        let (_id, _rx) = broker.request("bash", &serde_json::json!({}));
        let pending = broker.pending.lock().unwrap();
        let request = pending.values().next().unwrap().request.clone();
        assert_eq!(request.session_id, "sess-1");
        assert_eq!(request.gen, 7);
        assert_eq!(request.tool, "bash");
    }

    /// The timeout budget is configurable (used by the expiry test).
    #[test]
    fn timeout_budget_is_configurable() {
        let broker = ApprovalBroker::default();
        broker.set_timeout(Duration::from_millis(30));
        assert_eq!(broker.timeout(), Duration::from_millis(30));
    }

    /* ---- GatedAsyncTool (MCP) ---- */

    fn async_echo_tool() -> Arc<dyn AsyncTool> {
        Arc::new(agent::tools::AsyncFunctionTool::new(
            "files_read",
            "async echo",
            serde_json::json!({"type": "object"}),
            |input: &Value| {
                let value = input.clone();
                Box::pin(async move { Ok(value) })
            },
        ))
    }

    /// An MCP tool behind the gate must wait for approval before it runs.
    #[tokio::test]
    async fn gated_async_tool_waits_then_runs() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = GatedAsyncTool::new(async_echo_tool(), broker.clone());

        let task = tokio::spawn(async move { gate.run(&serde_json::json!({"msg": "hi"})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            broker.pending.lock().unwrap().len(),
            1,
            "must be pending before approval"
        );

        let id = broker
            .pending
            .lock()
            .unwrap()
            .keys()
            .next()
            .cloned()
            .unwrap();
        assert!(broker.resolve(&id, true), "pending id must match");
        let out = task.await.unwrap().expect("approved run succeeds");
        assert_eq!(out["msg"], "hi");
        assert!(broker.pending.lock().unwrap().is_empty(), "map drained");
    }

    /// A denial must surface as a tool error, matching the sync gate's wording.
    #[tokio::test]
    async fn gated_async_tool_denies_when_user_declines() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = GatedAsyncTool::new(async_echo_tool(), broker.clone());
        let task = tokio::spawn(async move { gate.run(&serde_json::json!({})).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let id = broker
            .pending
            .lock()
            .unwrap()
            .keys()
            .next()
            .cloned()
            .unwrap();
        assert!(broker.resolve(&id, false));
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("denied"), "unexpected: {err}");
    }

    /// The async gate keeps the namespaced MCP identity visible to the LLM.
    #[test]
    fn gated_async_tool_delegates_metadata() {
        let broker = Arc::new(ApprovalBroker::default());
        let gate = GatedAsyncTool::new(async_echo_tool(), broker);
        assert_eq!(gate.name(), "files_read");
        assert_eq!(gate.json_schema()["type"], "object");
        assert!(!gate.description().is_empty());
    }
}
