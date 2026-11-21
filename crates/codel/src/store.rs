//! Durable, workspace-scoped persistence for one evidence index generation.
//!
//! This is deliberately a separate layer: [`EvidenceIndex`] stays an in-memory
//! contract and knows nothing about files or formats. The store owns the durable
//! representation.
//!
//! # Isolation
//!
//! Each store root contains one directory per host-supplied [`WorkspaceId`]:
//! `<root>/<workspace-id>/`. Two Git worktrees of the same repository therefore
//! have separate snapshots even when they share one store root, and a snapshot
//! whose embedded identity disagrees with the expected workspace is rejected.
//!
//! # Durability
//!
//! A save writes a checksummed envelope to a temporary file, fsyncs it, moves the
//! current snapshot to `snapshot.prev.json`, and renames the temporary file into
//! place. A failed or interrupted save never replaces a readable snapshot with a
//! partial one. On load, a corrupt current generation falls back to the previous
//! one and reports [`Recovery::PreviousGeneration`].
//!
//! # What is persisted
//!
//! Evidence only: workspace-relative paths, source fingerprints, provider and
//! query identities, byte ranges, captures, coverage, and staleness. Source text
//! is never written. Diagnostics are regenerated on the next index pass rather
//! than persisted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::WorkspaceId;
use crate::classification::LanguageId;
use crate::evidence::{
    CaptureIdentity, EvidenceCoverage, EvidenceRecord, FingerprintAlgorithm, ParseCoverage,
    ProviderIdentity, QueryCoverage, QueryIdentity, SourceFingerprint,
};
use crate::evidence_index::{
    EvidenceIndex, FileEvidence, IndexError, IndexLimits, IndexSnapshot, QueryEvidence, StaleMarks,
    StaleReason,
};
use crate::graph::{CodeGraph, GraphSnapshot};
use crate::syntax::query::QueryOutputLimit;
use crate::syntax::{SourceRange, valid_relative_path};

/// Format version written by this store.
pub const STORE_SCHEMA_VERSION: u32 = 1;

const CURRENT_FILE: &str = "snapshot.json";
const PREVIOUS_FILE: &str = "snapshot.prev.json";
const TEMP_FILE: &str = "snapshot.json.tmp";

const GRAPH_FILE: &str = "graph.json";
const GRAPH_PREVIOUS_FILE: &str = "graph.prev.json";
const GRAPH_TEMP_FILE: &str = "graph.json.tmp";

/// Absolute ceiling on one serialized snapshot file.
pub const MAX_STORE_BYTES: u64 = 512 * 1024 * 1024;

/// Durable store rooted at a host-managed metadata directory.
#[derive(Clone, Debug)]
pub struct IndexStore {
    root: PathBuf,
    workspace_id: WorkspaceId,
    limits: IndexLimits,
}

impl IndexStore {
    /// Bind a store root to one workspace identity.
    ///
    /// `limits` is used only for an empty workspace; a persisted snapshot keeps
    /// the limits it was validated under.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, workspace_id: WorkspaceId, limits: IndexLimits) -> Self {
        Self {
            root: root.into(),
            workspace_id,
            limits,
        }
    }

    /// This workspace's isolated snapshot directory.
    #[must_use]
    pub fn workspace_dir(&self) -> PathBuf {
        self.root.join(self.workspace_id.to_string())
    }

    /// Persist one generated snapshot atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::WorkspaceMismatch`] when the snapshot belongs to a
    /// different workspace, and [`StoreError::Io`] when the directory, temporary
    /// file, fsync, or rename fails.
    pub fn save(&self, snapshot: &IndexSnapshot) -> Result<(), StoreError> {
        if snapshot.workspace_id() != self.workspace_id {
            return Err(StoreError::WorkspaceMismatch {
                found: snapshot.workspace_id().to_string(),
                expected: self.workspace_id.to_string(),
            });
        }
        snapshot.limits().validate()?;

        let stored = StoredSnapshot::from_snapshot(snapshot);
        let payload = serde_json::to_vec(&stored)?;
        let envelope = StoredEnvelope {
            schema_version: STORE_SCHEMA_VERSION,
            checksum: hex_digest(&payload),
            snapshot: stored,
        };
        let bytes = serde_json::to_vec(&envelope)?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(StoreError::TooLarge {
                bytes: bytes.len() as u64,
                max: MAX_STORE_BYTES,
            });
        }

        let directory = self.workspace_dir();
        fs::create_dir_all(&directory)?;
        let temporary = directory.join(TEMP_FILE);
        let current = directory.join(CURRENT_FILE);
        let previous = directory.join(PREVIOUS_FILE);

        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }

        if current.exists() {
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&current, &previous)?;
        }
        fs::rename(&temporary, &current)?;
        sync_directory(&directory)?;
        Ok(())
    }

    /// Load the newest readable generation for this workspace.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] when a snapshot exists but neither the
    /// current nor the previous generation can be read and validated.
    pub fn load(&self) -> Result<LoadedIndex, StoreError> {
        let directory = self.workspace_dir();
        let current = directory.join(CURRENT_FILE);
        let previous = directory.join(PREVIOUS_FILE);

        let mut current_error = None;
        if current.is_file() {
            match self.read_index(&current) {
                Ok(index) => {
                    return Ok(LoadedIndex {
                        index,
                        recovery: Recovery::Clean,
                    });
                }
                Err(error) => current_error = Some(error),
            }
        }

        let mut previous_error = None;
        if previous.is_file() {
            match self.read_index(&previous) {
                Ok(index) => {
                    return Ok(LoadedIndex {
                        index,
                        recovery: Recovery::PreviousGeneration,
                    });
                }
                Err(error) => previous_error = Some(error),
            }
        }

        if current_error.is_none() && previous_error.is_none() {
            let index = EvidenceIndex::new(self.workspace_id, self.limits)?;
            return Ok(LoadedIndex {
                index,
                recovery: Recovery::Empty,
            });
        }

        Err(StoreError::Corrupt(format!(
            "current: {}; previous: {}",
            describe(current_error),
            describe(previous_error),
        )))
    }

    fn read_index(&self, path: &Path) -> Result<EvidenceIndex, StoreError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_STORE_BYTES {
            return Err(StoreError::TooLarge {
                bytes: metadata.len(),
                max: MAX_STORE_BYTES,
            });
        }
        let bytes = fs::read(path)?;
        let envelope: StoredEnvelope = serde_json::from_slice(&bytes)?;
        if envelope.schema_version != STORE_SCHEMA_VERSION {
            return Err(StoreError::Schema {
                found: envelope.schema_version,
                expected: STORE_SCHEMA_VERSION,
            });
        }
        let payload = serde_json::to_vec(&envelope.snapshot)?;
        if hex_digest(&payload) != envelope.checksum {
            return Err(StoreError::Checksum);
        }
        envelope.snapshot.into_index(self.workspace_id)
    }
}

/// Durable store for one workspace's candidate graph.
#[derive(Clone, Debug)]
pub struct GraphStore {
    root: PathBuf,
    workspace_id: WorkspaceId,
}

impl GraphStore {
    /// Bind a graph store root to one workspace identity.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, workspace_id: WorkspaceId) -> Self {
        Self {
            root: root.into(),
            workspace_id,
        }
    }

    /// This workspace's isolated graph directory.
    #[must_use]
    pub fn workspace_dir(&self) -> PathBuf {
        self.root.join(self.workspace_id.to_string())
    }

    /// Persist one graph snapshot atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::WorkspaceMismatch`] when the graph belongs to a
    /// different workspace, and I/O or validation errors from the store.
    pub fn save(&self, graph: &CodeGraph) -> Result<(), StoreError> {
        self.save_with_digest(graph, "")
    }

    /// Persist one graph snapshot together with a source manifest digest.
    ///
    /// # Errors
    ///
    /// Returns StoreError::WorkspaceMismatch when the graph belongs to a
    /// different workspace, and I/O or validation errors from the store.
    pub fn save_with_digest(
        &self,
        graph: &CodeGraph,
        source_digest: &str,
    ) -> Result<(), StoreError> {
        self.save_with_digest_and_metadata(graph, source_digest, &BTreeMap::new())
    }

    /// Persist one graph snapshot with source and enrichment metadata.
    ///
    /// # Errors
    ///
    /// Returns StoreError::WorkspaceMismatch when the graph belongs to a
    /// different workspace, and I/O or validation errors from the store.
    pub fn save_with_digest_and_metadata(
        &self,
        graph: &CodeGraph,
        source_digest: &str,
        metadata: &BTreeMap<String, String>,
    ) -> Result<(), StoreError> {
        if graph.workspace_id() != self.workspace_id {
            return Err(StoreError::WorkspaceMismatch {
                found: graph.workspace_id().to_string(),
                expected: self.workspace_id.to_string(),
            });
        }
        let mut snapshot = graph.to_snapshot();
        snapshot.source_digest = source_digest.to_owned();
        snapshot.metadata = metadata.clone();
        let payload = serde_json::to_vec(&snapshot)?;
        let envelope = GraphEnvelope {
            schema_version: STORE_SCHEMA_VERSION,
            checksum: hex_digest(&payload),
            snapshot,
        };
        let bytes = serde_json::to_vec(&envelope)?;
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(StoreError::TooLarge {
                bytes: bytes.len() as u64,
                max: MAX_STORE_BYTES,
            });
        }

        let directory = self.workspace_dir();
        fs::create_dir_all(&directory)?;
        let temporary = directory.join(GRAPH_TEMP_FILE);
        let current = directory.join(GRAPH_FILE);
        let previous = directory.join(GRAPH_PREVIOUS_FILE);

        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        if current.exists() {
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&current, &previous)?;
        }
        fs::rename(&temporary, &current)?;
        sync_directory(&directory)?;
        Ok(())
    }

    /// Load this workspace's graph, or [`None`] when no snapshot exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] when a snapshot exists but neither the
    /// current nor the previous generation can be read and validated.
    pub fn load(&self) -> Result<Option<CodeGraph>, StoreError> {
        Ok(self.load_with_digest()?.map(|(graph, _digest)| graph))
    }

    /// Load this workspace's graph and its source manifest digest.
    ///
    /// # Errors
    ///
    /// Returns StoreError::Corrupt when a snapshot exists but neither the
    /// current nor the previous generation can be read and validated.
    pub fn load_with_digest(&self) -> Result<Option<(CodeGraph, String)>, StoreError> {
        Ok(self
            .load_with_metadata()?
            .map(|(graph, digest, _metadata)| (graph, digest)))
    }

    /// Load this workspace's graph, digest, and enrichment metadata.
    ///
    /// # Errors
    ///
    /// Returns StoreError::Corrupt when a snapshot exists but neither the
    /// current nor the previous generation can be read and validated.
    #[allow(clippy::type_complexity)]
    pub fn load_with_metadata(
        &self,
    ) -> Result<Option<(CodeGraph, String, BTreeMap<String, String>)>, StoreError> {
        let directory = self.workspace_dir();
        let current = directory.join(GRAPH_FILE);
        let previous = directory.join(GRAPH_PREVIOUS_FILE);

        let mut current_error = None;
        if current.is_file() {
            match self.read_graph_with_metadata(&current) {
                Ok(graph) => return Ok(Some(graph)),
                Err(error) => current_error = Some(error),
            }
        }

        let mut previous_error = None;
        if previous.is_file() {
            match self.read_graph_with_metadata(&previous) {
                Ok(graph) => return Ok(Some(graph)),
                Err(error) => previous_error = Some(error),
            }
        }

        if current_error.is_none() && previous_error.is_none() {
            return Ok(None);
        }
        Err(StoreError::Corrupt(format!(
            "current: {}; previous: {}",
            describe(current_error),
            describe(previous_error),
        )))
    }

    fn read_graph_with_metadata(
        &self,
        path: &Path,
    ) -> Result<(CodeGraph, String, BTreeMap<String, String>), StoreError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() > MAX_STORE_BYTES {
            return Err(StoreError::TooLarge {
                bytes: metadata.len(),
                max: MAX_STORE_BYTES,
            });
        }
        let bytes = fs::read(path)?;
        let envelope: GraphEnvelope = serde_json::from_slice(&bytes)?;
        if envelope.schema_version != STORE_SCHEMA_VERSION {
            return Err(StoreError::Schema {
                found: envelope.schema_version,
                expected: STORE_SCHEMA_VERSION,
            });
        }
        let payload = serde_json::to_vec(&envelope.snapshot)?;
        if hex_digest(&payload) != envelope.checksum {
            return Err(StoreError::Checksum);
        }
        let digest = envelope.snapshot.source_digest.clone();
        let metadata = envelope.snapshot.metadata.clone();
        let graph = CodeGraph::from_snapshot(envelope.snapshot)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if graph.workspace_id() != self.workspace_id {
            return Err(StoreError::WorkspaceMismatch {
                found: graph.workspace_id().to_string(),
                expected: self.workspace_id.to_string(),
            });
        }
        Ok((graph, digest, metadata))
    }
}

/// Which generation a load returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recovery {
    /// The current generation loaded and validated.
    Clean,
    /// The current generation was unreadable; the previous generation loaded.
    PreviousGeneration,
    /// No snapshot existed yet.
    Empty,
}

impl Recovery {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::PreviousGeneration => "previous-generation",
            Self::Empty => "empty",
        }
    }
}

/// One loaded index and how it was obtained.
#[derive(Debug)]
pub struct LoadedIndex {
    /// Rebuilt in-memory index.
    pub index: EvidenceIndex,
    /// Which generation supplied it.
    pub recovery: Recovery,
}

/// A store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The store directory or file could not be read or written.
    #[error("index store I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The stored envelope is not valid JSON.
    #[error("stored index is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The stored format version is not supported.
    #[error("stored index schema {found} is not supported (expected {expected})")]
    Schema {
        /// Version found on disk.
        found: u32,
        /// Version this build writes and accepts.
        expected: u32,
    },
    /// The snapshot belongs to another workspace.
    #[error("stored index belongs to workspace {found}, expected {expected}")]
    WorkspaceMismatch {
        /// Identity found on disk.
        found: String,
        /// Identity the store was opened for.
        expected: String,
    },
    /// The checksum and payload disagree.
    #[error("stored index checksum does not match its payload")]
    Checksum,
    /// The payload is structurally valid but violates an index invariant.
    #[error("stored index failed validation: {0}")]
    Invalid(String),
    /// The snapshot exceeds the documented size ceiling.
    #[error("stored index is {bytes} bytes, exceeding the {max}-byte ceiling")]
    TooLarge {
        /// Serialized size.
        bytes: u64,
        /// Documented ceiling.
        max: u64,
    },
    /// No generation could be read.
    #[error("stored index is corrupt: {0}")]
    Corrupt(String),
    /// The rebuilt index was rejected by the in-memory contract.
    #[error(transparent)]
    Index(#[from] IndexError),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEnvelope {
    schema_version: u32,
    checksum: String,
    snapshot: StoredSnapshot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GraphEnvelope {
    schema_version: u32,
    checksum: String,
    snapshot: GraphSnapshot,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredSnapshot {
    workspace_id: String,
    generation: u64,
    limits: IndexLimits,
    files: Vec<StoredFile>,
    stale: Vec<StoredStale>,
}

impl StoredSnapshot {
    fn from_snapshot(snapshot: &IndexSnapshot) -> Self {
        Self {
            workspace_id: snapshot.workspace_id().to_string(),
            generation: snapshot.generation(),
            limits: snapshot.limits(),
            files: snapshot.files().values().map(StoredFile::from).collect(),
            stale: snapshot
                .stale()
                .iter()
                .map(|(path, marks)| StoredStale::from_parts(path, marks))
                .collect(),
        }
    }

    fn into_index(self, expected: WorkspaceId) -> Result<EvidenceIndex, StoreError> {
        let workspace_id: WorkspaceId = self.workspace_id.parse().map_err(|_| {
            StoreError::Invalid(format!("invalid workspace id '{}'", self.workspace_id))
        })?;
        if workspace_id != expected {
            return Err(StoreError::WorkspaceMismatch {
                found: self.workspace_id,
                expected: expected.to_string(),
            });
        }
        self.limits.validate()?;

        let mut files = BTreeMap::new();
        for stored in self.files {
            let file = stored.into_file(workspace_id, self.limits)?;
            let path = file.relative_path.clone();
            if files.insert(path.clone(), file).is_some() {
                return Err(StoreError::Invalid(format!(
                    "duplicate file entry '{}'",
                    path.display()
                )));
            }
        }

        let mut stale = BTreeMap::new();
        for stored in self.stale {
            let path = stored.path.clone();
            if !valid_relative_path(&path) {
                return Err(StoreError::Invalid(format!(
                    "invalid stale path '{}'",
                    path.display()
                )));
            }
            let marks = stored.into_marks();
            if marks.is_stale() {
                stale.insert(path, marks);
            }
        }

        EvidenceIndex::restore(workspace_id, self.limits, self.generation, files, stale)
            .map_err(StoreError::from)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredFile {
    path: PathBuf,
    algorithm: String,
    digest: Vec<u8>,
    source_len: usize,
    language: LanguageId,
    provider_id: String,
    provider_version: String,
    queries: Vec<StoredQuery>,
}

impl From<&FileEvidence> for StoredFile {
    fn from(file: &FileEvidence) -> Self {
        Self {
            path: file.relative_path.clone(),
            algorithm: file.fingerprint.algorithm().as_str().to_owned(),
            digest: file.fingerprint.digest().to_vec(),
            source_len: file.fingerprint.source_len(),
            language: file.provider.language,
            provider_id: file.provider.provider_id.clone(),
            provider_version: file.provider.provider_version.clone(),
            queries: file.queries.values().map(StoredQuery::from).collect(),
        }
    }
}

impl StoredFile {
    fn into_file(
        self,
        workspace_id: WorkspaceId,
        limits: IndexLimits,
    ) -> Result<FileEvidence, StoreError> {
        if !valid_relative_path(&self.path) {
            return Err(StoreError::Invalid(format!(
                "invalid source path '{}'",
                self.path.display()
            )));
        }
        if self.algorithm != FingerprintAlgorithm::Sha256.as_str() {
            return Err(StoreError::Invalid(format!(
                "unsupported fingerprint algorithm '{}'",
                self.algorithm
            )));
        }
        let digest: [u8; 32] = self.digest.as_slice().try_into().map_err(|_| {
            StoreError::Invalid(format!(
                "fingerprint digest for '{}' is not 32 bytes",
                self.path.display()
            ))
        })?;
        let fingerprint =
            SourceFingerprint::from_parts(FingerprintAlgorithm::Sha256, digest, self.source_len);
        let provider = ProviderIdentity {
            language: self.language,
            provider_id: self.provider_id,
            provider_version: self.provider_version,
        };

        let mut queries = BTreeMap::new();
        for stored in self.queries {
            let query_id = stored.query_id.clone();
            if queries.contains_key(&query_id) {
                return Err(StoreError::Invalid(format!(
                    "duplicate query '{}' for '{}'",
                    query_id,
                    self.path.display()
                )));
            }
            let query = QueryIdentity {
                query_id: query_id.clone(),
                query_version: stored.query_version,
            };
            let coverage = stored.coverage.into();
            let records = stored
                .records
                .into_iter()
                .map(|record| {
                    record.into_record(
                        workspace_id,
                        &self.path,
                        fingerprint,
                        &provider,
                        &query,
                        coverage,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            queries.insert(
                query_id,
                QueryEvidence::new(query, records, Vec::new(), coverage),
            );
        }
        if queries
            .values()
            .any(|query| query.records.len() > limits.max_records_per_file)
        {
            return Err(StoreError::Invalid(format!(
                "record limit exceeded for '{}'",
                self.path.display()
            )));
        }

        Ok(FileEvidence {
            relative_path: self.path,
            fingerprint,
            provider,
            queries,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredQuery {
    query_id: String,
    query_version: String,
    coverage: StoredCoverage,
    records: Vec<StoredRecord>,
}

impl From<&QueryEvidence> for StoredQuery {
    fn from(query: &QueryEvidence) -> Self {
        Self {
            query_id: query.query.query_id.clone(),
            query_version: query.query.query_version.clone(),
            coverage: StoredCoverage::from(query.coverage),
            records: query.records.iter().map(StoredRecord::from).collect(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    capture_index: u32,
    capture_name: String,
    pattern_index: usize,
    match_index: usize,
    start_byte: usize,
    end_byte: usize,
}

impl From<&EvidenceRecord> for StoredRecord {
    fn from(record: &EvidenceRecord) -> Self {
        Self {
            capture_index: record.capture.capture_index,
            capture_name: record.capture.capture_name.clone(),
            pattern_index: record.pattern_index,
            match_index: record.match_index,
            start_byte: record.range.start_byte,
            end_byte: record.range.end_byte,
        }
    }
}

impl StoredRecord {
    fn into_record(
        self,
        workspace_id: WorkspaceId,
        path: &Path,
        fingerprint: SourceFingerprint,
        provider: &ProviderIdentity,
        query: &QueryIdentity,
        coverage: EvidenceCoverage,
    ) -> Result<EvidenceRecord, StoreError> {
        if self.start_byte > self.end_byte || self.end_byte > fingerprint.source_len() {
            return Err(StoreError::Invalid(format!(
                "record range {}..{} is outside '{}' ({} bytes)",
                self.start_byte,
                self.end_byte,
                path.display(),
                fingerprint.source_len()
            )));
        }
        Ok(EvidenceRecord {
            workspace_id,
            relative_path: path.to_path_buf(),
            fingerprint,
            provider: provider.clone(),
            query: query.clone(),
            capture: CaptureIdentity {
                capture_index: self.capture_index,
                capture_name: self.capture_name,
            },
            range: SourceRange {
                start_byte: self.start_byte,
                end_byte: self.end_byte,
            },
            pattern_index: self.pattern_index,
            match_index: self.match_index,
            coverage,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredStale {
    path: PathBuf,
    source: bool,
    reasons: BTreeSet<StaleReason>,
    queries: BTreeSet<String>,
}

impl StoredStale {
    fn from_parts(path: &Path, marks: &StaleMarks) -> Self {
        Self {
            path: path.to_path_buf(),
            source: marks.source,
            reasons: marks.reasons.clone(),
            queries: marks.queries.clone(),
        }
    }

    fn into_marks(self) -> StaleMarks {
        StaleMarks {
            source: self.source,
            reasons: self.reasons,
            queries: self.queries,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCoverage {
    parse: Option<StoredParseCoverage>,
    query: Option<StoredQueryCoverage>,
}

impl From<EvidenceCoverage> for StoredCoverage {
    fn from(coverage: EvidenceCoverage) -> Self {
        Self {
            parse: coverage.parse.map(StoredParseCoverage::from),
            query: coverage.query.map(StoredQueryCoverage::from),
        }
    }
}

impl From<StoredCoverage> for EvidenceCoverage {
    fn from(coverage: StoredCoverage) -> Self {
        Self {
            parse: coverage.parse.map(ParseCoverage::from),
            query: coverage.query.map(QueryCoverage::from),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredParseCoverage {
    Complete,
    Recovered,
}

impl From<ParseCoverage> for StoredParseCoverage {
    fn from(coverage: ParseCoverage) -> Self {
        match coverage {
            ParseCoverage::Complete => Self::Complete,
            ParseCoverage::Recovered => Self::Recovered,
        }
    }
}

impl From<StoredParseCoverage> for ParseCoverage {
    fn from(coverage: StoredParseCoverage) -> Self {
        match coverage {
            StoredParseCoverage::Complete => Self::Complete,
            StoredParseCoverage::Recovered => Self::Recovered,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum StoredQueryCoverage {
    Complete,
    NotExecuted,
    OutputLimited { limit: StoredOutputLimit },
    MatchLimitExceeded,
    Cancelled,
    DeadlineExceeded,
}

impl From<QueryCoverage> for StoredQueryCoverage {
    fn from(coverage: QueryCoverage) -> Self {
        match coverage {
            QueryCoverage::Complete => Self::Complete,
            QueryCoverage::NotExecuted => Self::NotExecuted,
            QueryCoverage::OutputLimited(limit) => Self::OutputLimited {
                limit: StoredOutputLimit::from(limit),
            },
            QueryCoverage::MatchLimitExceeded => Self::MatchLimitExceeded,
            QueryCoverage::Cancelled => Self::Cancelled,
            QueryCoverage::DeadlineExceeded => Self::DeadlineExceeded,
        }
    }
}

impl From<StoredQueryCoverage> for QueryCoverage {
    fn from(coverage: StoredQueryCoverage) -> Self {
        match coverage {
            StoredQueryCoverage::Complete => Self::Complete,
            StoredQueryCoverage::NotExecuted => Self::NotExecuted,
            StoredQueryCoverage::OutputLimited { limit } => Self::OutputLimited(limit.into()),
            StoredQueryCoverage::MatchLimitExceeded => Self::MatchLimitExceeded,
            StoredQueryCoverage::Cancelled => Self::Cancelled,
            StoredQueryCoverage::DeadlineExceeded => Self::DeadlineExceeded,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredOutputLimit {
    Matches,
    Captures,
}

impl From<QueryOutputLimit> for StoredOutputLimit {
    fn from(limit: QueryOutputLimit) -> Self {
        match limit {
            QueryOutputLimit::Matches => Self::Matches,
            QueryOutputLimit::Captures => Self::Captures,
        }
    }
}

impl From<StoredOutputLimit> for QueryOutputLimit {
    fn from(limit: StoredOutputLimit) -> Self {
        match limit {
            StoredOutputLimit::Matches => Self::Matches,
            StoredOutputLimit::Captures => Self::Captures,
        }
    }
}

fn describe(error: Option<StoreError>) -> String {
    error.map_or_else(|| "absent".to_owned(), |error| error.to_string())
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::evidence::{EvidenceCoverage, EvidenceRecord, ProviderIdentity, QueryIdentity};
    use crate::evidence_index::{
        ChangeKind, FileEvidence, IndexUpdate, QueryEvidence, SourceObservation,
    };
    use crate::graph::{
        EdgeKind, EvidenceClass, GraphCoverage, GraphEdge, GraphNode, GraphSnapshot, NodeId,
        NodeKind,
    };
    use crate::resolution::{SymbolId, SymbolKind};

    const SOURCE: &[u8] = b"fn main() {}\n";

    fn file_evidence(
        workspace: WorkspaceId,
        path: &str,
        source: &[u8],
        start_byte: usize,
        end_byte: usize,
    ) -> FileEvidence {
        let fingerprint = SourceFingerprint::of(source);
        let provider = ProviderIdentity {
            language: LanguageId::Rust,
            provider_id: "test.rust.tree-sitter".to_owned(),
            provider_version: "1".to_owned(),
        };
        let query = QueryIdentity {
            query_id: "codel.rust.symbols".to_owned(),
            query_version: "1".to_owned(),
        };
        let record = EvidenceRecord {
            workspace_id: workspace,
            relative_path: PathBuf::from(path),
            fingerprint,
            provider: provider.clone(),
            query: query.clone(),
            capture: CaptureIdentity {
                capture_index: 1,
                capture_name: "definition.function".to_owned(),
            },
            range: SourceRange {
                start_byte,
                end_byte,
            },
            pattern_index: 0,
            match_index: 0,
            coverage: EvidenceCoverage::complete(),
        };
        FileEvidence::single(
            PathBuf::from(path),
            fingerprint,
            provider,
            QueryEvidence::new(
                query,
                vec![record],
                Vec::new(),
                EvidenceCoverage::complete(),
            ),
        )
    }

    fn fixture(workspace: WorkspaceId) -> EvidenceIndex {
        let index = EvidenceIndex::new(workspace, IndexLimits::default()).expect("valid limits");
        index
            .apply(IndexUpdate::new().with_file(file_evidence(
                workspace,
                "src/main.rs",
                SOURCE,
                0,
                12,
            )))
            .expect("valid evidence");
        index
    }

    #[test]
    fn round_trips_generation_evidence_and_staleness() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        let index = fixture(workspace);
        index
            .mark_source_changed(Path::new("src/main.rs"))
            .expect("mark stale");
        let snapshot = index.snapshot().expect("snapshot");

        store.save(&snapshot).expect("save");
        let loaded = store.load().expect("load");

        assert_eq!(loaded.recovery, Recovery::Clean);
        assert_eq!(loaded.index.workspace_id(), workspace);
        assert_eq!(loaded.index.generation().expect("generation"), 1);
        let restored = loaded.index.snapshot().expect("restored snapshot");
        assert_eq!(restored.generation(), snapshot.generation());
        assert_eq!(restored.files().len(), 1);
        assert!(restored.is_stale(Path::new("src/main.rs")));
        let file = restored.file(Path::new("src/main.rs")).expect("file");
        assert_eq!(file.record_count(), 1);
        assert_eq!(
            file.fingerprint,
            snapshot
                .file(Path::new("src/main.rs"))
                .expect("source file")
                .fingerprint
        );
        assert_eq!(file.provider.language, LanguageId::Rust);
        assert_eq!(file.queries.len(), 1);
    }

    #[test]
    fn falls_back_to_the_previous_generation() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        let index = fixture(workspace);
        store
            .save(&index.snapshot().expect("snapshot"))
            .expect("save");

        index
            .apply(IndexUpdate::new().with_removal("src/main.rs"))
            .expect("remove file");
        store
            .save(&index.snapshot().expect("snapshot"))
            .expect("save second generation");
        fs::write(store.workspace_dir().join(CURRENT_FILE), b"not json")
            .expect("corrupt current generation");

        let loaded = store.load().expect("previous generation loads");
        assert_eq!(loaded.recovery, Recovery::PreviousGeneration);
        assert_eq!(loaded.index.generation().expect("generation"), 1);
        assert_eq!(loaded.index.snapshot().expect("snapshot").files().len(), 1);
    }

    #[test]
    fn keeps_worktrees_isolated_in_one_store_root() {
        let directory = tempdir().expect("temp dir");
        let first = WorkspaceId::new();
        let second = WorkspaceId::new();
        let first_store = IndexStore::new(directory.path(), first, IndexLimits::default());
        let second_store = IndexStore::new(directory.path(), second, IndexLimits::default());

        first_store
            .save(&fixture(first).snapshot().expect("snapshot"))
            .expect("save first");
        second_store
            .save(&fixture(second).snapshot().expect("snapshot"))
            .expect("save second");

        assert_ne!(first_store.workspace_dir(), second_store.workspace_dir());
        assert_eq!(
            first_store.load().expect("first").index.workspace_id(),
            first
        );
        assert_eq!(
            second_store.load().expect("second").index.workspace_id(),
            second
        );

        fs::create_dir_all(second_store.workspace_dir()).expect("second dir");
        fs::copy(
            first_store.workspace_dir().join(CURRENT_FILE),
            second_store.workspace_dir().join(CURRENT_FILE),
        )
        .expect("copy first snapshot into second workspace");
        let error = second_store.load().expect_err("identity must be checked");
        assert!(
            matches!(&error, StoreError::Corrupt(message) if message.contains("workspace")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_empty_store_starts_an_empty_index() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());

        let loaded = store.load().expect("empty load");

        assert_eq!(loaded.recovery, Recovery::Empty);
        assert_eq!(loaded.index.workspace_id(), workspace);
        assert_eq!(loaded.index.generation().expect("generation"), 0);
        assert!(
            loaded
                .index
                .snapshot()
                .expect("snapshot")
                .files()
                .is_empty()
        );
    }

    #[test]
    fn save_rejects_a_snapshot_from_another_workspace() {
        let directory = tempdir().expect("temp dir");
        let first = WorkspaceId::new();
        let second = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), first, IndexLimits::default());
        let snapshot = fixture(second).snapshot().expect("snapshot");

        let error = store
            .save(&snapshot)
            .expect_err("must reject other workspace");
        assert!(matches!(error, StoreError::WorkspaceMismatch { .. }));
    }

    #[test]
    fn detects_a_payload_change_by_checksum() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        store
            .save(&fixture(workspace).snapshot().expect("snapshot"))
            .expect("save");
        let path = store.workspace_dir().join(CURRENT_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        value["snapshot"]["files"][0]["source_len"] = serde_json::json!(999);
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("write");

        let error = store.read_index(&path).expect_err("checksum must fail");
        assert!(
            matches!(&error, StoreError::Checksum),
            "unexpected: {error}"
        );
    }

    #[test]
    fn rejects_an_unsupported_schema() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        store
            .save(&fixture(workspace).snapshot().expect("snapshot"))
            .expect("save");
        let path = store.workspace_dir().join(CURRENT_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        value["schema_version"] = serde_json::json!(999);
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("write");

        let error = store.read_index(&path).expect_err("schema must be checked");
        assert!(
            matches!(
                &error,
                StoreError::Schema {
                    found: 999,
                    expected: STORE_SCHEMA_VERSION
                }
            ),
            "unexpected: {error}"
        );
    }

    #[test]
    fn never_persists_source_text() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        let source = b"fn super_secret_body() {}\n";
        let index = EvidenceIndex::new(workspace, IndexLimits::default()).expect("valid limits");
        index
            .apply(IndexUpdate::new().with_file(file_evidence(
                workspace,
                "src/lib.rs",
                source,
                0,
                10,
            )))
            .expect("valid evidence");
        store
            .save(&index.snapshot().expect("snapshot"))
            .expect("save");

        let stored = fs::read_to_string(store.workspace_dir().join(CURRENT_FILE)).expect("read");
        assert!(!stored.contains("super_secret_body"));
        assert!(!stored.contains("fn super_secret_body"));
    }

    #[test]
    fn a_loaded_index_plans_incremental_changes() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = IndexStore::new(directory.path(), workspace, IndexLimits::default());
        store
            .save(&fixture(workspace).snapshot().expect("snapshot"))
            .expect("save");
        let loaded = store.load().expect("load");

        let observations = vec![SourceObservation {
            relative_path: PathBuf::from("src/main.rs"),
            fingerprint: SourceFingerprint::of(b"fn main() { changed }\n"),
            provider: ProviderIdentity {
                language: LanguageId::Rust,
                provider_id: "test.rust.tree-sitter".to_owned(),
                provider_version: "1".to_owned(),
            },
        }];
        let changes = loaded.index.plan(&observations).expect("plan");

        assert_eq!(changes.count(ChangeKind::Modified), 1);
    }
    fn graph_fixture(workspace: WorkspaceId) -> CodeGraph {
        let path = PathBuf::from("src/main.rs");
        let file = NodeId::file(&path);
        let symbol_id = SymbolId {
            workspace_id: workspace,
            path: path.clone(),
            start_byte: 0,
            end_byte: 12,
            kind: SymbolKind::Function,
        };
        let symbol = NodeId::symbol(&symbol_id);
        let snapshot = GraphSnapshot {
            workspace_id: workspace.to_string(),
            nodes: vec![
                GraphNode {
                    id: file.clone(),
                    kind: NodeKind::File,
                    name: "src/main.rs".to_owned(),
                    path: path.clone(),
                    range: None,
                },
                GraphNode {
                    id: symbol.clone(),
                    kind: NodeKind::Symbol(SymbolKind::Function),
                    name: "main".to_owned(),
                    path,
                    range: Some(SourceRange {
                        start_byte: 0,
                        end_byte: 12,
                    }),
                },
            ],
            edges: vec![GraphEdge {
                from: file,
                to: symbol,
                kind: EdgeKind::Contains,
                class: EvidenceClass::Extracted,
                references: Vec::new(),
            }],
            coverage: GraphCoverage {
                symbols: 1,
                files: 1,
                edges: 1,
                ..GraphCoverage::default()
            },
            source_digest: String::new(),
            metadata: BTreeMap::new(),
        };
        CodeGraph::from_snapshot(snapshot).expect("graph snapshot")
    }

    #[test]
    fn graph_store_round_trips_enrichment_metadata() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = GraphStore::new(directory.path(), workspace);
        let graph = graph_fixture(workspace);
        let mut metadata = BTreeMap::new();
        metadata.insert("scip_digest".to_owned(), "abc".to_owned());
        metadata.insert("symbol_types".to_owned(), "{}".to_owned());
        store
            .save_with_digest_and_metadata(&graph, "digest", &metadata)
            .expect("save");
        let (_, digest, loaded) = store.load_with_metadata().expect("load").expect("present");
        assert_eq!(digest, "digest");
        assert_eq!(loaded, metadata);
    }
    #[test]
    fn graph_store_round_trips_and_falls_back() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let store = GraphStore::new(directory.path(), workspace);

        assert!(store.load().expect("empty").is_none());
        store.save(&graph_fixture(workspace)).expect("save");
        let loaded = store.load().expect("load").expect("graph");
        assert_eq!(loaded.workspace_id(), workspace);
        assert_eq!(loaded.nodes().count(), 2);
        assert_eq!(loaded.edges().len(), 1);

        store.save(&graph_fixture(workspace)).expect("save second");
        fs::write(store.workspace_dir().join(GRAPH_FILE), b"not json").expect("corrupt");
        let loaded = store.load().expect("fallback").expect("previous graph");
        assert_eq!(loaded.nodes().count(), 2);
    }

    #[test]
    fn graph_store_checks_checksum_and_workspace() {
        let directory = tempdir().expect("temp dir");
        let workspace = WorkspaceId::new();
        let other = WorkspaceId::new();
        let store = GraphStore::new(directory.path(), workspace);
        store.save(&graph_fixture(workspace)).expect("save");

        let path = store.workspace_dir().join(GRAPH_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        value["snapshot"]["workspace_id"] = serde_json::json!(other.to_string());
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("write");
        let error = store.read_graph_with_metadata(&path).expect_err("checksum");
        assert!(matches!(error, StoreError::Checksum));

        let foreign = GraphStore::new(directory.path(), other);
        let error = foreign
            .save(&graph_fixture(workspace))
            .expect_err("workspace mismatch");
        assert!(matches!(error, StoreError::WorkspaceMismatch { .. }));
    }
}
