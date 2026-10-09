//! User-approval gate for side-effecting tools.
//!
//! When `require_approval` is enabled, tools that can change the machine
//! (`bash`, `write_file`) are wrapped in [`ApprovalGate`]. The gate pauses the
//! agent's tool call, emits an `approval-request` event to the webview, and
//! waits for [`ApprovalBroker::resolve`] from the UI. Denied or expired
//! requests never execute the inner tool.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
    /// Line diff of the pending write (old file vs new content), folded to
    /// a few context lines; `None` for tools without a reviewable change.
    pub(crate) diff: Option<String>,
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
    /// Tools the user allowed permanently. The broker holds no settings, so the
    /// command that records an "always" is what persists it.
    allowed: Mutex<HashSet<String>>,
    /// Tools the user allowed for the session that is running. Cleared when a
    /// different session becomes active, so "this session" means what it says.
    session_allowed: Mutex<HashSet<String>>,
    /// Session/generation that owns the current stream.
    active: Mutex<(String, u64)>,
    /// Webview handle, installed once during app setup.
    handle: OnceLock<AppHandle>,
    /// Per-request wait budget (overridden in tests).
    timeout: Mutex<Duration>,
    /// Workspace root used to resolve write targets for review diffs.
    workspace: Mutex<Option<PathBuf>>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            allowed: Mutex::new(HashSet::new()),
            session_allowed: Mutex::new(HashSet::new()),
            active: Mutex::new((String::new(), 0)),
            handle: OnceLock::new(),
            // NOTE: `Duration::default()` is zero, which would expire instantly.
            timeout: Mutex::new(APPROVAL_TIMEOUT),
            workspace: Mutex::new(None),
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
        let previous = self.active.lock().unwrap().0.clone();
        if previous != session_id {
            // "Allow for this session" ends with the session, not with the turn:
            // a new stream in the same session keeps it, another session does not.
            self.session_allowed.lock().unwrap().clear();
        }
        *self.active.lock().unwrap() = (session_id.to_string(), gen);
    }

    /// Permit `tool` for the rest of the session that is running.
    pub(crate) fn allow_for_session(&self, tool: &str) {
        self.session_allowed
            .lock()
            .unwrap()
            .insert(tool.to_string());
    }

    /// Permit `tool` until it is removed. Persisting it is the caller's job,
    /// because the broker deliberately holds no settings.
    pub(crate) fn allow_always(&self, tool: &str) {
        self.allowed.lock().unwrap().insert(tool.to_string());
    }

    /// Adopt the persistent allowances when a broker is built.
    pub(crate) fn seed_allowed<I: IntoIterator<Item = String>>(&self, tools: I) {
        let mut allowed = self.allowed.lock().unwrap();
        for tool in tools {
            allowed.insert(tool);
        }
    }

    /// Whether `tool` may run right now without asking.
    pub(crate) fn is_allowed(&self, tool: &str) -> bool {
        self.allowed.lock().unwrap().contains(tool)
            || self.session_allowed.lock().unwrap().contains(tool)
    }

    /// Take `tool` off both allowance lists, so it asks again.
    pub(crate) fn disallow(&self, tool: &str) {
        self.allowed.lock().unwrap().remove(tool);
        self.session_allowed.lock().unwrap().remove(tool);
    }

    /// Set the workspace root used to resolve `write_file` review diffs.
    pub(crate) fn set_workspace(&self, root: Option<PathBuf>) {
        *self.workspace.lock().unwrap() = root;
    }

    /// (session, gen) of the current stream.
    fn active_context(&self) -> (String, u64) {
        self.active.lock().unwrap().clone()
    }

    /// Register a pending request, notify the webview, and return its id and
    /// the receiver the gate awaits on.
    fn request(&self, tool: &str, args: &Value) -> (String, tokio::sync::oneshot::Receiver<bool>) {
        // An allowance short-circuits the question rather than answering it, so
        // an allowed tool never reaches the user at all. The empty id says the
        // same thing to the caller: there is nothing for the UI to answer.
        if self.is_allowed(tool) {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(true);
            return (String::new(), rx);
        }
        let (session_id, gen) = self.active_context();
        let id = Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Writes are reviewable: resolve the same target the tool will use
        // and attach a line diff of old vs proposed content.
        let diff = if tool == "write_file" {
            let root = self.workspace.lock().unwrap().clone();
            write_file_diff(&root, args)
        } else {
            None
        };
        let request = ApprovalRequest {
            id: id.clone(),
            session_id,
            gen,
            tool: tool.to_string(),
            args: format_args_preview(args),
            diff,
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

/// Max lines per side for the review diff (LCS is O(n·m)).
const MAX_DIFF_LINES: usize = 1500;
/// Max source size considered for a review diff (matches `read_file`).
const MAX_DIFF_INPUT: usize = 256 * 1024;
/// Context lines kept beside each change before runs fold.
const DIFF_CONTEXT: usize = 3;
/// Cap on the composed preview text.
const MAX_DIFF_BYTES: usize = 24 * 1024;

/// Compute the line diff shown in the approval dialog for a `write_file`.
///
/// Resolves `path` exactly like the tool will (workspace containment
/// included) and reads the current bytes as the old side: a missing file
/// diffs as an all-add, while a non-UTF-8 or oversized file skips the diff
/// (`None` → the dialog falls back to the plain argument preview).
fn write_file_diff(root: &Option<PathBuf>, args: &Value) -> Option<String> {
    let path = args.get("path")?.as_str()?;
    let content = args.get("content")?.as_str()?;
    if content.len() > MAX_DIFF_INPUT {
        return None;
    }
    let target = crate::caps::resolve_path(root, path).ok()?;
    let old = match std::fs::read(&target) {
        Ok(bytes) => {
            if bytes.len() > MAX_DIFF_INPUT || bytes.contains(&0) {
                return None; // oversized or binary: no readable review
            }
            String::from_utf8(bytes).ok()?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    line_diff(&old, content)
}

/// Line-based LCS diff of `old` vs `new`.
///
/// Each output line is prefixed with `' '` (context), `'-'` (old) or `'+'`
/// (new); unchanged runs longer than twice [`DIFF_CONTEXT`] fold into
/// `⋯ N unchanged lines`. Returns `None` when the sides are identical or
/// either exceeds [`MAX_DIFF_LINES`].
fn line_diff(old: &str, new: &str) -> Option<String> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES || a == b {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    // Full LCS table (≤ ~2.25M cells ≈ 9 MiB at the line cap).
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if a[i] == b[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    let mut ops: Vec<(u8, &str)> = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((b' ', a[i]));
            i += 1;
            j += 1;
        } else if table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1] {
            ops.push((b'-', a[i]));
            i += 1;
        } else {
            ops.push((b'+', b[j]));
            j += 1;
        }
    }
    ops.extend((i..n).map(|k| (b'-', a[k])));
    ops.extend((j..m).map(|k| (b'+', b[k])));

    fn push(out: &mut String, prefix: char, text: &str) -> bool {
        if out.len() + text.len() + 2 > MAX_DIFF_BYTES {
            out.push_str("… [diff truncated]\n");
            return false;
        }
        out.push(prefix);
        out.push_str(text);
        out.push('\n');
        true
    }

    // Fold long unchanged runs, keeping DIFF_CONTEXT lines on each side.
    let mut out = String::new();
    let mut k = 0;
    'outer: while k < ops.len() {
        if ops[k].0 != b' ' {
            let (tag, text) = ops[k];
            if !push(&mut out, tag as char, text) {
                break;
            }
            k += 1;
            continue;
        }
        let start = k;
        while k < ops.len() && ops[k].0 == b' ' {
            k += 1;
        }
        let run = k - start;
        if run > 2 * DIFF_CONTEXT {
            let front = if start == 0 { 0 } else { DIFF_CONTEXT };
            let back = if k == ops.len() { 0 } else { DIFF_CONTEXT };
            for (_, text) in &ops[start..start + front] {
                if !push(&mut out, ' ', text) {
                    break 'outer;
                }
            }
            if out.len() + 64 <= MAX_DIFF_BYTES {
                out.push_str(&format!("⋯ {} unchanged lines\n", run - front - back));
            } else {
                break;
            }
            for (_, text) in &ops[k - back..k] {
                if !push(&mut out, ' ', text) {
                    break 'outer;
                }
            }
        } else {
            for (_, text) in &ops[start..k] {
                if !push(&mut out, ' ', text) {
                    break 'outer;
                }
            }
        }
    }
    Some(out)
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
    async fn an_allowed_tool_never_raises_a_request() {
        let broker = ApprovalBroker::default();
        broker.seed_allowed(vec!["bash".to_string()]);
        let (id, rx) = broker.request("bash", &Value::Null);
        assert!(id.is_empty(), "an allowed tool mints no request id: {id:?}");
        assert!(rx.await.expect("resolved"), "it runs without asking");
        assert!(
            broker.pending.lock().unwrap().is_empty(),
            "nothing is left waiting for the user"
        );
    }

    #[test]
    fn a_session_allowance_ends_with_the_session() {
        let broker = ApprovalBroker::default();
        broker.set_active("s1", 1);
        broker.allow_for_session("bash");
        assert!(broker.is_allowed("bash"));
        broker.set_active("s1", 2);
        assert!(
            broker.is_allowed("bash"),
            "a new turn in the same session keeps it"
        );
        broker.set_active("s2", 3);
        assert!(!broker.is_allowed("bash"), "another session starts clean");
    }

    #[test]
    fn seeding_carries_the_allowances_into_a_new_broker() {
        let broker = ApprovalBroker::default();
        broker.seed_allowed(vec!["write_file".to_string(), "bash".to_string()]);
        broker.allow_for_session("other");
        assert!(broker.is_allowed("bash") && broker.is_allowed("write_file"));
    }

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

    /// The diff marks changed lines and folds distant context into a marker.
    #[test]
    fn line_diff_marks_changes_and_folds_context() {
        let old = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let new = old.replace("line 10", "LINE 10");
        let diff = line_diff(&old, &new).expect("one changed line");
        assert!(diff.contains("-line 10"), "old side: {diff}");
        assert!(diff.contains("+LINE 10"), "new side: {diff}");
        assert!(
            diff.contains("unchanged lines"),
            "distant context must fold: {diff}"
        );
        assert_eq!(line_diff(&old, &old), None, "identical sides have no diff");
        let huge = "x\n".repeat(MAX_DIFF_LINES + 1);
        assert_eq!(line_diff(&huge, "y\n"), None, "oversized sides skip");
    }

    /// The write preview reads the old side from disk under the root.
    #[test]
    fn write_file_diff_uses_old_side_and_all_adds_for_new_files() {
        let root = std::env::temp_dir().join(format!("bos-gui-apprdiff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "old line\n").unwrap();

        let diff = write_file_diff(
            &Some(root.clone()),
            &serde_json::json!({"path": "f.txt", "content": "new line\n"}),
        )
        .expect("existing file diffs");
        assert!(diff.contains("-old line"), "{diff}");
        assert!(diff.contains("+new line"), "{diff}");

        let fresh = write_file_diff(
            &Some(root.clone()),
            &serde_json::json!({"path": "fresh.txt", "content": "only add\n"}),
        )
        .expect("missing file diffs as all-add");
        assert!(fresh.starts_with('+'), "{fresh}");
        assert!(!fresh.contains('-'), "nothing to delete: {fresh}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Only write_file requests carry a diff; other tools stay preview-only.
    #[test]
    fn request_attaches_diff_only_for_writes() {
        let root =
            std::env::temp_dir().join(format!("bos-gui-apprdiff-req-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "before\n").unwrap();

        let broker = ApprovalBroker::default();
        broker.set_workspace(Some(root.clone()));
        let (id, _rx) = broker.request(
            "write_file",
            &serde_json::json!({"path": "f.txt", "content": "after\n"}),
        );
        let pending = broker.pending.lock().unwrap();
        let req = &pending[&id].request;
        assert!(req.diff.as_deref().is_some(), "write carries a diff");
        drop(pending);

        let (id2, _rx2) = broker.request("bash", &serde_json::json!({"command": "ls"}));
        let pending = broker.pending.lock().unwrap();
        assert!(
            pending[&id2].request.diff.is_none(),
            "bash stays preview-only"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
