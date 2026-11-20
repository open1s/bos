//! Atomic, generation-stamped evidence index with explicit invalidation.
//!
//! TS-03D/TS-03E: an index holds generic evidence per source file, keyed by
//! workspace-relative path, source fingerprint, provider identity, and query
//! identity. Updates are prepared off to the side, validated as a whole, and
//! published by swapping a consistent snapshot. A validation failure never
//! publishes a mixed generation: the last-known-good snapshot is retained and
//! the affected paths are marked stale against the current source.
//!
//! Nothing here persists evidence, selects a storage format, or touches a
//! filesystem. It is an in-memory contract the embedding host may feed from
//! [`crate::WorkspaceFs`] scans.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::WorkspaceId;
use crate::evidence::{
    EvidenceBatch, EvidenceCoverage, EvidenceDiagnostic, EvidenceRecord, ProviderIdentity,
    QueryIdentity, SourceFingerprint,
};

/// Hard ceiling on source files retained by one index.
pub const MAX_INDEX_FILES: usize = 200_000;

/// Hard ceiling on evidence records retained for one source file.
pub const MAX_RECORDS_PER_FILE: usize = 2_000_000;

/// Hard ceiling on evidence records retained across one index.
pub const MAX_TOTAL_RECORDS: u64 = 20_000_000;

/// Documented limits for one evidence index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexLimits {
    /// Maximum retained source files; 1 through [`MAX_INDEX_FILES`].
    pub max_files: usize,
    /// Maximum evidence records for one file; 1 through [`MAX_RECORDS_PER_FILE`].
    pub max_records_per_file: usize,
    /// Maximum evidence records across the whole index; 1 through
    /// [`MAX_TOTAL_RECORDS`].
    pub max_total_records: u64,
}

impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_records_per_file: 100_000,
            max_total_records: 2_000_000,
        }
    }
}

impl IndexLimits {
    /// Validate every limit against its documented range.
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.max_files == 0 || self.max_files > MAX_INDEX_FILES {
            return Err(IndexError::InvalidLimit {
                field: "max_files",
                value: self.max_files as u64,
                max: MAX_INDEX_FILES as u64,
            });
        }
        if self.max_records_per_file == 0 || self.max_records_per_file > MAX_RECORDS_PER_FILE {
            return Err(IndexError::InvalidLimit {
                field: "max_records_per_file",
                value: self.max_records_per_file as u64,
                max: MAX_RECORDS_PER_FILE as u64,
            });
        }
        if self.max_total_records == 0 || self.max_total_records > MAX_TOTAL_RECORDS {
            return Err(IndexError::InvalidLimit {
                field: "max_total_records",
                value: self.max_total_records,
                max: MAX_TOTAL_RECORDS,
            });
        }
        Ok(())
    }
}

/// Why an index operation was rejected.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum IndexError {
    /// One index limit is outside its documented range.
    #[error("invalid index limit `{field}` = {value}: expected 1 through {max}")]
    InvalidLimit {
        /// Rejected field name.
        field: &'static str,
        /// Rejected value.
        value: u64,
        /// Documented ceiling.
        max: u64,
    },
    /// Evidence was prepared for a different workspace.
    #[error("evidence belongs to workspace {actual}, but the index holds {expected}")]
    WorkspaceMismatch {
        /// Workspace identity of the index.
        expected: WorkspaceId,
        /// Workspace identity of the rejected evidence.
        actual: WorkspaceId,
    },
    /// A path is not a normalized workspace-relative path.
    #[error("`{0}` is not a normalized workspace-relative path")]
    InvalidRelativePath(PathBuf),
    /// A record's path disagrees with the file it was filed under.
    #[error("record path `{actual}` disagrees with `{expected}`")]
    RecordPathMismatch {
        /// Path the file entry was published under.
        expected: PathBuf,
        /// Path carried by the record.
        actual: PathBuf,
    },
    /// A record's fingerprint disagrees with the file snapshot fingerprint.
    #[error("record fingerprint `{actual}` disagrees with `{expected}` for `{path}`")]
    RecordFingerprintMismatch {
        /// Path of the rejected record.
        path: PathBuf,
        /// Fingerprint of the file entry.
        expected: String,
        /// Fingerprint carried by the record.
        actual: String,
    },
    /// A record's provider identity disagrees with the file entry.
    #[error("record provider `{actual}` disagrees with `{expected}` for `{path}`")]
    RecordProviderMismatch {
        /// Path of the rejected record.
        path: PathBuf,
        /// Provider of the file entry.
        expected: String,
        /// Provider carried by the record.
        actual: String,
    },
    /// A record's query id disagrees with the slice it was filed under.
    #[error("record query `{actual}` disagrees with `{expected}` for `{path}`")]
    RecordQueryMismatch {
        /// Path of the rejected record.
        path: PathBuf,
        /// Query id of the slice.
        expected: String,
        /// Query id carried by the record.
        actual: String,
    },
    /// A record's byte range is not inside the fingerprinted snapshot.
    #[error("record range {start}..{end} is outside `{path}` ({len} bytes)")]
    RangeOutOfBounds {
        /// Path of the rejected record.
        path: PathBuf,
        /// Range start.
        start: usize,
        /// Range end.
        end: usize,
        /// Snapshot length.
        len: usize,
    },
    /// The update would exceed the retained-file limit.
    #[error("index file limit {max} exceeded (would retain {value})")]
    TooManyFiles {
        /// Retained file count.
        value: usize,
        /// Documented ceiling.
        max: usize,
    },
    /// The update would exceed the per-file record limit.
    #[error("record limit {max} exceeded for `{path}` ({value} records)")]
    TooManyRecords {
        /// Offending path.
        path: PathBuf,
        /// Retained record count.
        value: usize,
        /// Documented ceiling.
        max: usize,
    },
    /// The update would exceed the whole-index record limit.
    #[error("index record limit {max} exceeded (would retain {value})")]
    TooManyTotalRecords {
        /// Retained record count.
        value: u64,
        /// Documented ceiling.
        max: u64,
    },
    /// A duplicate query id appeared twice in one prepared file.
    #[error("query id `{query_id}` appears twice for `{path}`")]
    DuplicateQuery {
        /// Offending path.
        path: PathBuf,
        /// Duplicated query id.
        query_id: String,
    },
    /// The index lock is poisoned by a panicking writer.
    #[error("evidence index lock poisoned")]
    Poisened,
}

/// Evidence produced by one query definition for one source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryEvidence {
    /// Query definition identity that produced the records.
    pub query: QueryIdentity,
    /// Canonically ordered records.
    pub records: Vec<EvidenceRecord>,
    /// Diagnostics raised while producing records for this query.
    pub diagnostics: Vec<EvidenceDiagnostic>,
    /// Worst coverage across the produced records.
    pub coverage: EvidenceCoverage,
}

impl QueryEvidence {
    /// Build a slice from unsorted records, applying canonical ordering.
    #[must_use]
    pub fn new(
        query: QueryIdentity,
        mut records: Vec<EvidenceRecord>,
        mut diagnostics: Vec<EvidenceDiagnostic>,
        coverage: EvidenceCoverage,
    ) -> Self {
        records.sort_by(|a, b| {
            a.pattern_index
                .cmp(&b.pattern_index)
                .then_with(|| a.match_index.cmp(&b.match_index))
                .then_with(|| a.capture.capture_index.cmp(&b.capture.capture_index))
                .then_with(|| a.range.start_byte.cmp(&b.range.start_byte))
                .then_with(|| a.range.end_byte.cmp(&b.range.end_byte))
        });
        diagnostics.sort_by(|a, b| {
            a.relative_path
                .cmp(&b.relative_path)
                .then_with(|| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)))
                .then_with(|| a.message.cmp(&b.message))
        });
        Self {
            query,
            records,
            diagnostics,
            coverage,
        }
    }
}

/// Evidence retained for one source snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEvidence {
    /// Workspace-relative source path.
    pub relative_path: PathBuf,
    /// Fingerprint of the exact source bytes the evidence was produced from.
    pub fingerprint: SourceFingerprint,
    /// Provider identity used to parse the snapshotted source.
    pub provider: ProviderIdentity,
    /// Evidence slices keyed by query id, in deterministic order.
    pub queries: BTreeMap<String, QueryEvidence>,
}

impl FileEvidence {
    /// Build a file entry holding one query result.
    #[must_use]
    pub fn single(
        relative_path: PathBuf,
        fingerprint: SourceFingerprint,
        provider: ProviderIdentity,
        evidence: QueryEvidence,
    ) -> Self {
        let mut queries = BTreeMap::new();
        queries.insert(evidence.query.query_id.clone(), evidence);
        Self {
            relative_path,
            fingerprint,
            provider,
            queries,
        }
    }

    /// Number of retained records across all queries.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.queries.values().map(|slice| slice.records.len()).sum()
    }

    /// Worst coverage across every retained query slice.
    #[must_use]
    pub fn coverage(&self) -> EvidenceCoverage {
        let mut coverage = EvidenceCoverage {
            parse: None,
            query: None,
        };
        for slice in self.queries.values() {
            coverage.merge(slice.coverage);
        }
        coverage
    }

    /// Evidence slice for a query id.
    #[must_use]
    pub fn query(&self, query_id: &str) -> Option<&QueryEvidence> {
        self.queries.get(query_id)
    }
}

/// Why a retained piece of evidence is no longer current.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StaleReason {
    /// The source bytes changed since the evidence was produced.
    SourceChanged,
    /// A re-index of this path failed before publishing.
    ReindexFailed,
    /// The query definition version changed.
    QueryVersionChanged,
    /// Some queries were dropped by a registry reconciliation.
    QueryWithdrawn,
    /// The provider or grammar version changed.
    ProviderVersionChanged,
}

/// Staleness marks for one retained path.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StaleMarks {
    /// Whether the retained evidence is stale relative to the whole source.
    pub source: bool,
    /// Reasons recorded for the path, in declaration order.
    pub reasons: BTreeSet<StaleReason>,
    /// Query slices additionally stale while the file evidence stays current
    /// for other queries.
    pub queries: BTreeSet<String>,
}

impl StaleMarks {
    /// Whether any mark is set.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.source || !self.queries.is_empty() || !self.reasons.is_empty()
    }

    fn mark(&mut self, reason: StaleReason) {
        self.reasons.insert(reason);
        if matches!(
            reason,
            StaleReason::SourceChanged
                | StaleReason::ReindexFailed
                | StaleReason::ProviderVersionChanged
        ) {
            self.source = true;
        }
    }
}

/// What changed between an observed source state and the retained index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    /// The path is new to the index.
    Added,
    /// The source bytes changed.
    Modified,
    /// The source bytes are unchanged and the provider matches.
    Unchanged,
    /// The provider or grammar version changed for the same source bytes.
    ProviderChanged,
    /// The path is retained but is absent from the observation set.
    Removed,
}

/// One path-level change.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FileChange {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// Change classification.
    pub kind: ChangeKind,
}

/// Deterministically ordered set of path changes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeSet {
    changes: Vec<FileChange>,
}

impl ChangeSet {
    /// Changes in path order.
    #[must_use]
    pub fn changes(&self) -> &[FileChange] {
        &self.changes
    }

    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Number of changes of one kind.
    #[must_use]
    pub fn count(&self, kind: ChangeKind) -> usize {
        self.changes.iter().filter(|c| c.kind == kind).count()
    }

    /// Paths that must be re-indexed, in path order.
    #[must_use]
    pub fn paths_to_reindex(&self) -> Vec<&Path> {
        self.changes
            .iter()
            .filter(|change| {
                matches!(
                    change.kind,
                    ChangeKind::Added | ChangeKind::Modified | ChangeKind::ProviderChanged
                )
            })
            .map(|change| change.path.as_path())
            .collect()
    }

    fn push(&mut self, path: PathBuf, kind: ChangeKind) {
        self.changes.push(FileChange { path, kind });
    }

    fn sort(&mut self) {
        self.changes.sort();
    }
}

/// One observed source file, as supplied by the host after a bounded read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceObservation {
    /// Workspace-relative path.
    pub relative_path: PathBuf,
    /// Fingerprint of the exact bytes just read.
    pub fingerprint: SourceFingerprint,
    /// Provider identity that would parse the current bytes.
    pub provider: ProviderIdentity,
}

/// Error category produced while preparing one file for publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreparedFailure {
    /// Validation failed; the path is retained but marked stale.
    Rejected(IndexError),
}

/// Evidence prepared for one file, replacing any retained entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileUpdate {
    /// Replacing entry.
    pub file: FileEvidence,
}

/// One atomic publication attempt.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexUpdate {
    /// Files to insert or replace.
    pub upserts: Vec<FileEvidence>,
    /// Paths to drop entirely.
    pub removals: Vec<PathBuf>,
}

impl IndexUpdate {
    /// Empty update.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file entry, replacing any entry for the same path.
    #[must_use]
    pub fn with_file(mut self, file: FileEvidence) -> Self {
        self.upserts.push(file);
        self
    }

    /// Add a removal.
    #[must_use]
    pub fn with_removal(mut self, path: impl Into<PathBuf>) -> Self {
        self.removals.push(path.into());
        self
    }

    /// Whether the update carries no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.upserts.is_empty() && self.removals.is_empty()
    }
}

/// Result of a published generation swap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Publication {
    /// Generation number of the newly published snapshot.
    pub generation: u64,
    /// Files inserted for the first time.
    pub added: u64,
    /// Files replaced.
    pub replaced: u64,
    /// Files dropped.
    pub removed: u64,
    /// Records retained in the published snapshot.
    pub records: u64,
}

/// Consistent, immutable view of one index generation.
#[derive(Clone, Debug)]
pub struct IndexSnapshot {
    workspace_id: WorkspaceId,
    generation: u64,
    limits: IndexLimits,
    files: Arc<BTreeMap<PathBuf, FileEvidence>>,
    stale: Arc<BTreeMap<PathBuf, StaleMarks>>,
}

impl IndexSnapshot {
    /// Workspace identity of the snapshot.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Monotonic generation number; ``0`` is the empty initial snapshot.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Limits the snapshot was built under.
    #[must_use]
    pub const fn limits(&self) -> IndexLimits {
        self.limits
    }

    /// Retained files in path order.
    #[must_use]
    pub fn files(&self) -> &BTreeMap<PathBuf, FileEvidence> {
        &self.files
    }

    /// Retained evidence for one path.
    #[must_use]
    pub fn file(&self, path: &Path) -> Option<&FileEvidence> {
        self.files.get(path)
    }

    /// Staleness marks in path order.
    #[must_use]
    pub fn stale(&self) -> &BTreeMap<PathBuf, StaleMarks> {
        &self.stale
    }

    /// Staleness marks for one path.
    #[must_use]
    pub fn stale_marks(&self, path: &Path) -> Option<&StaleMarks> {
        self.stale.get(path)
    }

    /// Whether evidence for a path is stale for any reason.
    #[must_use]
    pub fn is_stale(&self, path: &Path) -> bool {
        self.stale.get(path).is_some_and(StaleMarks::is_stale)
    }

    /// Number of retained records.
    #[must_use]
    pub fn record_count(&self) -> u64 {
        self.files
            .values()
            .map(|file| file.record_count() as u64)
            .sum()
    }

    /// Every retained record for one query id, in canonical order.
    #[must_use]
    pub fn records_for_query(&self, query_id: &str) -> Vec<&EvidenceRecord> {
        self.files
            .values()
            .filter_map(|file| file.query(query_id))
            .flat_map(|slice| slice.records.iter())
            .collect()
    }

    /// Every retained record, in canonical path/query order.
    #[must_use]
    pub fn all_records(&self) -> Vec<&EvidenceRecord> {
        self.files
            .values()
            .flat_map(|file| file.queries.values())
            .flat_map(|slice| slice.records.iter())
            .collect()
    }

    /// Copy this snapshot into a deterministic, source-free evidence batch.
    #[must_use]
    pub fn to_batch(&self) -> EvidenceBatch {
        let mut batch = EvidenceBatch::new(self.workspace_id);
        for file in self.files.values() {
            for slice in file.queries.values() {
                batch.records.extend(slice.records.iter().cloned());
                batch
                    .diagnostics
                    .extend(slice.diagnostics.iter().cloned());
                batch.files_queried += 1;
                batch.query_executions += 1;
            }
        }
        let mut coverage = EvidenceCoverage {
            parse: None,
            query: None,
        };
        for file in self.files.values() {
            coverage.merge(file.coverage());
        }
        batch.coverage = coverage;
        batch.sort_canonical();
        batch
    }
}

#[derive(Debug)]
struct State {
    generation: u64,
    files: Arc<BTreeMap<PathBuf, FileEvidence>>,
    stale: Arc<BTreeMap<PathBuf, StaleMarks>>,
}

/// In-memory evidence index with atomic generation publication.
///
/// Reads clone an [`Arc`] under a short read lock, so a reader never observes a
/// half-applied update no matter how many writers race.
#[derive(Debug)]
pub struct EvidenceIndex {
    workspace_id: WorkspaceId,
    limits: IndexLimits,
    state: RwLock<State>,
}

impl EvidenceIndex {
    /// Create an empty generation-zero index.
    pub fn new(workspace_id: WorkspaceId, limits: IndexLimits) -> Result<Self, IndexError> {
        limits.validate()?;
        Ok(Self {
            workspace_id,
            limits,
            state: RwLock::new(State {
                generation: 0,
                files: Arc::new(BTreeMap::new()),
                stale: Arc::new(BTreeMap::new()),
            }),
        })
    }

    /// Workspace identity this index serves.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Limits this index enforces.
    #[must_use]
    pub const fn limits(&self) -> IndexLimits {
        self.limits
    }

    fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, State>, IndexError> {
        self.state.read().map_err(|_| IndexError::Poisened)
    }

    /// Current generation-stamped snapshot.
    pub fn snapshot(&self) -> Result<IndexSnapshot, IndexError> {
        let state = self.read()?;
        Ok(IndexSnapshot {
            workspace_id: self.workspace_id,
            generation: state.generation,
            limits: self.limits,
            files: Arc::clone(&state.files),
            stale: Arc::clone(&state.stale),
        })
    }

    /// Current generation number.
    pub fn generation(&self) -> Result<u64, IndexError> {
        Ok(self.read()?.generation)
    }

    /// Compare observed source state against the retained index.
    ///
    /// Observations are assumed to be the complete set produced by one bounded
    /// scan. Paths retained by the index but absent here are reported as
    /// [`ChangeKind::Removed`].
    pub fn plan(&self, observations: &[SourceObservation]) -> Result<ChangeSet, IndexError> {
        for observation in observations {
            if !crate::syntax::valid_relative_path(&observation.relative_path) {
                return Err(IndexError::InvalidRelativePath(
                    observation.relative_path.clone(),
                ));
            }
        }
        let snapshot = self.snapshot()?;
        let mut changes = ChangeSet::default();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        for observation in observations {
            let path = observation.relative_path.clone();
            if !seen.insert(path.clone()) {
                continue;
            }
            match snapshot.file(&path) {
                None => changes.push(path, ChangeKind::Added),
                Some(file) if file.fingerprint != observation.fingerprint => {
                    changes.push(path, ChangeKind::Modified);
                }
                Some(file) if file.provider != observation.provider => {
                    changes.push(path, ChangeKind::ProviderChanged);
                }
                Some(_) => changes.push(path, ChangeKind::Unchanged),
            }
        }
        for path in snapshot.files().keys() {
            if !seen.contains(path) {
                changes.push(path.clone(), ChangeKind::Removed);
            }
        }
        changes.sort();
        Ok(changes)
    }

    /// Validate and publish one update as a new generation.
    ///
    /// Every entry is validated before the swap, so a rejected update leaves
    /// the published generation untouched. Paths named by the rejected update
    /// that already exist are additionally marked stale with
    /// [`StaleReason::ReindexFailed`].
    pub fn apply(&self, update: IndexUpdate) -> Result<Publication, IndexError> {
        let mut prepared = self.validate_update(&update)?;
        let mut state = self.state.write().map_err(|_| IndexError::Poisened)?;

        let mut files: BTreeMap<PathBuf, FileEvidence> = (*state.files).clone();
        let mut added = 0u64;
        let mut replaced = 0u64;
        for file in prepared.upserts.drain(..) {
            let path = file.relative_path.clone();
            if files.insert(path.clone(), file).is_some() {
                replaced += 1;
            } else {
                added += 1;
            }
            prepared.stale.remove(&path);
        }
        let mut removed = 0u64;
        for path in prepared.removals.drain(..) {
            if files.remove(&path).is_some() {
                removed += 1;
            }
            prepared.stale.remove(&path);
        }

        let mut stale: BTreeMap<PathBuf, StaleMarks> = (*state.stale).clone();
        for (path, marks) in prepared.stale {
            stale.insert(path, marks);
        }
        stale.retain(|path, marks| files.contains_key(path) && marks.is_stale());

        let records: u64 = files.values().map(|file| file.record_count() as u64).sum();
        if records > self.limits.max_total_records {
            return Err(self.reject_retained(files, &update));
        }
        if files.len() > self.limits.max_files {
            return Err(IndexError::TooManyFiles {
                value: files.len(),
                max: self.limits.max_files,
            });
        }

        state.generation = state.generation.saturating_add(1);
        state.files = Arc::new(files);
        state.stale = Arc::new(stale);

        Ok(Publication {
            generation: state.generation,
            added,
            replaced,
            removed,
            records,
        })
    }

    fn reject_retained(
        &self,
        files: BTreeMap<PathBuf, FileEvidence>,
        update: &IndexUpdate,
    ) -> IndexError {
        let records: u64 = files.values().map(|file| file.record_count() as u64).sum();
        if records > self.limits.max_total_records {
            let _ = update.upserts.len();
            return IndexError::TooManyTotalRecords {
                value: records,
                max: self.limits.max_total_records,
            };
        }
        IndexError::TooManyFiles {
            value: files.len(),
            max: self.limits.max_files,
        }
    }

    fn validate_update(&self, update: &IndexUpdate) -> Result<ValidatedUpdate, IndexError> {
        let mut seen_paths: BTreeSet<PathBuf> = BTreeSet::new();
        let mut seen_removals: BTreeSet<PathBuf> = BTreeSet::new();
        let snapshot = self.snapshot()?;
        let mut upserts = Vec::with_capacity(update.upserts.len());
        let mut removals = Vec::with_capacity(update.removals.len());
        let mut stale = BTreeMap::new();

        for file in &update.upserts {
            let outcome = self.validate_file(file);
            match outcome {
                Ok(()) => {
                    if !seen_paths.insert(file.relative_path.clone()) {
                        let error = IndexError::DuplicateQuery {
                            path: file.relative_path.clone(),
                            query_id: file
                                .queries
                                .keys()
                                .next()
                                .cloned()
                                .unwrap_or_default(),
                        };
                        if snapshot.file(&file.relative_path).is_some() {
                            stale.insert(
                                file.relative_path.clone(),
                                marks_of(&snapshot, &file.relative_path, StaleReason::ReindexFailed),
                            );
                        }
                        return Err(error);
                    }
                    upserts.push(file.clone());
                }
                Err(error) => {
                    if snapshot.file(&file.relative_path).is_some() {
                        stale.insert(
                            file.relative_path.clone(),
                            marks_of(&snapshot, &file.relative_path, StaleReason::ReindexFailed),
                        );
                    }
                    return Err(error);
                }
            }
        }

        for path in &update.removals {
            if !crate::syntax::valid_relative_path(path) {
                return Err(IndexError::InvalidRelativePath(path.clone()));
            }
            if seen_removals.insert(path.clone()) {
                removals.push(path.clone());
            }
        }

        let retained_after: u64 = {
            let current: u64 = snapshot.record_count();
            let dropped: u64 = removals
                .iter()
                .filter_map(|path| snapshot.file(path))
                .map(|file| file.record_count() as u64)
                .sum();
            let replaced: u64 = upserts
                .iter()
                .filter_map(|file| snapshot.file(&file.relative_path))
                .map(|existing| existing.record_count() as u64)
                .sum();
            let fresh: u64 = upserts
                .iter()
                .map(|file| file.record_count() as u64)
                .sum();
            current.saturating_sub(dropped).saturating_sub(replaced).saturating_add(fresh)
        };

        if retained_after > self.limits.max_total_records {
            return Err(IndexError::TooManyTotalRecords {
                value: retained_after,
                max: self.limits.max_total_records,
            });
        }

        let retained_files = {
            let mut count = snapshot.files().len();
            for path in &removals {
                if snapshot.file(path).is_some() {
                    count = count.saturating_sub(1);
                }
            }
            for file in &upserts {
                if snapshot.file(&file.relative_path).is_none() {
                    count += 1;
                }
            }
            count
        };
        if retained_files > self.limits.max_files {
            return Err(IndexError::TooManyFiles {
                value: retained_files,
                max: self.limits.max_files,
            });
        }

        Ok(ValidatedUpdate {
            upserts,
            removals,
            stale,
        })
    }

    fn validate_file(&self, file: &FileEvidence) -> Result<(), IndexError> {
        if !crate::syntax::valid_relative_path(&file.relative_path) {
            return Err(IndexError::InvalidRelativePath(file.relative_path.clone()));
        }
        if file.record_count() > self.limits.max_records_per_file {
            return Err(IndexError::TooManyRecords {
                path: file.relative_path.clone(),
                value: file.record_count(),
                max: self.limits.max_records_per_file,
            });
        }
        for (query_id, slice) in &file.queries {
            if slice.query.query_id != *query_id {
                return Err(IndexError::RecordQueryMismatch {
                    path: file.relative_path.clone(),
                    expected: query_id.clone(),
                    actual: slice.query.query_id.clone(),
                });
            }
            for record in &slice.records {
                if record.workspace_id != self.workspace_id {
                    return Err(IndexError::WorkspaceMismatch {
                        expected: self.workspace_id,
                        actual: record.workspace_id,
                    });
                }
                if record.relative_path != file.relative_path {
                    return Err(IndexError::RecordPathMismatch {
                        expected: file.relative_path.clone(),
                        actual: record.relative_path.clone(),
                    });
                }
                if record.fingerprint != file.fingerprint {
                    return Err(IndexError::RecordFingerprintMismatch {
                        path: file.relative_path.clone(),
                        expected: file.fingerprint.hex(),
                        actual: record.fingerprint.hex(),
                    });
                }
                if record.provider != file.provider {
                    return Err(IndexError::RecordProviderMismatch {
                        path: file.relative_path.clone(),
                        expected: record.provider.provider_id.clone(),
                        actual: file.provider.provider_id.clone(),
                    });
                }
                if record.range.end_byte > file.fingerprint.source_len()
                    || record.range.start_byte > record.range.end_byte
                {
                    return Err(IndexError::RangeOutOfBounds {
                        path: file.relative_path.clone(),
                        start: record.range.start_byte,
                        end: record.range.end_byte,
                        len: file.fingerprint.source_len(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Mark a path stale against the current source without dropping evidence.
    pub fn mark_source_changed(&self, path: &Path) -> Result<(), IndexError> {
        let mut state = self.state.write().map_err(|_| IndexError::Poisened)?;
        let mut stale = (*state.stale).clone();
        let mut marks = stale.remove(path).unwrap_or_default();
        marks.mark(StaleReason::SourceChanged);
        stale.insert(path.to_path_buf(), marks);
        state.stale = Arc::new(stale);
        Ok(())
    }

    /// Drop evidence produced by a superseded query definition version.
    ///
    /// Only that query id is affected; evidence from other queries stays
    /// current. Affected files are marked stale for that query id only.
    pub fn reconcile_query_version(
        &self,
        query_id: &str,
        current_version: &str,
    ) -> Result<u64, IndexError> {
        self.reconcile_queries(std::slice::from_ref(
            &QueryIdentity {
                query_id: query_id.to_owned(),
                query_version: current_version.to_owned(),
            },
        ))
    }

    /// Drop evidence for any query whose identity is absent from `current`.
    pub fn reconcile_queries(&self, current: &[QueryIdentity]) -> Result<u64, IndexError> {
        let valid: BTreeMap<&str, &str> = current
            .iter()
            .map(|identity| (identity.query_id.as_str(), identity.query_version.as_str()))
            .collect();
        let mut state = self.state.write().map_err(|_| IndexError::Poisened)?;
        let mut files: BTreeMap<PathBuf, FileEvidence> = (*state.files).clone();
        let mut stale: BTreeMap<PathBuf, StaleMarks> = (*state.stale).clone();
        let mut dropped = 0u64;
        for (path, file) in files.iter_mut() {
            let mut removed_queries: Vec<String> = Vec::new();
            file.queries.retain(|query_id, slice| {
                let keep = valid
                    .get(query_id.as_str())
                    .is_some_and(|version| *version == slice.query.query_version.as_str());
                if !keep {
                    removed_queries.push(query_id.clone());
                    dropped += slice.records.len() as u64;
                }
                keep
            });
            for query_id in removed_queries {
                let marks = stale.entry(path.clone()).or_default();
                marks.queries.insert(query_id);
                marks.reasons.insert(StaleReason::QueryVersionChanged);
            }
        }
        files.retain(|_, file| !file.queries.is_empty());
        stale.retain(|path, _| files.contains_key(path) && stale_or_kept(files.get(path)));

        state.generation = state.generation.saturating_add(1);
        state.files = Arc::new(files);
        state.stale = Arc::new(stale);
        Ok(dropped)
    }

    /// Drop all evidence produced by a provider version that is no longer
    /// installed, marking its paths stale.
    pub fn invalidate_provider(&self, provider_id: &str, current_version: &str) -> Result<u64, IndexError> {
        let mut state = self.state.write().map_err(|_| IndexError::Poisened)?;
        let mut files: BTreeMap<PathBuf, FileEvidence> = (*state.files).clone();
        let mut stale: BTreeMap<PathBuf, StaleMarks> = (*state.stale).clone();
        let mut dropped = 0u64;
        let mut invalid: Vec<PathBuf> = Vec::new();
        for (path, file) in files.iter() {
            if file.provider.provider_id == provider_id
                && file.provider.provider_version != current_version
            {
                invalid.push(path.clone());
                dropped += file.record_count() as u64;
            }
        }
        for path in invalid {
            files.remove(&path);
            let marks = stale.entry(path).or_default();
            marks.reasons.insert(StaleReason::ProviderVersionChanged);
            marks.source = true;
        }
        stale.retain(|path, marks| marks.is_stale() || files.contains_key(path));

        state.generation = state.generation.saturating_add(1);
        state.files = Arc::new(files);
        state.stale = Arc::new(stale);
        Ok(dropped)
    }
}

fn stale_or_kept(file: Option<&FileEvidence>) -> bool {
    file.is_some()
}

fn marks_of(snapshot: &IndexSnapshot, path: &Path, reason: StaleReason) -> StaleMarks {
    let mut marks = snapshot.stale_marks(path).cloned().unwrap_or_default();
    marks.mark(reason);
    marks
}

struct ValidatedUpdate {
    upserts: Vec<FileEvidence>,
    removals: Vec<PathBuf>,
    stale: BTreeMap<PathBuf, StaleMarks>,
}
