//! Headless stdio JSON-RPC 2.0 driver for BOS.
//!
//! Reads newline-delimited JSON-RPC 2.0 requests on stdin and writes exactly one
//! JSON response per line on stdout. Supported methods:
//!
//! - `session.create` — create (or return) a named session.
//! - `session.list` — list the sessions created so far.
//! - `message.send` — record a message and return a deterministic offline echo.
//!   No LLM or network is contacted.
//! - `shutdown` — acknowledge, then exit cleanly.
//!
//! An unknown method produces a JSON-RPC error object; a malformed line produces
//! a parse-error response. Neither ever panics, and the driver keeps reading
//! after either.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

/// JSON-RPC code for an unparseable line.
const PARSE_ERROR: i32 = -32700;
/// JSON-RPC code for a structurally invalid request.
const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC code for an unknown method.
const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC code for invalid method parameters.
const INVALID_PARAMS: i32 = -32602;

/// One in-memory session.
struct Session {
    id: String,
    created_at_ms: u64,
    messages: Vec<String>,
}

/// The request handler and its session state.
#[derive(Default)]
struct Driver {
    sessions: BTreeMap<String, Session>,
    next_id: u64,
}

impl Driver {
    /// Handle one input line, returning the response and whether to shut down.
    fn handle_line(&mut self, line: &str) -> (Value, bool) {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                return (
                    error_response(Value::Null, PARSE_ERROR, format!("parse error: {err}")),
                    false,
                );
            }
        };

        let id = parsed.get("id").cloned().unwrap_or(Value::Null);

        let Some(object) = parsed.as_object() else {
            return (
                error_response(id, INVALID_REQUEST, "request must be a JSON object"),
                false,
            );
        };

        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return (
                error_response(id, INVALID_REQUEST, "missing string 'method'"),
                false,
            );
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "session.create" => (result_response(id, self.session_create(&params)), false),
            "session.list" => (result_response(id, self.session_list()), false),
            "message.send" => match self.message_send(&params) {
                Ok(result) => (result_response(id, result), false),
                Err(message) => (error_response(id, INVALID_PARAMS, message), false),
            },
            "shutdown" => (result_response(id, json!({"ok": true})), true),
            other => (
                error_response(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
                false,
            ),
        }
    }

    fn session_create(&mut self, params: &Value) -> Value {
        let requested = params
            .get("session_id")
            .or_else(|| params.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.trim().is_empty());

        let id = match requested {
            Some(id) => id,
            None => {
                self.next_id += 1;
                format!("session-{}", self.next_id)
            }
        };

        if let Some(existing) = self.sessions.get(&id) {
            return json!({
                "session_id": existing.id,
                "created": false,
                "created_at_ms": existing.created_at_ms,
                "message_count": existing.messages.len(),
            });
        }

        let created_at_ms = now_ms();
        self.sessions.insert(
            id.clone(),
            Session {
                id: id.clone(),
                created_at_ms,
                messages: Vec::new(),
            },
        );

        json!({
            "session_id": id,
            "created": true,
            "created_at_ms": created_at_ms,
            "message_count": 0,
        })
    }

    fn session_list(&self) -> Value {
        let sessions: Vec<Value> = self
            .sessions
            .values()
            .map(|session| {
                json!({
                    "session_id": session.id,
                    "created_at_ms": session.created_at_ms,
                    "message_count": session.messages.len(),
                })
            })
            .collect();
        let count = sessions.len();
        json!({ "sessions": sessions, "count": count })
    }

    fn message_send(&mut self, params: &Value) -> Result<Value, String> {
        let session_id = params
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing string param 'session_id'".to_string())?;
        let message = params
            .get("message")
            .or_else(|| params.get("content"))
            .or_else(|| params.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| "missing string param 'message'".to_string())?;

        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("unknown session '{session_id}'"))?;
        session.messages.push(message.to_string());
        let message_count = session.messages.len();

        // No LLM is configured in the headless driver, so the reply is a
        // deterministic offline echo and no network call is made.
        Ok(json!({
            "session_id": session_id,
            "reply": format!("echo: {message}"),
            "echoed": true,
            "deterministic": true,
            "llm": "none",
            "message_count": message_count,
        }))
    }
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut driver = Driver::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let (response, shutdown) = driver.handle_line(&line);
        if writeln!(out, "{response}").is_err() {
            break;
        }
        let _ = out.flush();
        if shutdown {
            break;
        }
    }
}
