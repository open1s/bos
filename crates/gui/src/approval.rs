//! User-approval gate for side-effecting tools.
//!
//! When `require_approval` is enabled, tools that can change the machine
//! (`bash`, `write_file`) are wrapped in [`ApprovalGate`]. The gate pauses the
//! agent's tool call, emits an `approval-request` event to the webview, and
//! waits for [`ApprovalBroker::resolve`] from the UI. Denied or expired
//! requests never execute the inner tool.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent::tools::AsyncTool;
use agent::{Agent, Tool, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use uuid::Uuid;

use crate::session::unix_now;

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
    /// Scoped persistent rules (M4). Without a book the broker behaves
    /// exactly as before: allowances plus a question to the user.
    rules: Mutex<Option<Arc<RuleBook>>>,
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
            rules: Mutex::new(None),
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

    /// Attach the scoped rule book this broker consults before asking.
    pub(crate) fn set_rules(&self, rules: Arc<RuleBook>) {
        *self.rules.lock().unwrap() = Some(rules);
    }

    /// The rule book attached to this broker, if any.
    pub(crate) fn rules(&self) -> Option<Arc<RuleBook>> {
        self.rules.lock().unwrap().clone()
    }

    /// The workspace/session a rule is evaluated against.
    fn rule_context(&self) -> RuleContext {
        RuleContext {
            session: self.active.lock().unwrap().0.clone(),
            workspace: self
                .workspace
                .lock()
                .unwrap()
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    /// (session, gen) of the current stream.
    fn active_context(&self) -> (String, u64) {
        self.active.lock().unwrap().clone()
    }

    /// Register a pending request, notify the webview, and return its id and
    /// the receiver the gate awaits on.
    fn request(&self, tool: &str, args: &Value) -> (String, tokio::sync::oneshot::Receiver<bool>) {
        // Scoped persistent rules decide first: a deny must beat a stale
        // allowance, and an allow must never reach the user. A rule that
        // matches nothing leaves the answer to the allowances and the dialog.
        if let Some(book) = self.rules() {
            let decision = book.decide(tool, args, &self.rule_context());
            match decision.decision {
                Decision::Allow => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(true);
                    return (String::new(), rx);
                }
                Decision::Deny => {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(false);
                    return (String::new(), rx);
                }
                Decision::Ask => {}
            }
        }
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

/// How many decisions the audit ring keeps before it drops the oldest.
pub(crate) const AUDIT_CAP: usize = 200;

/// What a matching rule says a tool call may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RuleEffect {
    /// Run the tool without asking.
    Allow,
    /// Refuse the tool without asking.
    Deny,
}

/// How long a rule is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RuleScope {
    /// Every workspace and session.
    Global,
    /// The workspace the rule was created in.
    Workspace,
    /// One session only.
    Session,
}

/// How an argument predicate compares the call's argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MatchKind {
    /// The argument equals the rule's value exactly.
    Exact,
    /// The argument starts with the rule's value.
    Prefix,
    /// The argument contains the rule's value anywhere.
    Contains,
}

/// Optional argument predicate narrowing a rule to one kind of call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArgPredicate {
    /// Argument name (JSON object key) the predicate reads.
    pub(crate) key: String,
    /// How `value` is compared.
    pub(crate) match_kind: MatchKind,
    /// The text compared against the argument.
    pub(crate) value: String,
}

/// One persistent allow/deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApprovalRule {
    /// Stable identifier used to revoke the rule.
    pub(crate) id: String,
    /// Tool name, exactly or as a `*`/`?` glob.
    pub(crate) tool: String,
    /// Argument predicate, when the rule is narrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) arg: Option<ArgPredicate>,
    /// How long the rule lasts.
    pub(crate) scope: RuleScope,
    /// Whether a match allows or denies.
    pub(crate) effect: RuleEffect,
    /// Unix seconds the rule was created.
    pub(crate) created_at: u64,
    /// Workspace a `workspace`-scoped rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace: Option<String>,
    /// Session a `session`-scoped rule applies to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
}

/// What the rule book decided for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// A matching rule allows the call.
    Allow,
    /// A matching rule denies the call.
    Deny,
    /// No rule matched; the user is asked.
    Ask,
}

/// A decision plus the rule that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuleDecision {
    /// The decided outcome.
    pub(crate) decision: Decision,
    /// The winning rule, when one matched.
    pub(crate) rule: Option<ApprovalRule>,
}

/// Workspace/session a rule is matched against.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuleContext {
    /// Active session id (empty when none is known).
    pub(crate) session: String,
    /// Active workspace root (empty when none is set).
    pub(crate) workspace: String,
}

/// One decision the gate made, kept in a bounded ring for `/audit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AuditEntry {
    /// Unix seconds the decision was made.
    pub(crate) at: u64,
    /// Tool the decision was about.
    pub(crate) tool: String,
    /// Bounded preview of the call's arguments.
    pub(crate) args: String,
    /// `"allow"`, `"deny"`, or `"ask"`.
    pub(crate) decision: String,
    /// Rule that produced the decision, when one matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rule_id: Option<String>,
    /// The winning rule's scope, when one matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    /// Session the call belonged to (empty when unknown).
    pub(crate) session: String,
}

/// The rules a caller wants to add, before the book assigns an id and time.
#[derive(Debug, Clone)]
pub(crate) struct NewRule {
    /// Tool name, exactly or as a glob.
    pub(crate) tool: String,
    /// Optional argument predicate.
    pub(crate) arg: Option<ArgPredicate>,
    /// How long the rule lasts.
    pub(crate) scope: RuleScope,
    /// Allow or deny.
    pub(crate) effect: RuleEffect,
    /// Workspace a `workspace`-scoped rule applies to.
    pub(crate) workspace: Option<String>,
    /// Session a `session`-scoped rule applies to.
    pub(crate) session: Option<String>,
}

/// On-disk shape of the rules file.
#[derive(Debug, Serialize, Deserialize)]
struct RulesFile {
    /// Format version; bumped only if the shape ever changes incompatibly.
    version: u32,
    /// The rules themselves.
    rules: Vec<ApprovalRule>,
}

/// The rules file version this build writes and understands.
const RULES_VERSION: u32 = 1;

/// Scoped persistent allow/deny rules plus the audit ring of decisions.
///
/// The book owns its file, so a malformed document is a warning rather than a
/// failure: unparseable entries are skipped and reported, and the rest of the
/// book still works.
pub(crate) struct RuleBook {
    /// Where the rules persist.
    path: PathBuf,
    /// Live rules, in insertion order.
    rules: Mutex<Vec<ApprovalRule>>,
    /// Bounded ring of the most recent decisions.
    audit: Mutex<VecDeque<AuditEntry>>,
    /// Problems found while loading (a malformed file or bad entries).
    warnings: Mutex<Vec<String>>,
}

impl RuleBook {
    /// The conventional rules file: beside the settings file the GUI already
    /// uses (`~/.bos/gui/approval_rules.json`).
    pub(crate) fn default_path() -> PathBuf {
        crate::settings::Settings::default_path()
            .parent()
            .map(|dir| dir.join("approval_rules.json"))
            .unwrap_or_else(|| PathBuf::from("approval_rules.json"))
    }

    /// Open the book at its conventional path.
    pub(crate) fn open_default() -> Self {
        Self::open(Self::default_path())
    }

    /// Load the book from `path`, tolerating a missing or malformed file.
    pub(crate) fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (rules, warnings) = load_rules(&path);
        Self {
            path,
            rules: Mutex::new(rules),
            audit: Mutex::new(VecDeque::new()),
            warnings: Mutex::new(warnings),
        }
    }

    /// The rules, in insertion order.
    pub(crate) fn rules(&self) -> Vec<ApprovalRule> {
        self.rules.lock().unwrap().clone()
    }

    /// Problems found while loading, oldest first.
    pub(crate) fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }

    /// The most recent `limit` decisions, newest first.
    pub(crate) fn audit(&self, limit: usize) -> Vec<AuditEntry> {
        let ring = self.audit.lock().unwrap();
        ring.iter().rev().take(limit).cloned().collect()
    }

    /// Add a rule, persist the book, and return the stored rule.
    pub(crate) fn add(&self, spec: NewRule) -> Result<ApprovalRule, String> {
        let tool = spec.tool.trim().to_string();
        if tool.is_empty() {
            return Err("a rule needs a tool name".to_string());
        }
        if let Some(arg) = &spec.arg {
            if arg.key.trim().is_empty() {
                return Err("an argument predicate needs a key".to_string());
            }
        }
        let rule = ApprovalRule {
            id: Uuid::new_v4().to_string(),
            tool,
            arg: spec.arg.map(|mut arg| {
                arg.key = arg.key.trim().to_string();
                arg
            }),
            scope: spec.scope,
            effect: spec.effect,
            created_at: unix_now(),
            workspace: spec.workspace.filter(|w| !w.trim().is_empty()),
            session: spec.session.filter(|s| !s.trim().is_empty()),
        };
        {
            let mut rules = self.rules.lock().unwrap();
            rules.push(rule.clone());
        }
        self.save().map_err(|e| e.to_string())?;
        Ok(rule)
    }

    /// Remove a rule by id. Returns whether it was there.
    pub(crate) fn revoke(&self, id: &str) -> Result<bool, String> {
        let removed = {
            let mut rules = self.rules.lock().unwrap();
            let before = rules.len();
            rules.retain(|rule| rule.id != id);
            rules.len() != before
        };
        if removed {
            self.save().map_err(|e| e.to_string())?;
        }
        Ok(removed)
    }

    /// Write the rules (and clear the load warnings) to disk.
    pub(crate) fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let rules = self.rules.lock().unwrap().clone();
        let doc = RulesFile {
            version: RULES_VERSION,
            rules,
        };
        let text = serde_json::to_string_pretty(&doc)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&self.path, text)
    }

    /// Decide one call and record it in the audit ring.
    ///
    /// Precedence: a matching deny beats a matching allow, and within one
    /// effect the most specific rule wins (session over workspace over global,
    /// an exact tool over a glob, an argument predicate over none). No match
    /// means the user is asked.
    pub(crate) fn decide(&self, tool: &str, args: &Value, ctx: &RuleContext) -> RuleDecision {
        let mut best: Option<ApprovalRule> = None;
        {
            let rules = self.rules.lock().unwrap();
            for rule in rules.iter().filter(|r| rule_matches(r, tool, args, ctx)) {
                if best
                    .as_ref()
                    .is_none_or(|cur| rule_rank(rule) > rule_rank(cur))
                {
                    best = Some(rule.clone());
                }
            }
        }
        let decision = match best.as_ref().map(|r| r.effect) {
            Some(RuleEffect::Allow) => Decision::Allow,
            Some(RuleEffect::Deny) => Decision::Deny,
            None => Decision::Ask,
        };
        let entry = AuditEntry {
            at: unix_now(),
            tool: tool.to_string(),
            args: format_args_preview(args),
            decision: match decision {
                Decision::Allow => "allow",
                Decision::Deny => "deny",
                Decision::Ask => "ask",
            }
            .to_string(),
            rule_id: best.as_ref().map(|r| r.id.clone()),
            scope: best.as_ref().map(|r| scope_name(r.scope).to_string()),
            session: ctx.session.clone(),
        };
        self.record(entry);
        RuleDecision {
            decision,
            rule: best,
        }
    }

    /// Push one decision onto the bounded ring, dropping the oldest.
    fn record(&self, entry: AuditEntry) {
        let mut ring = self.audit.lock().unwrap();
        if ring.len() >= AUDIT_CAP {
            ring.pop_front();
        }
        ring.push_back(entry);
    }
}

/// Lower-case name of a scope, for audit entries and the UI.
fn scope_name(scope: RuleScope) -> &'static str {
    match scope {
        RuleScope::Global => "global",
        RuleScope::Workspace => "workspace",
        RuleScope::Session => "session",
    }
}

/// Match `pattern` (`*` any run, `?` one character) against `text`.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = i;
            i += 1;
            mark = j;
        } else if star != usize::MAX {
            i = star + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// Whether a rule's argument predicate accepts the call's arguments.
fn predicate_matches(pred: &ArgPredicate, args: &Value) -> bool {
    let Some(raw) = args.get(&pred.key) else {
        return false;
    };
    let text = match raw {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match pred.match_kind {
        MatchKind::Exact => text == pred.value,
        MatchKind::Prefix => text.starts_with(&pred.value),
        MatchKind::Contains => text.contains(&pred.value),
    }
}

/// Whether `rule` applies to this call at all.
fn rule_matches(rule: &ApprovalRule, tool: &str, args: &Value, ctx: &RuleContext) -> bool {
    if !wildcard_match(&rule.tool, tool) {
        return false;
    }
    match rule.scope {
        RuleScope::Global => {}
        RuleScope::Workspace => {
            if rule.workspace.as_deref() != Some(ctx.workspace.as_str()) {
                return false;
            }
        }
        RuleScope::Session => {
            if rule.session.as_deref() != Some(ctx.session.as_str()) {
                return false;
            }
        }
    }
    rule.arg
        .as_ref()
        .is_none_or(|pred| predicate_matches(pred, args))
}

/// Sort key for rule precedence: effect first (deny beats allow), then
/// specificity (session > workspace > global, exact tool > glob, predicate > none).
fn rule_rank(rule: &ApprovalRule) -> (u8, u8, u8, u8) {
    let effect = match rule.effect {
        RuleEffect::Allow => 0,
        RuleEffect::Deny => 1,
    };
    let scope = match rule.scope {
        RuleScope::Global => 0,
        RuleScope::Workspace => 1,
        RuleScope::Session => 2,
    };
    let tool = if rule.tool.contains(['*', '?']) { 0 } else { 1 };
    let arg = u8::from(rule.arg.is_some());
    (effect, scope, tool, arg)
}

/// Read the rules file, skipping entries that do not parse.
///
/// A missing file is normal (no rules yet). A file that is not valid JSON, or
/// is not the expected document, yields no rules and one warning; a single
/// bad entry inside an otherwise valid document is skipped with its own
/// warning, so one broken rule never discards the rest.
fn load_rules(path: &Path) -> (Vec<ApprovalRule>, Vec<String>) {
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), Vec::new());
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(err) => {
            return (
                Vec::new(),
                vec![format!("{}: not valid JSON ({err})", path.display())],
            )
        }
    };
    let entries = match value.get("rules").and_then(|rules| rules.as_array()) {
        Some(entries) => entries,
        None => {
            return (
                Vec::new(),
                vec![format!(
                    "{}: expected an object with a \"rules\" array",
                    path.display()
                )],
            )
        }
    };
    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match serde_json::from_value::<ApprovalRule>(entry.clone()) {
            Ok(rule) if rule.tool.trim().is_empty() => {
                warnings.push(format!("rule {index}: empty tool name — skipped"));
            }
            Ok(rule) => rules.push(rule),
            Err(err) => warnings.push(format!("rule {index}: {err} — skipped")),
        }
    }
    (rules, warnings)
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

    /* ---- M4: scoped persistent rules ---- */

    /// A scratch rules file for one test.
    fn book(tag: &str) -> (RuleBook, PathBuf) {
        let dir = std::env::temp_dir().join(format!("bos-gui-rules-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("approval_rules.json");
        (RuleBook::open(&path), path)
    }

    fn spec(tool: &str, effect: RuleEffect, scope: RuleScope) -> NewRule {
        NewRule {
            tool: tool.to_string(),
            arg: None,
            scope,
            effect,
            workspace: None,
            session: None,
        }
    }

    /// A glob tool name matches by pattern, and an exact name does not overreach.
    #[test]
    fn glob_tool_names_match_by_pattern() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("mcp_*", "mcp_files_read"));
        assert!(wildcard_match("*_read", "files_read"));
        assert!(wildcard_match("file?", "files"));
        assert!(!wildcard_match("mcp_*", "bash"));
        assert!(!wildcard_match("file?", "filetool"));
        assert!(wildcard_match("bash", "bash"));
    }

    /// Each predicate kind accepts exactly its own shape of argument.
    #[test]
    fn every_predicate_kind_is_honoured() {
        let exact = ArgPredicate {
            key: "command".into(),
            match_kind: MatchKind::Exact,
            value: "ls".into(),
        };
        assert!(predicate_matches(
            &exact,
            &serde_json::json!({"command": "ls"})
        ));
        assert!(!predicate_matches(
            &exact,
            &serde_json::json!({"command": "ls -la"})
        ));

        let prefix = ArgPredicate {
            key: "path".into(),
            match_kind: MatchKind::Prefix,
            value: "src/".into(),
        };
        assert!(predicate_matches(
            &prefix,
            &serde_json::json!({"path": "src/main.rs"})
        ));
        assert!(!predicate_matches(
            &prefix,
            &serde_json::json!({"path": "docs/x.md"})
        ));

        let contains = ArgPredicate {
            key: "command".into(),
            match_kind: MatchKind::Contains,
            value: "rm -rf".into(),
        };
        assert!(predicate_matches(
            &contains,
            &serde_json::json!({"command": "echo hi && rm -rf /tmp/x"})
        ));
        assert!(!predicate_matches(
            &contains,
            &serde_json::json!({"command": "ls"})
        ));
        // A missing key never matches, and a non-string argument is stringified.
        assert!(!predicate_matches(
            &exact,
            &serde_json::json!({"other": "ls"})
        ));
        let number = ArgPredicate {
            key: "n".into(),
            match_kind: MatchKind::Exact,
            value: "7".into(),
        };
        assert!(predicate_matches(&number, &serde_json::json!({"n": 7})));
    }

    /// Nothing matched means the user is asked, and the audit records it.
    #[test]
    fn no_match_asks_and_is_audited() {
        let (book, path) = book("ask");
        let decision = book.decide("bash", &serde_json::json!({}), &RuleContext::default());
        assert_eq!(decision.decision, Decision::Ask);
        assert!(decision.rule.is_none());
        let audit = book.audit(10);
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].decision, "ask");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// An allow rule skips the dialog; a deny rule refuses outright.
    #[test]
    fn allow_and_deny_rules_decide_without_asking() {
        let (book, path) = book("allow-deny");
        book.add(spec("bash", RuleEffect::Allow, RuleScope::Global))
            .expect("add allow");
        let ctx = RuleContext::default();
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &ctx).decision,
            Decision::Allow
        );
        book.add(spec("bash", RuleEffect::Deny, RuleScope::Global))
            .expect("add deny");
        // Deny beats allow even when the allow was added first.
        let denied = book.decide("bash", &serde_json::json!({}), &ctx);
        assert_eq!(denied.decision, Decision::Deny);
        assert_eq!(denied.rule.as_ref().unwrap().effect, RuleEffect::Deny);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A deny beats an allow even when the allow is the more specific rule.
    #[test]
    fn deny_beats_allow_across_specificity() {
        let (book, path) = book("deny-wins");
        let mut allow = spec("bash", RuleEffect::Allow, RuleScope::Session);
        allow.arg = Some(ArgPredicate {
            key: "command".into(),
            match_kind: MatchKind::Exact,
            value: "ls".into(),
        });
        allow.session = Some("s1".into());
        book.add(allow).expect("add narrow allow");
        book.add(spec("*", RuleEffect::Deny, RuleScope::Global))
            .expect("add broad deny");
        let ctx = RuleContext {
            session: "s1".into(),
            workspace: "/w".into(),
        };
        let decision = book.decide("bash", &serde_json::json!({"command": "ls"}), &ctx);
        assert_eq!(decision.decision, Decision::Deny, "deny must win");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Among rules of the same effect the most specific one is chosen.
    #[test]
    fn more_specific_rules_win_within_an_effect() {
        let (book, path) = book("specific");
        book.add(spec("*", RuleEffect::Allow, RuleScope::Global))
            .expect("broad global");
        book.add(spec("bash", RuleEffect::Allow, RuleScope::Global))
            .expect("exact tool");
        let ctx = RuleContext {
            session: "s1".into(),
            workspace: "/w".into(),
        };
        let exact = book.decide("bash", &serde_json::json!({}), &ctx);
        assert_eq!(exact.rule.as_ref().unwrap().tool, "bash", "exact tool wins");

        let mut session_rule = spec("bash", RuleEffect::Allow, RuleScope::Session);
        session_rule.session = Some("s1".into());
        let session_rule = book.add(session_rule).expect("session rule");
        let chosen = book.decide("bash", &serde_json::json!({}), &ctx);
        assert_eq!(
            chosen.rule.as_ref().unwrap().id,
            session_rule.id,
            "session beats global"
        );
        // Another session is not covered by the session-scoped rule.
        let other = RuleContext {
            session: "s2".into(),
            workspace: "/w".into(),
        };
        let chosen = book.decide("bash", &serde_json::json!({}), &other);
        assert_eq!(chosen.rule.as_ref().unwrap().tool, "bash");
        assert_eq!(chosen.rule.as_ref().unwrap().scope, RuleScope::Global);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A workspace-scoped rule only applies inside its workspace.
    #[test]
    fn workspace_scope_is_confined_to_its_root() {
        let (book, path) = book("workspace");
        let mut rule = spec("bash", RuleEffect::Allow, RuleScope::Workspace);
        rule.workspace = Some("/w/one".into());
        book.add(rule).expect("add");
        let inside = RuleContext {
            session: "s".into(),
            workspace: "/w/one".into(),
        };
        let outside = RuleContext {
            session: "s".into(),
            workspace: "/w/two".into(),
        };
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &inside)
                .decision,
            Decision::Allow
        );
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &outside)
                .decision,
            Decision::Ask
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Revoking a rule stops it applying; revoking an unknown id is harmless.
    #[test]
    fn revoke_removes_the_rule_and_unknown_ids_are_clean() {
        let (book, path) = book("revoke");
        let rule = book
            .add(spec("bash", RuleEffect::Allow, RuleScope::Global))
            .expect("add");
        assert!(book.revoke(&rule.id).expect("revoke"));
        assert_eq!(book.rules().len(), 0);
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &RuleContext::default())
                .decision,
            Decision::Ask,
            "a revoked rule no longer matches"
        );
        assert!(!book.revoke("no-such-id").expect("clean no-op"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Rules survive a write/read round trip through the file.
    #[test]
    fn rules_round_trip_through_the_file() {
        let (book, path) = book("roundtrip");
        let mut rule = spec("write_file", RuleEffect::Deny, RuleScope::Workspace);
        rule.arg = Some(ArgPredicate {
            key: "path".into(),
            match_kind: MatchKind::Prefix,
            value: ".env".into(),
        });
        rule.workspace = Some("/w".into());
        let added = book.add(rule).expect("add");
        assert!(book.save().is_ok());

        let reopened = RuleBook::open(&path);
        assert!(reopened.warnings().is_empty(), "{:?}", reopened.warnings());
        assert_eq!(reopened.rules(), vec![added.clone()]);
        let ctx = RuleContext {
            session: "s".into(),
            workspace: "/w".into(),
        };
        assert_eq!(
            reopened
                .decide(
                    "write_file",
                    &serde_json::json!({"path": ".env.local"}),
                    &ctx
                )
                .decision,
            Decision::Deny
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A malformed file never panics: bad entries are skipped and reported.
    #[test]
    fn malformed_files_are_tolerated_and_reported() {
        let dir = std::env::temp_dir().join(format!("bos-gui-rules-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Not JSON at all → no rules, one warning.
        let garbage = dir.join("garbage.json");
        std::fs::write(&garbage, b"{not json").unwrap();
        let book = RuleBook::open(&garbage);
        assert!(book.rules().is_empty());
        assert_eq!(book.warnings().len(), 1, "{:?}", book.warnings());
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &RuleContext::default())
                .decision,
            Decision::Ask
        );

        // Valid document, one good entry and two bad ones.
        let mixed = dir.join("mixed.json");
        std::fs::write(
            &mixed,
            br#"{
                "version": 1,
                "rules": [
                    {"id": "a", "tool": "bash", "scope": "global", "effect": "allow", "created_at": 1},
                    {"id": "b", "tool": "bash", "scope": "nonsense", "effect": "allow", "created_at": 1},
                    {"tool": "   ", "scope": "global", "effect": "allow", "created_at": 1},
                    17
                ]
            }"#,
        )
        .unwrap();
        let book = RuleBook::open(&mixed);
        assert_eq!(book.rules().len(), 1, "the good entry survives");
        assert_eq!(book.rules()[0].id, "a");
        assert_eq!(book.warnings().len(), 3, "{:?}", book.warnings());
        assert_eq!(
            book.decide("bash", &serde_json::json!({}), &RuleContext::default())
                .decision,
            Decision::Allow
        );

        // A missing file is simply an empty book, with no warning.
        let book = RuleBook::open(dir.join("absent.json"));
        assert!(book.rules().is_empty());
        assert!(book.warnings().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The audit ring is bounded and reports newest first.
    #[test]
    fn the_audit_ring_is_bounded_and_newest_first() {
        let (book, path) = book("audit");
        let ctx = RuleContext::default();
        for _ in 0..(AUDIT_CAP + 5) {
            book.decide("bash", &serde_json::json!({}), &ctx);
        }
        assert_eq!(book.audit(AUDIT_CAP * 2).len(), AUDIT_CAP, "ring is capped");
        let recent = book.audit(3);
        assert_eq!(recent.len(), 3);
        assert!(recent[0].at >= recent[2].at, "newest first");
        assert_eq!(recent[0].tool, "bash");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The broker consults attached rules before asking the user.
    #[tokio::test]
    async fn broker_consults_the_rule_book_before_asking() {
        let (book, path) = book("broker");
        let book = Arc::new(book);
        let broker = ApprovalBroker::default();
        broker.set_rules(Arc::clone(&book));
        broker.set_active("s1", 1);

        book.add(spec("bash", RuleEffect::Deny, RuleScope::Global))
            .expect("add deny");
        let (id, rx) = broker.request("bash", &serde_json::json!({}));
        assert!(id.is_empty(), "a rule-matching call mints no request id");
        assert!(!rx.await.expect("resolved"), "the deny refuses the call");
        assert!(broker.pending.lock().unwrap().is_empty());

        book.add(spec("write_file", RuleEffect::Allow, RuleScope::Global))
            .expect("add allow");
        let (id, rx) = broker.request("write_file", &serde_json::json!({}));
        assert!(id.is_empty());
        assert!(rx.await.expect("resolved"), "the allow runs it");

        // A tool no rule mentions still reaches the dialog.
        let (id, _rx) = broker.request("other", &serde_json::json!({}));
        assert!(!id.is_empty(), "an unmatched tool is still asked about");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
