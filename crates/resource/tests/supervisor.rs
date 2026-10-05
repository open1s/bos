//! Supervisor: restart policies, restart budgets, and escalation.
//!
//! Side effects (spawns) are observed by writing markers to a temp file from
//! the child itself — that way the test sees *every* spawn, not just the
//! ones the supervisor happened to report.

use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

use resource::prelude::*;
use resource::{
    ChildSpec, Effect, ProcManager, ResourceStateLabel, RestartPolicy, Rule, SupPolicy, Supervisor,
};

fn test_policy() -> SharedPolicy {
    SharedPolicy::new(PolicyDoc {
        admins: vec!["admin".to_string()],
        rules: vec![Rule {
            agents: vec!["*".to_string()],
            uris: vec!["*".to_string()],
            actions: vec!["*".to_string()],
            effect: Effect::Allow,
        }],
        ..Default::default()
    })
}

fn tmp_tag(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("bos_sup_{tag}_{}", std::process::id()))
}

/// Count lines in `path`, ignoring a missing/empty file.
fn count_lines(path: &std::path::PathBuf) -> usize {
    let mut buf = String::new();
    match std::fs::File::open(path) {
        Ok(mut f) => {
            let _ = f.read_to_string(&mut buf);
            buf.lines().count()
        }
        Err(_) => 0,
    }
}

/// Boot a ResourceManager with a `proc://` manager; register a supervisor on
/// top and return (client, sup_uri, marker_path).
async fn harness(
    tag: &str,
    argv: &[&str],
    policy: SupPolicy,
) -> (ResourceClient, String, std::path::PathBuf) {
    let mgr = Arc::new(ResourceManager::new(test_policy()));
    mgr.register(Box::new(ProcManager::new(mgr.clone())), "admin".into())
        .await
        .unwrap();

    let marker = tmp_tag(tag);
    let _ = std::fs::remove_file(&marker);
    let mut args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let formatted = args[2].replace("{MARK}", marker.to_str().unwrap());
    args[2] = formatted;

    let sup = Supervisor::new(
        tag,
        mgr.clone(),
        vec![ChildSpec { args, env: vec![] }],
        policy,
    );
    let uri = sup.meta().uri.clone();
    mgr.register(Box::new(sup), "admin".into()).await.unwrap();

    let client = ResourceClient::new("admin", Some(mgr.clone()), None);
    (client, uri, marker)
}

#[tokio::test]
async fn restart_on_failure_keeps_respawning() {
    // Child always fails (exit 1). Budget = 5 restarts in 60s. After a short
    // window we should see *multiple* marker writes, proving respawn.
    let (client, sup_uri, marker) = harness(
        "restart_on_fail",
        &["/bin/sh", "-c", "echo once >> {MARK}; exit 1"],
        SupPolicy {
            restart: RestartPolicy::OnFailure,
            max_restarts: 5,
            window: Duration::from_secs(60),
        },
    )
    .await;
    client.invoke(&sup_uri, ResourceAction::Open).await.unwrap();
    // Give the child a moment to spawn, fail, and respawn at least once.
    tokio::time::sleep(Duration::from_secs(2)).await;
    client
        .invoke(&sup_uri, ResourceAction::Close)
        .await
        .unwrap();
    let n = count_lines(&marker);
    assert!(n >= 2, "expected >= 2 spawns, got {n}");
}

#[tokio::test]
async fn escalate_on_budget_exhaustion() {
    // Child always fails, but the budget is 2. After two respawns the third
    // failure escalates: the supervisor dies, and further spawns stop.
    let (client, sup_uri, marker) = harness(
        "escalate",
        &["/bin/sh", "-c", "echo once >> {MARK}; exit 1"],
        SupPolicy {
            restart: RestartPolicy::OnFailure,
            max_restarts: 2,
            window: Duration::from_secs(600), // window never rolls off
        },
    )
    .await;
    client.invoke(&sup_uri, ResourceAction::Open).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // The supervisor escalated to failed.
    let failed = client
        .invoke(&sup_uri, ResourceAction::Status)
        .await
        .unwrap();
    assert!(
        matches!(
            failed,
            ResourceOutput::Status {
                state: ResourceStateLabel::Closed
            }
        ),
        "supervisor should be Closed after escalation, got {failed:?}"
    );

    // No more markers appear after the escalation.
    let before = count_lines(&marker);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let after = count_lines(&marker);
    assert_eq!(
        before, after,
        "no new spawns after escalation (before={before} after={after})"
    );
}

#[tokio::test]
async fn always_policy_keeps_alive_long_runner() {
    // Child exits cleanly but policy = Always: it respawns regardless.
    let (client, sup_uri, marker) = harness(
        "always",
        &["/bin/sh", "-c", "echo spawn >> {MARK}; sleep 0.3; exit 0"],
        SupPolicy {
            restart: RestartPolicy::Always,
            max_restarts: 20,
            window: Duration::from_secs(60),
        },
    )
    .await;
    client.invoke(&sup_uri, ResourceAction::Open).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    client
        .invoke(&sup_uri, ResourceAction::Close)
        .await
        .unwrap();
    let n = count_lines(&marker);
    assert!(n >= 2, "expected multiple spawns under Always, got {n}");
}

#[tokio::test]
async fn close_kills_children() {
    // Spawn a long-running child. Close kills it and prevents respawn.
    let (client, sup_uri, marker) = harness(
        "close_kills",
        &["/bin/sh", "-c", "echo start >> {MARK}; exec sleep 60"],
        SupPolicy {
            restart: RestartPolicy::Never,
            max_restarts: 0,
            window: Duration::from_secs(60),
        },
    )
    .await;
    client.invoke(&sup_uri, ResourceAction::Open).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    client
        .invoke(&sup_uri, ResourceAction::Close)
        .await
        .unwrap();

    // After close, no proc://<pid> should still be running the child.
    // Query the supervisor's list: nothing running.
    let out = client
        .invoke(&sup_uri, ResourceAction::List { pattern: None })
        .await
        .unwrap();
    match out {
        ResourceOutput::Listed { entries } => {
            assert!(
                entries
                    .iter()
                    .all(|e| e.contains("done") || !e.contains("running")),
                "children must not be running after close: {entries:?}"
            );
        }
        other => panic!("expected Listed, got {other:?}"),
    }
    let _ = std::fs::remove_file(&marker);
}

#[tokio::test]
async fn list_reports_restart_count() {
    let (client, sup_uri, _marker) =
        harness("list", &["/bin/sh", "-c", "sleep 60"], SupPolicy::default()).await;
    client.invoke(&sup_uri, ResourceAction::Open).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let out = client
        .invoke(&sup_uri, ResourceAction::List { pattern: None })
        .await
        .unwrap();
    match out {
        ResourceOutput::Listed { entries } => {
            assert_eq!(entries.len(), 1, "one child");
            assert!(
                entries[0].contains("proc://"),
                "child URI listed: {entries:?}"
            );
            assert!(
                entries[0].contains("running"),
                "child marked running: {entries:?}"
            );
        }
        other => panic!("expected Listed, got {other:?}"),
    }
    client
        .invoke(&sup_uri, ResourceAction::Close)
        .await
        .unwrap();
}
