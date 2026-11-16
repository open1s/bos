//! The shell-backed host.runs service.
//!
//! The service is the handle extensions hold: it assigns run ids, records the
//! snapshot an observer reads, and hands start/cancel commands to the shell's
//! pump over a channel. The shell owns real work; this module owns the seam's
//! bookkeeping, so the shell wiring is one receiver loop and nothing more.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::extension::runs::{RunHandle, RunSnapshot, RunSpec, RunState, RunsService};

/// What the shell pump receives.
#[derive(Clone, Debug)]
pub(crate) enum RunCommand {
    /// Start this run in its session.
    Start {
        /// The service-assigned id.
        id: String,
        /// The narrow-only request.
        spec: RunSpec,
    },
    /// Stop this run.
    Cancel {
        /// The service-assigned id.
        id: String,
    },
}

/// The installed service and the pump's receiver.
pub(crate) struct TaskRuns {
    sender: async_channel::Sender<RunCommand>,
    runs: Arc<Mutex<BTreeMap<String, RunSnapshot>>>,
    next: AtomicU64,
}

impl TaskRuns {
    /// Create the service and the receiver the shell pump awaits.
    pub(crate) fn new() -> (Arc<Self>, async_channel::Receiver<RunCommand>) {
        let (sender, receiver) = async_channel::unbounded();
        let service = Arc::new(Self {
            sender,
            runs: Arc::default(),
            next: AtomicU64::new(1),
        });
        (service, receiver)
    }

    /// Update one run's snapshot. Unknown ids are ignored.
    pub(crate) fn record(
        &self,
        id: &str,
        state: RunState,
        result: Option<String>,
        tokens: u64,
        steps: u32,
        error: Option<String>,
    ) {
        let Ok(mut runs) = self.runs.lock() else {
            return;
        };
        let Some(run) = runs.get_mut(id) else {
            return;
        };
        run.state = state;
        if result.is_some() {
            run.result = result;
        }
        run.tokens = tokens;
        run.steps = steps;
        if error.is_some() {
            run.error = error;
        }
    }
}

impl RunsService for TaskRuns {
    fn start(&self, spec: RunSpec) -> Result<RunHandle, String> {
        let id = format!("run-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let snapshot = RunSnapshot {
            id: id.clone(),
            state: RunState::Queued,
            result: None,
            tokens: 0,
            steps: 0,
            error: None,
        };
        if let Ok(mut runs) = self.runs.lock() {
            runs.insert(id.clone(), snapshot);
        }
        if self
            .sender
            .try_send(RunCommand::Start {
                id: id.clone(),
                spec,
            })
            .is_err()
        {
            let message = "the host is not accepting runs".to_string();
            self.record(&id, RunState::Failed, None, 0, 0, Some(message.clone()));
            return Err(message);
        }
        Ok(RunHandle { id })
    }

    fn observe(&self, id: &str) -> Option<RunSnapshot> {
        self.runs.lock().ok().and_then(|runs| runs.get(id).cloned())
    }

    fn cancel(&self, id: &str) -> Result<(), String> {
        {
            let mut runs = self
                .runs
                .lock()
                .map_err(|_| "the run store is poisoned".to_string())?;
            let run = runs
                .get_mut(id)
                .ok_or_else(|| format!("unknown run {id}"))?;
            if run.state.is_settled() {
                return Ok(());
            }
            run.state = RunState::Cancelled;
        }
        let _ = self
            .sender
            .try_send(RunCommand::Cancel { id: id.to_string() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runs::RunTier;

    fn spec() -> RunSpec {
        RunSpec {
            session: "ses-1".to_string(),
            objective: "review".to_string(),
            tier: RunTier::Turn,
            limits: crate::extension::runs::Limits::default(),
            target: None,
            capability: None,
            trace: "task-1".to_string(),
        }
    }

    /// Starting assigns stable ids, queues a snapshot and hands the command to
    /// the pump with the exact spec.
    #[test]
    fn start_queues_and_hands_off() {
        let (service, receiver) = TaskRuns::new();
        let first = service.start(spec()).expect("first");
        let second = service.start(spec()).expect("second");
        assert_eq!(first.id, "run-1");
        assert_eq!(second.id, "run-2");
        assert_eq!(
            service.observe("run-1").expect("snapshot").state,
            RunState::Queued
        );

        let command = receiver.try_recv().expect("a queued command");
        match command {
            RunCommand::Start { id, spec } => {
                assert_eq!(id, "run-1");
                assert_eq!(spec.session, "ses-1");
                assert_eq!(spec.tier, RunTier::Turn);
            }
            other => panic!("expected a start, got {other:?}"),
        }
    }

    /// The shell records progress and the observer reads it back.
    #[test]
    fn record_updates_the_snapshot() {
        let (service, _receiver) = TaskRuns::new();
        let handle = service.start(spec()).expect("start");
        service.record(&handle.id, RunState::Running, None, 0, 0, None);
        let running = service.observe(&handle.id).expect("snapshot");
        assert_eq!(running.state, RunState::Running);
        service.record(
            &handle.id,
            RunState::Succeeded,
            Some("done".to_string()),
            42,
            3,
            None,
        );
        let settled = service.observe(&handle.id).expect("snapshot");
        assert_eq!(settled.state, RunState::Succeeded);
        assert_eq!(settled.result.as_deref(), Some("done"));
        assert_eq!(settled.tokens, 42);
        assert_eq!(settled.steps, 3);
        assert!(settled.state.is_settled());
    }

    /// Cancel marks the snapshot and sends the stop to the pump; a settled run
    /// is left alone and no command is sent.
    #[test]
    fn cancel_marks_and_notifies_once() {
        let (service, receiver) = TaskRuns::new();
        let handle = service.start(spec()).expect("start");
        let _ = receiver.try_recv().expect("start command");
        service.cancel(&handle.id).expect("cancel");
        assert_eq!(
            service.observe(&handle.id).expect("snapshot").state,
            RunState::Cancelled
        );
        match receiver.try_recv().expect("cancel command") {
            RunCommand::Cancel { id } => assert_eq!(id, handle.id),
            other => panic!("expected a cancel, got {other:?}"),
        }
        service
            .cancel(&handle.id)
            .expect("a second cancel is a no-op");
        assert!(receiver.try_recv().is_err(), "no command for a settled run");
        assert_eq!(
            service.observe(&handle.id).map(|run| run.state),
            Some(RunState::Cancelled)
        );
    }

    /// A closed pump refuses the start instead of leaving a phantom run.
    #[test]
    fn a_closed_pump_refuses() {
        let (service, receiver) = TaskRuns::new();
        drop(receiver);
        let error = service.start(spec()).expect_err("refused");
        assert_eq!(error, "the host is not accepting runs");
        assert_eq!(
            service.observe("run-1").map(|run| run.state),
            Some(RunState::Failed)
        );
        assert_eq!(
            service.observe("run-1").expect("snapshot").error.as_deref(),
            Some("the host is not accepting runs")
        );
    }

    /// Cancelling an id that was never started is an error, not silence.
    #[test]
    fn cancelling_an_unknown_run_is_an_error() {
        let (service, _receiver) = TaskRuns::new();
        assert!(service.cancel("run-404").is_err());
        assert!(service.observe("run-404").is_none());
    }
}
