//! Executable long-horizon reliability harness (issue #431).
//!
//! Every scenario in `tests/data/reliability_long_horizon.json` is executed by
//! `long_horizon_reliability_corpus_executes`. The harness uses real session
//! checkpoint persistence, task stores, task retries and controlled shells; it
//! never calls a live model or the network.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent::agent::context::{
    extract_critical_facts, validate_checkpoint, AgentSession, CheckpointSourceStatus,
};
use serde_json::{json, Value};

use crate::extension::runs::{RunHandle, RunSnapshot, RunState, RunsService};
use crate::extension::tasks::{
    AttemptStatus, FileTasksStore, TaskStatus, TasksStore, VerificationContract, VerificationStatus,
};
use crate::task_tools::{self, TaskToolsContext};

const CORPUS: &str = include_str!("../tests/data/reliability_long_horizon.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureClass {
    Functional,
    DataCorruption,
    StateInconsistent,
    ResourceExceeded,
}

impl std::fmt::Display for FailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureClass::Functional => write!(f, "functional"),
            FailureClass::DataCorruption => write!(f, "data-corruption"),
            FailureClass::StateInconsistent => write!(f, "state-inconsistent"),
            FailureClass::ResourceExceeded => write!(f, "resource-exceeded"),
        }
    }
}

#[derive(Debug)]
struct HarnessFailure {
    class: FailureClass,
    message: String,
    trace: Vec<String>,
}

impl HarnessFailure {
    fn new(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            trace: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct ScenarioEvidence {
    notes: Vec<String>,
    archive_bytes: usize,
    messages: usize,
    json_bytes: usize,
}

impl ScenarioEvidence {
    fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

fn functional(message: impl Into<String>) -> HarnessFailure {
    HarnessFailure::new(FailureClass::Functional, message)
}

fn corruption(message: impl Into<String>) -> HarnessFailure {
    HarnessFailure::new(FailureClass::DataCorruption, message)
}

fn inconsistent(message: impl Into<String>) -> HarnessFailure {
    HarnessFailure::new(FailureClass::StateInconsistent, message)
}

fn resource(message: impl Into<String>) -> HarnessFailure {
    HarnessFailure::new(FailureClass::ResourceExceeded, message)
}

fn scenario_ids() -> Vec<String> {
    let corpus: Value = serde_json::from_str(CORPUS).expect("the reliability corpus parses");
    assert_eq!(corpus["version"].as_u64(), Some(1), "corpus version");
    corpus["scenarios"]
        .as_array()
        .expect("scenarios")
        .iter()
        .map(|scenario| scenario["id"].as_str().expect("id").to_string())
        .collect()
}

fn temp_workspace(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("bsh-reliability-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("scratch workspace");
    path
}

fn seed_session(turns: usize) -> AgentSession {
    let mut session = AgentSession::new();
    for index in 0..turns {
        session.add_user(format!("turn {index} request {}", "context ".repeat(20)));
        session.add_assistant(format!("turn {index} answer {}", "detail ".repeat(20)));
    }
    session
}

fn checkpoint_summary(constraint: &str) -> String {
    format!(
        "## Primary Request and Intent\n- long-horizon reliability\n\n\
         ## Key Technical Concepts\n- persistence\n\n\
         ## Files and Code\n- reliability_harness\n\n\
         ## Errors and Fixes\n- none\n\n\
         ## Pending Jobs\n- none\n\n\
         ## Current Work\n- executing scenarios\n\n\
         ## Next Step\n- continue\n\n\
         ## Critical Context\n{constraint}"
    )
}

fn round_trip(session: &mut AgentSession) -> Result<String, HarnessFailure> {
    let json = session
        .to_json_string()
        .map_err(|error| corruption(format!("session could not be serialized: {error}")))?;
    let mut restored = AgentSession::new();
    restored
        .restore_from_json(&json)
        .map_err(|error| corruption(format!("session could not be restored: {error}")))?;
    *session = restored;
    Ok(json)
}

fn tamper_first_string(value: &mut Value) -> bool {
    match value {
        Value::String(text) => {
            *text = "tampered".to_string();
            true
        }
        Value::Array(items) => items.iter_mut().any(tamper_first_string),
        Value::Object(map) => map.values_mut().any(tamper_first_string),
        _ => false,
    }
}

// --- task fixtures -------------------------------------------------------

struct FakeRuns {
    next: AtomicU64,
    runs: Mutex<BTreeMap<String, RunSnapshot>>,
}

impl FakeRuns {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(0),
            runs: Mutex::new(BTreeMap::new()),
        })
    }

    fn succeed(&self, run: &str, result: &str) {
        let mut runs = self.runs.lock().unwrap();
        let snapshot = runs.get_mut(run).expect("run");
        snapshot.state = RunState::Succeeded;
        snapshot.result = Some(result.to_string());
    }
}

impl RunsService for FakeRuns {
    fn start(&self, _spec: crate::extension::runs::RunSpec) -> Result<RunHandle, String> {
        let id = format!("run-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        self.runs.lock().unwrap().insert(
            id.clone(),
            RunSnapshot {
                id: id.clone(),
                state: RunState::Running,
                result: None,
                tokens: 0,
                steps: 0,
                error: None,
            },
        );
        Ok(RunHandle { id })
    }

    fn observe(&self, id: &str) -> Option<RunSnapshot> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    fn cancel(&self, id: &str) -> Result<(), String> {
        if let Some(run) = self.runs.lock().unwrap().get_mut(id) {
            run.state = RunState::Cancelled;
        }
        Ok(())
    }
}

fn task_context(workspace: &std::path::Path) -> TaskToolsContext {
    let bash = agent::tools::BashTool::new("bash")
        .with_workspace(workspace.to_str().expect("utf-8 workspace"));
    TaskToolsContext {
        session: "reliability".to_string(),
        workspace: workspace.to_path_buf(),
        subagents: None,
        bash: Some(Arc::new(bash)),
        jobs: None,
    }
}

async fn create_and_settle(
    runs: &crate::extension::runs::Runs,
    fake: &Arc<FakeRuns>,
    context: &TaskToolsContext,
    args: Value,
) -> Result<String, HarnessFailure> {
    let value = task_tools::create_with(runs, context, &args)
        .await
        .map_err(|error| functional(format!("task_create failed: {error:?}")))?;
    let id = value["task"]
        .as_str()
        .ok_or_else(|| inconsistent("task_create returned no id"))?
        .to_string();
    let task = FileTasksStore
        .load(&context.workspace, &id)
        .map_err(|error| corruption(format!("task load failed: {error}")))?
        .ok_or_else(|| inconsistent("task record missing after create"))?;
    let run = task
        .attempts
        .last()
        .and_then(|attempt| attempt.run.clone())
        .ok_or_else(|| inconsistent("task attempt has no run"))?;
    fake.succeed(&run, "ok");
    task_tools::join_with(runs, context, &json!({"tasks": [id.clone()]}))
        .await
        .map_err(|error| functional(format!("task_join failed: {error:?}")))?;
    Ok(id)
}

// --- scenarios -----------------------------------------------------------

fn scenario_context_multi_request_consistency() -> Result<ScenarioEvidence, HarnessFailure> {
    use crate::context_assembly::{
        AssemblySnapshot, ContextBlock, ContextSource, SourceVersions, Trust,
    };
    let block = |text: &str| ContextBlock {
        source: ContextSource::HostRuntime,
        trust: Trust::HostAuthority,
        text: text.to_string(),
        token_estimate: 3,
        provenance: Some("harness".to_string()),
    };
    let versions = SourceVersions {
        provider: Some("vendor/model".to_string()),
        tool_guidance: Some("tools".to_string()),
        turn: Some(1),
        ..SourceVersions::default()
    };
    let first = AssemblySnapshot::build(vec![block("one")], versions.clone(), None);
    let second = AssemblySnapshot::build(vec![block("one")], versions.clone(), None);
    if first.fingerprint().composition != second.fingerprint().composition {
        return Err(inconsistent(
            "equivalent requests have different identities",
        ));
    }
    let changed = AssemblySnapshot::build(vec![block("one"), block("two")], versions, None);
    if changed.fingerprint().composition == first.fingerprint().composition {
        return Err(functional(
            "context change did not move composition identity",
        ));
    }
    Ok(ScenarioEvidence::default().note("two requests share identity; change moves it"))
}

fn scenario_context_growth() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut session = seed_session(5);
    let before = session.messages().len();
    for index in 0..100 {
        session.add_user(format!("growth {index} {}", "x".repeat(200)));
    }
    if session.messages().len() <= before {
        return Err(functional("history did not grow"));
    }
    let installed =
        session.install_priced_checkpoint(0, &checkpoint_summary("- keep growth bounded"));
    if installed == 0 {
        return Err(inconsistent("checkpoint did not reduce the history"));
    }
    if session.messages().len() >= 100 {
        return Err(resource("compaction did not bound the history"));
    }
    Ok(ScenarioEvidence {
        messages: session.messages().len(),
        ..ScenarioEvidence::default()
    }
    .note("history grew then compacted"))
}

fn scenario_memory_stale_conflict() -> Result<ScenarioEvidence, HarnessFailure> {
    use crate::context_assembly::{
        AssemblySnapshot, ContextBlock, ContextSource, SourceVersions, Trust,
    };
    let memory = |text: &str| ContextBlock {
        source: ContextSource::RetrievedMemory,
        trust: Trust::UntrustedData,
        text: text.to_string(),
        token_estimate: 8,
        provenance: Some("rec-1@1".to_string()),
    };
    let snapshot = AssemblySnapshot::build(
        vec![
            memory("stale memory: deploy on Friday"),
            memory("memory: do not deploy on Friday"),
            memory("wrong memory: tests passed"),
        ],
        SourceVersions::default(),
        None,
    );
    let memories: Vec<_> = snapshot
        .blocks()
        .iter()
        .filter(|block| block.source == ContextSource::RetrievedMemory)
        .collect();
    if memories.len() != 3 {
        return Err(inconsistent(
            "stale/conflicting/wrong memories were collapsed",
        ));
    }
    if memories
        .iter()
        .any(|block| block.trust != Trust::UntrustedData)
    {
        return Err(inconsistent("memory lost its untrusted boundary"));
    }
    Ok(ScenarioEvidence::default()
        .note("stale, conflicting and wrong memory records all preserved"))
}

fn scenario_compaction_multi_round() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut session = seed_session(6);
    session.add_user("CONSTRAINT: keep the public API stable".to_string());
    session.add_assistant("Understood.".to_string());
    if session.install_priced_checkpoint(
        0,
        &checkpoint_summary("- CONSTRAINT: keep the public API stable"),
    ) == 0
    {
        return Err(functional("first compaction installed nothing"));
    }
    for index in 0..6 {
        session.add_user(format!("second round {index} {}", "y".repeat(200)));
        session.add_assistant("ack".to_string());
    }
    if session.install_priced_checkpoint(
        0,
        &checkpoint_summary("- CONSTRAINT: keep the public API stable"),
    ) == 0
    {
        return Err(functional("second compaction installed nothing"));
    }
    let index = session.checkpoints().len() - 1;
    let facts = extract_critical_facts(
        session
            .checkpoint_source(index)
            .ok_or_else(|| corruption("checkpoint source unavailable"))?,
    );
    if !facts
        .iter()
        .any(|fact| fact.text.contains("keep the public API stable"))
    {
        return Err(corruption(
            "critical constraint missing after repeated compaction",
        ));
    }
    if session.checkpoint_source_status(index) != CheckpointSourceStatus::Valid {
        return Err(corruption("checkpoint source is not valid"));
    }
    Ok(ScenarioEvidence {
        archive_bytes: session.archive_bytes(),
        messages: session.messages().len(),
        ..ScenarioEvidence::default()
    }
    .note("two compactions retained the constraint"))
}

fn scenario_compaction_failure_preserved() -> Result<ScenarioEvidence, HarnessFailure> {
    let session = seed_session(3);
    let before = session.messages().len();
    let error = validate_checkpoint("## Current Work\n- incomplete", session.messages())
        .expect_err("lossy summary must fail");
    if !error.contains("missing required section") {
        return Err(functional(format!("unexpected gate error: {error}")));
    }
    if session.messages().len() != before {
        return Err(inconsistent("rejected summary changed the history"));
    }
    Ok(ScenarioEvidence::default().note("lossy checkpoint rejected without mutation"))
}

fn scenario_checkpoint_corruption_rejected() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut session = seed_session(4);
    session.install_priced_checkpoint(0, &checkpoint_summary("- constraint"));
    let index = session.checkpoints().len() - 1;
    let digest = session.checkpoints()[index].source_digest.clone();
    let json = session
        .to_json_string()
        .map_err(|error| corruption(error.to_string()))?;
    let mut value: Value =
        serde_json::from_str(&json).map_err(|error| corruption(error.to_string()))?;
    let archived = value["archive"]
        .get_mut(&digest)
        .ok_or_else(|| corruption("archive entry missing"))?;
    if !tamper_first_string(archived) {
        return Err(corruption("archive payload had no tamperable content"));
    }
    let mut target = AgentSession::new();
    target.add_user("keep me".to_string());
    let before = target.messages().len();
    if target.restore_from_json(&value.to_string()).is_ok() {
        return Err(corruption("tampered archive was accepted"));
    }
    if target.messages().len() != before {
        return Err(inconsistent("rejected restore mutated the target session"));
    }
    Ok(ScenarioEvidence::default().note("tampered archive rejected; target untouched"))
}

fn scenario_checkpoint_version_migration() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut session = seed_session(4);
    session.install_priced_checkpoint(0, &checkpoint_summary("- migrate me"));
    let json = session
        .to_json_string()
        .map_err(|error| corruption(error.to_string()))?;
    let mut value: Value =
        serde_json::from_str(&json).map_err(|error| corruption(error.to_string()))?;
    let checkpoint = &mut value["checkpoints"][0];
    let digest = checkpoint["source_digest"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    checkpoint["archive_version"] = json!(0);
    checkpoint.as_object_mut().unwrap().remove("archive_id");
    checkpoint.as_object_mut().unwrap().remove("archive_digest");
    checkpoint.as_object_mut().unwrap().remove("archive_bytes");
    value["archive_entries"]
        .as_object_mut()
        .unwrap()
        .remove(&digest);
    let mut restored = AgentSession::new();
    restored
        .restore_from_json(&value.to_string())
        .map_err(|error| corruption(format!("legacy migration failed: {error}")))?;
    let status = restored.checkpoint_source_status(0);
    if status != CheckpointSourceStatus::Valid {
        return Err(corruption(format!("legacy source invalid: {status}")));
    }
    Ok(ScenarioEvidence::default().note("pre-v5 checkpoint migrated and validated"))
}

async fn scenario_task_retry_protocol() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("retry");
    let context = task_context(&workspace);
    let fake = FakeRuns::new();
    let runs = crate::extension::runs::Runs::with(fake.clone());
    let id = create_and_settle(
        &runs,
        &fake,
        &context,
        json!({"intent": "deploy", "retry_policy": "non-idempotent"}),
    )
    .await?;
    let denied = task_tools::retry_with(&runs, &context, &json!({"task": id}))
        .await
        .expect_err("model confirmation is not authorization");
    if !format!("{denied:?}").contains("host authorization") {
        return Err(functional(format!("unexpected retry error: {denied:?}")));
    }
    task_tools::retry_with_authorization(
        &runs,
        &context,
        &json!({"task": id}),
        crate::extension::tasks::RetryAuthorization {
            approved_by: "harness".to_string(),
            reason: "verified no effect".to_string(),
            side_effect_state: crate::extension::tasks::RetrySideEffectState::NoEffect,
            compensation_command: None,
            compensation_verification: None,
            approved_ms: 1,
        },
    )
    .await
    .map_err(|error| functional(format!("authorized retry failed: {error:?}")))?;
    let task = FileTasksStore
        .load(&workspace, &id)
        .map_err(|error| corruption(error.to_string()))?
        .ok_or_else(|| inconsistent("task disappeared"))?;
    if task.attempts.len() != 2 {
        return Err(inconsistent(
            "authorized retry did not create a new attempt",
        ));
    }
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("model denied; host authorization retried"))
}

async fn scenario_task_duplicate_side_effect() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("dedup");
    let context = task_context(&workspace);
    let fake = FakeRuns::new();
    let runs = crate::extension::runs::Runs::with(fake.clone());
    let id = create_and_settle(
        &runs,
        &fake,
        &context,
        json!({
            "intent": "pay once",
            "retry_policy": "idempotent",
            "idempotency_key": "payment-1"
        }),
    )
    .await?;
    let before = fake.runs.lock().unwrap().len();
    let value = task_tools::retry_with(&runs, &context, &json!({"task": id}))
        .await
        .map_err(|error| functional(format!("dedup retry failed: {error:?}")))?;
    if value["deduplicated"] != true {
        return Err(inconsistent("same idempotency key executed again"));
    }
    if fake.runs.lock().unwrap().len() != before {
        return Err(inconsistent("dedup started a new run"));
    }
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("same key returned the recorded result"))
}

async fn scenario_task_verification_failure() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("verify-fail");
    let context = task_context(&workspace);
    let fake = FakeRuns::new();
    let runs = crate::extension::runs::Runs::with(fake.clone());
    let id = create_and_settle(
        &runs,
        &fake,
        &context,
        json!({
            "intent": "produce artifact",
            "retry_policy": "idempotent",
            "verification_contract": {"kind": "artifact", "path": "missing.txt"}
        }),
    )
    .await?;
    let task = FileTasksStore
        .load(&workspace, &id)
        .map_err(|error| corruption(error.to_string()))?
        .ok_or_else(|| inconsistent("task disappeared"))?;
    if task.verification.status != VerificationStatus::Failed {
        return Err(functional("missing artifact did not fail verification"));
    }
    if task.verified_completion() {
        return Err(inconsistent(
            "failed verification reported verified completion",
        ));
    }
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("missing artifact failed verification"))
}

async fn scenario_execution_tool_failure() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("tool-fail");
    let shell: task_tools::VerificationShell = Arc::new(|_| Ok((3, String::new())));
    let task = task_tools::verify_contract(
        &VerificationContract::Command {
            command: "exit 3".to_string(),
        },
        &workspace,
        Some(&shell),
    )
    .await;
    let _ = std::fs::remove_dir_all(&workspace);
    if task.status != VerificationStatus::Failed {
        return Err(functional("non-zero command was not a failed verification"));
    }
    Ok(ScenarioEvidence::default().note("tool failure classified as VerificationStatus::Failed"))
}

async fn scenario_execution_cancel_timeout() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("cancel-timeout");
    let context = task_context(&workspace);
    let fake = FakeRuns::new();
    let runs = crate::extension::runs::Runs::with(fake.clone());
    let value = task_tools::create_with(&runs, &context, &json!({"intent": "cancel"}))
        .await
        .map_err(|error| functional(format!("create failed: {error:?}")))?;
    let id = value["task"].as_str().expect("id").to_string();
    task_tools::cancel_with(&runs, &context, &json!({"task": id.clone()}))
        .await
        .map_err(|error| functional(format!("cancel failed: {error:?}")))?;
    let record = FileTasksStore
        .load(&workspace, &id)
        .map_err(|error| corruption(error.to_string()))?
        .ok_or_else(|| inconsistent("cancelled task missing"))?;
    if record.status != TaskStatus::Cancelled {
        return Err(inconsistent("cancel did not persist"));
    }
    use agent::Tool as _;
    let bash = agent::tools::BashTool::new("bash")
        .with_workspace(workspace.to_str().expect("utf-8 workspace"))
        .with_config(agent::tools::bash::BashToolConfig {
            timeout_secs: 1,
            ..Default::default()
        });
    let started = std::time::Instant::now();
    let command = format!(
        "cd '{}' && sh -c 'sleep 5 & echo $! > child.pid; wait'",
        workspace.display()
    );
    let response = tokio::task::spawn_blocking(move || bash.run(&json!({"command": command})))
        .await
        .map_err(|error| functional(error.to_string()))?;
    let elapsed = started.elapsed();
    if elapsed >= std::time::Duration::from_secs(4) {
        return Err(resource("controlled shell did not enforce its timeout"));
    }
    if let Ok(pid) = std::fs::read_to_string(workspace.join("child.pid")) {
        let pid = pid.trim();
        let alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if alive {
            let _ = std::process::Command::new("kill")
                .args(["-9", pid])
                .status();
            return Err(resource("child process survived timeout/termination"));
        }
    }
    let _ = response;
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("cancel persisted; timed-out shell stopped its child"))
}

fn scenario_recovery_process_restart() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("restart");
    let path = workspace.join("session.json");
    let mut session = seed_session(5);
    session.install_priced_checkpoint(0, &checkpoint_summary("- CONSTRAINT: survive restart"));
    let json = session
        .to_json_string()
        .map_err(|error| corruption(error.to_string()))?;
    std::fs::write(&path, &json).map_err(|error| corruption(error.to_string()))?;
    let checkpoint_count = session.checkpoints().len();
    drop(session);

    let bytes = std::fs::read_to_string(&path).map_err(|error| corruption(error.to_string()))?;
    let mut restored = AgentSession::new();
    restored
        .restore_from_json(&bytes)
        .map_err(|error| corruption(error.to_string()))?;
    if restored.checkpoints().len() != checkpoint_count {
        return Err(inconsistent("checkpoint count changed across restart"));
    }
    if restored.checkpoint_source_status(0) != CheckpointSourceStatus::Valid {
        return Err(corruption("restored checkpoint source invalid"));
    }
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("fresh session restored from disk"))
}

async fn scenario_recovery_reconciliation() -> Result<ScenarioEvidence, HarnessFailure> {
    let workspace = temp_workspace("reconcile");
    let context = task_context(&workspace);
    let fake = FakeRuns::new();
    let runs = crate::extension::runs::Runs::with(fake.clone());
    let value = task_tools::create_with(
        &runs,
        &context,
        &json!({"intent": "crash", "retry_policy": "non-idempotent"}),
    )
    .await
    .map_err(|error| functional(format!("create failed: {error:?}")))?;
    let id = value["task"].as_str().expect("id").to_string();
    let mut record = FileTasksStore
        .load(&workspace, &id)
        .map_err(|error| corruption(error.to_string()))?
        .ok_or_else(|| inconsistent("task missing"))?;
    record.attempts[0].status = AttemptStatus::Queued;
    record.attempts[0].run = None;
    record.attempts[0].started_ms = None;
    if let Some(effect) = record.side_effects.get_mut(&record.attempts[0].id) {
        effect.status = crate::extension::tasks::SideEffectStatus::Unknown;
    }
    FileTasksStore
        .save(&workspace, &record)
        .map_err(|error| corruption(error.to_string()))?;
    let error = task_tools::retry_with(&runs, &context, &json!({"task": id.clone()}))
        .await
        .expect_err("run-less attempt needs authorization");
    if !format!("{error:?}").contains("host authorization") {
        return Err(inconsistent(format!(
            "unexpected reconciliation error: {error:?}"
        )));
    }
    let stored = FileTasksStore
        .load(&workspace, &id)
        .map_err(|error| corruption(error.to_string()))?
        .ok_or_else(|| inconsistent("task missing after reconcile"))?;
    if stored.attempts[0].status != AttemptStatus::Interrupted {
        return Err(inconsistent(
            "run-less attempt was not reconciled to Interrupted",
        ));
    }
    let _ = std::fs::remove_dir_all(&workspace);
    Ok(ScenarioEvidence::default().note("crash remnant marked Interrupted and not re-executed"))
}

fn scenario_isolation_multi_session() -> Result<ScenarioEvidence, HarnessFailure> {
    let run = |label: &str| -> Result<(usize, String), HarnessFailure> {
        let mut session = seed_session(3);
        session.install_priced_checkpoint(0, &checkpoint_summary(&format!("- {label}")));
        let json = session
            .to_json_string()
            .map_err(|error| corruption(error.to_string()))?;
        Ok((session.checkpoints().len(), json))
    };
    let handles: Vec<_> = ["one", "two", "three"]
        .into_iter()
        .map(|label| std::thread::spawn(move || run(label)))
        .collect();
    let mut payloads = Vec::new();
    for handle in handles {
        let (count, json) = handle
            .join()
            .map_err(|_| inconsistent("session thread panicked"))??;
        if count != 1 {
            return Err(inconsistent("cross-session checkpoint leakage"));
        }
        payloads.push(json);
    }
    for json in &payloads {
        let mut session = AgentSession::new();
        session
            .restore_from_json(json)
            .map_err(|error| corruption(error.to_string()))?;
        if session.checkpoints().len() != 1 {
            return Err(inconsistent("session restore mixed checkpoints"));
        }
    }
    Ok(ScenarioEvidence::default().note("three concurrent sessions remained isolated"))
}

fn scenario_provider_request_consistency() -> Result<ScenarioEvidence, HarnessFailure> {
    use react::llm::{
        Content, LlmContext, LlmMessage, LlmRequest, LlmSession, LlmTool, ProviderRequest,
    };
    let request = || ProviderRequest {
        model: "vendor/model".to_string(),
        provider: Some("vendor".to_string()),
        provider_adapter: Some("vendor:chat".to_string()),
        system_prompt: Some("secret system".to_string()),
        messages: vec![
            LlmMessage::user("private hello"),
            LlmMessage::assistant("private answer"),
        ],
        input: Content::text("private hello"),
        tools: vec![LlmTool {
            name: "bash".to_string(),
            description: "run".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            kind: Default::default(),
            config: None,
        }],
        composition: "composition".to_string(),
        execution: "execution".to_string(),
        ..Default::default()
    };
    let first = request().audit();
    let second = request().audit();
    if first != second {
        return Err(inconsistent(
            "identical provider requests audited differently",
        ));
    }
    let text = first.to_string();
    if text.contains("private hello") || text.contains("secret system") {
        return Err(functional("request audit leaked private content"));
    }
    if first["message_count"] != 2 || first["tools"][0] != "bash" {
        return Err(functional("audit omitted request structure"));
    }
    let typed = request();
    let llm = LlmRequest {
        model: typed.model.clone(),
        input: typed.input.clone(),
        provider_request: Some(typed.clone()),
        ..Default::default()
    };
    let openai = react::llm::vendor::openai::OpenAiVendor::wire_preview(
        &llm,
        &LlmSession::new(),
        &LlmContext::default(),
    );
    if openai["messages"][0]["content"] != "secret system" {
        return Err(inconsistent(
            "OpenAI adapter did not use the typed system prompt",
        ));
    }
    if openai["tools"][0]["function"]["name"] != "bash" {
        return Err(inconsistent("OpenAI adapter did not use the typed tools"));
    }
    let responses = react::llm::vendor::responses::build_request(
        None,
        &llm,
        &LlmSession::new(),
        &LlmContext::default(),
        false,
        true,
    );
    let responses =
        serde_json::to_value(responses).map_err(|error| functional(error.to_string()))?;
    if responses["instructions"] != "secret system" {
        return Err(inconsistent(
            "Responses adapter did not use the typed system prompt",
        ));
    }
    if !responses.to_string().contains("bash") {
        return Err(inconsistent(
            "Responses adapter did not use the typed tools",
        ));
    }
    Ok(ScenarioEvidence::default()
        .note("typed request audit stable; OpenAI and Responses wire previews match"))
}

fn scenario_resource_growth_bounded() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut session = AgentSession::new();
    let mut max_json = 0usize;
    for cycle in 0..20 {
        for index in 0..2 {
            session.add_user(format!(
                "cycle {cycle} item {index} {}",
                "resource ".repeat(80)
            ));
            session.add_assistant("ack".to_string());
        }
        session.install_priced_checkpoint(0, &checkpoint_summary(&format!("- cycle {cycle}")));
        let json = session
            .to_json_string()
            .map_err(|error| resource(format!("session serialization failed: {error}")))?;
        max_json = max_json.max(json.len());
        if session.messages().len() > 80 {
            return Err(resource("history grew without bound"));
        }
        if session.archive_bytes() > 8 * 1024 * 1024 {
            return Err(resource("archive exceeded its byte budget"));
        }
        if json.len() > 4 * 1024 * 1024 {
            return Err(resource("session JSON exceeded its byte budget"));
        }
    }
    Ok(ScenarioEvidence {
        archive_bytes: session.archive_bytes(),
        messages: session.messages().len(),
        json_bytes: max_json,
        ..ScenarioEvidence::default()
    }
    .note("20 cycles stayed within bounded history, archive and JSON"))
}

async fn scenario_long_run_multi_session_restart() -> Result<ScenarioEvidence, HarnessFailure> {
    let mut total_archive = 0usize;
    let mut total_json = 0usize;
    for label in ["alpha", "beta", "gamma"] {
        let mut session = seed_session(2);
        session.add_user(format!("CONSTRAINT: {label} stays isolated"));
        session.add_assistant("Understood.".to_string());
        for cycle in 0..4 {
            for index in 0..2 {
                session.add_user(format!(
                    "{label} cycle {cycle} item {index} {}",
                    "z".repeat(200)
                ));
                session.add_assistant("ack".to_string());
            }
            session.install_priced_checkpoint(
                0,
                &checkpoint_summary(&format!("- CONSTRAINT: {label} stays isolated")),
            );
            let json = round_trip(&mut session)?;
            total_json = total_json.max(json.len());
        }
        total_archive += session.archive_bytes();
        let index = session.checkpoints().len() - 1;
        if session.checkpoint_source_status(index) != CheckpointSourceStatus::Valid {
            return Err(corruption(format!(
                "{label} checkpoint invalid after restart"
            )));
        }
        let source = session
            .checkpoint_source(index)
            .ok_or_else(|| corruption("source missing"))?;
        let debug = format!("{source:?}");
        if !debug.contains(&format!("{label} stays isolated")) {
            return Err(corruption(format!("{label} lost its constraint")));
        }
    }
    Ok(ScenarioEvidence {
        archive_bytes: total_archive,
        json_bytes: total_json,
        ..ScenarioEvidence::default()
    }
    .note("three sessions x four restart/compaction cycles"))
}

async fn run_scenario(id: &str) -> Result<ScenarioEvidence, HarnessFailure> {
    match id {
        "context_multi_request_consistency" => scenario_context_multi_request_consistency(),
        "context_growth" => scenario_context_growth(),
        "memory_stale_conflict" => scenario_memory_stale_conflict(),
        "compaction_multi_round" => scenario_compaction_multi_round(),
        "compaction_failure_preserved" => scenario_compaction_failure_preserved(),
        "checkpoint_corruption_rejected" => scenario_checkpoint_corruption_rejected(),
        "checkpoint_version_migration" => scenario_checkpoint_version_migration(),
        "task_retry_protocol" => scenario_task_retry_protocol().await,
        "task_duplicate_side_effect" => scenario_task_duplicate_side_effect().await,
        "task_verification_failure" => scenario_task_verification_failure().await,
        "execution_tool_failure" => scenario_execution_tool_failure().await,
        "execution_cancel_timeout" => scenario_execution_cancel_timeout().await,
        "recovery_process_restart" => scenario_recovery_process_restart(),
        "recovery_reconciliation" => scenario_recovery_reconciliation().await,
        "isolation_multi_session" => scenario_isolation_multi_session(),
        "provider_request_consistency" => scenario_provider_request_consistency(),
        "resource_growth_bounded" => scenario_resource_growth_bounded(),
        "long_run_multi_session_restart" => scenario_long_run_multi_session_restart().await,
        other => Err(functional(format!(
            "corpus scenario {other} has no executor"
        ))),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn long_horizon_reliability_corpus_executes() {
    let ids = scenario_ids();
    let mut executed = BTreeSet::new();
    let mut failures = Vec::new();
    let mut evidence = Vec::new();
    for id in &ids {
        executed.insert(id.clone());
        match run_scenario(id).await {
            Ok(result) => evidence.push((id.clone(), result)),
            Err(failure) => failures.push((id.clone(), failure)),
        }
    }
    let unknown: Vec<_> = executed
        .iter()
        .filter(|id| !ids.contains(id))
        .cloned()
        .collect();
    assert!(unknown.is_empty(), "unknown scenario ids: {unknown:?}");
    if !failures.is_empty() {
        let mut report = String::from("reliability harness failures:\n");
        for (id, failure) in &failures {
            report.push_str(&format!(
                "- {id} [{}]: {}\n",
                failure.class, failure.message
            ));
            for line in &failure.trace {
                report.push_str(&format!("    trace: {line}\n"));
            }
        }
        for (id, result) in &evidence {
            report.push_str(&format!(
                "- {id}: PASS archive={} messages={} json={} {:?}\n",
                result.archive_bytes, result.messages, result.json_bytes, result.notes
            ));
        }
        panic!("{report}");
    }
    assert_eq!(executed.len(), ids.len());
}
