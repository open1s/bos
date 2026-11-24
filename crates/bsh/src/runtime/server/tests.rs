//! End-to-end local IPC tests: a real Host, a real endpoint, a real client.
//!
//! The client here is the same blocking [`RuntimeClient`] the terminal
//! interface uses, so these tests cover the discovery record, the handshake,
//! peer-principal derivation, commands, queries, live events and shutdown in
//! one run rather than exercising the transport in pieces.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::runtime::client::{ClientError, RuntimeClient};
use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::{
    CommandOutcome, Cursor, EventPayload, ExecutionStatus, HostPhase, PrincipalKind, Response,
    ServerFrame, ShutdownMode,
};
use crate::runtime::host::RuntimeHost;
use crate::runtime::ids::ExecutionId;

#[cfg(unix)]
#[test]
fn a_terminal_client_creates_a_session_and_reads_a_turn_over_local_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = workspace;
    config.echo = true;
    config.shutdown_grace = Duration::ZERO;
    let data_dir = config.data_dir.clone();

    let server_config = config.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        runtime.block_on(async move {
            let host = RuntimeHost::open(server_config)
                .await
                .expect("the Host opens");
            let serving =
                tokio::spawn(crate::runtime::server::IpcServer::new(host.clone()).serve());
            ready_tx.send(()).expect("test thread is listening");
            let stopped = tokio::time::timeout(Duration::from_secs(20), async {
                while host.phase() == HostPhase::Ready {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            assert!(stopped.is_ok(), "the client never stopped the Host");
            host.drain().await;
            serving
                .await
                .expect("transport task")
                .expect("transport served cleanly");
        });
    });

    ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the Host thread started");

    // The socket is bound before the discovery record is written, so retry
    // rather than assuming the first connect after `ready` succeeds.
    let deadline = Instant::now() + Duration::from_secs(10);
    let (client, frames) = loop {
        match RuntimeClient::connect(&data_dir, crate::runtime::client::tui_info()) {
            Ok(pair) => break pair,
            Err(error) => {
                assert!(Instant::now() < deadline, "cannot connect: {error}");
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    assert_eq!(client.principal().kind, PrincipalKind::LocalUser);
    assert!(matches!(client.ping().expect("liveness"), Response::Pong));

    // Subscribe from the current head: every boundary of the next turn must
    // reach the client as a committed event.
    let head = client.status().expect("status").journal_head;
    match client
        .subscribe(Some(Cursor { sequence: head }))
        .expect("subscribe")
    {
        Response::Subscribed(subscribed) => {
            // next_cursor is the last sequence the replay covers, so live
            // events start at head + 1 rather than the cursor being head + 1.
            assert_eq!(subscribed.next_cursor.sequence, head);
        }
        other => panic!("subscribe answered with {other:?}"),
    }

    let session = client.create_session("local ipc").expect("create session");
    let sessions = client.sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session);

    // The bin travels over the same socket, in the facts the terminal's
    // `/trash` and `/restore` ask for: parking hides the chat from the list, the
    // bin is the one reader that shows it, and a restore brings it back.
    match client.trash_session(session).expect("park the session") {
        CommandOutcome::Accepted { .. } => {}
        other => panic!("parking rejected: {other:?}"),
    }
    assert!(
        client.sessions().expect("list sessions").is_empty(),
        "a parked chat is out of the list"
    );
    let bin = client.trash().expect("the bin");
    assert_eq!(bin.len(), 1, "the bin lists the parked chat");
    assert_eq!(bin[0].session_id, session);
    match client
        .restore_session(session)
        .expect("restore the session")
    {
        CommandOutcome::Accepted { .. } => {}
        other => panic!("restore rejected: {other:?}"),
    }
    assert_eq!(
        client.sessions().expect("list sessions").len(),
        1,
        "restore puts the chat back"
    );
    assert!(client.trash().expect("the bin").is_empty());

    // The Review query travels over the same socket, listing and naming alike:
    // the terminal's `/review` asks this. An echo Host has no review surface,
    // and it says so rather than answering "nothing changed" - which is the
    // answer a client must not read as "your files are untouched".
    let listed = client.review(session, None).expect("review query");
    assert_eq!(listed.state, "unsupported");
    assert!(listed.changes.is_empty());
    let named = client
        .review(session, Some("src/lib.rs"))
        .expect("named review query");
    assert!(named.diff.is_none());

    let outcome = client
        .send_message(session, "hello over the socket")
        .expect("send turn");
    let execution: ExecutionId = match outcome {
        CommandOutcome::Accepted { operation_id, .. } => {
            operation_id.as_str().parse().expect("execution id")
        }
        other => panic!("turn rejected: {other:?}"),
    };

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let projection = client.session(session).expect("session query");
        let projection = projection.expect("session exists");
        if projection
            .executions
            .iter()
            .any(|view| view.execution_id == execution && view.status == ExecutionStatus::Completed)
        {
            assert_eq!(projection.messages.len(), 2);
            assert!(
                projection.messages[1]
                    .text
                    .contains("hello over the socket")
            );
            break;
        }
        assert!(Instant::now() < deadline, "the turn never completed");
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut seen: Vec<&'static str> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !seen.contains(&"turn_succeeded") {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match frames.recv_timeout(remaining) {
            Ok(ServerFrame::Event { event, .. }) => match &event.payload {
                EventPayload::TurnRequested { .. } => seen.push("turn_requested"),
                EventPayload::TurnSucceeded { .. } => seen.push("turn_succeeded"),
                _ => {}
            },
            Ok(_) => {}
            Err(error) => panic!("no committed turn event arrived: {error}"),
        }
    }
    assert!(seen.contains(&"turn_requested"));

    // A second client is a second view of the same Host, not a second Host.
    let (other, _other_frames) =
        RuntimeClient::connect(&data_dir, crate::runtime::client::tui_info())
            .expect("second client");
    assert_eq!(other.workspace_id(), client.workspace_id());
    assert_eq!(other.principal().id, client.principal().id);
    assert_eq!(other.sessions().expect("list").len(), 1);

    match client.shutdown(ShutdownMode::Graceful).expect("shutdown") {
        CommandOutcome::Accepted { .. } => {}
        other => panic!("shutdown rejected: {other:?}"),
    }
    server.join().expect("server thread");
    assert!(
        !data_dir.join("runtime.json").exists(),
        "the discovery record outlived the Host"
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client.ping() {
            Err(ClientError::Closed) => break,
            Err(other) => panic!("unexpected disconnect error: {other}"),
            Ok(response) => {
                assert!(
                    Instant::now() < deadline,
                    "client still connected: {response:?}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn a_client_without_a_host_reports_that_the_host_is_not_running() {
    let dir = tempfile::tempdir().unwrap();
    match RuntimeClient::connect(dir.path(), crate::runtime::client::tui_info()) {
        Err(ClientError::Discovery(error)) => {
            assert!(error.to_string().contains("start `bsh host` first"));
        }
        Ok(_) => panic!("a client connected without a Host"),
        Err(other) => panic!("expected discovery failure, got {other}"),
    }
}
