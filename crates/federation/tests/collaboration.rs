//! Collaboration graphs: deterministic scheduling, failure containment, recovery.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use federation::{
    ArtifactRef, CollaborationError, Orchestrator, TaskExecutor, TaskGraph, TaskNode, TaskNodeId,
    TaskOutcome, TaskState,
};
use serde_json::json;

fn id(value: &str) -> TaskNodeId {
    TaskNodeId::new(value)
}

fn graph(edges: &[(&str, &str)]) -> TaskGraph {
    let mut graph = TaskGraph::new();
    for (task, dependency) in edges {
        if !graph.tasks.contains_key(&id(task)) {
            graph.add(id(task), *task, Vec::new()).expect("task");
        }
        if !graph.tasks.contains_key(&id(dependency)) {
            graph
                .add(id(dependency), *dependency, Vec::new())
                .expect("task");
        }
    }
    for (task, dependency) in edges {
        graph
            .tasks
            .get_mut(&id(task))
            .expect("task")
            .depends_on
            .push(id(dependency));
    }
    graph
}

struct Recorder {
    events: std::sync::Mutex<Vec<String>>,
    active: std::sync::atomic::AtomicUsize,
    max_active: std::sync::atomic::AtomicUsize,
    fail: BTreeSet<String>,
    compensated: std::sync::atomic::AtomicUsize,
}

impl Recorder {
    fn new(fail: &[&str]) -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            active: std::sync::atomic::AtomicUsize::new(0),
            max_active: std::sync::atomic::AtomicUsize::new(0),
            fail: fail.iter().map(|value| value.to_string()).collect(),
            compensated: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TaskExecutor for Recorder {
    async fn execute(&self, task: &TaskNode) -> Result<TaskOutcome, String> {
        use std::sync::atomic::Ordering;
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.events
            .lock()
            .expect("events")
            .push(task.id.to_string());
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.fail.contains(task.id.as_str()) {
            return Err(format!("{} failed", task.id));
        }
        Ok(TaskOutcome::value(json!({ "task": task.id.as_str() })))
    }

    async fn compensate(&self, _task: &TaskNode) -> Result<(), String> {
        self.compensated
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn compensation_evidence(&self, _task: &TaskNode) -> Option<serde_json::Value> {
        Some(json!({ "kind": "local", "ok": true }))
    }
}

#[tokio::test]
async fn a_dependency_graph_runs_dependencies_first_and_bounds_concurrency() {
    let graph = graph(&[("b", "a"), ("c", "a"), ("d", "b"), ("d", "c")]);
    let executor = Arc::new(Recorder::new(&[]));
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator
        .run(executor.clone(), 2)
        .await
        .expect("the graph runs");

    assert_eq!(progress.succeeded, 4);
    assert_eq!(progress.failed, 0);
    let events = executor.events.lock().expect("events").clone();
    let position = |task: &str| {
        events
            .iter()
            .position(|event| event == task)
            .unwrap_or_else(|| panic!("{task} did not run: {events:?}"))
    };
    assert!(position("a") < position("b"));
    assert!(position("a") < position("c"));
    assert!(position("b") < position("d"));
    assert!(position("c") < position("d"));
    assert!(
        executor
            .max_active
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 2,
        "concurrency is bounded"
    );
}

#[tokio::test]
async fn a_failure_cancels_only_dependents() {
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    graph.add(id("b"), "b", vec![id("a")]).expect("b");
    graph.add(id("c"), "c", Vec::new()).expect("c");
    let executor = Arc::new(Recorder::new(&["a"]));
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator.run(executor, 2).await.expect("runs");

    assert_eq!(progress.failed, 1);
    assert_eq!(progress.cancelled, 1);
    assert_eq!(progress.succeeded, 1);
    let graph = orchestrator.graph();
    assert_eq!(graph.tasks[&id("a")].state, TaskState::Failed);
    assert_eq!(graph.tasks[&id("b")].state, TaskState::Cancelled);
    assert_eq!(graph.tasks[&id("c")].state, TaskState::Succeeded);
}

#[tokio::test]
async fn a_graph_persists_and_recovers_a_running_task() {
    let path = std::env::temp_dir().join(format!("bos-collaboration-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    graph.add(id("b"), "b", vec![id("a")]).expect("b");

    let executor = Arc::new(Recorder::new(&[]));
    let mut orchestrator = Orchestrator::new(graph).with_persistence(&path);
    // Run one scheduling step by hand to leave a Running record on disk.
    orchestrator.graph_mut().start(&id("a")).expect("started");
    orchestrator.graph_mut().save(&path).expect("saved");
    drop(orchestrator);

    let recovered = TaskGraph::load(&path).expect("loaded");
    assert_eq!(recovered.tasks[&id("a")].state, TaskState::Failed);
    assert_eq!(recovered.tasks[&id("b")].state, TaskState::Cancelled);
    assert!(recovered.tasks[&id("a")]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("indeterminate")));
    let _ = std::fs::remove_file(path);
    let _ = executor;
}

#[test]
fn cycles_and_unknown_dependencies_are_refused() {
    let mut graph = graph(&[("a", "b"), ("b", "a")]);
    assert!(matches!(
        graph.validate(),
        Err(CollaborationError::Cycle { .. })
    ));
    graph
        .add(id("c"), "c", vec![id("ghost")])
        .expect("forward reference");
    assert!(matches!(
        graph.validate(),
        Err(CollaborationError::UnknownDependency { .. })
    ));
}

#[test]
fn the_ready_order_is_deterministic() {
    let graph = graph(&[("b", "a"), ("c", "a")]);
    assert_eq!(graph.ready(), vec![id("a")]);
    assert_eq!(
        graph.order().expect("order"),
        vec![id("a"), id("b"), id("c")]
    );
}

struct Flaky {
    attempts: std::sync::Mutex<std::collections::BTreeMap<String, u32>>,
}

#[async_trait]
impl TaskExecutor for Flaky {
    async fn execute(&self, task: &TaskNode) -> Result<TaskOutcome, String> {
        let mut attempts = self.attempts.lock().expect("attempts");
        let count = attempts.entry(task.id.to_string()).or_insert(0);
        *count += 1;
        if *count == 1 {
            return Err("transient".to_string());
        }
        Ok(TaskOutcome {
            result: json!({ "attempt": *count }),
            artifacts: vec![ArtifactRef::new("report", "file:///tmp/report.json")],
        })
    }
}

#[tokio::test]
async fn a_task_retries_until_its_ceiling_then_succeeds() {
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    graph.set_max_attempts(&id("a"), 2).expect("retries");
    graph
        .set_acceptance(&id("a"), "the report exists")
        .expect("acceptance");
    let executor = Arc::new(Flaky {
        attempts: std::sync::Mutex::new(std::collections::BTreeMap::new()),
    });
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator.run(executor, 1).await.expect("runs");

    assert_eq!(progress.succeeded, 1);
    let task = &orchestrator.graph().tasks[&id("a")];
    assert_eq!(task.state, TaskState::Succeeded);
    assert_eq!(task.attempts, 2);
    assert_eq!(task.acceptance.as_deref(), Some("the report exists"));
    assert_eq!(task.artifacts.len(), 1);
    assert_eq!(task.artifacts[0].name, "report");
}
#[tokio::test]
async fn a_final_failure_runs_its_compensation() {
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    graph
        .set_compensation(&id("a"), "delete the staging file")
        .expect("compensation");
    let executor = Arc::new(Recorder::new(&["a"]));
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator.run(executor.clone(), 1).await.expect("runs");

    assert_eq!(progress.failed, 1);
    assert_eq!(
        executor
            .compensated
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let task = &orchestrator.graph().tasks[&id("a")];
    assert!(task.compensated);
    assert_eq!(
        task.compensation.as_deref(),
        Some("delete the staging file")
    );
    assert_eq!(
        task.compensation_receipt
            .as_ref()
            .and_then(|value| value.get("kind")),
        Some(&json!("local"))
    );
}

#[test]
fn artifacts_expire_after_their_retention_window() {
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    graph.start(&id("a")).expect("started");
    graph
        .succeed(
            &id("a"),
            TaskOutcome {
                result: json!({"ok": true}),
                artifacts: vec![
                    ArtifactRef::new("short", "file:///short").with_retention(100),
                    ArtifactRef::new("keep", "file:///keep"),
                ],
            },
        )
        .expect("succeeded");
    graph.tasks.get_mut(&id("a")).expect("task").finished_at_ms = Some(1_000);

    assert_eq!(graph.prune_artifacts(1_050), 0);
    assert_eq!(graph.tasks[&id("a")].artifacts.len(), 2);
    assert_eq!(graph.prune_artifacts(1_101), 1);
    assert_eq!(graph.tasks[&id("a")].artifacts.len(), 1);
    assert_eq!(graph.tasks[&id("a")].artifacts[0].name, "keep");
}

struct Cleaner {
    cleaned: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl TaskExecutor for Cleaner {
    async fn execute(&self, task: &TaskNode) -> Result<TaskOutcome, String> {
        Ok(TaskOutcome {
            result: json!({ "task": task.id.as_str() }),
            artifacts: vec![ArtifactRef::new("expired", "file:///expired").with_retention(0)],
        })
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    async fn cleanup_artifacts(&self, artifacts: &[ArtifactRef]) -> Result<(), String> {
        self.cleaned
            .fetch_add(artifacts.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn expired_artifacts_are_cleaned_at_the_end_of_a_run() {
    let mut graph = TaskGraph::new();
    graph.add(id("a"), "a", Vec::new()).expect("a");
    let executor = Arc::new(Cleaner {
        cleaned: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator.run(executor.clone(), 1).await.expect("runs");

    assert_eq!(progress.succeeded, 1);
    assert_eq!(
        executor.cleaned.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert!(orchestrator.graph().tasks[&id("a")].artifacts.is_empty());
}
