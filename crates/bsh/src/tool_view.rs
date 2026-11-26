//! Declarative model-facing projections for the built-in tools.
//!
//! The runtime, the audit log and the UI keep a tool's whole payload. The model
//! is sent only the part that can change its next call, because a host path, a
//! duration or a status word the envelope already carries costs tokens on every
//! turn and teaches the model nothing.
//!
//! A projection is declared here as data, not as code: [Field] selects a
//! dot-path of the payload, renames it, and can elide it when its value carries
//! no information ([Keep]). Nothing is synthesized: a path that is absent
//! contributes no key, so a view can never invent a fact.
//!
//! [wrap] is the one adapter the host installs: a tool with a declared view is
//! wrapped so the engine's model_view seam returns the projection, while every
//! other trait method (including run) is forwarded unchanged. A tool with no
//! view, and every extension or MCP tool, is handed back as-is.

use std::sync::Arc;

use react::tool::registry::AsyncTool;
use react::tool::{Tool, ToolDefinition, ToolError, ToolMetadata};
use serde_json::{Map, Value};

/// One selected field of a tool payload.
pub(crate) struct Field {
    /// Dot-separated path into the payload, for example "stdout.text".
    pub(crate) path: &'static str,
    /// The key the value is emitted under in the projection.
    pub(crate) name: &'static str,
    /// When the field is emitted at all.
    pub(crate) keep: Keep,
}

/// The per-field elision rule of one projection.
///
/// Elision is a property of the field's VALUE, not of its presence: a path that
/// is missing always emits nothing, while a present value is kept only when the
/// rule says it carries a fact.
#[derive(Clone, Copy)]
pub(crate) enum Keep {
    /// Emit the value whatever it is.
    Always,
    /// Emit non-empty strings, arrays and objects; a null is empty.
    NonEmpty,
    /// Emit only the boolean true.
    True,
    /// Emit numbers other than zero and non-empty strings.
    NonZero,
    /// Emit strings other than this sentinel (a "complete" status, for example).
    Not(&'static str),
}

/// Build a projection by selecting fields from a payload.
///
/// A missing path emits nothing. The result is always an object; it is empty
/// only when no selected field was present and kept, which the gate test rules
/// out for every built-in view.
pub(crate) fn project(payload: &Value, fields: &[Field]) -> Value {
    let mut object = Map::new();
    for field in fields {
        let Some(value) = lookup(payload, field.path) else {
            continue;
        };
        if keeps(value, field.keep) {
            object.insert(field.name.to_string(), value.clone());
        }
    }
    Value::Object(object)
}

/// Resolve one dot-separated path through nested objects.
fn lookup<'a>(payload: &'a Value, path: &str) -> Option<&'a Value> {
    let mut value = payload;
    for segment in path.split('.') {
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

/// Whether a present value survives its field's elision rule.
fn keeps(value: &Value, keep: Keep) -> bool {
    match keep {
        Keep::Always => true,
        Keep::NonEmpty => match value {
            Value::Null => false,
            Value::String(text) => !text.is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(object) => !object.is_empty(),
            _ => true,
        },
        Keep::True => value.as_bool() == Some(true),
        Keep::NonZero => match value {
            Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
            Value::String(text) => !text.is_empty(),
            _ => false,
        },
        Keep::Not(sentinel) => value.as_str().is_some_and(|text| text != sentinel),
    }
}

/// Define one projection function from a complete field list.
macro_rules! projection {
    ($view:ident { $( $path:literal => $name:literal : $keep:expr ),* $(,)? }) => {
        /// The model-facing projection for one built-in tool.
        fn $view(payload: &Value) -> Value {
            const FIELDS: &[Field] = &[
                $(Field { path: $path, name: $name, keep: $keep }),*
            ];
            project(payload, FIELDS)
        }
    };
}

// The shell. A failed command is an error envelope, which carries no data, so
// the view describes a command that ran: its output and, when the call ran
// unconfined, why.
projection!(view_bash {
    "stdout" => "stdout": Keep::NonEmpty,
    "stderr" => "stderr": Keep::NonEmpty,
    "exit_code" => "exit_code": Keep::NonZero,
    "sandbox_reason" => "sandbox_reason": Keep::NonEmpty,
    // Audit reads these two from the stored result. A confined call is the
    // norm and says nothing to the model; an unconfined one is exactly the
    // fact the audit exists to show.
    "sandbox" => "sandbox": Keep::Not("heel"),
    "network" => "network": Keep::Not("deny-all"),
});

// run_code's answer is the program's own result plus the log lines it printed.
projection!(view_run_code {
    "result" => "result": Keep::Always,
    "logs" => "logs": Keep::NonEmpty,
});

// The file family. status, tool and summary are omitted: the envelope carries
// the verdict and the tool name, and a sentence derived from both adds no fact.
projection!(view_read_file {
    "path" => "path": Keep::Always,
    "content" => "content": Keep::Always,
    "start_line" => "start_line": Keep::Always,
    "end_line" => "end_line": Keep::Always,
    "total_lines" => "total_lines": Keep::Always,
    "next_offset" => "next_offset": Keep::Always,
    "continued" => "continued": Keep::True,
    "hint" => "hint": Keep::NonEmpty,
});

projection!(view_read_files {
    "files" => "files": Keep::Always,
    "returned" => "returned": Keep::Always,
    "failed" => "failed": Keep::NonZero,
});

projection!(view_write_file {
    "path" => "path": Keep::Always,
    "outcome" => "outcome": Keep::Always,
    "bytes" => "bytes": Keep::Always,
});

projection!(view_edit_file {
    "path" => "path": Keep::Always,
    "replacements" => "replacements": Keep::NonZero,
    "outcome" => "outcome": Keep::Always,
    "added" => "added": Keep::NonZero,
    "deleted" => "deleted": Keep::NonZero,
    "diff" => "diff": Keep::NonEmpty,
    "diff_status" => "diff_status": Keep::Not("complete"),
    "hint" => "hint": Keep::NonEmpty,
});

projection!(view_sed {
    "path" => "path": Keep::Always,
    "outcome" => "outcome": Keep::Always,
    "mode" => "mode": Keep::Not("applied"),
    "content" => "content": Keep::NonEmpty,
    "content_kind" => "content_kind": Keep::Not("text"),
    "content_status" => "content_status": Keep::Not("complete"),
    "diff" => "diff": Keep::NonEmpty,
    "added" => "added": Keep::NonZero,
    "deleted" => "deleted": Keep::NonZero,
    "diff_status" => "diff_status": Keep::Not("complete"),
    "hint" => "hint": Keep::NonEmpty,
    "quiet" => "quiet": Keep::True,
    "exit_code" => "exit_code": Keep::NonZero,
});

projection!(view_patch {
    "files" => "files": Keep::Always,
    "failed" => "failed": Keep::NonZero,
    "changed_files" => "changed_files": Keep::Always,
    "diff_status" => "diff_status": Keep::Not("complete"),
});

projection!(view_list_dir {
    "path" => "path": Keep::Always,
    "entries" => "entries": Keep::Always,
    "hint" => "hint": Keep::NonEmpty,
});

projection!(view_glob {
    "path" => "path": Keep::Always,
    "matches" => "matches": Keep::Always,
    "hint" => "hint": Keep::NonEmpty,
});

projection!(view_grep {
    "path" => "path": Keep::Always,
    "matches" => "matches": Keep::Always,
    "rows" => "rows": Keep::NonZero,
    "binary" => "binary": Keep::NonEmpty,
    "files_without_match" => "files_without_match": Keep::NonEmpty,
    "hint" => "hint": Keep::NonEmpty,
});

// Code intelligence. One view covers every action: a search returns hits, an
// inspect returns the explained node and its neighbour lists, the diagnostics
// return their own rows, and a missing key is simply absent. workspaceId and
// root are host identity, not answers.
projection!(view_codel {
    "path" => "path": Keep::Always,
    "hits" => "hits": Keep::Always,
    "node" => "node": Keep::Always,
    "incoming" => "incoming": Keep::Always,
    "outgoing" => "outgoing": Keep::Always,
    "estimatedTokens" => "estimated_tokens": Keep::NonZero,
    "actions" => "actions": Keep::NonEmpty,
    "example" => "example": Keep::NonEmpty,
    "nodes" => "nodes": Keep::NonZero,
    "edges" => "edges": Keep::NonZero,
    "loadedFromStore" => "loaded_from_store": Keep::True,
    "parsedFiles" => "parsed_files": Keep::NonZero,
    "language" => "language": Keep::NonEmpty,
    "preset" => "preset": Keep::NonEmpty,
    "matches" => "matches": Keep::Always,
    "count" => "count": Keep::NonZero,
    "cycles" => "cycles": Keep::Always,
    "candidates" => "candidates": Keep::Always,
    "changedFiles" => "changed_files": Keep::NonEmpty,
    "changedNodes" => "changed_nodes": Keep::NonEmpty,
    "affected" => "affected": Keep::Always,
    "truncated" => "truncated": Keep::True,
});

// The plan. The counts are derivable from the items, so only the items are
// sent; each carries the step text and its progress state.
projection!(view_update_plan {
    "items" => "items": Keep::Always,
});

// The goal tools share one wire view; flattening it lets the model read the
// objective, phase and round count without a nested object's braces.
projection!(view_goal {
    "goal.objective" => "objective": Keep::Always,
    "goal.state" => "state": Keep::Always,
    "goal.rounds" => "rounds": Keep::Always,
    "goal.max_rounds" => "max_rounds": Keep::Always,
    "goal.blocked_reason" => "blocked": Keep::NonEmpty,
    "goal.activation" => "activation": Keep::Always,
});

// The reader's questions. The note is the instruction to end the turn, which
// the tool description repeats; keeping it lets the payload stand alone.
projection!(view_ask_user {
    "questions" => "questions": Keep::NonZero,
    "note" => "note": Keep::NonEmpty,
});

// Background jobs. The job's status is domain state, not the call's verdict,
// and it is what tells the model whether to keep waiting.
projection!(view_job_output {
    "id" => "id": Keep::Always,
    "status" => "status": Keep::Always,
    "exit_code" => "exit_code": Keep::NonZero,
    "output" => "output": Keep::NonEmpty,
    "output_truncated" => "output_truncated": Keep::True,
});

projection!(view_job_list {
    "jobs" => "jobs": Keep::Always,
});

projection!(view_job_kill {
    "id" => "id": Keep::Always,
    "killed" => "killed": Keep::Always,
    "status" => "status": Keep::Always,
});

// Automation tasks. applies is the one sentence that says the change lands
// when the turn ends, so it survives; session_id names the only session the
// task can run in, which a later call may need to match.
projection!(view_schedule_create {
    "id" => "id": Keep::Always,
    "name" => "name": Keep::Always,
    "session_id" => "session_id": Keep::Always,
    "every_seconds" => "every_seconds": Keep::Always,
    "status" => "status": Keep::Always,
    "applies" => "applies": Keep::NonEmpty,
});

projection!(view_schedule_mutation {
    "id" => "id": Keep::Always,
    "status" => "status": Keep::Always,
    "applies" => "applies": Keep::NonEmpty,
});

projection!(view_schedule_list {
    "schedules" => "schedules": Keep::Always,
    "pending" => "pending": Keep::NonEmpty,
});

// Durable tasks. One view per tool because each returns a different slice; the
// ids and states are the fields a follow-up call needs.
projection!(view_task_create {
    "task" => "task": Keep::Always,
    "run" => "run": Keep::NonEmpty,
    "state" => "state": Keep::Always,
    "peer" => "peer": Keep::NonEmpty,
    "next_fire_ms" => "next_fire_ms": Keep::NonZero,
    "verification_contract" => "verification_contract": Keep::NonEmpty,
    "verification" => "verification": Keep::NonEmpty,
});

projection!(view_task_cancel {
    "task" => "task": Keep::Always,
    "state" => "state": Keep::Always,
});

projection!(view_task_join {
    "settled" => "settled": Keep::Always,
    "tasks" => "tasks": Keep::Always,
});

projection!(view_task_list {
    "total" => "total": Keep::Always,
    "tasks" => "tasks": Keep::Always,
});

projection!(view_task_retry {
    "task" => "task": Keep::Always,
    "run" => "run": Keep::NonEmpty,
    "state" => "state": Keep::Always,
    "deduplicated" => "deduplicated": Keep::True,
    "result" => "result": Keep::NonEmpty,
});

projection!(view_task_status {
    "id" => "id": Keep::Always,
    "title" => "title": Keep::Always,
    "status" => "status": Keep::Always,
    "tier" => "tier": Keep::Always,
    "verified_completion" => "verified_completion": Keep::True,
    "verification" => "verification": Keep::NonEmpty,
    "attempts" => "attempts": Keep::Always,
    "next_fire_ms" => "next_fire_ms": Keep::NonZero,
});

projection!(view_task_graph {
    "succeeded" => "succeeded": Keep::Always,
    "failed" => "failed": Keep::Always,
    "tasks" => "tasks": Keep::Always,
});

// Delegated children. Elapsed time and token usage describe the child's cost,
// not what the parent should ask next, so they stay in the audit.
projection!(view_subagent {
    "subagent_id" => "subagent_id": Keep::Always,
    "title" => "title": Keep::Always,
    "status" => "status": Keep::Always,
    "note" => "note": Keep::NonEmpty,
});

projection!(view_subagent_result {
    "runs" => "runs": Keep::Always,
    "subagent_id" => "subagent_id": Keep::Always,
    "title" => "title": Keep::Always,
    "status" => "status": Keep::Always,
    "result" => "result": Keep::NonEmpty,
    "error" => "error": Keep::NonEmpty,
    "note" => "note": Keep::NonEmpty,
});

projection!(view_send_message {
    "subagent_id" => "subagent_id": Keep::Always,
    "status" => "status": Keep::Always,
    "note" => "note": Keep::NonEmpty,
});

projection!(view_interrupt_agent {
    "subagent_id" => "subagent_id": Keep::Always,
    "interrupted" => "interrupted": Keep::Always,
    "status" => "status": Keep::Always,
    "note" => "note": Keep::NonEmpty,
});

// Presented surfaces. The id is the handle every later patch or close needs.
projection!(view_present_surface {
    "id" => "id": Keep::Always,
    "slot" => "slot": Keep::Always,
    "note" => "note": Keep::NonEmpty,
});

projection!(view_patch_surface {
    "id" => "id": Keep::Always,
    "revision" => "revision": Keep::Always,
});

projection!(view_close_surface {
    "id" => "id": Keep::Always,
    "closed" => "closed": Keep::Always,
});

projection!(view_surface_catalogue {
    "schema_version" => "schema_version": Keep::Always,
    "limits" => "limits": Keep::NonEmpty,
    "node_types" => "node_types": Keep::Always,
    "required" => "required": Keep::NonEmpty,
    "slots" => "slots": Keep::NonEmpty,
});

// Web search: the sources are the evidence, and each carries its own citation
// fields. The query echo and the standing notice are omitted.
projection!(view_web_search {
    "sources" => "sources": Keep::Always,
});

// Federation. state is the delegation's domain state, which is exactly what
// tells the model whether to await, read or report.
projection!(view_federation_status {
    "node" => "node": Keep::Always,
    "name" => "name": Keep::Always,
    "peers" => "peers": Keep::Always,
    "delegations" => "delegations": Keep::Always,
    "leases" => "leases": Keep::Always,
});

projection!(view_federation_peers {
    "peers" => "peers": Keep::Always,
});

projection!(view_federation_delegation {
    "target" => "target": Keep::Always,
    "delegation" => "delegation": Keep::Always,
    "state" => "state": Keep::Always,
    "terminal" => "terminal": Keep::Always,
    "executable" => "executable": Keep::Always,
    "result" => "result": Keep::NonEmpty,
    "error" => "error": Keep::NonEmpty,
    "note" => "note": Keep::NonEmpty,
});

projection!(view_federation_cancel {
    "request.id" => "delegation": Keep::Always,
    "request.target" => "target": Keep::Always,
    "state" => "state": Keep::Always,
    "result" => "result": Keep::NonEmpty,
    "error" => "error": Keep::NonEmpty,
});

projection!(view_federation_query {
    "responder" => "responder": Keep::Always,
    "items" => "items": Keep::Always,
    "truncated" => "truncated": Keep::True,
});

projection!(view_federation_cleanup {
    "removed" => "removed": Keep::Always,
    "failed" => "failed": Keep::NonEmpty,
});

/// The identity view for mcp_gateway.
///
/// The gateway is a pass-through: an action=list result is bsh's own tool
/// catalogue, while an action=call result is an arbitrary third-party MCP
/// payload whose shape bsh does not own. Projecting it here would drop data a
/// server chose to send, so the payload is forwarded whole.
fn view_mcp_gateway(payload: &Value) -> Value {
    payload.clone()
}

/// The identity view for federation_capabilities.
///
/// This family member is declared but its dispatch arm is not implemented in
/// this build (the family's execute match refuses the name), so its payload
/// shape cannot be confirmed from a producer. An identity view keeps whatever
/// a later implementation returns instead of dropping every field of it.
fn view_federation_capabilities(payload: &Value) -> Value {
    payload.clone()
}

/// The names that carry a declared view, in the same order as the built-in
/// metadata table.
pub(crate) const VIEW_TOOLS: &[&str] = &[
    "patch",
    "ask_user",
    "bash",
    "close_surface",
    "codel",
    "edit_file",
    "federation_await",
    "federation_cancel",
    "federation_capabilities",
    "federation_cleanup",
    "federation_delegate",
    "federation_peers",
    "federation_query",
    "federation_status",
    "get_goal",
    "glob",
    "grep",
    "interrupt_agent",
    "job_kill",
    "job_list",
    "job_output",
    "list_dir",
    "mcp_gateway",
    "patch_surface",
    "present_surface",
    "read_file",
    "read_files",
    "run_code",
    "schedule_create",
    "schedule_delete",
    "schedule_edit",
    "schedule_list",
    "sed",
    "send_message",
    "subagent",
    "subagent_result",
    "surface_catalogue",
    "task_cancel",
    "task_create",
    "task_graph",
    "task_join",
    "task_list",
    "task_retry",
    "task_status",
    "update_goal",
    "update_plan",
    "web_search",
    "write_file",
];

/// The one table mapping a built-in tool name to its model-facing view.
///
/// None means the tool declares no view; the engine then sends its payload
/// unchanged, which is the safe answer for a tool this host does not own.
pub(crate) fn view_for(name: &str) -> Option<fn(&Value) -> Value> {
    // The declared roster is the gate: a name outside it has no view even if
    // the match below somehow grew an arm for it, so the table cannot expand
    // by accident.
    if !VIEW_TOOLS.contains(&name) {
        return None;
    }
    Some(match name {
        "patch" => view_patch,
        "ask_user" => view_ask_user,
        "bash" => view_bash,
        "close_surface" => view_close_surface,
        "codel" => view_codel,
        "edit_file" => view_edit_file,
        "federation_await" => view_federation_delegation,
        "federation_cancel" => view_federation_cancel,
        "federation_capabilities" => view_federation_capabilities,
        "federation_cleanup" => view_federation_cleanup,
        "federation_delegate" => view_federation_delegation,
        "federation_peers" => view_federation_peers,
        "federation_query" => view_federation_query,
        "federation_status" => view_federation_status,
        "get_goal" => view_goal,
        "glob" => view_glob,
        "grep" => view_grep,
        "interrupt_agent" => view_interrupt_agent,
        "job_kill" => view_job_kill,
        "job_list" => view_job_list,
        "job_output" => view_job_output,
        "list_dir" => view_list_dir,
        "mcp_gateway" => view_mcp_gateway,
        "patch_surface" => view_patch_surface,
        "present_surface" => view_present_surface,
        "read_file" => view_read_file,
        "read_files" => view_read_files,
        "run_code" => view_run_code,
        "schedule_create" => view_schedule_create,
        "schedule_delete" => view_schedule_mutation,
        "schedule_edit" => view_schedule_mutation,
        "schedule_list" => view_schedule_list,
        "sed" => view_sed,
        "send_message" => view_send_message,
        "subagent" => view_subagent,
        "subagent_result" => view_subagent_result,
        "surface_catalogue" => view_surface_catalogue,
        "task_cancel" => view_task_cancel,
        "task_create" => view_task_create,
        "task_graph" => view_task_graph,
        "task_join" => view_task_join,
        "task_list" => view_task_list,
        "task_retry" => view_task_retry,
        "task_status" => view_task_status,
        "update_goal" => view_goal,
        "update_plan" => view_update_plan,
        "web_search" => view_web_search,
        "write_file" => view_write_file,
        _ => return None,
    })
}

/// Wrap one built-in tool so its result payload reads through [view_for].
///
/// A tool with no declared view is returned unchanged, so an unrecognized tool
/// keeps its current behavior at no cost. Every method except model_view is
/// forwarded to the wrapped tool: this adapter changes what the MODEL reads,
/// never what any other reader or the tool itself does.
pub(crate) fn wrap(tool: Arc<dyn agent::tools::Tool>) -> Arc<dyn agent::tools::Tool> {
    match view_for(tool.name()) {
        Some(view) => Arc::new(ViewedTool { inner: tool, view }),
        None => tool,
    }
}

/// The tool adapter that adds a declared model view to another tool.
struct ViewedTool {
    /// The tool whose behavior and metadata are forwarded.
    inner: Arc<dyn agent::tools::Tool>,
    /// The projection this tool's results are read through.
    view: fn(&Value) -> Value,
}

impl Tool for ViewedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        self.inner.run(input)
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        self.inner.metadata()
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn model_view(&self, payload: &Value) -> Value {
        (self.view)(payload)
    }

    fn to_openai_definition(&self) -> ToolDefinition {
        self.inner.to_openai_definition()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id)
    }

    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        self.inner.proxy_target(args)
    }
}

/// Wrap an asynchronous built-in with its declared view.
///
/// The async builtins register through a different agent seam than the bundled
/// ones, so the adapter exists in both flavors. A tool with no view, including
/// every extension or MCP tool, is returned unchanged.
pub(crate) fn wrap_async(tool: Arc<dyn AsyncTool>) -> Arc<dyn AsyncTool> {
    match view_for(tool.name()) {
        Some(view) => Arc::new(ViewedAsyncTool { inner: tool, view }),
        None => tool,
    }
}

/// The async flavor of [`ViewedTool`]: forwards everything, projects the result.
struct ViewedAsyncTool {
    /// The tool whose behavior and metadata are forwarded.
    inner: Arc<dyn AsyncTool>,
    /// The projection this tool's results are read through.
    view: fn(&Value) -> Value,
}

#[async_trait::async_trait]
impl AsyncTool for ViewedAsyncTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn category(&self) -> String {
        self.inner.category()
    }

    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        self.inner.run(input).await
    }

    fn metadata(&self) -> Option<&ToolMetadata> {
        self.inner.metadata()
    }

    async fn run_with_call_id(&self, input: &Value, call_id: &str) -> Result<Value, ToolError> {
        self.inner.run_with_call_id(input, call_id).await
    }

    fn json_schema(&self) -> Value {
        self.inner.json_schema()
    }

    fn model_view(&self, payload: &Value) -> Value {
        (self.view)(payload)
    }

    fn to_openai_definition(&self) -> ToolDefinition {
        self.inner.to_openai_definition()
    }

    fn is_skill(&self) -> bool {
        self.inner.is_skill()
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }

    fn is_cancelable(&self) -> bool {
        self.inner.is_cancelable()
    }

    fn cancel(&self, call_id: &str) {
        self.inner.cancel(call_id)
    }

    fn deferred(&self) -> bool {
        self.inner.deferred()
    }

    fn proxy_target(&self, args: &Value) -> Option<(String, Value)> {
        self.inner.proxy_target(args)
    }

    async fn run_streaming(
        &self,
        input: &Value,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<String, ToolError>> + Send>>,
        ToolError,
    > {
        self.inner.run_streaming(input).await
    }

    async fn run_streaming_with_call_id(
        &self,
        input: &Value,
        call_id: &str,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<String, ToolError>> + Send>>,
        ToolError,
    > {
        self.inner.run_streaming_with_call_id(input, call_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use react::tool::result::ToolResult;
    use serde_json::json;

    /// The full built-in roster, in the order the metadata table declares it.
    const BUILTINS: [&str; 48] = [
        "patch",
        "ask_user",
        "bash",
        "close_surface",
        "codel",
        "edit_file",
        "federation_await",
        "federation_cancel",
        "federation_capabilities",
        "federation_cleanup",
        "federation_delegate",
        "federation_peers",
        "federation_query",
        "federation_status",
        "get_goal",
        "glob",
        "grep",
        "interrupt_agent",
        "job_kill",
        "job_list",
        "job_output",
        "list_dir",
        "mcp_gateway",
        "patch_surface",
        "present_surface",
        "read_file",
        "read_files",
        "run_code",
        "schedule_create",
        "schedule_delete",
        "schedule_edit",
        "schedule_list",
        "sed",
        "send_message",
        "subagent",
        "subagent_result",
        "surface_catalogue",
        "task_cancel",
        "task_create",
        "task_graph",
        "task_join",
        "task_list",
        "task_retry",
        "task_status",
        "update_goal",
        "update_plan",
        "web_search",
        "write_file",
    ];

    /// The model document one call produces, the way the engine renders it.
    fn model_document(tool: &str, payload: Value) -> String {
        let view = view_for(tool).unwrap_or_else(|| panic!("{tool} has a view"));
        let envelope = ToolResult::from_value(tool, payload);
        let projected = Some(view(&envelope.data));
        envelope.to_model_text(projected, 0)
    }

    /// The model document's data object, parsed back from the text.
    fn model_data(tool: &str, payload: Value) -> Value {
        let document = model_document(tool, payload);
        let value: Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("not JSON: {error}: {document}"));
        value["data"].clone()
    }

    #[test]
    fn every_builtin_declares_a_view_and_every_view_names_a_builtin() {
        assert_eq!(VIEW_TOOLS, &BUILTINS[..], "the table and the roster agree");
        for name in BUILTINS {
            assert!(view_for(name).is_some(), "{name} has no model view");
            assert!(
                crate::tool_metadata::builtin_metadata(name).is_some(),
                "{name} is not a declared built-in"
            );
        }
        for name in VIEW_TOOLS {
            assert!(BUILTINS.contains(name), "{name} is not a built-in");
        }
    }

    #[test]
    fn a_missing_path_contributes_nothing_and_a_kept_path_is_renamed() {
        let fields = [
            Field {
                path: "a.b",
                name: "renamed",
                keep: Keep::Always,
            },
            Field {
                path: "a.absent",
                name: "absent",
                keep: Keep::Always,
            },
        ];
        let projected = project(&json!({"a": {"b": 7}}), &fields);
        assert_eq!(projected, json!({"renamed": 7}));
    }

    #[test]
    fn a_non_object_between_path_segments_is_a_miss() {
        let fields = [Field {
            path: "a.b",
            name: "b",
            keep: Keep::Always,
        }];
        assert_eq!(project(&json!({"a": "text"}), &fields), json!({}));
    }

    #[test]
    fn non_empty_elides_every_empty_shape() {
        let fields = [
            Field {
                path: "s",
                name: "s",
                keep: Keep::NonEmpty,
            },
            Field {
                path: "a",
                name: "a",
                keep: Keep::NonEmpty,
            },
            Field {
                path: "o",
                name: "o",
                keep: Keep::NonEmpty,
            },
            Field {
                path: "n",
                name: "n",
                keep: Keep::NonEmpty,
            },
        ];
        assert_eq!(
            project(&json!({"s": "x", "a": [1], "o": {"k": 1}, "n": 2}), &fields),
            json!({"s": "x", "a": [1], "o": {"k": 1}, "n": 2})
        );
        assert_eq!(
            project(&json!({"s": "", "a": [], "o": {}, "n": null}), &fields),
            json!({})
        );
    }

    #[test]
    fn true_keeps_only_the_boolean_true() {
        let fields = [
            Field {
                path: "yes",
                name: "yes",
                keep: Keep::True,
            },
            Field {
                path: "no",
                name: "no",
                keep: Keep::True,
            },
            Field {
                path: "other",
                name: "other",
                keep: Keep::True,
            },
        ];
        assert_eq!(
            project(&json!({"yes": true, "no": false, "other": "true"}), &fields),
            json!({"yes": true})
        );
    }

    #[test]
    fn non_zero_keeps_numbers_and_non_empty_strings() {
        let fields = [
            Field {
                path: "n",
                name: "n",
                keep: Keep::NonZero,
            },
            Field {
                path: "z",
                name: "z",
                keep: Keep::NonZero,
            },
            Field {
                path: "s",
                name: "s",
                keep: Keep::NonZero,
            },
            Field {
                path: "e",
                name: "e",
                keep: Keep::NonZero,
            },
            Field {
                path: "b",
                name: "b",
                keep: Keep::NonZero,
            },
        ];
        assert_eq!(
            project(
                &json!({"n": 3, "z": 0, "s": "x", "e": "", "b": true}),
                &fields
            ),
            json!({"n": 3, "s": "x"})
        );
    }

    #[test]
    fn not_keeps_strings_other_than_its_sentinel() {
        let fields = [
            Field {
                path: "kept",
                name: "kept",
                keep: Keep::Not("complete"),
            },
            Field {
                path: "dropped",
                name: "dropped",
                keep: Keep::Not("complete"),
            },
            Field {
                path: "other",
                name: "other",
                keep: Keep::Not("complete"),
            },
        ];
        assert_eq!(
            project(
                &json!({"kept": "truncated", "dropped": "complete", "other": 1}),
                &fields
            ),
            json!({"kept": "truncated"})
        );
    }

    #[test]
    fn a_summary_status_and_tool_echo_never_reach_the_model() {
        let fields = view_for("read_file").expect("a read view");
        let projected = fields(&json!({
            "path": "a.txt",
            "content": "x",
            "tool": "read_file",
            "status": "ok",
            "summary": "read_file completed",
        }));
        assert_eq!(projected, json!({"path": "a.txt", "content": "x"}));
    }

    #[test]
    fn a_bash_echo_hi_projection_is_the_output_only() {
        let payload = json!({
            "stdout": "hi\n",
            "stderr": "",
            "exit_code": 0,
            "success": true,
            "sandbox": "heel",
            "network": "deny-all",
        });
        let document = model_document("bash", payload.clone());
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({"status": "success", "data": {"stdout": "hi\n"}})
        );
        // The full payload a reader or the audit keeps, for the size contrast:
        // the model document is 45 bytes against 124 before the projection.
        let full = ToolResult::from_value("bash", payload).to_model_text(None, 0);
        assert_eq!(full.len(), 124, "the unprojected document: {full}");
        assert_eq!(document.len(), 45, "the projected document: {document}");
        assert!(document.len() < 64, "the bash budget holds: {document}");
    }

    #[test]
    fn a_bash_result_that_ran_unconfined_names_why() {
        let payload = json!({
            "stdout": "hi\n",
            "stderr": "",
            "exit_code": 0,
            "success": true,
            "sandbox": "none",
            "sandbox_reason": "a preset turned the sandbox off",
            "network": "unconfined",
        });
        let data = model_data("bash", payload);
        assert_eq!(
            data,
            json!({
                "stdout": "hi\n",
                "sandbox": "none",
                "sandbox_reason": "a preset turned the sandbox off",
                "network": "unconfined",
            })
        );
    }

    #[test]
    fn a_read_file_projection_keeps_the_page_and_its_continuation() {
        let payload = json!({
            "path": "notes.txt",
            "content": "alpha\nbeta\n",
            "count": 2,
            "start_line": 1,
            "end_line": 2,
            "total_lines": 2,
            "bytes": 11,
            "next_offset": null,
            "tool": "read_file",
            "status": "ok",
        });
        let document = model_document("read_file", payload);
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({
                "status": "success",
                "data": {
                    "path": "notes.txt",
                    "content": "alpha\nbeta\n",
                    "start_line": 1,
                    "end_line": 2,
                    "total_lines": 2,
                    "next_offset": null,
                }
            })
        );
        assert!(
            document.len() <= 256,
            "the read budget holds: {}",
            document.len()
        );
    }

    #[test]
    fn a_glob_projection_keeps_the_paths_and_the_search_root() {
        let payload = json!({
            "path": "src",
            "pattern": "*.rs",
            "count": 2,
            "matches": ["main.rs", "lib.rs"],
            "tool": "glob",
            "status": "ok",
        });
        let document = model_document("glob", payload);
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({
                "status": "success",
                "data": {"path": "src", "matches": ["main.rs", "lib.rs"]}
            })
        );
        assert!(
            document.len() <= 160,
            "the glob budget holds: {}",
            document.len()
        );
    }

    #[test]
    fn a_grep_projection_keeps_matches_and_the_coverage_markers() {
        let payload = json!({
            "path": "src",
            "pattern": "fn main",
            "count": 1,
            "matches": [{"path": "main.rs", "line": 3, "text": "fn main() {"}],
            "rows": 1,
            "binary": [],
            "files_without_match": [],
            "hint": "row limit reached; raise limit to continue",
            "tool": "grep",
            "status": "ok",
        });
        let document = model_document("grep", payload);
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({
                "status": "success",
                "data": {
                    "path": "src",
                    "matches": [{"path": "main.rs", "line": 3, "text": "fn main() {"}],
                    "rows": 1,
                    "hint": "row limit reached; raise limit to continue",
                }
            })
        );
        assert!(
            document.len() <= 320,
            "the grep budget holds: {}",
            document.len()
        );
    }

    #[test]
    fn a_job_output_projection_keeps_the_id_state_and_output() {
        let payload = json!({
            "id": "job-1",
            "status": "succeeded",
            "exit_code": 0,
            "running": false,
            "duration_ms": 42,
            "output": "hi\n",
            "output_bytes": 3,
            "output_truncated": false,
        });
        let document = model_document("job_output", payload);
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({
                "status": "success",
                "data": {"id": "job-1", "status": "succeeded", "output": "hi\n"}
            })
        );
        assert!(
            document.len() <= 128,
            "the job budget holds: {}",
            document.len()
        );
    }

    #[test]
    fn an_update_plan_projection_keeps_the_steps() {
        let payload = json!({
            "total": 2,
            "pending": 1,
            "in_progress": 0,
            "completed": 1,
            "items": [
                {"text": "read the file", "status": "completed"},
                {"text": "apply the edit", "status": "pending"},
            ],
        });
        let document = model_document("update_plan", payload);
        assert_eq!(
            serde_json::from_str::<Value>(&document).expect("JSON"),
            json!({
                "status": "success",
                "data": {
                    "items": [
                        {"text": "read the file", "status": "completed"},
                        {"text": "apply the edit", "status": "pending"},
                    ]
                }
            })
        );
        assert!(
            document.len() <= 320,
            "the plan budget holds: {}",
            document.len()
        );
    }

    #[test]
    fn a_wrapped_tool_forwards_everything_but_the_model_view() {
        use agent::tools::FunctionTool;

        let inner: Arc<dyn agent::tools::Tool> = Arc::new(FunctionTool::new(
            "bash",
            "run a command",
            json!({"type": "object", "properties": {}}),
            |args| Ok(args.clone()),
        ));
        let wrapped = wrap(Arc::clone(&inner));
        assert_eq!(wrapped.name(), "bash");
        assert_eq!(wrapped.description(), "run a command");
        assert_eq!(
            wrapped.category(),
            inner.category(),
            "category is forwarded"
        );
        assert_eq!(
            wrapped.json_schema(),
            json!({"type": "object", "properties": {}})
        );
        assert!(wrapped.metadata().is_none());
        assert!(!wrapped.is_skill());
        assert!(!wrapped.is_cancelable());
        assert!(!wrapped.deferred());
        assert!(wrapped.proxy_target(&json!({})).is_none());
        assert_eq!(
            wrapped.model_view(&json!({"stdout": "hi\n", "cwd": "/"})),
            json!({"stdout": "hi\n"})
        );
        assert_eq!(
            wrapped.to_openai_definition().function.name,
            inner.to_openai_definition().function.name
        );
        assert_eq!(
            wrapped.run(&json!({"k": 1})).expect("the forwarded run"),
            json!({"k": 1})
        );
    }

    #[test]
    fn a_tool_without_a_view_is_returned_unchanged() {
        use agent::tools::FunctionTool;

        let inner: Arc<dyn agent::tools::Tool> = Arc::new(FunctionTool::new(
            "not_a_builtin",
            "an extension tool",
            json!({"type": "object", "properties": {}}),
            |_args| Ok(json!({"payload": "untouched"})),
        ));
        let wrapped = wrap(Arc::clone(&inner));
        assert!(
            Arc::ptr_eq(&wrapped, &inner),
            "an unrecognized tool is not wrapped"
        );
        assert_eq!(
            wrapped.model_view(&json!({"payload": "untouched"})),
            json!({"payload": "untouched"})
        );
    }
}
