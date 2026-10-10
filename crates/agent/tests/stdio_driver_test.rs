//! Integration test for the headless stdio JSON-RPC driver.
//!
//! Spawns the `bos-stdio` binary, drives a `session.create` → `message.send` →
//! `shutdown` round trip, and asserts both the responses and a clean exit.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

/// Spawn the driver with piped stdio.
fn spawn_driver() -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let exe = env!("CARGO_BIN_EXE_bos-stdio");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bos-stdio");

    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    (child, stdin, BufReader::new(stdout))
}

/// Write one newline-delimited request.
fn send(stdin: &mut ChildStdin, request: &Value) {
    writeln!(stdin, "{request}").expect("write request");
    stdin.flush().expect("flush request");
}

/// Read one response line.
fn recv(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    let read = reader.read_line(&mut line).expect("read response");
    assert!(read > 0, "unexpected EOF from driver");
    serde_json::from_str(line.trim()).expect("response is JSON")
}

#[test]
fn session_create_send_shutdown_round_trip() {
    let (mut child, mut stdin, mut reader) = spawn_driver();

    send(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "session.create",
            "params": { "name": "alpha" }
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "message.send",
            "params": { "session_id": "alpha", "message": "hello" }
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }),
    );

    let created = recv(&mut reader);
    assert_eq!(created["jsonrpc"], "2.0");
    assert_eq!(created["id"], 1);
    assert!(created.get("error").is_none(), "{created}");
    assert_eq!(created["result"]["session_id"], "alpha");
    assert_eq!(created["result"]["created"], true);

    let echoed = recv(&mut reader);
    assert_eq!(echoed["id"], 2);
    assert!(echoed.get("error").is_none(), "{echoed}");
    assert_eq!(echoed["result"]["session_id"], "alpha");
    assert_eq!(echoed["result"]["reply"], "echo: hello");
    assert_eq!(echoed["result"]["echoed"], true);
    assert_eq!(echoed["result"]["message_count"], 1);

    let shutdown = recv(&mut reader);
    assert_eq!(shutdown["id"], 3);
    assert_eq!(shutdown["result"]["ok"], true);

    drop(stdin);

    let status = child.wait().expect("wait for driver");
    assert!(status.success(), "driver exited with {status}");
}

#[test]
fn bad_input_is_reported_and_the_driver_keeps_running() {
    let (mut child, mut stdin, mut reader) = spawn_driver();

    // A malformed line, then an unknown method, then a valid list request.
    writeln!(stdin, "this is not json").unwrap();
    stdin.flush().unwrap();
    send(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "no.such.method" }),
    );
    send(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 8, "method": "session.list" }),
    );
    send(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 9, "method": "shutdown" }),
    );

    let parse_error = recv(&mut reader);
    assert_eq!(parse_error["error"]["code"], -32700);
    assert_eq!(parse_error["id"], Value::Null);

    let not_found = recv(&mut reader);
    assert_eq!(not_found["id"], 7);
    assert_eq!(not_found["error"]["code"], -32601);

    let listed = recv(&mut reader);
    assert_eq!(listed["id"], 8);
    assert!(listed["result"]["sessions"].is_array());

    let shutdown = recv(&mut reader);
    assert_eq!(shutdown["result"]["ok"], true);

    drop(stdin);
    let status = child.wait().expect("wait for driver");
    assert!(status.success(), "driver exited with {status}");
}

#[test]
fn message_send_rejects_an_unknown_session() {
    let (mut child, mut stdin, mut reader) = spawn_driver();

    send(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message.send",
            "params": { "session_id": "missing", "message": "hi" }
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
    );

    let error = recv(&mut reader);
    assert_eq!(error["error"]["code"], -32602);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown session"));

    let shutdown = recv(&mut reader);
    assert_eq!(shutdown["result"]["ok"], true);

    drop(stdin);
    let status = child.wait().expect("wait for driver");
    assert!(status.success(), "driver exited with {status}");
}
