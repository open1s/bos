//! The BSH memory capture coordinator (issue #343).
//!
//! Triggered at the completed-turn boundary, the coordinator turns the turn's
//! evidence into typed candidates, deduplicates and governs them, and commits
//! through the fallible repository - ASYNCHRONOUSLY, so a capture failure never
//! invalidates the turn. Raw conversation retention (auto-remember) stays a
//! separate policy; this pipeline extracts reusable knowledge only.
//!
//! Every run is observable: the last summary is readable for diagnostics, and
//! the log names counts and ids, never memory content.

use std::sync::{Arc, Mutex};

use agent::memory::capture::{
    commit_candidates, CandidateExtractor, CapturePolicy, DeterministicExtractor, TurnEvidence,
};
use agent::memory::FileMemory;

/// A content-free account of one capture run, for diagnostics.
#[derive(Debug, Clone, Default)]
pub(crate) struct CaptureSummary {
    /// How many records were durably committed.
    pub(crate) committed: usize,
    /// The committed record ids.
    pub(crate) ids: Vec<String>,
    /// Candidates skipped as duplicates.
    pub(crate) duplicates: usize,
    /// Candidates flagged as contradicting an existing record.
    pub(crate) contradictions: usize,
    /// Candidates rejected by governance.
    pub(crate) rejected: usize,
    /// The commit failure label, when the run could not reach durability.
    pub(crate) failure: Option<String>,
}

/// The capture handle a build attaches to its turns.
pub(crate) struct CaptureHandle {
    workspace: Arc<FileMemory>,
    policy: CapturePolicy,
    last: Mutex<Option<CaptureSummary>>,
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureHandle")
            .field("enabled", &self.policy.enabled)
            .finish_non_exhaustive()
    }
}

impl CaptureHandle {
    /// Build a coordinator over the workspace store.
    pub(crate) fn new(workspace: Arc<FileMemory>, policy: CapturePolicy) -> Arc<Self> {
        Arc::new(Self {
            workspace,
            policy,
            last: Mutex::new(None),
        })
    }

    /// The most recent run's summary, when one has run.
    ///
    /// The diagnostic read seam for the capture pane a later slice adds; until
    /// then the tests are the reader, so the field is deliberately kept wired
    /// to the log above and readable here (the same pattern as the build's
    /// policy ring).
    #[allow(dead_code)]
    pub(crate) fn last_summary(&self) -> Option<CaptureSummary> {
        self.last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Run one capture to completion; failures are reported, never raised.
    pub(crate) async fn capture_turn(&self, evidence: TurnEvidence) -> CaptureSummary {
        let extractor = DeterministicExtractor;
        let candidates = extractor.extract(&evidence, &self.policy).await;
        let report =
            commit_candidates(&self.workspace.repository(), candidates, &self.policy).await;
        let summary = CaptureSummary {
            committed: report.committed.len(),
            ids: report.committed.clone(),
            duplicates: report.duplicates,
            contradictions: report.contradictions.len(),
            rejected: report.rejected.len(),
            failure: report
                .failure
                .as_ref()
                .map(|error| error.label().to_string()),
        };
        match &summary.failure {
            Some(label) => log::warn!(
                target: "bsh::memory",
                "capture: commit failed ({label}); the turn is unaffected"
            ),
            None => {
                log::info!(
                    target: "bsh::memory",
                    "capture: committed={} duplicates={} contradictions={} rejected={}",
                    summary.committed,
                    summary.duplicates,
                    summary.contradictions,
                    summary.rejected
                );
                log::debug!(target: "bsh::memory", "capture: ids={:?}", summary.ids);
            }
        }
        *self
            .last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(summary.clone());
        summary
    }

    /// Fire-and-forget one capture on the runner runtime (issue #343): the
    /// completed turn returns immediately; capture runs beside it.
    pub(crate) fn spawn(self: &Arc<Self>, rt: &tokio::runtime::Handle, evidence: TurnEvidence) {
        let handle = Arc::clone(self);
        rt.spawn(async move {
            let _ = handle.capture_turn(evidence).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::memory::{MemoryRepository, MemoryScope, MemoryStore};

    async fn handle(dir: &std::path::Path, enabled: bool) -> Arc<CaptureHandle> {
        let store = Arc::new(
            FileMemory::open_scoped(
                dir.join("ws.jsonl"),
                MemoryScope::workspace("w"),
                Some(MemoryScope::session("w", "s1")),
            )
            .await
            .unwrap(),
        );
        let policy = CapturePolicy {
            enabled,
            scope: MemoryScope::session("w", "s1"),
            ..Default::default()
        };
        CaptureHandle::new(store, policy)
    }

    fn evidence(user: &str) -> TurnEvidence {
        TurnEvidence {
            user_text: user.to_string(),
            assistant_text: "done".to_string(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            tool_evidence: Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_completed_turn_captures_explicit_knowledge_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let capture = handle(dir.path(), true).await;
        let summary = capture
            .capture_turn(evidence("Remember this: the release gate runs clippy"))
            .await;
        assert_eq!(summary.committed, 1);
        assert!(summary.failure.is_none());
        let records = capture.workspace.repository().records().await.unwrap();
        let record = records.iter().find(|r| r.id == summary.ids[0]).unwrap();
        assert_eq!(record.provenance.session.as_deref(), Some("s1"));
        assert_eq!(record.provenance.turn.as_deref(), Some("t1"));
        assert_eq!(record.scope, MemoryScope::session("w", "s1"));
    }

    #[tokio::test]
    async fn disabled_capture_commits_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let capture = handle(dir.path(), false).await;
        let summary = capture
            .capture_turn(evidence("Remember this: never stored"))
            .await;
        assert_eq!(summary.committed, 0);
        assert!(summary.rejected > 0);
        assert_eq!(capture.workspace.all().await.len(), 0);
    }

    #[tokio::test]
    async fn a_retried_capture_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let capture = handle(dir.path(), true).await;
        let first = capture
            .capture_turn(evidence("Remember this: idempotent capture"))
            .await;
        assert_eq!(first.committed, 1);
        let retry = capture
            .capture_turn(evidence("Remember this: idempotent capture"))
            .await;
        assert_eq!(retry.committed, 0);
        assert_eq!(retry.duplicates, 1);
        assert_eq!(capture.workspace.all().await.len(), 1);
    }

    #[tokio::test]
    async fn capture_failure_is_observable_and_does_not_raise() {
        struct FailGate;
        impl hostkit::durable::CommitGate for FailGate {
            fn pass(
                &self,
                stage: hostkit::durable::IoStage,
                _path: &std::path::Path,
            ) -> std::io::Result<()> {
                if stage == hostkit::durable::IoStage::Sync {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected",
                    ))
                } else {
                    Ok(())
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            FileMemory::open_scoped(
                dir.path().join("ws.jsonl"),
                MemoryScope::workspace("w"),
                Some(MemoryScope::session("w", "s1")),
            )
            .await
            .unwrap()
            .with_commit_gate(Arc::new(FailGate)),
        );
        let policy = CapturePolicy {
            enabled: true,
            scope: MemoryScope::session("w", "s1"),
            ..Default::default()
        };
        let capture = CaptureHandle::new(store, policy);
        let summary = capture
            .capture_turn(evidence("Remember this: fails durably"))
            .await;
        assert_eq!(summary.committed, 0);
        assert_eq!(summary.failure.as_deref(), Some("storage"));
        assert_eq!(capture.workspace.all().await.len(), 0);
        assert!(
            capture.last_summary().is_some(),
            "the failure is observable"
        );
    }

    #[tokio::test]
    async fn spawn_runs_beside_the_turn_without_blocking_it() {
        let dir = tempfile::tempdir().unwrap();
        let capture = handle(dir.path(), true).await;
        let rt = tokio::runtime::Handle::current();
        capture.spawn(&rt, evidence("Remember this: async capture"));
        // The turn returns immediately; the capture settles on its own task.
        for _ in 0..200 {
            if capture.last_summary().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let summary = capture.last_summary().expect("the capture ran");
        assert_eq!(summary.committed, 1);
    }
}
