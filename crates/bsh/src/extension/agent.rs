//! Agent integration: tools, commands, panels, prompts and contexts.
//!
//! Registration and authorization are deliberately separate. A package that
//! can register a tool (agent.tools.register, a low-risk capability) does not
//! therefore get the capability the tool's body needs; ExtensionTool asks the
//! policy engine on every call, with the call's own path, host and secret as
//! the scope. A denied call is a tool error the model can read, not a panic and
//! not a silent no-op.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::Stream;
use react::tool::registry::AsyncTool as _;
use serde_json::Value;

use crate::plugins::PluginInfo;

use ferris_sdk::capability::{CallScope, Capability, PolicyEngine};
use ferris_sdk::composable::Fiber;
use ferris_sdk::events::{topics, EventBus, ExtensionEvent};
use ferris_sdk::host_runtime::{CallContext, Runtime, ToolDescriptor};
use ferris_sdk::types::{ExtensionError, ExtensionId, TrustLevel};

use super::errors::ToToolError;

/// The largest prompt or context section one call may append.
const MAX_PROMPT_SECTION: usize = 8 * 1024;

/// The largest total prompt material all extensions may append to one build.
const MAX_TOTAL_PROMPT: usize = 64 * 1024;

/// One extension's registered contributions.
#[derive(Clone)]
pub(crate) struct Contributed {
    /// Extension id.
    pub(crate) extension: ExtensionId,
    /// Display name.
    pub(crate) name: String,
    /// Runtime that executes the tools.
    pub(crate) runtime: Arc<dyn Runtime>,
    /// Trust level used for policy.
    pub(crate) trust: TrustLevel,
    /// Tool descriptors.
    pub(crate) tools: Vec<ToolDescriptor>,
    /// Commands.
    pub(crate) commands: Vec<ferris_sdk::host_runtime::declarative::CommandContribution>,
    /// Panels.
    pub(crate) panels: Vec<ferris_sdk::host_runtime::declarative::PanelContribution>,
    /// Prompts.
    pub(crate) prompts: Vec<ferris_sdk::host_runtime::declarative::TextContribution>,
    /// Contexts.
    pub(crate) contexts: Vec<ferris_sdk::host_runtime::declarative::TextContribution>,
    /// Workspace open for this build, used to scope contexts.
    pub(crate) workspace: Option<std::path::PathBuf>,
    /// Package skills directory, when the manifest declares skills.
    pub(crate) skills_dir: Option<std::path::PathBuf>,
    /// Workflows, recorded only.
    pub(crate) workflows: Vec<ferris_sdk::host_runtime::declarative::RawContribution>,
    /// Themes, recorded only.
    pub(crate) themes: Vec<ferris_sdk::host_runtime::declarative::RawContribution>,
    /// One-call wall-clock ceiling.
    pub(crate) timeout_ms: u64,
    /// The manifest's per-extension concurrent-call ceiling, already resolved
    /// against the host default and hard maximum.
    pub(crate) max_concurrent_calls: usize,
    /// One-result output ceiling, measured as serialized JSON bytes.
    ///
    /// Runtimes enforce this before host hooks run; the Agent adapter enforces
    /// it again on the final post-hook value, because a `tool.after`
    /// interceptor can expand a result the runtime already approved.
    pub(crate) max_output_bytes: u64,
}

/// A revocation handle for the contributions installed into one Agent build.
///
/// The lease is deliberately tied to the Agent/PluginRegistry pair that
/// received the contributions. It must not be reused with another build.
#[derive(Clone)]
pub(crate) struct ContributionLease {
    owner: String,
    tools: Vec<String>,
    tool_handles: Vec<Arc<dyn react::tool::registry::AsyncTool>>,
    prompt_sections: Vec<String>,
    skill_paths: Vec<PathBuf>,
    /// The skill content this activation installed, kept for a resume.
    ///
    /// `skill_paths` is what a suspend removes; a resume cannot re-read the
    /// package (it may be gone), so the parsed content travels with the lease.
    skills: Vec<agent::skills::SkillContent>,
    active: Arc<AtomicBool>,
    calls: Arc<CallTracker>,
}

impl ContributionLease {
    /// Signal every in-flight call owned by this lease. The calls observe the
    /// token on their next poll and return a cancellation error.
    pub(crate) fn cancel_in_flight(&self) {
        self.calls.cancel_all();
    }

    /// Revoke this extension's tools, prompts, skills, commands and panels.
    /// Returns the number of contributions removed and whether every in-flight
    /// call drained within the grace period.
    pub(crate) fn revoke(
        &self,
        agent: &mut agent::Agent,
        registry: &mut agent::plugins::PluginRegistry,
    ) -> (usize, bool) {
        self.active.store(false, Ordering::SeqCst);
        self.calls.close();
        // The call future observes its token within one poll interval; wait
        // briefly so a revoked tool cannot keep writing after this returns.
        let calls = Arc::clone(&self.calls);
        let drained = crate::extension::block_on_tokio(async move {
            calls.wait_empty(Duration::from_secs(2)).await
        });
        let tools = agent.remove_live_async_tools(&self.tools);
        let prompts = self
            .prompt_sections
            .iter()
            .filter(|section| agent.remove_live_prompt_section(section))
            .count();
        let skills = agent.remove_live_skills_by_paths(&self.skill_paths);
        let items = registry.remove_owner(&self.owner);
        (tools + prompts + skills + items, drained)
    }

    /// Remove only the live overlay pieces, keeping commands/panels in place.
    pub(crate) fn suspend_live(&self, agent: &agent::Agent) -> usize {
        // Close admission first: a retained tool handle must not enter the
        // runtime after the overlay is removed.
        self.calls.set_suspended(true);
        self.cancel_in_flight();
        let calls = Arc::clone(&self.calls);
        let _ = crate::extension::block_on_tokio(async move {
            calls.wait_empty(Duration::from_secs(2)).await
        });
        let tools = agent.remove_live_async_tools(&self.tools);
        let prompts = self
            .prompt_sections
            .iter()
            .filter(|section| agent.remove_live_prompt_section(section))
            .count();
        let skills = agent.remove_live_skills_by_paths(&self.skill_paths);
        tools + prompts + skills
    }

    /// Re-install the live overlay pieces after a suspend.
    ///
    /// Skills are part of the overlay a suspend removes, so they are part of
    /// what a resume owes back: without them the model keeps the catalogue
    /// entry but `load_skill` answers "not found" for a package that is active
    /// again.
    pub(crate) fn resume_live(&self, agent: &agent::Agent) -> usize {
        self.calls.set_suspended(false);
        let mut restored = 0;
        for tool in &self.tool_handles {
            if agent.add_live_async_tool(Arc::clone(tool)).is_ok() {
                restored += 1;
            }
        }
        for section in &self.prompt_sections {
            agent.add_live_prompt_section(section.clone());
            restored += 1;
        }
        for skill in &self.skills {
            agent.add_live_skill(skill.clone());
            restored += 1;
        }
        restored
    }

    /// Bind the lease's liveness to the owning Fiber.
    pub(crate) fn attach_to_fiber(&self, fiber: &mut Fiber) {
        let active = Arc::clone(&self.active);
        let calls = Arc::clone(&self.calls);
        fiber.own(move || {
            active.store(false, Ordering::SeqCst);
            calls.close();
        });
    }

    /// Names of the async tools owned by this lease.
    pub(crate) fn tool_names(&self) -> &[String] {
        &self.tools
    }

    /// Extension owner identifier.
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }
}

#[async_trait::async_trait]
impl ferris_sdk::ContributionLease for ContributionLease {
    async fn revoke(
        &self,
        host: &mut dyn ferris_sdk::LeaseHost,
    ) -> Result<(), ferris_sdk::ExtError> {
        self.active.store(false, Ordering::SeqCst);
        self.calls.close();
        host.remove_tools(&self.owner, &self.tools);
        host.remove_prompts(&self.owner, &self.prompt_sections);
        host.remove_skills(&self.owner, &self.skill_paths);
        host.remove_registry_owner(&self.owner);
        // Cancellation is a signal; wait, bounded, for the calls to actually
        // leave before reporting the lease revoked.
        if !self.calls.wait_empty(Duration::from_secs(2)).await {
            return Err(ferris_sdk::ExtError {
                code: ferris_sdk::ErrorCode::Lifecycle,
                message: "lease removal completed but in-flight calls did not drain".to_string(),
                details: serde_json::Value::Null,
            });
        }
        Ok(())
    }
}

/// One extension tool exposed to the agent.
#[derive(Clone)]
pub(crate) struct ExtensionTool {
    owner: ExtensionId,
    runtime: Arc<dyn Runtime>,
    descriptor: ToolDescriptor,
    trust: TrustLevel,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    workspace: Option<PathBuf>,
    timeout_ms: u64,
    max_output_bytes: u64,
    dispatch: Arc<ferris_sdk::composable::dispatch::EventDispatcher>,
    qualified: String,
    calls: Arc<CallTracker>,
    active: Arc<AtomicBool>,
    metadata: agent::tools::ToolMetadata,
}

/// What an extension tool declares about itself.
///
/// A package runs code this host did not read at build time: medium risk and a
/// dedicated permission scope say so. The tool carries no version of its own
/// (a package versions as a whole), so the catalogue records Unknown rather
/// than inventing one.
pub(crate) fn extension_tool_metadata() -> agent::tools::ToolMetadata {
    agent::tools::ToolMetadata::new()
        .with_risk("medium")
        .with_side_effects(["extension"])
        .with_permissions(["extension.call"])
}

/// The in-flight calls of one installed extension tool, keyed by call id.
///
/// A single shared cancellation flag cannot isolate two overlapping calls:
/// cancelling one would cancel both, and a later call would clear the other
/// cancellation. Each invocation owns a token here; a lease revocation
/// signals every token and then waits for the calls to drain.
pub(crate) struct CallTracker {
    state: std::sync::Mutex<CallState>,
    changed: tokio::sync::Notify,
    limit: usize,
}

struct CallState {
    calls: std::collections::HashMap<String, Arc<AtomicBool>>,
    /// Admission is closed for good; a revoked lease can never register again.
    closed: bool,
    /// A runtime-wide cancel is in progress. New admission waits for it to
    /// finish, so a call started after the last-call decision cannot be hit by
    /// a cancel aimed at an earlier call.
    runtime_cancel: bool,
    /// Suspension closes admission temporarily; resume reopens it, unlike
    /// closed which is permanent.
    suspended: bool,
}

impl CallTracker {
    fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            state: std::sync::Mutex::new(CallState {
                calls: std::collections::HashMap::new(),
                closed: false,
                runtime_cancel: false,
                suspended: false,
            }),
            changed: tokio::sync::Notify::new(),
            limit: limit.clamp(1, 64),
        })
    }

    /// Admit one call, or return None when the lease has been revoked.
    ///
    /// Capacity and liveness are decided under the same lock as registration,
    /// so a revoke cannot land between the check and the insert.
    async fn admit(&self, call_id: &str) -> Option<Arc<AtomicBool>> {
        loop {
            // Register the waiter BEFORE checking the predicate. Notify does
            // not store a permit for notify_waiters(), so a finish() between
            // the check and the first poll of the future would otherwise be
            // lost and this call could sleep while a slot is free.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.closed || state.suspended {
                    return None;
                }
                if !state.runtime_cancel && state.calls.len() < self.limit {
                    let token = Arc::new(AtomicBool::new(false));
                    state.calls.insert(call_id.to_string(), Arc::clone(&token));
                    return Some(token);
                }
            }
            notified.await;
        }
    }

    /// Remove a finished call and wake an admission or drain waiter.
    fn finish(&self, call_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.calls.remove(call_id);
        }
        self.changed.notify_waiters();
    }

    /// Signal one call. Returns false when the call already finished.
    fn cancel(&self, call_id: &str) -> bool {
        let token = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.calls.get(call_id).cloned());
        match token {
            Some(token) => {
                token.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Close admission and signal every in-flight call. Revocation only.
    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            for token in state.calls.values() {
                token.store(true, Ordering::SeqCst);
            }
        }
        self.changed.notify_waiters();
    }

    /// Close admission temporarily while a contribution is suspended.
    fn set_suspended(&self, suspended: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.suspended = suspended;
        }
        self.changed.notify_waiters();
    }

    /// Signal every in-flight call without closing admission (suspend).
    fn cancel_all(&self) {
        if let Ok(state) = self.state.lock() {
            for token in state.calls.values() {
                token.store(true, Ordering::SeqCst);
            }
        }
        self.changed.notify_waiters();
    }

    /// Take the runtime-wide-cancel gate when this is the last live call.
    ///
    /// While the gate is held no new call is admitted, closing the
    /// check-then-cancel race: a call started after this decision cannot be
    /// interrupted by a cancel aimed at an earlier call.
    fn begin_runtime_cancel(&self, call_id: &str) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.calls.keys().any(|id| id != call_id) {
            return false;
        }
        if state.runtime_cancel {
            return false;
        }
        state.runtime_cancel = true;
        true
    }

    /// Release the runtime-wide-cancel gate and admit waiters again.
    fn end_runtime_cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.runtime_cancel = false;
        }
        self.changed.notify_waiters();
    }

    fn in_flight(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.calls.len())
            .unwrap_or(0)
    }

    /// Wait, bounded, for every call to leave the tracker.
    async fn wait_empty(&self, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.in_flight() == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return false;
            }
        }
    }
}

/// Removes a call from its tracker even when the invocation returns early.
struct CallGuard {
    calls: Arc<CallTracker>,
    id: String,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.calls.finish(&self.id);
    }
}

/// Signals a streaming call when its consumer drops the stream.
struct CancelOnDrop {
    tool: ExtensionTool,
    id: String,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.tool.cancel(&self.id);
    }
}

/// The progress events one tool may emit in a one-second window.
const PROGRESS_PER_WINDOW: u32 = 200;
const PROGRESS_WINDOW: Duration = Duration::from_secs(1);

/// Streaming items buffered for a reader that has not polled yet.
const STREAM_QUEUE: usize = 64;

/// Progress values the runtime may enqueue before the forwarder drains them.
/// A full queue drops the newest progress value; progress is advisory, while
/// the final result has its own bounded queue.
const PROGRESS_QUEUE: usize = 32;

/// The largest single progress chunk forwarded to a stream.
const MAX_PROGRESS_BYTES: usize = 16 * 1024;

/// A fixed-window limiter for progress chunks, so a chatty guest cannot turn a
/// tool call into an unbounded stream of model-visible messages.
struct ProgressWindow {
    started: Instant,
    count: u32,
}

impl ProgressWindow {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            count: 0,
        }
    }

    fn allow(&mut self) -> bool {
        self.allow_at(Instant::now())
    }

    fn allow_at(&mut self, now: Instant) -> bool {
        if now.duration_since(self.started) >= PROGRESS_WINDOW {
            self.started = now;
            self.count = 0;
        }
        if self.count < PROGRESS_PER_WINDOW {
            self.count += 1;
            true
        } else {
            false
        }
    }
}

impl ExtensionTool {
    /// Build one tool. The name is qualified with the extension namespace.
    pub(crate) fn new(
        contributed: &Contributed,
        descriptor: ToolDescriptor,
        policy: Arc<PolicyEngine>,
        events: Arc<EventBus>,
        workspace: Option<PathBuf>,
    ) -> Self {
        Self::new_with_liveness(
            contributed,
            descriptor,
            policy,
            events,
            workspace,
            Arc::new(AtomicBool::new(true)),
        )
    }

    fn new_with_liveness(
        contributed: &Contributed,
        descriptor: ToolDescriptor,
        policy: Arc<PolicyEngine>,
        events: Arc<EventBus>,
        workspace: Option<PathBuf>,
        active: Arc<AtomicBool>,
    ) -> Self {
        Self::with_dispatcher(
            contributed,
            descriptor,
            policy,
            events,
            workspace,
            active,
            CallTracker::new(contributed.max_concurrent_calls),
            crate::extension::dispatcher(),
        )
    }

    /// Build a tool against an explicit hook dispatcher and call tracker.
    ///
    /// Production uses the process-wide dispatcher; tests inject a private one
    /// so a `tool.after` interceptor registered by one case cannot observe (or
    /// rewrite) another case's calls.
    #[allow(clippy::too_many_arguments)]
    fn with_dispatcher(
        contributed: &Contributed,
        descriptor: ToolDescriptor,
        policy: Arc<PolicyEngine>,
        events: Arc<EventBus>,
        workspace: Option<PathBuf>,
        active: Arc<AtomicBool>,
        calls: Arc<CallTracker>,
        dispatch: Arc<ferris_sdk::composable::dispatch::EventDispatcher>,
    ) -> Self {
        let qualified = contributed.extension.qualify(&descriptor.id);
        Self {
            owner: contributed.extension.clone(),
            runtime: Arc::clone(&contributed.runtime),
            descriptor,
            trust: contributed.trust,
            policy,
            events,
            workspace,
            timeout_ms: contributed.timeout_ms,
            max_output_bytes: contributed.max_output_bytes,
            dispatch,
            qualified,
            calls,
            active,
            metadata: extension_tool_metadata(),
        }
    }

    fn scope(&self, arguments: &Value) -> CallScope {
        ferris_sdk::capability::call_scope(arguments, self.workspace.as_deref())
    }

    fn emit(&self, topic: &str, payload: Value, correlation: &str) {
        if let Ok(event) = ExtensionEvent::new(self.owner.as_str(), topic, payload) {
            let _ = self.events.publish(&event.correlated(correlation));
        }
    }
}

#[async_trait]
impl react::tool::registry::AsyncTool for ExtensionTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    fn description(&self) -> String {
        self.descriptor.description.clone()
    }

    fn category(&self) -> String {
        "extension".to_string()
    }

    fn json_schema(&self) -> Value {
        self.descriptor.input_schema.clone()
    }

    fn metadata(&self) -> Option<&agent::tools::ToolMetadata> {
        Some(&self.metadata)
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn is_cancelable(&self) -> bool {
        true
    }

    /// A package contributes a FAMILY of tools at install time, and every one
    /// of them used to ride in the request's tool array on every step - three
    /// installed packages are eighteen schemas, most of them unrelated to the
    /// turn.
    ///
    /// They stay registered and callable; the model reaches one through
    /// `find_tool`, whose reported definition joins the next request, and the
    /// build advertises it as soon as it withholds anything. The Tools pane still lists them, because the pane
    /// lists capabilities (`Agent::all_tools`), not advertisement.
    fn deferred(&self) -> bool {
        true
    }

    fn cancel(&self, call_id: &str) {
        if !self.calls.cancel(call_id) {
            // The call already finished; there is nothing to signal.
            return;
        }
        // Take the runtime-wide cancellation gate atomically: while it is held
        // no new call is admitted, so a call that starts after the last-call
        // decision cannot be interrupted by a cancel aimed at this one.
        if !self.calls.begin_runtime_cancel(call_id) {
            return;
        }
        let runtime = Arc::clone(&self.runtime);
        let calls = Arc::clone(&self.calls);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), runtime.cancel()).await;
                calls.end_runtime_cancel();
            });
        } else {
            self.calls.end_runtime_cancel();
        }
    }

    async fn run(&self, arguments: &Value) -> Result<Value, agent::tools::ToolError> {
        self.invoke(arguments, None, None).await
    }

    async fn run_with_call_id(
        &self,
        arguments: &Value,
        call_id: &str,
    ) -> Result<Value, agent::tools::ToolError> {
        self.invoke(arguments, None, Some(call_id)).await
    }

    async fn run_streaming(
        &self,
        input: &Value,
    ) -> Result<
        std::pin::Pin<Box<dyn Stream<Item = Result<String, agent::tools::ToolError>> + Send>>,
        agent::tools::ToolError,
    > {
        let call_id = uuid::Uuid::new_v4().to_string();
        self.run_streaming_with_call_id(input, &call_id).await
    }

    async fn run_streaming_with_call_id(
        &self,
        input: &Value,
        call_id: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn Stream<Item = Result<String, agent::tools::ToolError>> + Send>>,
        agent::tools::ToolError,
    > {
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<Value>(PROGRESS_QUEUE);
        // The consumer-facing queue is bounded: a stalled reader must not let
        // progress messages accumulate without limit.
        let (stream_tx, stream_rx) =
            tokio::sync::mpsc::channel::<Result<String, agent::tools::ToolError>>(STREAM_QUEUE);
        let tool = self.clone();
        let input = input.clone();
        let id = call_id.to_string();
        tokio::spawn(async move {
            let progress_stream = stream_tx.clone();
            let max_progress = tool.max_output_bytes;
            let forward = tokio::spawn(async move {
                let mut window = ProgressWindow::new();
                let mut forwarded = 0u64;
                while let Some(value) = progress_rx.recv().await {
                    if !window.allow() {
                        continue;
                    }
                    let text = value.to_string();
                    // One oversized chunk is dropped rather than copied, and
                    // the whole stream is held to the call output budget.
                    if text.len() > MAX_PROGRESS_BYTES {
                        continue;
                    }
                    let bytes = text.len() as u64;
                    if forwarded.saturating_add(bytes) > max_progress {
                        continue;
                    }
                    forwarded = forwarded.saturating_add(bytes);
                    if progress_stream.try_send(Ok(text)).is_err() {
                        break;
                    }
                }
            });
            let result = tool.invoke(&input, Some(progress_tx), Some(&id)).await;
            let _ = forward.await;
            match result {
                Ok(value) => {
                    let text = match value {
                        Value::String(text) => text,
                        other => serde_json::to_string(&other).unwrap_or_default(),
                    };
                    let _ = stream_tx.send(Ok(text)).await;
                }
                Err(error) => {
                    let _ = stream_tx.send(Err(error)).await;
                }
            }
        });
        // Dropping the returned stream cancels the invocation, and it only
        // stops the runtime when the call was still running.
        let guard = CancelOnDrop {
            tool: self.clone(),
            id: call_id.to_string(),
        };
        Ok(Box::pin(futures::stream::unfold(
            (stream_rx, guard),
            |(mut receiver, guard)| async move {
                receiver.recv().await.map(|item| (item, (receiver, guard)))
            },
        )))
    }
}

impl ExtensionTool {
    async fn invoke(
        &self,
        arguments: &Value,
        progress: Option<tokio::sync::mpsc::Sender<Value>>,
        call_id: Option<&str>,
    ) -> Result<Value, agent::tools::ToolError> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(agent::tools::ToolError::Failed(
                "extension contribution has been revoked".to_string(),
            ));
        }
        let call_id = call_id
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Admission and the liveness check are one atomic transition: a revoke
        // that won the race closes admission and this call never reaches the
        // runtime, rather than registering after cancel_all already ran.
        let Some(token) = self.calls.admit(&call_id).await else {
            return Err(agent::tools::ToolError::Failed(
                "extension contribution is not accepting calls".to_string(),
            ));
        };
        let _guard = CallGuard {
            calls: Arc::clone(&self.calls),
            id: call_id,
        };
        let started = std::time::Instant::now();
        let correlation = uuid::Uuid::new_v4().to_string();
        let arguments = self
            .dispatch
            .waterfall("tool.before", arguments.clone())
            .unwrap_or_else(|| arguments.clone());
        let scope = self.scope(&arguments);
        if let Err(error) = ferris_sdk::capability::authorize_each(
            &self.policy,
            self.owner.as_str(),
            self.trust,
            &self.descriptor.capabilities,
            &scope,
        ) {
            self.emit(
                topics::TOOL_FAILED,
                serde_json::json!({
                    "tool": self.descriptor.id.as_str(),
                    "fiber": self.owner.as_str(),
                    "error": error.to_string(),
                }),
                &correlation,
            );
            return Err(error.to_tool_error());
        }
        self.emit(
            topics::TOOL_INVOKED,
            serde_json::json!({
                "tool": self.descriptor.id.as_str(),
                "fiber": self.owner.as_str(),
            }),
            &correlation,
        );
        let context = CallContext::new(
            self.owner.clone(),
            correlation.clone(),
            self.workspace.clone(),
            self.timeout_ms,
        );
        let call = match progress {
            Some(progress) => self.runtime.call_tool_progress(
                self.descriptor.id.as_str(),
                arguments.clone(),
                &context,
                Some(progress),
            ),
            None => self.runtime.call_tool_result(
                self.descriptor.id.as_str(),
                arguments.clone(),
                &context,
            ),
        };
        let cancel = Arc::clone(&token);
        let cancelled = async move {
            while !cancel.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let outcome = tokio::select! {
            _ = cancelled => Err(ExtensionError::Runtime {
                extension: self.owner.to_string(),
                message: "cancelled".to_string(),
            }),
            result = tokio::time::timeout(
                Duration::from_millis(self.timeout_ms.max(1)),
                call,
            ) => match result {
                Ok(result) => result,
                Err(_) => Err(ExtensionError::Timeout {
                    extension: self.owner.to_string(),
                    operation: self.qualified.clone(),
                    millis: self.timeout_ms,
                }),
            },
        };
        // The result and the cancellation can become ready together; a
        // cancelled call must not report a value the caller already abandoned.
        let outcome = match outcome {
            Ok(_) if token.load(Ordering::SeqCst) => Err(ExtensionError::Runtime {
                extension: self.owner.to_string(),
                message: "cancelled".to_string(),
            }),
            other => other,
        };
        match outcome {
            Ok(result) => {
                let value = result.to_value();
                let value = self
                    .dispatch
                    .waterfall("tool.after", value.clone())
                    .unwrap_or(value);
                // The final, post-hook representation is what the agent keeps
                // and what the model sees, so the manifest ceiling applies
                // here: a runtime's own pre-hook check cannot constrain an
                // interceptor that expands the value afterwards.
                let bytes = serde_json::to_vec(&value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                if bytes as u64 > self.max_output_bytes {
                    super::host::record_tool_call(
                        started.elapsed().as_millis() as u64,
                        bytes as u64,
                        false,
                    );
                    let error = ExtensionError::Limit {
                        extension: self.owner.to_string(),
                        message: format!(
                            "tool result is {bytes} bytes; the ceiling is {}",
                            self.max_output_bytes
                        ),
                    };
                    self.emit(
                        topics::TOOL_FAILED,
                        serde_json::json!({
                            "tool": self.descriptor.id.as_str(),
                            "fiber": self.owner.as_str(),
                            "error": truncate(&error.to_string(), 512),
                        }),
                        &correlation,
                    );
                    return Err(error.to_tool_error());
                }
                super::host::record_tool_call(
                    started.elapsed().as_millis() as u64,
                    bytes as u64,
                    true,
                );
                self.emit(
                    topics::TOOL_COMPLETED,
                    serde_json::json!({
                        "tool": self.descriptor.id.as_str(),
                        "fiber": self.owner.as_str(),
                        "bytes": bytes,
                    }),
                    &correlation,
                );
                Ok(value)
            }
            Err(error) => {
                super::host::record_tool_call(started.elapsed().as_millis() as u64, 0, false);
                self.emit(
                    topics::TOOL_FAILED,
                    serde_json::json!({
                        "tool": self.descriptor.id.as_str(),
                        "fiber": self.owner.as_str(),
                        "error": truncate(&error.to_string(), 512),
                    }),
                    &correlation,
                );
                Err(error.to_tool_error())
            }
        }
    }
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Register every contribution a package offered.
///
/// A duplicate tool name is reported and skipped rather than allowed to replace
/// a built-in: the load order of two packages must not decide which body the
/// model calls. Commands and panels share the host plugin namespace, which has
/// the same collision rule.
/// Whether a context's workspace globs match the open workspace.
///
/// An empty scope is unscoped and always matches; a scoped context with no open
/// workspace never matches, because there is no path tree to test.
fn context_matches(
    context: &ferris_sdk::host_runtime::declarative::TextContribution,
    workspace: Option<&std::path::Path>,
) -> bool {
    if context.scope.is_empty() {
        return true;
    }
    let Some(workspace) = workspace else {
        return false;
    };
    context
        .scope
        .iter()
        .any(|pattern| scope_matches(workspace, pattern))
}

/// Entries one scope walk may inspect before it gives up.
///
/// The open workspace can be a dependency-heavy repository with hundreds of
/// thousands of files, or the filesystem root itself, and a recursive wildcard
/// gives a scope no natural end. These scopes are evaluated while the launch
/// agent is built and again for every turn, so the walk is bounded: a project a
/// context describes is far below this budget, and the glob tool's own walk has
/// already pruned the build, dependency and hidden trees that make the count
/// large.
const SCOPE_WALK_BUDGET: usize = 20_000;

/// Whether one scope glob matches the open workspace.
///
/// A literal path is one existence check. A pattern walks the glob tool's own
/// tree - hidden, ignored and build/dependency entries never enter it - seeded
/// by the literal directories in front of the first wildcard. A walk that cannot
/// finish within [SCOPE_WALK_BUDGET] entries reports no match instead of
/// freezing its caller.
fn scope_matches(workspace: &std::path::Path, pattern: &str) -> bool {
    scope_matches_within(workspace, pattern, SCOPE_WALK_BUDGET)
}

/// [scope_matches] with the entry budget named, so a test can prove the bound.
fn scope_matches_within(workspace: &std::path::Path, pattern: &str, budget: usize) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    // A package may write the scope as an absolute path, or with the workspace
    // already in front of a relative one; both name the same tree. A scope
    // outside the workspace can never match it.
    let relative = match std::path::Path::new(pattern).strip_prefix(workspace) {
        Ok(relative) => scope_text(relative).into_owned(),
        Err(_) if std::path::Path::new(pattern).is_absolute() => return false,
        Err(_) => pattern.to_string(),
    };
    // Split at the first wildcard: the literal directories seed the walk, and
    // the rest becomes the matcher, so a pattern under a literal base never
    // scans a tree outside it and a base that is gone is one failed is_dir
    // check rather than a walk of the workspace.
    let mut prefix = PathBuf::new();
    let mut rest: Vec<&str> = Vec::new();
    let mut wildcard = false;
    for component in relative.split('/') {
        let literal = !component.is_empty()
            && !component
                .chars()
                .any(|character| matches!(character, '*' | '?' | '['));
        if !wildcard && literal {
            prefix.push(component);
        } else {
            wildcard = true;
            rest.push(component);
        }
    }
    if !wildcard {
        return workspace.join(&prefix).exists();
    }
    let base = workspace.join(&prefix);
    if !base.is_dir() {
        return false;
    }
    let Ok(matcher) = glob::Pattern::new(&rest.join("/")) else {
        return false;
    };
    // The glob crate forces a literal separator (a star stops at a slash);
    // matching a path by hand has to ask for the same rule.
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..glob::MatchOptions::new()
    };
    let mut seen = 0usize;
    for entry in crate::glob::build_walker(&base, false).build().flatten() {
        if entry.depth() == 0 {
            continue;
        }
        if seen >= budget {
            log::debug!(
                "extension scope {pattern:?} under {} stopped after {budget} entries",
                workspace.display()
            );
            return false;
        }
        seen += 1;
        let Ok(path) = entry.path().strip_prefix(&base) else {
            continue;
        };
        if matcher.matches_with(&scope_text(path), options) {
            return true;
        }
    }
    false
}

/// One path as the scope matcher reads it: forward slashes on every platform.
fn scope_text(path: &std::path::Path) -> std::borrow::Cow<'_, str> {
    let text = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        text
    } else {
        std::borrow::Cow::Owned(text.replace(std::path::MAIN_SEPARATOR, "/"))
    }
}

pub(crate) fn install(
    agent: &mut agent::Agent,
    registry: &mut agent::plugins::PluginRegistry,
    contributed: &Contributed,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    workspace: Option<PathBuf>,
) -> PluginInfo {
    install_with_lease(agent, registry, contributed, policy, events, workspace).0
}

/// Install contributions and return the exact revocation handle for this build.
pub(crate) fn install_with_lease(
    agent: &mut agent::Agent,
    registry: &mut agent::plugins::PluginRegistry,
    contributed: &Contributed,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    workspace: Option<PathBuf>,
) -> (PluginInfo, ContributionLease) {
    let mut installed_tools = Vec::new();
    let mut installed_handles = Vec::new();
    let active = Arc::new(AtomicBool::new(true));
    let calls = CallTracker::new(contributed.max_concurrent_calls);
    for descriptor in &contributed.tools {
        let tool = ExtensionTool::with_dispatcher(
            contributed,
            descriptor.clone(),
            Arc::clone(&policy),
            Arc::clone(&events),
            workspace.clone(),
            Arc::clone(&active),
            Arc::clone(&calls),
            crate::extension::dispatcher(),
        );
        let name = tool.name().to_string();
        let handle: Arc<dyn react::tool::registry::AsyncTool> = Arc::new(tool);
        match agent.add_live_async_tool(Arc::clone(&handle)) {
            Ok(()) => {
                installed_tools.push(name);
                installed_handles.push(handle);
            }
            Err(error) => log::warn!(
                "extension {}: tool {} was not registered: {error}",
                contributed.extension,
                name
            ),
        }
    }
    for command in &contributed.commands {
        let name = contributed.extension.qualify(&command.id);
        let command = agent::plugins::PluginCommand::new(
            name,
            format!("[{}] {}", contributed.name, command.description),
        );
        if let Err(error) = registry.register_command(contributed.extension.as_str(), command) {
            log::warn!(
                "extension {}: command was not registered: {error}",
                contributed.extension
            );
        }
    }
    for panel in &contributed.panels {
        let name = contributed.extension.qualify(&panel.id);
        let panel = agent::plugins::UiPanel::new(name, panel.title.clone());
        if let Err(error) = registry.register_panel(contributed.extension.as_str(), panel) {
            log::warn!(
                "extension {}: panel was not registered: {error}",
                contributed.extension
            );
        }
    }
    let mut prompt_budget = 0usize;
    let mut prompt_sections = Vec::new();
    for prompt in &contributed.prompts {
        if let Some(section) = append_prompt(
            agent,
            &contributed.extension,
            contributed.trust,
            &policy,
            "Extension prompt",
            prompt,
            &mut prompt_budget,
        ) {
            prompt_sections.push(section);
        }
    }
    for context in &contributed.contexts {
        if !context_matches(context, contributed.workspace.as_deref()) {
            continue;
        }
        if let Some(section) = append_prompt(
            agent,
            &contributed.extension,
            contributed.trust,
            &policy,
            "Extension context",
            context,
            &mut prompt_budget,
        ) {
            prompt_sections.push(section);
        }
    }
    let mut skill_paths = Vec::new();
    let mut skills = Vec::new();
    if let Some(dir) = &contributed.skills_dir {
        if dir.is_dir() {
            let before: std::collections::HashSet<PathBuf> =
                agent.live_skill_paths().into_iter().collect();
            if let Err(error) = agent.add_live_skills_from_dir(dir.clone()) {
                log::warn!(
                    target: "bsh::extension",
                    "{}: skills were declared but could not be read: {error}",
                    contributed.extension
                );
            } else {
                // Record both the paths a suspend removes and the content a
                // resume puts back; the overlay is the only place the parsed
                // form lives.
                for skill in agent.live_skills() {
                    if before.contains(&skill.skill_dir) {
                        continue;
                    }
                    skill_paths.push(skill.skill_dir.clone());
                    skills.push(skill);
                }
            }
        }
    }
    let lease = ContributionLease {
        owner: contributed.extension.as_str().to_string(),
        tools: installed_tools.clone(),
        tool_handles: installed_handles,
        prompt_sections,
        skill_paths,
        skills,
        active,
        calls,
    };
    (
        PluginInfo {
            name: contributed.extension.as_str().to_string(),
            tools: installed_tools,
            enabled: true,
        },
        lease,
    )
}

fn append_prompt(
    agent: &mut agent::Agent,
    extension: &ExtensionId,
    trust: TrustLevel,
    policy: &PolicyEngine,
    heading: &str,
    contribution: &ferris_sdk::host_runtime::declarative::TextContribution,
    budget: &mut usize,
) -> Option<String> {
    let capability = if heading == "Extension prompt" {
        Capability::AgentPromptsProvide
    } else {
        Capability::AgentContextProvide
    };
    if policy
        .authorize_call(extension.as_str(), trust, capability, &CallScope::default())
        .is_err()
    {
        return None;
    }
    let text = contribution.text.trim();
    if text.is_empty() {
        return None;
    }
    let section = format!(
        "## {heading}: {} ({})
{}",
        contribution.description,
        extension,
        truncate(text, MAX_PROMPT_SECTION)
    );
    if *budget + section.len() > MAX_TOTAL_PROMPT {
        return None;
    }
    *budget += section.len();
    agent.add_live_prompt_section(&section);
    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_sdk::capability::PolicyMode;
    use ferris_sdk::host_runtime::empty_schema;
    use ferris_sdk::types::RuntimeKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn progress_is_bounded_to_a_fixed_window() {
        let mut window = ProgressWindow::new();
        let start = Instant::now();
        for _ in 0..PROGRESS_PER_WINDOW {
            assert!(window.allow_at(start));
        }
        assert!(!window.allow_at(start));
        assert!(window.allow_at(start + PROGRESS_WINDOW + Duration::from_millis(1)));
    }

    #[test]
    fn a_context_scope_matches_workspace_globs() {
        let scratch = Scratch::new("context-scope");
        std::fs::create_dir_all(scratch.0.join("src")).expect("src");
        std::fs::write(scratch.0.join("src/lib.rs"), "fn main() {}").expect("file");
        let mut context = ferris_sdk::host_runtime::declarative::TextContribution {
            id: ferris_sdk::types::ContributionId::parse("scoped").expect("id"),
            description: "Scoped".to_string(),
            text: "Text".to_string(),
            scope: vec!["src/**/*.rs".to_string()],
        };
        assert!(context_matches(&context, Some(&scratch.0)));
        assert!(!context_matches(&context, None));
        context.scope = vec!["tests/**/*.rs".to_string()];
        assert!(!context_matches(&context, Some(&scratch.0)));
        context.scope.clear();
        assert!(context_matches(&context, None));
    }

    /// Build output, hidden directories and ignored trees are not evidence
    /// that a workspace is the kind of project a context describes.
    #[test]
    fn a_context_scope_walks_only_project_files() {
        let scratch = Scratch::new("context-scope-policy");
        for tree in ["target/docs", ".hidden/docs", "generated/docs"] {
            std::fs::create_dir_all(scratch.0.join(tree)).expect("tree");
            std::fs::write(scratch.0.join(tree).join("design.md"), "").expect("file");
        }
        std::fs::write(scratch.0.join(".gitignore"), "generated/\n").expect("gitignore");
        let context = ferris_sdk::host_runtime::declarative::TextContribution {
            id: ferris_sdk::types::ContributionId::parse("scoped").expect("id"),
            description: "Scoped".to_string(),
            text: "Text".to_string(),
            scope: vec!["**/docs/design.md".to_string()],
        };
        assert!(
            !context_matches(&context, Some(&scratch.0)),
            "a build tree, a hidden directory and a gitignored tree are not a match"
        );
        std::fs::create_dir_all(scratch.0.join("docs")).expect("docs");
        std::fs::write(scratch.0.join("docs/design.md"), "").expect("design");
        assert!(context_matches(&context, Some(&scratch.0)));
    }

    /// A walk that runs out of budget reports no match instead of freezing the
    /// build that asked, which is what keeps a `**` scope over a large (or
    /// root) workspace from hiding the window.
    #[test]
    fn a_scope_walk_is_bounded() {
        let scratch = Scratch::new("context-scope-budget");
        std::fs::create_dir_all(scratch.0.join("docs")).expect("docs");
        std::fs::write(scratch.0.join("docs/design.md"), "").expect("design");
        assert!(scope_matches_within(
            &scratch.0,
            "**/docs/design.md",
            SCOPE_WALK_BUDGET
        ));
        assert!(
            !scope_matches_within(&scratch.0, "**/docs/design.md", 0),
            "an exhausted walk stops rather than blocking its caller"
        );
    }

    /// The literal directories in front of the first wildcard seed the walk, so
    /// an absent base is one failed directory check, not a scan of the workspace.
    #[test]
    fn a_scope_seeds_its_walk_with_its_literal_directories() {
        let scratch = Scratch::new("context-scope-prefix");
        std::fs::create_dir_all(scratch.0.join("docs")).expect("docs");
        std::fs::write(scratch.0.join("docs/design.md"), "").expect("design");
        assert!(scope_matches(&scratch.0, "docs/design.md"));
        assert!(!scope_matches(&scratch.0, "docs/other.md"));
        std::fs::create_dir_all(scratch.0.join("packages/p")).expect("package");
        std::fs::write(scratch.0.join("packages/p/extension.toml"), "").expect("manifest");
        assert!(scope_matches(&scratch.0, "packages/**/extension.toml"));
        assert!(
            !scope_matches(&scratch.0, "missing/**/extension.toml"),
            "a base that is not a directory cannot match"
        );
        let absolute = scratch.0.join("docs/design.md");
        assert!(scope_matches(&scratch.0, &absolute.to_string_lossy()));
        assert!(
            !scope_matches(&scratch.0, "/definitely/not/here/**/*.md"),
            "a scope outside the workspace cannot match it"
        );
    }
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-agent-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct EchoRuntime;

    #[async_trait]
    impl Runtime for EchoRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(arguments)
        }
    }

    struct ProgressRuntime;

    #[async_trait]
    impl Runtime for ProgressRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::String("done".to_string()))
        }

        async fn call_tool_progress(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
            progress: Option<tokio::sync::mpsc::Sender<Value>>,
        ) -> Result<ferris_sdk::host_runtime::ToolResult, ExtensionError> {
            if let Some(sender) = progress {
                let _ = sender.try_send(serde_json::json!({"message": "half"}));
            }
            Ok(ferris_sdk::host_runtime::ToolResult::text("done"))
        }
    }
    struct FixedRuntime(Value);

    #[async_trait]
    impl Runtime for FixedRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(self.0.clone())
        }
    }

    fn fixed_tool(
        scratch: &Scratch,
        value: Value,
        max_output_bytes: u64,
    ) -> (
        ExtensionTool,
        Arc<ferris_sdk::composable::dispatch::EventDispatcher>,
    ) {
        let mut contributed = contributed(vec![], TrustLevel::Builtin);
        contributed.runtime = Arc::new(FixedRuntime(value));
        contributed.max_output_bytes = max_output_bytes;
        let dispatch = Arc::new(ferris_sdk::composable::dispatch::EventDispatcher::new());
        let tool = ExtensionTool::with_dispatcher(
            &contributed,
            contributed.tools[0].clone(),
            policy(scratch),
            EventBus::new(),
            None,
            Arc::new(AtomicBool::new(true)),
            CallTracker::new(4),
            Arc::clone(&dispatch),
        );
        (tool, dispatch)
    }

    fn policy(scratch: &Scratch) -> Arc<PolicyEngine> {
        Arc::new(
            PolicyEngine::open(
                scratch.0.join("grants.toml"),
                scratch.0.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        )
    }

    fn contributed(capabilities: Vec<Capability>, trust: TrustLevel) -> Contributed {
        let extension = ExtensionId::parse("com.example.tool").expect("id");
        Contributed {
            extension,
            name: "Tool".to_string(),
            runtime: Arc::new(EchoRuntime),
            trust,
            tools: vec![ToolDescriptor {
                id: ferris_sdk::types::ContributionId::parse("echo").expect("tool id"),
                description: "Echo".to_string(),
                input_schema: empty_schema(),
                capabilities,
            }],
            commands: Vec::new(),
            panels: Vec::new(),
            prompts: Vec::new(),
            contexts: Vec::new(),
            workspace: None,
            skills_dir: None,
            workflows: Vec::new(),
            themes: Vec::new(),
            timeout_ms: 1000,
            max_output_bytes: 1024 * 1024,
            max_concurrent_calls: 4,
        }
    }

    #[tokio::test]
    async fn a_builtin_tool_is_qualified_and_runs() {
        let scratch = Scratch::new("run");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Builtin);
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        assert_eq!(tool.name(), "tool__echo");
        let result = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({"x": 1}))
            .await
            .expect("run");
        assert_eq!(result["x"], 1);
    }

    #[tokio::test]
    async fn run_streaming_forwards_progress_and_the_final_value() {
        use futures::StreamExt as _;
        let scratch = Scratch::new("stream");
        let mut contributed = contributed(vec![], TrustLevel::Builtin);
        contributed.runtime = Arc::new(ProgressRuntime);
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            None,
        );
        let mut stream =
            react::tool::registry::AsyncTool::run_streaming(&tool, &serde_json::json!({}))
                .await
                .expect("stream");
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item.expect("item"));
        }
        assert!(items.iter().any(|item| item.contains("half")), "{items:?}");
        assert_eq!(items.last().map(String::as_str), Some("done"));
    }

    #[tokio::test]
    async fn a_revoked_extension_tool_cannot_start_a_new_call() {
        let scratch = Scratch::new("revoked");
        let contributed = contributed(vec![], TrustLevel::Builtin);
        let tool = ExtensionTool::new_with_liveness(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            None,
            Arc::new(AtomicBool::new(false)),
        );
        let error = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({}))
            .await
            .expect_err("revoked tool must not run");
        assert!(error.to_string().contains("revoked"), "{error}");
    }

    #[tokio::test]
    async fn a_sensitive_capability_is_denied_without_a_grant() {
        let scratch = Scratch::new("deny");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Local);
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy(&scratch),
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        let error = react::tool::registry::AsyncTool::run(
            &tool,
            &serde_json::json!({"path": "/tmp/workspace/a.txt"}),
        )
        .await
        .expect_err("denied");
        assert!(error.to_string().contains("permission denied"), "{error}");
    }

    #[tokio::test]
    async fn an_explicit_grant_allows_the_call() {
        let scratch = Scratch::new("grant");
        let contributed = contributed(vec![Capability::WorkspaceWrite], TrustLevel::Local);
        let policy = policy(&scratch);
        policy
            .grant(
                "com.example.tool",
                Capability::WorkspaceWrite,
                ferris_sdk::capability::GrantScope::Workspace,
                None,
            )
            .expect("grant");
        let tool = ExtensionTool::new(
            &contributed,
            contributed.tools[0].clone(),
            policy,
            EventBus::new(),
            Some(PathBuf::from("/tmp/workspace")),
        );
        let result = react::tool::registry::AsyncTool::run(
            &tool,
            &serde_json::json!({"path": "/tmp/workspace/a.txt"}),
        )
        .await
        .expect("run");
        assert_eq!(result["path"], "/tmp/workspace/a.txt");
    }

    #[tokio::test]
    async fn a_result_exactly_at_the_output_ceiling_is_returned() {
        let scratch = Scratch::new("budget-at");
        let value = Value::String("a".repeat(100));
        let bytes = serde_json::to_vec(&value).expect("json").len() as u64;
        let (tool, _dispatch) = fixed_tool(&scratch, value.clone(), bytes);
        let result = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({}))
            .await
            .expect("a result at the ceiling is allowed");
        assert_eq!(result, value);
    }

    #[tokio::test]
    async fn a_result_one_byte_over_the_output_ceiling_is_refused() {
        let scratch = Scratch::new("budget-over");
        let value = Value::String("a".repeat(100));
        let bytes = serde_json::to_vec(&value).expect("json").len() as u64;
        let (tool, _dispatch) = fixed_tool(&scratch, value, bytes - 1);
        let error = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({}))
            .await
            .expect_err("a result over the ceiling must be refused");
        let message = error.to_string();
        assert!(message.contains("ceiling is 101"), "{message}");
        assert!(message.contains("tool result is 102 bytes"), "{message}");
    }

    #[tokio::test]
    async fn a_tool_after_hook_cannot_expand_a_result_past_the_ceiling() {
        use ferris_sdk::composable::dispatch::DispatchMode;

        let scratch = Scratch::new("budget-hook");
        let value = Value::String("ok".to_string());
        let budget = serde_json::to_vec(&value).expect("json").len() as u64;
        let (tool, dispatch) = fixed_tool(&scratch, value, budget);
        let expanded = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&expanded);
        let _hook = dispatch.on(
            "tool.after",
            DispatchMode::Waterfall,
            0,
            move |_value, chain| {
                marker.store(true, Ordering::SeqCst);
                chain.next(Value::String("x".repeat(4096)))
            },
        );
        let error = react::tool::registry::AsyncTool::run(&tool, &serde_json::json!({}))
            .await
            .expect_err("the post-hook value is over the ceiling");
        assert!(expanded.load(Ordering::SeqCst), "the hook did not run");
        let message = error.to_string();
        assert!(message.contains("tool result is 4098 bytes"), "{message}");
        assert!(message.contains("ceiling is 4"), "{message}");
    }

    #[tokio::test]
    async fn run_streaming_reports_the_output_ceiling_instead_of_the_final_value() {
        use futures::StreamExt as _;

        let scratch = Scratch::new("budget-stream");
        let value = Value::String("z".repeat(256));
        let (tool, _dispatch) = fixed_tool(&scratch, value, 32);
        let mut stream =
            react::tool::registry::AsyncTool::run_streaming(&tool, &serde_json::json!({}))
                .await
                .expect("stream");
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }
        assert_eq!(items.len(), 1, "{items:?}");
        let message = items[0]
            .as_ref()
            .expect_err("an over-budget result is an error item")
            .to_string();
        assert!(message.contains("ceiling is 32"), "{message}");
    }

    struct BlockingRuntime {
        started: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Notify>,
        cancels: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Runtime for BlockingRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            self.started.add_permits(1);
            self.release.notified().await;
            Ok(arguments)
        }

        async fn cancel(&self) -> Result<(), ExtensionError> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn tool_with(
        scratch: &Scratch,
        runtime: Arc<dyn Runtime>,
        calls: Arc<CallTracker>,
        max_output_bytes: u64,
    ) -> Arc<ExtensionTool> {
        let mut contributed = contributed(vec![], TrustLevel::Builtin);
        contributed.runtime = runtime;
        contributed.max_output_bytes = max_output_bytes;
        Arc::new(ExtensionTool::with_dispatcher(
            &contributed,
            contributed.tools[0].clone(),
            policy(scratch),
            EventBus::new(),
            None,
            Arc::new(AtomicBool::new(true)),
            calls,
            Arc::new(ferris_sdk::composable::dispatch::EventDispatcher::new()),
        ))
    }

    fn blocking_tool(
        scratch: &Scratch,
        calls: Arc<CallTracker>,
    ) -> (
        Arc<ExtensionTool>,
        Arc<tokio::sync::Semaphore>,
        Arc<tokio::sync::Notify>,
        Arc<AtomicUsize>,
    ) {
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let cancels = Arc::new(AtomicUsize::new(0));
        let runtime = Arc::new(BlockingRuntime {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            cancels: Arc::clone(&cancels),
        });
        (
            tool_with(scratch, runtime, calls, 1024 * 1024),
            started,
            release,
            cancels,
        )
    }

    #[tokio::test]
    async fn wait_empty_reports_whether_calls_drained() {
        let calls = CallTracker::new(2);
        let _call = calls.admit("a").await.expect("admitted");
        assert!(
            !calls.wait_empty(Duration::from_millis(20)).await,
            "a still-running call must report an incomplete drain"
        );
        calls.finish("a");
        assert!(calls.wait_empty(Duration::from_millis(20)).await);
    }
    #[tokio::test]
    async fn suspension_closes_admission_for_a_retained_tool_handle() {
        let scratch = Scratch::new("suspend-admission");
        let contributor = contributed(Vec::new(), TrustLevel::Local);
        let descriptor = contributor.tools[0].clone();
        let tool = ExtensionTool::new(
            &contributor,
            descriptor,
            policy(&scratch),
            EventBus::new(),
            None,
        );
        tool.run(&serde_json::json!({})).await.expect("admitted");
        tool.calls.set_suspended(true);
        let error = tool
            .run(&serde_json::json!({}))
            .await
            .expect_err("suspended tools must refuse new calls");
        assert!(error.to_string().contains("not accepting calls"), "{error}");
        tool.calls.set_suspended(false);
        tool.run(&serde_json::json!({})).await.expect("resumed");
    }
    #[tokio::test]
    async fn revoking_closes_admission_and_signals_in_flight_calls() {
        let calls = CallTracker::new(2);
        let token = calls.admit("a").await.expect("admitted");
        calls.close();
        assert!(
            calls.admit("b").await.is_none(),
            "a closed tracker must not admit a call after the liveness check"
        );
        assert!(
            token.load(Ordering::SeqCst),
            "closing the tracker signals the in-flight call"
        );
    }

    #[tokio::test]
    async fn the_concurrency_limit_is_enforced_at_admission() {
        let calls = CallTracker::new(1);
        let _first = calls.admit("a").await.expect("first call");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), calls.admit("b"))
                .await
                .is_err(),
            "a second call must wait while the limit is one"
        );
        calls.finish("a");
        let second = tokio::time::timeout(Duration::from_millis(500), calls.admit("b"))
            .await
            .expect("the waiter wakes")
            .expect("admitted");
        drop(second);
    }

    #[tokio::test]
    async fn a_runtime_cancel_gate_stops_new_admission() {
        let calls = CallTracker::new(4);
        let _call = calls.admit("a").await.expect("admitted");
        assert!(calls.cancel("a"));
        assert!(calls.begin_runtime_cancel("a"));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), calls.admit("b"))
                .await
                .is_err(),
            "admission waits while the runtime-wide cancel runs"
        );
        calls.end_runtime_cancel();
        let next = tokio::time::timeout(Duration::from_millis(500), calls.admit("b"))
            .await
            .expect("the waiter wakes")
            .expect("admitted");
        drop(next);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_one_call_leaves_a_sibling_running() {
        let scratch = Scratch::new("per-call-cancel");
        let (tool, started, release, cancels) = blocking_tool(&scratch, CallTracker::new(4));
        let a = {
            let tool = Arc::clone(&tool);
            tokio::spawn(async move {
                react::tool::registry::AsyncTool::run_with_call_id(
                    &*tool,
                    &serde_json::json!({"n": 1}),
                    "a",
                )
                .await
            })
        };
        started.acquire().await.expect("a started").forget();
        let b = {
            let tool = Arc::clone(&tool);
            tokio::spawn(async move {
                react::tool::registry::AsyncTool::run_with_call_id(
                    &*tool,
                    &serde_json::json!({"n": 2}),
                    "b",
                )
                .await
            })
        };
        started.acquire().await.expect("b started").forget();
        tool.cancel("a");
        let error = tokio::time::timeout(Duration::from_secs(1), a)
            .await
            .expect("a returns")
            .expect("a join")
            .expect_err("a is cancelled");
        assert!(error.to_string().contains("cancelled"), "{error}");
        assert!(!b.is_finished(), "cancelling a must not cancel b");
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            0,
            "a shared runtime must keep serving b"
        );
        release.notify_waiters();
        let result = tokio::time::timeout(Duration::from_secs(1), b)
            .await
            .expect("b returns")
            .expect("b join")
            .expect("b completes");
        assert_eq!(result["n"], 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lease_revocation_cancels_an_in_flight_call() {
        let scratch = Scratch::new("lease-cancel");
        let calls = CallTracker::new(4);
        let (tool, started, _release, _cancels) = blocking_tool(&scratch, Arc::clone(&calls));
        let lease = ContributionLease {
            owner: "com.example.tool".to_string(),
            tools: vec![tool.name().to_string()],
            tool_handles: vec![tool.clone() as Arc<dyn react::tool::registry::AsyncTool>],
            prompt_sections: Vec::new(),
            skill_paths: Vec::new(),
            skills: Vec::new(),
            active: Arc::clone(&tool.active),
            calls: Arc::clone(&calls),
        };
        let call = {
            let tool = Arc::clone(&tool);
            tokio::spawn(async move {
                react::tool::registry::AsyncTool::run_with_call_id(
                    &*tool,
                    &serde_json::json!({}),
                    "lease",
                )
                .await
            })
        };
        started.acquire().await.expect("call started").forget();
        lease.cancel_in_flight();
        let error = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("call returns")
            .expect("call join")
            .expect_err("the revoked call is cancelled");
        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_a_stream_cancels_its_call() {
        let scratch = Scratch::new("stream-drop");
        let (tool, started, _release, cancels) = blocking_tool(&scratch, CallTracker::new(4));
        let stream = react::tool::registry::AsyncTool::run_streaming_with_call_id(
            &*tool,
            &serde_json::json!({}),
            "stream-drop",
        )
        .await
        .expect("stream");
        started.acquire().await.expect("call started").forget();
        drop(stream);
        for _ in 0..20 {
            if cancels.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            cancels.load(Ordering::SeqCst),
            1,
            "dropping the stream must cancel the only in-flight call"
        );
    }

    struct FloodRuntime;

    #[async_trait]
    impl Runtime for FloodRuntime {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn call_tool_progress(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
            progress: Option<tokio::sync::mpsc::Sender<Value>>,
        ) -> Result<ferris_sdk::host_runtime::ToolResult, ExtensionError> {
            if let Some(sender) = progress {
                for _ in 0..500 {
                    let _ = sender.try_send(serde_json::json!({"text": "x".repeat(100)}));
                }
            }
            Ok(ferris_sdk::host_runtime::ToolResult::text("done"))
        }
    }

    #[tokio::test]
    async fn streaming_progress_is_held_to_the_output_budget() {
        use futures::StreamExt as _;

        let scratch = Scratch::new("stream-budget");
        let tool = tool_with(&scratch, Arc::new(FloodRuntime), CallTracker::new(4), 1_000);
        let mut stream =
            react::tool::registry::AsyncTool::run_streaming(&*tool, &serde_json::json!({}))
                .await
                .expect("stream");
        // Let the producer run while nobody reads: the queue stays bounded.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item.expect("item"));
        }
        let progress: Vec<&String> = items
            .iter()
            .filter(|item| item.as_str() != "done")
            .collect();
        let bytes: usize = progress.iter().map(|item| item.len()).sum();
        assert!(bytes <= 1_000, "progress used {bytes} bytes");
        assert!(
            progress.iter().all(|item| item.len() <= MAX_PROGRESS_BYTES),
            "a single progress chunk exceeded the cap"
        );
        assert_eq!(items.last().map(String::as_str), Some("done"));
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::extension_tool_metadata;

    #[test]
    fn an_extension_tool_declares_medium_risk_and_no_version() {
        let metadata = extension_tool_metadata();

        assert!(metadata.version.is_none(), "a package versions as a whole");
        assert_eq!(metadata.risk.as_deref(), Some("medium"));
        assert!(metadata
            .side_effects
            .iter()
            .any(|effect| effect == "extension"));
        assert!(metadata
            .permissions
            .iter()
            .any(|scope| scope == "extension.call"));
    }
}
