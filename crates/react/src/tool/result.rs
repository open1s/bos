//! One result envelope for every tool, whatever the tool returns.
//!
//! A model reads a tool result as data, and the runtime reads it as a verdict.
//! Before this module existed the verdict was prose: the engine prefixed a
//! failure with `Error: ` and a host guessed from that prefix, while a tool that
//! returned `{"success": false}` was a success to every caller that only looked
//! at the `Result`. The envelope makes the verdict explicit and keeps the
//! tool's own payload untouched underneath it.
//!
//! ~~~json
//! {
//!   "status": "partial",
//!   "summary": "read_file returned a partial result",
//!   "data": { "content": "...", "truncated": true },
//!   "continuation": { "tool": "read_file", "args": { "offset": 401 } },
//!   "truncated": true
//! }
//! ~~~
//!
//! Invariants:
//!
//! - Every result has a valid `status` and a one-sentence `summary`.
//! - `error` is present exactly when `status` is `error`.
//! - `pending` is not completion and `partial` is not failure; both keep the
//!   context needed to continue.
//! - `data` is the tool's own payload, unmodified: the envelope adds
//!   semantics, it never discards domain data.
//! - A result too large for the context is cut explicitly, stays valid JSON,
//!   and says so with `truncated`.
//!
//! `effects` is declared in the contract but no producer fills it yet: the
//! runtime knows a tool's declared side effects, not what one call actually
//! did. It stays empty until an observation seam exists, rather than being
//! filled with a guess.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::error::{ToolError, ToolErrorCode};

/// The default ceiling for one serialized result.
///
/// A tool that prints a megabyte (a build log, a base64 dump) must not become
/// the next request. The head and tail are kept because the failure and the
/// summary usually sit at opposite ends; the middle is the part a model can
/// ask for again with a narrower call.
pub const DEFAULT_RESULT_CAP: usize = 48 * 1024;

/// How far an operation got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// The operation completed.
    Success,
    /// Some results were returned, but the set is incomplete.
    Partial,
    /// The operation was accepted and has not finished.
    Pending,
    /// The operation failed.
    Error,
}

impl ToolStatus {
    /// The stable lower-case name used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Pending => "pending",
            Self::Error => "error",
        }
    }

    /// Whether this status is a failure.
    pub fn is_error(self) -> bool {
        matches!(self, Self::Error)
    }

    /// Whether the result is known to be incomplete.
    pub fn is_incomplete(self) -> bool {
        matches!(self, Self::Partial | Self::Pending)
    }
}

/// Whether a payload speaks the tool result contract rather than carrying
/// domain state that happens to use the word `status`.
///
/// The built-in file tools declare it through `bsh::file_result::finish`, which
/// writes the tool name beside the status; the engine's own synthetic
/// cancellation record does the same.
fn is_result_contract(object: &Map<String, Value>) -> bool {
    object.get("status").is_some() && object.get("tool").and_then(Value::as_str).is_some()
}

/// Classify a status word a tool put in its own payload.
///
/// Tools predate the envelope and some speak their own vocabulary ("ok",
/// "empty", "running"); that vocabulary is translated here instead of being
/// rewritten in every tool, so one table owns the mapping.
fn parse_status(raw: &str) -> Option<ToolStatus> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ok" | "success" | "succeeded" | "complete" | "completed" | "done" | "empty"
        | "no_results" => Some(ToolStatus::Success),
        "partial" | "incomplete" | "truncated" | "some_failed" => Some(ToolStatus::Partial),
        "pending" | "submitted" | "accepted" | "queued" | "running" | "in_progress" | "waiting" => {
            Some(ToolStatus::Pending)
        }
        "error" | "failed" | "failure" | "cancelled" | "canceled" => Some(ToolStatus::Error),
        _ => None,
    }
}

/// A structured failure inside the envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolErrorInfo {
    /// The stable error class, for example `timeout` or `invalid_input`.
    pub code: String,
    /// What happened, in one sentence.
    pub message: String,
    /// Whether an unchanged retry may succeed.
    pub retryable: bool,
    /// Whether the world may already have changed despite the failure.
    pub indeterminate: bool,
    /// The offending argument, when the failure names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Whether that argument was absent rather than malformed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
}

impl ToolErrorInfo {
    /// Classify one tool error.
    pub fn from_error(error: &ToolError) -> Self {
        Self {
            code: error.code().as_str().to_string(),
            message: error.to_string(),
            retryable: error.retryable(),
            indeterminate: error.indeterminate(),
            field: error.field().map(str::to_string),
            missing: error.missing_field(),
        }
    }

    /// A failure the tool reported in its payload rather than as an `Err`.
    fn reported(code: ToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_str().to_string(),
            message: message.into(),
            retryable: false,
            indeterminate: true,
            field: None,
            missing: false,
        }
    }
}

/// How to continue an incomplete result.
///
/// The continuation names the same tool and the exact arguments for the next
/// call. It never invents a new operation: a tool that has no continuation
/// omits the field, which is what tells the model the result is all there is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolContinuation {
    /// The tool to call again.
    pub tool: String,
    /// The exact arguments for that call.
    pub args: Value,
    /// Why the result is incomplete, when the tool says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The shared outer contract for one tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// How far the operation got.
    pub status: ToolStatus,
    /// One sentence a model can read without parsing the payload.
    pub summary: String,
    /// The tool's own payload, unmodified.
    pub data: Value,
    /// The structured failure; present exactly when `status` is `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolErrorInfo>,
    /// What one call actually changed, when the runtime observed it.
    #[serde(default, skip_serializing_if = "std::vec::Vec::is_empty")]
    pub effects: Vec<String>,
    /// How to get the rest of an incomplete result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ToolContinuation>,
    /// The result was cut to fit the context.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl ToolResult {
    /// Wrap one `Result` as an envelope.
    pub fn from_outcome(tool: &str, outcome: &Result<Value, ToolError>) -> Self {
        match outcome {
            Ok(value) => Self::from_value(tool, value.clone()),
            Err(error) => Self::from_error(tool, error),
        }
    }

    /// A failed call.
    pub fn from_error(tool: &str, error: &ToolError) -> Self {
        let info = ToolErrorInfo::from_error(error);
        Self {
            status: ToolStatus::Error,
            summary: format!("{tool} failed: {}", info.message),
            data: Value::Null,
            error: Some(info),
            effects: Vec::new(),
            continuation: None,
            truncated: false,
        }
    }

    /// Wrap a tool's payload.
    ///
    /// A tool that reports `"success": false`, or one that speaks the result
    /// contract (`"tool"` plus `"status"`) and says `partial` or `error`, is
    /// telling the truth about the call; the envelope reads that verdict
    /// instead of assuming a returned value means success. A bare `status` is
    /// domain state and stays inside `data`.
    pub fn from_value(tool: &str, value: Value) -> Self {
        let mut status = ToolStatus::Success;
        let mut summary: Option<String> = None;
        let mut error: Option<ToolErrorInfo> = None;
        let mut continuation: Option<ToolContinuation> = None;
        let mut truncated = false;

        if let Some(object) = value.as_object() {
            // Only a payload that names its tool AND a status word is speaking
            // the result contract. A bare `status` is domain state - a job's
            // state, a task's - and mistaking it for the call's outcome would
            // report a successful `job_output` as a failure because the job it
            // described failed.
            if is_result_contract(object)
                && let Some(raw) = object.get("status").and_then(Value::as_str)
                && let Some(parsed) = parse_status(raw)
            {
                status = parsed;
            }
            if object.get("success").and_then(Value::as_bool) == Some(false) {
                status = ToolStatus::Error;
            }
            if object.get("truncated").and_then(Value::as_bool) == Some(true) {
                truncated = true;
            }
            if let Some(text) = object.get("summary").and_then(Value::as_str)
                && !text.trim().is_empty()
            {
                summary = Some(text.to_string());
            }
            continuation = continuation_of(tool, object);
            if status == ToolStatus::Error {
                error = error_of(object, object.get("status").and_then(Value::as_str));
            }
        }

        // A cut payload that did not say so in its own field is still cut: the
        // file tools report it through their status and the next pointer.
        if continuation.is_some() && status == ToolStatus::Success {
            status = ToolStatus::Partial;
        }

        let summary = summary.unwrap_or_else(|| default_summary(tool, status, error.as_ref()));

        Self {
            status,
            summary,
            data: value,
            error,
            effects: Vec::new(),
            continuation,
            truncated,
        }
    }

    /// Whether this result is a failure.
    pub fn is_error(&self) -> bool {
        self.status.is_error()
    }

    /// Serialize the envelope, cutting it to `cap` bytes when it does not fit.
    ///
    /// A cut result keeps both ends of the payload, marks `truncated`, and
    /// leaves `status` alone: how far the operation got and whether the report
    /// fits are different facts.
    ///
    /// The output is always valid JSON. A cap too small to hold even a minimal
    /// envelope (a few dozen bytes) cannot also bound the document, so the
    /// minimal envelope wins over the cap at that floor.
    pub fn to_text(&self, cap: usize) -> String {
        let cap = if cap == 0 { DEFAULT_RESULT_CAP } else { cap };
        if let Ok(text) = serde_json::to_string(self)
            && text.len() <= cap
        {
            return text;
        }

        let mut reduced = self.clone();
        reduced.truncated = true;
        let raw = serde_json::to_string(&self.data).unwrap_or_default();
        let budget = cap.saturating_sub(512).max(64);
        reduced.data = Value::String(cap_text(&raw, budget));
        if let Ok(text) = serde_json::to_string(&reduced)
            && text.len() <= cap
        {
            return text;
        }

        // The envelope's own fields are oversized too. Keep a valid, minimal
        // envelope instead of handing the model a cut-off document.
        let minimal = ToolResult {
            status: self.status,
            summary: cap_text(&self.summary, cap.saturating_sub(192).max(32)),
            data: Value::Null,
            error: self.error.as_ref().map(|error| ToolErrorInfo {
                message: cap_text(&error.message, 192),
                ..error.clone()
            }),
            effects: Vec::new(),
            continuation: None,
            truncated: true,
        };
        serde_json::to_string(&minimal).unwrap_or_else(|_| {
            format!(
                "{{\"status\":\"{}\",\"summary\":\"truncated\",\"data\":null,\"truncated\":true}}",
                self.status.as_str()
            )
        })
    }

    /// Whether serialized result text reports a failure.
    ///
    /// The envelope is authoritative; the legacy `Error: ` prefix and a
    /// payload-level `"success":false` are still recognized so a transcript
    /// written before the envelope keeps its verdict.
    pub fn text_is_error(text: &str) -> bool {
        if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) {
            if let Some(raw) = object.get("status").and_then(Value::as_str) {
                return parse_status(raw)
                    .map(ToolStatus::is_error)
                    .unwrap_or(matches!(raw, "error" | "failed" | "failure"));
            }
            if object.get("success").and_then(Value::as_bool) == Some(false) {
                return true;
            }
        }
        text.starts_with("Error:") || text.contains("\"success\":false")
    }
}

fn default_summary(tool: &str, status: ToolStatus, error: Option<&ToolErrorInfo>) -> String {
    match status {
        ToolStatus::Success => format!("{tool} completed"),
        ToolStatus::Partial => format!("{tool} returned a partial result"),
        ToolStatus::Pending => format!("{tool} is still in progress"),
        ToolStatus::Error => error.map_or_else(
            || format!("{tool} failed"),
            |error| format!("{tool} failed: {}", error.message),
        ),
    }
}

fn continuation_of(tool: &str, object: &Map<String, Value>) -> Option<ToolContinuation> {
    // The file-tool contract names the exact next arguments as `next`; a tool
    // that already built a structured continuation may send that instead.
    if let Some(next) = object.get("next")
        && next.is_object()
    {
        return Some(ToolContinuation {
            tool: tool.to_string(),
            args: next.clone(),
            reason: None,
        });
    }

    let explicit = object.get("continuation")?.as_object()?;
    Some(ToolContinuation {
        tool: explicit
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or(tool)
            .to_string(),
        args: explicit.get("args").cloned().unwrap_or(Value::Null),
        reason: explicit
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn error_of(object: &Map<String, Value>, status: Option<&str>) -> Option<ToolErrorInfo> {
    // A payload that only says "cancelled" names the class in its status word;
    // reporting that as an internal failure would hide the one distinction a
    // recovery decision depends on.
    let (fallback_code, fallback_message) = match status {
        Some("cancelled" | "canceled") => (ToolErrorCode::Cancelled, "the call was cancelled"),
        _ => (ToolErrorCode::Internal, "the tool reported failure"),
    };
    match object.get("error") {
        Some(Value::String(message)) => {
            Some(ToolErrorInfo::reported(fallback_code, message.clone()))
        }
        Some(Value::Object(error)) => {
            let message = error
                .get("message")
                .or_else(|| error.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    object
                        .get("hint")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| fallback_message.to_string());
            let mut info = ToolErrorInfo::reported(fallback_code, message);
            if let Some(code) = error.get("code").and_then(Value::as_str) {
                info.code = code.to_string();
            }
            if let Some(retryable) = error.get("retryable").and_then(Value::as_bool) {
                info.retryable = retryable;
            }
            if let Some(indeterminate) = error.get("indeterminate").and_then(Value::as_bool) {
                info.indeterminate = indeterminate;
            }
            Some(info)
        }
        _ => {
            let message = object
                .get("hint")
                .or_else(|| object.get("message"))
                .or_else(|| object.get("summary"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| fallback_message.to_string());
            Some(ToolErrorInfo::reported(fallback_code, message))
        }
    }
}

/// Cut `text` to `cap` bytes, keeping both ends and naming the omission.
///
/// The marker is inside the budget, so the result never exceeds `cap`. Byte
/// boundaries are respected, so a multi-byte character is never split.
pub(crate) fn cap_text(text: &str, cap: usize) -> String {
    let cap = if cap == 0 { DEFAULT_RESULT_CAP } else { cap };
    if text.len() <= cap {
        return text.to_string();
    }

    const SHORT_MARKER: &str = "\n[truncated]\n";
    if cap <= SHORT_MARKER.len() {
        return SHORT_MARKER[..cap].to_string();
    }

    // Keep both ends, but include the omission marker INSIDE the byte budget.
    // Very small caps use a short marker; larger caps also report the byte gap.
    let marker_budget = if cap < 128 { SHORT_MARKER.len() } else { 64 };
    let mut head_budget = (cap - marker_budget) / 2;
    let mut tail_budget = cap - marker_budget - head_budget;
    loop {
        let mut head_end = head_budget.min(text.len());
        while head_end > 0 && !text.is_char_boundary(head_end) {
            head_end -= 1;
        }
        let mut tail_start = text.len().saturating_sub(tail_budget);
        while tail_start < text.len() && !text.is_char_boundary(tail_start) {
            tail_start += 1;
        }

        let omitted = tail_start.saturating_sub(head_end);
        let marker = if cap < 128 {
            SHORT_MARKER.to_string()
        } else {
            format!("\n\n[... {omitted} bytes omitted; ask for a narrower read or grep ...]\n\n")
        };
        let tail_len = text.len().saturating_sub(tail_start);
        if head_end + marker.len() + tail_len <= cap {
            return format!("{}{marker}{}", &text[..head_end], &text[tail_start..]);
        }

        if head_budget >= tail_budget && head_budget > 0 {
            head_budget -= 1;
        } else if tail_budget > 0 {
            tail_budget -= 1;
        } else {
            return SHORT_MARKER[..cap.min(SHORT_MARKER.len())].to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap_or_else(|error| panic!("not JSON: {error}: {text}"))
    }

    #[test]
    fn a_successful_result_keeps_its_payload_and_gains_a_verdict() {
        let result = ToolResult::from_value("read_file", json!({"content": "hi", "lines": 2}));
        assert_eq!(result.status, ToolStatus::Success);
        assert_eq!(result.summary, "read_file completed");
        assert!(result.error.is_none());
        assert!(result.continuation.is_none());
        let text = result.to_text(0);
        let value = parse(&text);
        assert_eq!(value["status"], "success");
        assert_eq!(value["summary"], "read_file completed");
        assert_eq!(value["data"]["content"], "hi", "the payload is not moved");
        assert_eq!(value["data"]["lines"], 2);
        assert!(value.get("error").is_none(), "no error on success");
    }

    #[test]
    fn a_payload_status_is_translated_into_the_envelope() {
        for (raw, expected) in [
            ("ok", ToolStatus::Success),
            ("empty", ToolStatus::Success),
            ("partial", ToolStatus::Partial),
            ("running", ToolStatus::Pending),
            ("queued", ToolStatus::Pending),
            ("error", ToolStatus::Error),
            ("failed", ToolStatus::Error),
        ] {
            let result =
                ToolResult::from_value("t", json!({"tool": "t", "status": raw, "value": 1}));
            assert_eq!(result.status, expected, "{raw}");
            assert_eq!(
                result.data["status"], raw,
                "the tool's own word is not rewritten"
            );
        }
    }

    /// A job's state is not the call's outcome: `job_output` succeeded even
    /// when it reports that the job failed.
    #[test]
    fn a_domain_status_is_not_mistaken_for_the_call_outcome() {
        let job = ToolResult::from_value(
            "job_output",
            json!({"id": "job-1", "status": "failed", "exit_code": 2, "output": ""}),
        );
        assert_eq!(job.status, ToolStatus::Success, "the call returned the job");
        assert!(job.error.is_none());
        assert_eq!(job.data["status"], "failed", "the job's state is preserved");

        let running = ToolResult::from_value(
            "job_output",
            json!({"id": "job-1", "status": "running", "output": "partial"}),
        );
        assert_eq!(
            running.status,
            ToolStatus::Success,
            "a running job is still a successful query"
        );
    }

    /// A read names its continuation in `next_offset` and nowhere else: the
    /// envelope must not repeat the same offset as a `continuation` object.
    #[test]
    fn a_next_offset_is_the_reads_only_continuation() {
        let partial = ToolResult::from_value(
            "read_file",
            json!({
                "tool": "read_file",
                "status": "partial",
                "content": "two lines",
                "start_line": 1,
                "end_line": 2,
                "total_lines": 10,
                "next_offset": 3
            }),
        );
        assert_eq!(partial.status, ToolStatus::Partial);
        assert!(
            partial.continuation.is_none(),
            "next_offset is the one representation; no duplicate continuation"
        );
        assert_eq!(partial.data["next_offset"], 3);

        let complete = ToolResult::from_value(
            "read_file",
            json!({
                "tool": "read_file",
                "status": "ok",
                "next_offset": null
            }),
        );
        assert_eq!(complete.status, ToolStatus::Success);
        assert!(complete.continuation.is_none());
    }

    #[test]
    fn a_partial_payload_carries_its_continuation() {
        let result = ToolResult::from_value(
            "read_file",
            json!({"status": "partial", "content": "first", "next": {"offset": 401}}),
        );
        assert_eq!(result.status, ToolStatus::Partial);
        let continuation = result
            .continuation
            .expect("the next pointer is a continuation");
        assert_eq!(continuation.tool, "read_file");
        assert_eq!(continuation.args["offset"], 401);
        assert!(result.error.is_none(), "partial is not failure");
    }

    #[test]
    fn a_payload_that_reports_failure_is_not_a_success() {
        let result = ToolResult::from_value(
            "write_file",
            json!({"success": false, "error": {"message": "the disk is full", "retryable": true}}),
        );
        assert_eq!(result.status, ToolStatus::Error);
        let error = result.error.clone().expect("an error is present");
        assert_eq!(error.message, "the disk is full");
        assert!(error.retryable);
        assert!(result.summary.contains("the disk is full"));
        assert!(ToolResult::text_is_error(&result.to_text(0)));
    }

    #[test]
    fn an_error_result_carries_the_class_and_the_recovery_advice() {
        let timeout = ToolError::Timeout("bash exceeded 600 s".into());
        let result = ToolResult::from_error("bash", &timeout);
        assert_eq!(result.status, ToolStatus::Error);
        assert!(result.data.is_null());
        let error = result.error.clone().expect("an error is present");
        assert_eq!(error.code, "timeout");
        assert!(!error.retryable);
        assert!(error.indeterminate);
        assert!(error.field.is_none());

        let cancelled = ToolResult::from_error("bash", &ToolError::cancelled("turn stopped"));
        let cancelled_code = cancelled.error.clone().expect("error").code;
        assert_eq!(cancelled_code, "cancelled");
        assert_ne!(
            cancelled_code,
            result.error.expect("error").code,
            "cancellation and timeout stay distinct"
        );

        let denied = ToolResult::from_error("bash", &ToolError::denied("policy"));
        let denied = denied.error.expect("error");
        assert_eq!(denied.code, "permission_denied");
        assert!(!denied.retryable, "a refusal is not retried");
    }

    #[test]
    fn a_field_failure_names_the_field_structurally() {
        let result = ToolResult::from_error("update_plan", &ToolError::missing("items"));
        let error = result.error.expect("error");
        assert_eq!(error.code, "invalid_input");
        assert_eq!(error.field.as_deref(), Some("items"));
        assert!(error.missing);
        assert!(error.message.contains("items"));
    }

    #[test]
    fn text_is_error_reads_the_envelope_before_the_legacy_prefix() {
        assert!(!ToolResult::text_is_error("plain output"));
        assert!(ToolResult::text_is_error("Error: nope"));
        assert!(ToolResult::text_is_error("{\"success\":false}"));

        let success = ToolResult::from_value("t", json!({"content": "x"})).to_text(0);
        assert!(!ToolResult::text_is_error(&success));
        // The payload may itself mention a failure; only the envelope's own
        // status decides the verdict.
        let nested =
            ToolResult::from_value("t", json!({"report": {"status": "error"}, "lines": 1}))
                .to_text(0);
        assert!(!ToolResult::text_is_error(&nested), "{nested}");

        let failure = ToolResult::from_error("t", &ToolError::Failed("boom".into())).to_text(0);
        assert!(ToolResult::text_is_error(&failure));
    }

    #[test]
    fn a_result_that_fits_is_not_marked_truncated() {
        let text = ToolResult::from_value("t", json!({"content": "short"})).to_text(1_024);
        let value = parse(&text);
        assert!(value.get("truncated").is_none());
        assert_eq!(value["status"], "success");
    }

    #[test]
    fn an_oversized_result_is_cut_explicitly_and_stays_valid_json() {
        let big = "x".repeat(10_000);
        let result = ToolResult::from_value("read_file", json!({"content": big}));
        let text = result.to_text(1_024);
        assert!(text.len() <= 1_024, "the cap holds: {} bytes", text.len());
        let value = parse(&text);
        assert_eq!(value["status"], "success", "the operation still succeeded");
        assert_eq!(value["truncated"], true, "the cut is stated");
        let data = value["data"].as_str().unwrap_or_default();
        assert!(
            data.starts_with("{\"content\":\"x"),
            "the head survives: {data}"
        );
        assert!(data.ends_with("x\"}"), "the tail survives: {data}");
        assert!(data.contains("omitted"), "the cut names itself: {data}");
    }

    #[test]
    fn an_oversized_summary_still_yields_a_parseable_envelope() {
        let result = ToolResult {
            status: ToolStatus::Error,
            summary: "s".repeat(4_000),
            data: json!({"payload": "p".repeat(4_000)}),
            error: Some(ToolErrorInfo::reported(
                ToolErrorCode::Internal,
                "m".repeat(4_000),
            )),
            effects: Vec::new(),
            continuation: None,
            truncated: false,
        };
        let text = result.to_text(256);
        assert!(text.len() <= 512, "bounded: {} bytes", text.len());
        let value = parse(&text);
        assert_eq!(value["status"], "error");
        assert_eq!(value["truncated"], true);
    }

    #[test]
    fn a_cut_cap_is_still_bounded_at_the_floor() {
        let text = ToolResult::from_value("t", json!({"content": "x".repeat(5_000)})).to_text(16);
        assert!(text.len() <= 512, "{}", text.len());
        parse(&text);
    }
}
