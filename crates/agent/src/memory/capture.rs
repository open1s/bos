//! Governed memory capture (issue #343).
//!
//! The pipeline turns a completed turn's evidence into typed candidates,
//! distinguishes explicit user statements from model inferences and tool
//! evidence, validates/normalizes/deduplicates them, and commits only what the
//! governance policy allows. The baseline extractor is deterministic (no model
//! call); model-assisted extraction implements the same [`CandidateExtractor`]
//! seam. Capture runs asynchronously at the completed-turn boundary, so a
//! capture failure never invalidates the turn itself.
//!
//! Raw conversation retention (auto-remember) and long-term knowledge
//! extraction are SEPARATE policies: this module never stores raw exchanges.

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::error::MemoryError;
use super::record::{
    Confidence, ConfidenceLevel, EvidenceSource, Lifecycle, MemoryKind, MemoryRecord, MemoryScope,
    Provenance,
};
use super::repository::MemoryRepository;
use super::retrieval::normalize;

/// The evidence one completed turn offers to capture.
#[derive(Debug, Clone, Default)]
pub struct TurnEvidence {
    /// The user's input text for the turn.
    pub user_text: String,
    /// The assistant's final answer text.
    pub assistant_text: String,
    /// The session identity.
    pub session: Option<String>,
    /// The turn identity inside the session.
    pub turn: Option<String>,
    /// Tool-derived evidence references (tool-call ids, file paths).
    pub tool_evidence: Vec<String>,
}

/// One typed candidate produced from turn evidence.
#[derive(Debug, Clone)]
pub struct CaptureCandidate {
    /// The kind of knowledge claimed.
    pub kind: MemoryKind,
    /// The candidate content, normalized for storage.
    pub content: String,
    /// The scope the candidate belongs to.
    pub scope: MemoryScope,
    /// Where the claim came from.
    pub source: EvidenceSource,
    /// Evidence references back to the source turn.
    pub evidence: Vec<String>,
    /// Heuristic confidence (never a probability).
    pub confidence: ConfidenceLevel,
    /// Optional subject key for contradiction detection.
    pub subject: Option<String>,
    /// The source session, when known.
    pub session: Option<String>,
    /// The source turn, when known.
    pub turn: Option<String>,
}

impl CaptureCandidate {
    /// The deduplication key: kind + scope + normalized content hash.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.kind.label().as_bytes());
        hasher.update([0u8]);
        hasher.update(
            serde_json::to_string(&self.scope)
                .unwrap_or_default()
                .as_bytes(),
        );
        hasher.update([0u8]);
        hasher.update(normalize(&self.content).as_bytes());
        let digest = hasher.finalize();
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Convert into a record ready for governance.
    #[must_use]
    pub fn into_record(self) -> MemoryRecord {
        let mut record = MemoryRecord::new(
            self.kind,
            self.scope,
            self.content,
            Provenance {
                source: self.source,
                session: self.session,
                turn: self.turn,
                evidence: self.evidence,
            },
        );
        record.confidence = Confidence {
            level: self.confidence,
            verified: false,
        };
        if let Some(subject) = self.subject {
            let mut metadata = record.metadata.take().unwrap_or(serde_json::json!({}));
            if let Some(object) = metadata.as_object_mut() {
                object.insert("subject".to_string(), serde_json::Value::String(subject));
            }
            record.metadata = Some(metadata);
        }
        record
    }
}

/// The governance policy for automatic capture (issue #343).
#[derive(Debug, Clone)]
pub struct CapturePolicy {
    /// Whether automatic capture runs at all.
    pub enabled: bool,
    /// The kinds automatic extraction may commit.
    pub auto_kinds: Vec<MemoryKind>,
    /// The scope captured records land in.
    pub scope: MemoryScope,
    /// The most candidates one turn may commit.
    pub max_per_turn: usize,
    /// Whether explicit "remember this" requests are honored.
    pub explicit_enabled: bool,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_kinds: vec![
                MemoryKind::Fact,
                MemoryKind::Preference,
                MemoryKind::Decision,
                MemoryKind::Lesson,
            ],
            scope: MemoryScope::workspace("default"),
            max_per_turn: 5,
            explicit_enabled: true,
        }
    }
}

/// Why a candidate was not committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// Capture is disabled by policy.
    Disabled,
    /// The kind is not in the policy's allowlist.
    KindNotPermitted,
    /// An identical record already exists in scope.
    Duplicate,
    /// The content is empty after normalization.
    Empty,
    /// The per-turn commit cap was reached.
    CapReached,
    /// The repository refused the commit; the detail is typed separately.
    Failed,
}

/// The observable outcome of one capture run (issue #343).
#[derive(Debug, Default)]
pub struct CaptureReport {
    /// Ids of the records committed this run.
    pub committed: Vec<String>,
    /// Candidates skipped as duplicates.
    pub duplicates: usize,
    /// Candidates flagged as contradicting an existing record (both kept).
    pub contradictions: Vec<String>,
    /// Rejected candidates with reasons (content never leaves the pipeline).
    pub rejected: Vec<Rejection>,
    /// The commit failure, when the run could not reach durability.
    pub failure: Option<MemoryError>,
}

/// The extraction seam: deterministic baseline today, model-assisted later.
#[async_trait]
pub trait CandidateExtractor: Send + Sync {
    /// Produce candidates from one completed turn's evidence.
    async fn extract(
        &self,
        evidence: &TurnEvidence,
        policy: &CapturePolicy,
    ) -> Vec<CaptureCandidate>;
}

/// The deterministic baseline extractor (no model call, no network).
///
/// It reads EXPLICIT user statements only: "remember ..." requests, decision,
/// preference and lesson markers in English and Chinese. Model inferences and
/// tool evidence are never promoted to user-statement confidence here.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeterministicExtractor;

/// One extraction rule: markers, the kind they produce and the subject prefix.
struct Rule {
    markers: &'static [&'static str],
    kind: MemoryKind,
}

const RULES: &[Rule] = &[
    Rule {
        markers: &[
            "we decided",
            "we have decided",
            "let's use",
            "we will use",
            "决定",
            "采用",
            "改用",
        ],
        kind: MemoryKind::Decision,
    },
    Rule {
        markers: &[
            "i prefer",
            "i like",
            "we prefer",
            "prefer ",
            "我喜欢",
            "我偏好",
            "偏好",
        ],
        kind: MemoryKind::Preference,
    },
    Rule {
        markers: &["lesson learned", "next time we", "教训", "下次"],
        kind: MemoryKind::Lesson,
    },
];

impl DeterministicExtractor {
    fn candidate(
        kind: MemoryKind,
        content: &str,
        source: EvidenceSource,
        confidence: ConfidenceLevel,
        evidence: &TurnEvidence,
        policy: &CapturePolicy,
    ) -> CaptureCandidate {
        CaptureCandidate {
            kind,
            content: content.trim().to_string(),
            scope: policy.scope.clone(),
            source,
            evidence: {
                let mut refs = Vec::new();
                if let (Some(session), Some(turn)) = (&evidence.session, &evidence.turn) {
                    refs.push(format!("turn:{session}/{turn}"));
                }
                refs.extend(evidence.tool_evidence.iter().cloned());
                refs
            },
            confidence,
            subject: None,
            session: evidence.session.clone(),
            turn: evidence.turn.clone(),
        }
    }
}

#[async_trait]
impl CandidateExtractor for DeterministicExtractor {
    async fn extract(
        &self,
        evidence: &TurnEvidence,
        policy: &CapturePolicy,
    ) -> Vec<CaptureCandidate> {
        let mut candidates = Vec::new();
        let text = evidence.user_text.trim();
        if text.is_empty() {
            return candidates;
        }
        let lower = text.to_lowercase();

        // Explicit "remember this" requests: deterministic, highest priority.
        if policy.explicit_enabled {
            for prefix in [
                "remember this:",
                "remember that:",
                "remember:",
                "请记住",
                "记住",
            ] {
                if lower.starts_with(prefix) {
                    // Recover the original-case remainder by char count.
                    let remainder: String = text.chars().skip(prefix.chars().count()).collect();
                    let content = if remainder.trim().is_empty() {
                        text.to_string()
                    } else {
                        remainder.trim().to_string()
                    };
                    candidates.push(Self::candidate(
                        MemoryKind::Fact,
                        &content,
                        EvidenceSource::UserStatement,
                        ConfidenceLevel::High,
                        evidence,
                        policy,
                    ));
                    return candidates;
                }
            }
        }

        // Marker-based extraction from explicit user statements.
        for rule in RULES {
            if !policy.auto_kinds.contains(&rule.kind) {
                continue;
            }
            if rule.markers.iter().any(|marker| lower.contains(marker)) {
                candidates.push(Self::candidate(
                    rule.kind,
                    text,
                    EvidenceSource::UserStatement,
                    ConfidenceLevel::Medium,
                    evidence,
                    policy,
                ));
            }
        }
        candidates
    }
}

/// Commit candidates under governance: validate, dedupe, flag contradictions,
/// cap, and commit through the repository (issue #343).
///
/// Duplicates are skipped, never appended. A candidate with the same subject
/// and kind but different content is committed WITH a `contradicts` relation
/// to the existing record, and reported - both stay readable for review.
pub async fn commit_candidates(
    repository: &dyn MemoryRepository,
    candidates: Vec<CaptureCandidate>,
    policy: &CapturePolicy,
) -> CaptureReport {
    let mut report = CaptureReport::default();
    if !policy.enabled {
        report.rejected = candidates.iter().map(|_| Rejection::Disabled).collect();
        return report;
    }
    let existing = match repository.records().await {
        Ok(records) => records,
        Err(error) => {
            report.failure = Some(error);
            return report;
        }
    };
    let mut committed_keys: Vec<String> = Vec::new();
    for candidate in candidates {
        if report.committed.len() >= policy.max_per_turn {
            report.rejected.push(Rejection::CapReached);
            continue;
        }
        if normalize(&candidate.content).is_empty() {
            report.rejected.push(Rejection::Empty);
            continue;
        }
        if candidate.source != EvidenceSource::UserStatement
            && !policy.auto_kinds.contains(&candidate.kind)
        {
            report.rejected.push(Rejection::KindNotPermitted);
            continue;
        }
        if candidate.source == EvidenceSource::UserStatement
            && !policy.explicit_enabled
            && candidate.confidence == ConfidenceLevel::High
        {
            // Explicit requests are governed by their own switch.
            report.rejected.push(Rejection::KindNotPermitted);
            continue;
        }
        let key = candidate.dedupe_key();
        let duplicate = committed_keys.contains(&key)
            || existing.iter().any(|record| {
                record.status == Lifecycle::Active
                    && record.kind == candidate.kind
                    && record.scope == candidate.scope
                    && normalize(&record.content) == normalize(&candidate.content)
            });
        if duplicate {
            report.duplicates += 1;
            report.rejected.push(Rejection::Duplicate);
            continue;
        }
        let mut record = candidate.into_record();
        // Contradiction detection: same subject and kind, different content.
        if let Some(subject) = record
            .metadata
            .as_ref()
            .and_then(|m| m.get("subject"))
            .and_then(|s| s.as_str())
        {
            for prior in &existing {
                if prior.status == Lifecycle::Active
                    && prior.kind == record.kind
                    && prior.scope == record.scope
                    && prior
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("subject"))
                        .and_then(|s| s.as_str())
                        == Some(subject)
                    && normalize(&prior.content) != normalize(&record.content)
                {
                    record.relations.contradicts.push(prior.id.clone());
                    report.contradictions.push(prior.id.clone());
                }
            }
        }
        match repository.add_record(record).await {
            Ok(committed) => {
                committed_keys.push(key);
                report.committed.push(committed.id);
            }
            Err(error) => {
                report.rejected.push(Rejection::Failed);
                report.failure = Some(error);
                return report;
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{FileMemory, MemoryStore};

    fn evidence(user: &str) -> TurnEvidence {
        TurnEvidence {
            user_text: user.to_string(),
            assistant_text: "ok".to_string(),
            session: Some("s1".into()),
            turn: Some("t1".into()),
            tool_evidence: Vec::new(),
        }
    }

    fn policy() -> CapturePolicy {
        CapturePolicy {
            enabled: true,
            scope: MemoryScope::workspace("w"),
            ..Default::default()
        }
    }

    async fn workspace_memory(path: &std::path::Path) -> FileMemory {
        FileMemory::open_scoped(path, MemoryScope::workspace("w"), None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn explicit_remember_requests_are_deterministic() {
        let extractor = DeterministicExtractor;
        for text in [
            "Remember this: the release gate runs clippy",
            "记住：发布前必须跑 clippy",
            "请记住部署流程",
        ] {
            let candidates = extractor.extract(&evidence(text), &policy()).await;
            assert_eq!(candidates.len(), 1, "text: {text}");
            assert_eq!(candidates[0].kind, MemoryKind::Fact);
            assert_eq!(candidates[0].source, EvidenceSource::UserStatement);
            assert_eq!(candidates[0].confidence, ConfidenceLevel::High);
            assert!(candidates[0].evidence.iter().any(|e| e == "turn:s1/t1"));
        }
    }

    #[tokio::test]
    async fn markers_extract_decisions_preferences_and_lessons() {
        let extractor = DeterministicExtractor;
        let decision = extractor
            .extract(
                &evidence("We decided to use the durable kernel."),
                &policy(),
            )
            .await;
        assert!(decision.iter().any(|c| c.kind == MemoryKind::Decision));
        let preference = extractor
            .extract(&evidence("我偏好中文简洁回答"), &policy())
            .await;
        assert!(preference.iter().any(|c| c.kind == MemoryKind::Preference));
        let lesson = extractor
            .extract(
                &evidence("Lesson learned: inject failures at every stage."),
                &policy(),
            )
            .await;
        assert!(lesson.iter().any(|c| c.kind == MemoryKind::Lesson));
        // Plain chit-chat produces nothing.
        let none = extractor.extract(&evidence("hello there"), &policy()).await;
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn disabled_capture_commits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let memory = workspace_memory(&dir.path().join("m.jsonl")).await;
        let mut disabled = policy();
        disabled.enabled = false;
        let candidates = DeterministicExtractor
            .extract(&evidence("Remember this: x"), &disabled)
            .await;
        let report = commit_candidates(&memory.repository(), candidates, &disabled).await;
        assert!(report.committed.is_empty());
        assert!(report.rejected.iter().all(|r| *r == Rejection::Disabled));
        assert_eq!(memory.all().await.len(), 0);
    }

    #[tokio::test]
    async fn duplicates_are_detected_not_appended() {
        let dir = tempfile::tempdir().unwrap();
        let memory = workspace_memory(&dir.path().join("m.jsonl")).await;
        let candidates = DeterministicExtractor
            .extract(&evidence("Remember this: the gate runs clippy"), &policy())
            .await;
        let first = commit_candidates(&memory.repository(), candidates.clone(), &policy()).await;
        assert_eq!(first.committed.len(), 1);
        let second = commit_candidates(&memory.repository(), candidates, &policy()).await;
        assert!(second.committed.is_empty());
        assert_eq!(second.duplicates, 1);
        assert_eq!(memory.all().await.len(), 1);
    }

    #[tokio::test]
    async fn kind_allowlist_and_cap_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let memory = workspace_memory(&dir.path().join("m.jsonl")).await;
        let mut restricted = policy();
        restricted.auto_kinds = vec![MemoryKind::Preference];
        let candidates = DeterministicExtractor
            .extract(&evidence("We decided to use tokio"), &restricted)
            .await;
        // Decision is not allowlisted: the extractor already skips it.
        assert!(candidates.is_empty());
        let mut capped = policy();
        capped.max_per_turn = 1;
        let many = vec![
            DeterministicExtractor
                .extract(&evidence("Remember this: one"), &capped)
                .await
                .pop()
                .unwrap(),
            DeterministicExtractor
                .extract(&evidence("Remember this: two"), &capped)
                .await
                .pop()
                .unwrap(),
        ];
        let report = commit_candidates(&memory.repository(), many, &capped).await;
        assert_eq!(report.committed.len(), 1);
        assert!(report.rejected.contains(&Rejection::CapReached));
    }

    #[tokio::test]
    async fn commit_failures_are_observable_and_typed() {
        struct FailGate;
        impl hostkit::durable::CommitGate for FailGate {
            fn pass(
                &self,
                stage: hostkit::durable::IoStage,
                _path: &std::path::Path,
            ) -> std::io::Result<()> {
                if stage == hostkit::durable::IoStage::Replace {
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
        let memory = workspace_memory(&dir.path().join("m.jsonl"))
            .await
            .with_commit_gate(std::sync::Arc::new(FailGate));
        let candidates = DeterministicExtractor
            .extract(&evidence("Remember this: durable"), &policy())
            .await;
        let report = commit_candidates(&memory.repository(), candidates, &policy()).await;
        assert!(report.committed.is_empty());
        assert!(report.failure.is_some());
        assert!(report.rejected.contains(&Rejection::Failed));
        assert_eq!(
            memory.all().await.len(),
            0,
            "nothing uncommitted is visible"
        );
    }

    #[tokio::test]
    async fn contradictory_candidates_are_flagged_not_blindly_appended() {
        let dir = tempfile::tempdir().unwrap();
        let memory = workspace_memory(&dir.path().join("m.jsonl")).await;
        let mut first = DeterministicExtractor
            .extract(
                &evidence("Remember this: the deploy target is staging"),
                &policy(),
            )
            .await
            .pop()
            .unwrap();
        first.subject = Some("deploy-target".to_string());
        let first_report = commit_candidates(&memory.repository(), vec![first], &policy()).await;
        assert_eq!(first_report.committed.len(), 1);
        let mut second = DeterministicExtractor
            .extract(
                &evidence("Remember this: the deploy target is production"),
                &policy(),
            )
            .await
            .pop()
            .unwrap();
        second.subject = Some("deploy-target".to_string());
        let report = commit_candidates(&memory.repository(), vec![second], &policy()).await;
        assert_eq!(
            report.committed.len(),
            1,
            "the contradiction is kept for review"
        );
        assert_eq!(
            report.contradictions, first_report.committed,
            "and flagged, not silent"
        );
        let records = memory.repository().records().await.unwrap();
        let newer = records
            .iter()
            .find(|r| r.id == report.committed[0])
            .unwrap();
        assert_eq!(newer.relations.contradicts, first_report.committed);
    }

    #[tokio::test]
    async fn every_committed_candidate_carries_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let memory = workspace_memory(&dir.path().join("m.jsonl")).await;
        let candidates = DeterministicExtractor
            .extract(&evidence("Remember this: provenance matters"), &policy())
            .await;
        let report = commit_candidates(&memory.repository(), candidates, &policy()).await;
        let records = memory.repository().records().await.unwrap();
        let committed = records
            .iter()
            .find(|r| r.id == report.committed[0])
            .unwrap();
        assert_eq!(committed.provenance.session.as_deref(), Some("s1"));
        assert_eq!(committed.provenance.turn.as_deref(), Some("t1"));
        assert!(
            committed
                .provenance
                .evidence
                .iter()
                .any(|e| e == "turn:s1/t1")
        );
    }
}
