//! Hybrid local-first retrieval: lexical, dense, structural, then remote.
//!
//! Retrieval runs over the units registered in a [`HybridIndex`]. Three local
//! signals are fused by weighted reciprocal rank fusion (RRF):
//!
//! - **lexical** — Okapi BM25 over the unit's text
//!   ([`LexicalIndex`]);
//! - **vector** — cosine similarity against caller-supplied embeddings
//!   ([`VectorIndex`]), so a host can back it with a dense index (zvec, ONNX, or
//!   a board-side model) without this crate taking a dependency;
//! - **structure** — an additive bonus for a [`UnitKind::Symbol`] whose declared
//!   name matches the query, which is what makes an identifier query land on the
//!   declaration rather than on every mention of it.
//!
//! # Local first
//!
//! A remote tier is never consulted unless the local ranking is *insufficient*:
//! fewer than [`RetrievalConfig::min_local_hits`] visible local candidates, and
//! the policy is [`Escalation::WhenInsufficient`]. The remote tier is a
//! caller-supplied [`RemoteSearch`] object, so the engine itself performs no
//! I/O, holds no credentials, and stays deterministic and testable. Remote hits
//! are fused at a lower, configurable weight and by *rank*, so a remote service
//! cannot displace a local match of the same rank by returning scores on a
//! larger scale.
//!
//! # Access control
//!
//! Every candidate is filtered through [`AccessContext::allows`] against the
//! [`Acl`] registered with the unit, *before* ranking, so a denied unit can
//! neither appear nor consume a rank. Remote hits are filtered again on
//! arrival: a remote service is not trusted to enforce this workspace's
//! boundaries.
//!
//! # Incremental indexing
//!
//! [`HybridIndex::upsert_unit`] replaces a unit's postings, embedding and ACL in
//! one step and bumps the generation counter, so an edit never leaves stale
//! terms behind and a caller can tell which index generation produced a result.
//! [`HybridIndex::remove_unit`] retires a unit from every signal at once.
//!
//! Ordering is deterministic: fused score descending, then stable unit key.

use std::collections::BTreeMap;

use crate::lexical::LexicalIndex;
use crate::model::{Provenance, UnitKind, UnitRef, tokenize};
use crate::scope::{AccessContext, Acl};

/// Default reciprocal-rank-fusion constant.
///
/// Matches the constant used by the original TREC RRF study: a one-based rank
/// `r` contributes `weight / (rrf_k + r)`.
pub const DEFAULT_RRF_K: f32 = 60.0;

/// Default weight of the lexical ranking.
pub const DEFAULT_LEXICAL_WEIGHT: f32 = 1.0;

/// Default weight of the dense-vector ranking.
pub const DEFAULT_VECTOR_WEIGHT: f32 = 1.0;

/// Default weight of the remote ranking, deliberately below the local weights
/// so a remote tier refines a local result set instead of replacing it.
pub const DEFAULT_REMOTE_WEIGHT: f32 = 0.5;

/// Additive bonus for a symbol whose declared name matches the query.
///
/// Sized at twice one rank-zero lexical contribution
/// (`2 / (60 + 1)` ≈ `0.0328`) so an exact identifier match outranks a rank-zero
/// lexical hit without erasing the ordering of everything else.
pub const EXACT_NAME_BONUS: f32 = 0.03;

/// Which tier produced a candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrievalTier {
    /// Found by the in-process index.
    Local,
    /// Found by the remote searcher.
    Remote,
}

impl RetrievalTier {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

/// Reliability of the returned candidate set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetrievalCoverage {
    /// The local ranking met the configured hit floor; remote was not consulted.
    LocalOnly,
    /// Local coverage was insufficient and remote hits were fused in.
    EscalatedRemote,
    /// Local coverage was insufficient and the remote searcher failed.
    EscalationFailed,
    /// Local coverage was insufficient and no remote searcher was supplied.
    EscalationUnavailable,
}

impl RetrievalCoverage {
    /// Whether the result set is presented as complete for its request.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::LocalOnly)
    }

    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalOnly => "local-only",
            Self::EscalatedRemote => "escalated-remote",
            Self::EscalationFailed => "escalation-failed",
            Self::EscalationUnavailable => "escalation-unavailable",
        }
    }
}

/// When the remote tier may be consulted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Escalation {
    /// Never consult a remote tier.
    Never,
    /// Consult a remote tier only when the local ranking is insufficient.
    #[default]
    WhenInsufficient,
}

impl Escalation {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::WhenInsufficient => "when-insufficient",
        }
    }
}

/// One fused, ranked candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct RetrievalCandidate {
    /// Indexed unit.
    pub unit: UnitRef,
    /// Fused RRF score.
    pub score: f32,
    /// Tier the unit was found in; a unit found in both is reported as local.
    pub tier: RetrievalTier,
    /// One-based BM25 rank among visible units, when the unit matched lexically.
    pub lexical_rank: Option<usize>,
    /// One-based cosine rank among visible units, when the unit has a vector.
    pub vector_rank: Option<usize>,
    /// One-based rank assigned by the remote tier.
    pub remote_rank: Option<usize>,
    /// Declared symbol name matched the query exactly.
    pub exact_name: bool,
    /// Distinct query terms that occurred in the unit, in query order.
    pub matched_terms: Vec<String>,
    /// Source edge the unit was derived from, when it is source-backed.
    ///
    /// Local candidates registered through
    /// [`HybridIndex::upsert_sourced_unit`] carry the exact document, snapshot
    /// fingerprint, and byte range that produced them, so a caller can verify
    /// the excerpt it quotes. Remote-only candidates and memory units registered
    /// without provenance report `None`: a remote service is never trusted to
    /// invent a local source edge.
    pub provenance: Option<Provenance>,
}

/// Request for a hybrid retrieval pass.
#[derive(Clone, Debug, PartialEq)]
pub struct RetrievalQuery {
    /// Natural-language or identifier query text.
    pub text: String,
    /// Restrict candidates to one unit kind.
    pub kind: Option<UnitKind>,
    /// Override the configured candidate limit for this request.
    pub limit: Option<usize>,
    /// Embedding of the query, in the same space as the indexed vectors.
    ///
    /// The engine never computes an embedding: a host that has one supplies it,
    /// and a host without one gets lexical and structural retrieval alone.
    pub vector: Option<Vec<f32>>,
}

impl RetrievalQuery {
    /// Query text with no kind filter, no embedding, and the configured limit.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: None,
            limit: None,
            vector: None,
        }
    }

    /// Attach the query embedding.
    #[must_use]
    pub fn with_vector(mut self, vector: impl Into<Vec<f32>>) -> Self {
        self.vector = Some(vector.into());
        self
    }

    /// Restrict the query to one unit kind.
    #[must_use]
    pub fn with_kind(mut self, kind: UnitKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Override the candidate limit for this query.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Whether this query names one unit kind, so it is structural rather than
    /// free text.
    #[must_use]
    pub const fn is_kind_filtered(&self) -> bool {
        self.kind.is_some()
    }
}

/// Retrieval tuning. Every field is a plain value so a caller can persist it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetrievalConfig {
    /// Candidates returned by default.
    pub limit: usize,
    /// Visible local candidates required before the local tier counts as
    /// sufficient.
    pub min_local_hits: usize,
    /// Escalation policy.
    pub escalation: Escalation,
    /// RRF constant.
    pub rrf_k: f32,
    /// Lexical ranking weight.
    pub lexical_weight: f32,
    /// Dense-vector ranking weight.
    pub vector_weight: f32,
    /// Remote ranking weight.
    pub remote_weight: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            limit: 20,
            min_local_hits: 3,
            escalation: Escalation::WhenInsufficient,
            rrf_k: DEFAULT_RRF_K,
            lexical_weight: DEFAULT_LEXICAL_WEIGHT,
            vector_weight: DEFAULT_VECTOR_WEIGHT,
            remote_weight: DEFAULT_REMOTE_WEIGHT,
        }
    }
}

impl RetrievalConfig {
    /// Clamp nonsensical values so a caller cannot divide by a zero constant,
    /// request a zero-length result set, or demand more hits than it accepts.
    #[must_use]
    pub fn normalized(self) -> Self {
        let limit = self.limit.max(1);
        Self {
            limit,
            min_local_hits: self.min_local_hits.min(limit),
            rrf_k: positive_or(self.rrf_k, DEFAULT_RRF_K),
            lexical_weight: non_negative(self.lexical_weight, DEFAULT_LEXICAL_WEIGHT),
            vector_weight: non_negative(self.vector_weight, DEFAULT_VECTOR_WEIGHT),
            remote_weight: non_negative(self.remote_weight, DEFAULT_REMOTE_WEIGHT),
            escalation: self.escalation,
        }
    }
}

fn non_negative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

fn positive_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

/// Reciprocal-rank-fusion contribution of one one-based rank.
#[must_use]
pub fn rrf_contribution(weight: f32, rrf_k: f32, rank: usize) -> f32 {
    if weight <= 0.0 || rrf_k <= 0.0 || rank == 0 {
        return 0.0;
    }
    weight / (rrf_k + rank as f32)
}

/// Request forwarded to a remote searcher.
///
/// It carries only what a remote service needs to answer: the query text, the
/// number of candidates the caller can still use, and an optional kind filter.
/// No source text of other units, no ACL identities and no local index handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteQuery {
    /// Query text.
    pub text: String,
    /// Maximum number of candidates the caller can still use.
    pub limit: usize,
    /// Optional unit-kind restriction.
    pub kind: Option<UnitKind>,
}

/// One result returned by a remote searcher.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteHit {
    /// Unit the remote service matched.
    pub unit: UnitRef,
    /// Remote relevance in `0.0..=1.0`.
    ///
    /// Retained for display and auditing only: fusion uses the remote *rank*, so
    /// a remote service cannot outrank local results by returning scores on a
    /// larger scale.
    pub score: f32,
}

/// A remote searcher could not answer.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("remote search failed: {message}")]
pub struct RemoteError {
    /// Host-supplied description; must not include credentials or source text.
    pub message: String,
}

impl RemoteError {
    /// Build an error from a description.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Remote retrieval tier supplied by the host.
///
/// The engine calls [`RemoteSearch::search`] only when local coverage is
/// insufficient.
pub trait RemoteSearch {
    /// Answer one remote query.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteError`] when the remote tier cannot answer; retrieval
    /// then reports [`RetrievalCoverage::EscalationFailed`] and keeps its local
    /// results instead of failing the request.
    fn search(&self, query: &RemoteQuery) -> Result<Vec<RemoteHit>, RemoteError>;
}

/// What happened to the remote tier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteStatus {
    /// Remote was not consulted.
    NotConsulted,
    /// Remote was consulted and this many hits were accepted.
    Consulted {
        /// Accepted remote hits after kind and ACL filtering.
        hits: usize,
    },
    /// Remote was consulted and failed.
    Failed {
        /// Description supplied by the remote searcher.
        message: String,
    },
    /// Escalation was required but the caller supplied no remote searcher.
    Unavailable,
}

impl RemoteStatus {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotConsulted => "not-consulted",
            Self::Consulted { .. } => "consulted",
            Self::Failed { .. } => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Outcome of one hybrid retrieval pass.
#[derive(Clone, Debug, PartialEq)]
pub struct RetrievalOutcome {
    /// Fused candidates in deterministic order.
    pub candidates: Vec<RetrievalCandidate>,
    /// Coverage of the request.
    pub coverage: RetrievalCoverage,
    /// Visible local candidates the ranking produced before truncation.
    pub local_hits: usize,
    /// What happened to the remote tier.
    pub remote: RemoteStatus,
}

impl RetrievalOutcome {
    /// Whether the outcome carries at least one candidate.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Keys of the returned candidates, in rank order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.candidates
            .iter()
            .map(|candidate| candidate.unit.key())
            .collect()
    }
}

/// A dense vector was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VectorError {
    /// The vector was empty or had zero magnitude.
    #[error("vector has no magnitude")]
    Empty,
    /// The vector contained a NaN or infinite component.
    #[error("vector contains a non-finite component")]
    NotFinite,
    /// The vector length disagrees with the index dimension.
    #[error("vector has {actual} dimensions, index stores {expected}")]
    Dimension {
        /// Dimension established by the index.
        expected: usize,
        /// Dimension of the rejected vector.
        actual: usize,
    },
}

/// Cosine-similarity index over unit embeddings.
///
/// Vectors are stored L2-normalized, so a search is a dot product over the
/// visible vectors. This is an exact, brute-force scan: deterministic,
/// dependency-free, and the seam an approximate (zvec- or HNSW-backed) index
/// plugs into without changing [`HybridIndex`]'s fusion contract.
#[derive(Clone, Debug, Default)]
pub struct VectorIndex {
    dimension: Option<usize>,
    vectors: BTreeMap<String, Vec<f32>>,
}

impl VectorIndex {
    /// Empty index that adopts the dimension of its first vector.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dimension: None,
            vectors: BTreeMap::new(),
        }
    }

    /// Stored dimension, once a vector has been inserted.
    #[must_use]
    pub const fn dimension(&self) -> Option<usize> {
        self.dimension
    }

    /// Number of stored vectors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Whether no vector is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Whether `key` has a vector.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.vectors.contains_key(key)
    }

    /// Store the normalized form of `vector` for `key`.
    ///
    /// The first vector fixes the index dimension; every later vector must match
    /// it, so cosine similarity is never computed across mismatched spaces.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] when the vector is empty, has zero magnitude,
    /// contains a non-finite component, or disagrees with the index dimension.
    pub fn upsert(&mut self, key: &str, vector: &[f32]) -> Result<(), VectorError> {
        if vector.is_empty() {
            return Err(VectorError::Empty);
        }
        if !vector.iter().all(|value| value.is_finite()) {
            return Err(VectorError::NotFinite);
        }
        if let Some(expected) = self.dimension
            && expected != vector.len()
        {
            return Err(VectorError::Dimension {
                expected,
                actual: vector.len(),
            });
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !(norm.is_finite() && norm > 0.0) {
            return Err(VectorError::Empty);
        }
        let normalized: Vec<f32> = vector.iter().map(|value| *value / norm).collect();
        self.dimension = Some(vector.len());
        self.vectors.insert(key.to_owned(), normalized);
        Ok(())
    }

    /// Drop the vector stored for `key`, returning whether one was stored.
    pub fn remove(&mut self, key: &str) -> bool {
        let removed = self.vectors.remove(key).is_some();
        if self.vectors.is_empty() {
            self.dimension = None;
        }
        removed
    }

    /// Rank stored vectors by cosine similarity to `query`.
    ///
    /// Returns `(key, similarity)` pairs, best first, ties broken by key. Keys
    /// rejected by `visible` are removed before ranking, so a filtered-out unit
    /// cannot occupy a rank. A non-finite or zero-magnitude query, or one whose
    /// length disagrees with the index, yields no results.
    #[must_use]
    pub fn search(&self, query: &[f32], visible: impl Fn(&str) -> bool) -> Vec<(String, f32)> {
        if self.vectors.is_empty() || query.is_empty() {
            return Vec::new();
        }
        let Some(expected) = self.dimension else {
            return Vec::new();
        };
        if query.len() != expected || !query.iter().all(|value| value.is_finite()) {
            return Vec::new();
        }
        let norm = query.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !(norm.is_finite() && norm > 0.0) {
            return Vec::new();
        }
        let unit: Vec<f32> = query.iter().map(|value| *value / norm).collect();
        let mut hits: Vec<(String, f32)> = self
            .vectors
            .iter()
            .filter(|(key, _)| visible(key))
            .map(|(key, stored)| {
                let similarity = stored
                    .iter()
                    .zip(&unit)
                    .map(|(left, right)| left * right)
                    .sum::<f32>();
                (key.clone(), similarity)
            })
            .collect();
        hits.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        hits
    }
}

/// Registered facts about one indexed unit.
#[derive(Clone, Debug)]
struct UnitMeta {
    unit: UnitRef,
    acl: Acl,
    /// Source edge the unit was derived from, when the caller supplied one.
    provenance: Option<Provenance>,
    /// Declared name and its scope path, for structural matching.
    name_tokens: Vec<String>,
}

/// One unit's accumulated rank evidence during a local pass.
#[derive(Clone, Debug)]
struct RankedCandidate {
    unit: UnitRef,
    tier: RetrievalTier,
    lexical_rank: Option<usize>,
    vector_rank: Option<usize>,
    remote_rank: Option<usize>,
    exact_name: bool,
    matched_terms: Vec<String>,
    provenance: Option<Provenance>,
    score: f32,
}

impl RankedCandidate {
    fn new(unit: UnitRef, tier: RetrievalTier, provenance: Option<Provenance>) -> Self {
        Self {
            unit,
            tier,
            lexical_rank: None,
            vector_rank: None,
            remote_rank: None,
            exact_name: false,
            matched_terms: Vec::new(),
            provenance,
            score: 0.0,
        }
    }

    fn into_candidate(self) -> RetrievalCandidate {
        RetrievalCandidate {
            unit: self.unit,
            score: self.score,
            tier: self.tier,
            lexical_rank: self.lexical_rank,
            vector_rank: self.vector_rank,
            remote_rank: self.remote_rank,
            exact_name: self.exact_name,
            matched_terms: self.matched_terms,
            provenance: self.provenance,
        }
    }
}

/// Incremental hybrid index over retrievable units.
///
/// One index owns the lexical postings, the embeddings and the ACL of every
/// unit, so an update is applied atomically per unit and no signal can go stale
/// while another is refreshed.
#[derive(Clone, Debug, Default)]
pub struct HybridIndex {
    lexical: LexicalIndex,
    vectors: VectorIndex,
    units: BTreeMap<String, UnitMeta>,
    generation: u64,
}

impl HybridIndex {
    /// Empty index at generation zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Index generation, bumped by every successful unit update or removal.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of registered units.
    #[must_use]
    pub fn unit_count(&self) -> usize {
        self.units.len()
    }

    /// Number of units carrying an embedding.
    #[must_use]
    pub fn vector_count(&self) -> usize {
        self.vectors.len()
    }

    /// The lexical sub-index.
    #[must_use]
    pub const fn lexical(&self) -> &LexicalIndex {
        &self.lexical
    }

    /// The dense sub-index.
    #[must_use]
    pub const fn vectors(&self) -> &VectorIndex {
        &self.vectors
    }

    /// ACL registered for `unit`, if the unit is indexed.
    #[must_use]
    pub fn acl(&self, unit: &UnitRef) -> Option<&Acl> {
        self.units.get(&unit.key()).map(|meta| &meta.acl)
    }

    /// Whether `unit` is indexed.
    #[must_use]
    pub fn contains(&self, unit: &UnitRef) -> bool {
        self.units.contains_key(&unit.key())
    }

    /// Insert or replace one unsourced unit's text, embedding and ACL.
    ///
    /// This is [`Self::upsert_sourced_unit`] without a source edge: use it for
    /// durable memory and for units whose source is not represented by a
    /// fingerprint. Candidates for such units report no provenance.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] when `vector` is unusable, exactly as
    /// [`Self::upsert_sourced_unit`] does.
    pub fn upsert_unit(
        &mut self,
        unit: &UnitRef,
        acl: Acl,
        text: &str,
        vector: Option<&[f32]>,
    ) -> Result<(), VectorError> {
        self.upsert_sourced_unit(unit, acl, None, text, vector)
    }

    /// Insert or replace one unit's text, embedding, ACL and source edge.
    ///
    /// `provenance` is the verifiable source edge — document, exact snapshot
    /// fingerprint, byte range, and parser identity — that retrieval returns on
    /// every candidate for this unit. Use [`Self::upsert_unit`] for units with
    /// no source file.
    ///
    /// Replacing a unit recomputes its lexical postings from scratch, replaces
    /// its embedding when one is supplied, and adopts the new ACL, so a
    /// visibility narrowing takes effect before the next search. Passing `None`
    /// for `vector` leaves any stored embedding in place; use
    /// [`Self::remove_unit`] to drop it.
    ///
    /// # Errors
    ///
    /// Returns [`VectorError`] when `vector` is unusable. The unit's lexical
    /// entry, ACL and symbol name are still updated, so a bad embedding never
    /// makes a unit unsearchable by text.
    pub fn upsert_sourced_unit(
        &mut self,
        unit: &UnitRef,
        acl: Acl,
        provenance: Option<Provenance>,
        text: &str,
        vector: Option<&[f32]>,
    ) -> Result<(), VectorError> {
        let key = unit.key();
        self.lexical.upsert(&key, text);
        let vector_result = match vector {
            Some(vector) => self.vectors.upsert(&key, vector),
            None => Ok(()),
        };
        // Only symbols have a declared name; the structural signal compares the
        // name's tokens, not the body text, so a mention inside another unit's
        // body cannot masquerade as a declaration.
        let name_tokens = match unit {
            UnitRef::Symbol(id) => tokenize(&id.name),
            _ => Vec::new(),
        };
        self.units.insert(
            key,
            UnitMeta {
                unit: unit.clone(),
                acl,
                provenance,
                name_tokens,
            },
        );
        self.generation = self.generation.saturating_add(1);
        vector_result
    }

    /// Retire `unit` from every signal, returning whether it was indexed.
    pub fn remove_unit(&mut self, unit: &UnitRef) -> bool {
        let key = unit.key();
        let removed = self.units.remove(&key).is_some();
        self.lexical.remove(&key);
        self.vectors.remove(&key);
        if removed {
            self.generation = self.generation.saturating_add(1);
        }
        removed
    }

    /// Local ranking only: no remote tier is consulted, whatever the coverage.
    #[must_use]
    pub fn search_local(
        &self,
        access: &AccessContext,
        query: &RetrievalQuery,
        config: RetrievalConfig,
    ) -> Vec<RetrievalCandidate> {
        let config = config.normalized();
        let limit = query.limit.unwrap_or(config.limit).max(1);
        self.local_ranking(access, query, config)
            .into_iter()
            .take(limit)
            .map(RankedCandidate::into_candidate)
            .collect()
    }

    /// Hybrid retrieval: local signals first, remote only when insufficient.
    #[must_use]
    pub fn retrieve(
        &self,
        access: &AccessContext,
        query: &RetrievalQuery,
        config: RetrievalConfig,
        remote: Option<&dyn RemoteSearch>,
    ) -> RetrievalOutcome {
        let config = config.normalized();
        let limit = query.limit.unwrap_or(config.limit).max(1);
        let local = self.local_ranking(access, query, config);

        let sufficient =
            local.len() >= config.min_local_hits || config.escalation == Escalation::Never;
        if sufficient {
            return RetrievalOutcome {
                candidates: local
                    .into_iter()
                    .take(limit)
                    .map(RankedCandidate::into_candidate)
                    .collect(),
                coverage: RetrievalCoverage::LocalOnly,
                local_hits: 0,
                remote: RemoteStatus::NotConsulted,
            };
        }

        let local_hits = local.len();
        let mut merged: BTreeMap<String, RankedCandidate> = local
            .into_iter()
            .map(|ranked| (ranked.unit.key(), ranked))
            .collect();

        let Some(remote_search) = remote else {
            let candidates = merged
                .into_values()
                .take(limit)
                .map(RankedCandidate::into_candidate)
                .collect();
            return RetrievalOutcome {
                candidates,
                coverage: RetrievalCoverage::EscalationUnavailable,
                local_hits,
                remote: RemoteStatus::Unavailable,
            };
        };

        let remote_query = RemoteQuery {
            text: query.text.clone(),
            limit,
            kind: query.kind,
        };
        let (remote_status, coverage) = match remote_search.search(&remote_query) {
            Ok(hits) => {
                let accepted = self.accept_remote(hits, access, query, config, &mut merged);
                (
                    RemoteStatus::Consulted { hits: accepted },
                    RetrievalCoverage::EscalatedRemote,
                )
            }
            Err(error) => (
                RemoteStatus::Failed {
                    message: error.message,
                },
                RetrievalCoverage::EscalationFailed,
            ),
        };

        let mut candidates: Vec<RetrievalCandidate> = merged
            .into_values()
            .map(RankedCandidate::into_candidate)
            .collect();
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.unit.key().cmp(&right.unit.key()))
        });
        candidates.truncate(limit);

        RetrievalOutcome {
            candidates,
            coverage,
            local_hits,
            remote: remote_status,
        }
    }

    /// Filter a remote response and fuse it into `merged`.
    ///
    /// Returns the number of hits accepted; rejected hits still consume their
    /// remote rank, so filtering cannot promote an unauthorised unit into a
    /// higher fused position.
    fn accept_remote(
        &self,
        hits: Vec<RemoteHit>,
        access: &AccessContext,
        query: &RetrievalQuery,
        config: RetrievalConfig,
        merged: &mut BTreeMap<String, RankedCandidate>,
    ) -> usize {
        let mut accepted = 0usize;
        for (position, hit) in hits.into_iter().enumerate() {
            if !hit.score.is_finite() {
                continue;
            }
            if let Some(kind) = query.kind
                && hit.unit.kind() != kind
            {
                continue;
            }
            let key = hit.unit.key();
            // A remote service is not trusted to enforce this workspace's
            // boundaries: re-check the locally registered ACL. A unit this index
            // has never seen (a remote-only memory) has no local ACL to check and
            // is admitted, labelled as a remote hit.
            if let Some(meta) = self.units.get(&key)
                && !access.allows(&meta.acl)
            {
                continue;
            }
            accepted += 1;
            // A locally known unit keeps its source edge even when the remote
            // tier is what surfaced it; a remote-only unit has none.
            let provenance = self
                .units
                .get(&key)
                .and_then(|meta| meta.provenance.clone());
            let entry = merged.entry(key).or_insert_with(|| {
                RankedCandidate::new(hit.unit.clone(), RetrievalTier::Remote, provenance)
            });
            entry.remote_rank = Some(position + 1);
            entry.score += rrf_contribution(config.remote_weight, config.rrf_k, position + 1);
        }
        accepted
    }

    fn visible(&self, access: &AccessContext, key: &str, kind: Option<UnitKind>) -> bool {
        let Some(meta) = self.units.get(key) else {
            return false;
        };
        if let Some(kind) = kind
            && meta.unit.kind() != kind
        {
            return false;
        }
        access.allows(&meta.acl)
    }

    fn local_ranking(
        &self,
        access: &AccessContext,
        query: &RetrievalQuery,
        config: RetrievalConfig,
    ) -> Vec<RankedCandidate> {
        let query_terms = tokenize(&query.text);
        if query_terms.is_empty() || self.units.is_empty() {
            return Vec::new();
        }
        let mut merged: BTreeMap<String, RankedCandidate> = BTreeMap::new();

        // Lexical signal: rank only visible units, so a denied unit neither
        // appears nor consumes a rank.
        let lexical_hits: Vec<_> = self
            .lexical
            .search(&query.text)
            .into_iter()
            .filter(|hit| self.visible(access, &hit.key, query.kind))
            .collect();
        for (position, hit) in lexical_hits.into_iter().enumerate() {
            let rank = position + 1;
            let Some(meta) = self.units.get(&hit.key) else {
                continue;
            };
            let entry = merged.entry(hit.key).or_insert_with(|| {
                RankedCandidate::new(
                    meta.unit.clone(),
                    RetrievalTier::Local,
                    meta.provenance.clone(),
                )
            });
            entry.lexical_rank = Some(rank);
            entry.exact_name = name_matches(&meta.name_tokens, &query_terms);
            entry.matched_terms = hit.matched_terms;
            entry.score += rrf_contribution(config.lexical_weight, config.rrf_k, rank);
            if entry.exact_name {
                entry.score += EXACT_NAME_BONUS;
            }
        }

        // Dense signal over the same visibility rule. A query without an
        // embedding simply has no dense signal; it never fails retrieval.
        if let Some(vector) = query.vector.as_deref() {
            for (position, (key, _similarity)) in self
                .vectors
                .search(vector, |key| self.visible(access, key, query.kind))
                .into_iter()
                .enumerate()
            {
                let rank = position + 1;
                let Some(meta) = self.units.get(&key) else {
                    continue;
                };
                let entry = merged.entry(key).or_insert_with(|| {
                    RankedCandidate::new(
                        meta.unit.clone(),
                        RetrievalTier::Local,
                        meta.provenance.clone(),
                    )
                });
                entry.vector_rank = Some(rank);
                entry.score += rrf_contribution(config.vector_weight, config.rrf_k, rank);
            }
        }

        // Structural signal. A unit whose declared name is exactly the query is
        // ranked even when neither its body text nor its embedding matched, which
        // is what makes an identifier query find a declaration that never spells
        // its own name out.
        for (key, meta) in &self.units {
            if !name_matches(&meta.name_tokens, &query_terms) {
                continue;
            }
            if !self.visible(access, key, query.kind) {
                continue;
            }
            let mut entry = RankedCandidate::new(
                meta.unit.clone(),
                RetrievalTier::Local,
                meta.provenance.clone(),
            );
            entry.exact_name = true;
            entry.matched_terms = query_terms.clone();
            entry.score += EXACT_NAME_BONUS;
            merged
                .entry(key.clone())
                .and_modify(|existing| {
                    existing.exact_name = true;
                    existing.score += EXACT_NAME_BONUS;
                })
                .or_insert(entry);
        }

        let mut candidates: Vec<RankedCandidate> = merged.into_values().collect();
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.unit.key().cmp(&right.unit.key()))
        });
        candidates
    }
}

/// Whether a symbol's declared name equals the query, ignoring case and
/// separator style via token comparison.
fn name_matches(name_tokens: &[String], query_terms: &[String]) -> bool {
    !name_tokens.is_empty() && name_tokens == query_terms
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::WorkspaceId;
    use crate::evidence::SourceFingerprint;
    use crate::model::{DocumentId, Provenance, SymbolUnitId};
    use crate::scope::{OwnerScope, TenantId, UserId, Visibility};
    use crate::syntax::SourceRange;

    struct Fixture {
        workspace: WorkspaceId,
        owner: OwnerScope,
        access: AccessContext,
    }

    impl Fixture {
        fn new() -> Self {
            let workspace = WorkspaceId::new();
            let owner = OwnerScope::durable(TenantId::new(), UserId::new());
            Self {
                workspace,
                owner,
                access: AccessContext::new(owner, workspace),
            }
        }

        fn symbol(&self, path: &str, start: usize, name: &str) -> UnitRef {
            UnitRef::Symbol(SymbolUnitId::new(
                DocumentId::new(self.workspace, path),
                start,
                name,
            ))
        }

        fn workspace_acl(&self) -> Acl {
            Acl::workspace(self.owner, self.workspace)
        }

        fn private_acl(&self) -> Acl {
            Acl::private(self.owner, Some(self.workspace))
        }

        fn foreign_acl(&self) -> Acl {
            Acl {
                owner: OwnerScope::durable(TenantId::new(), UserId::new()),
                visibility: Visibility::Tenant,
                workspace: None,
            }
        }
    }

    /// A remote tier that records its calls and returns a fixed response.
    struct StubRemote {
        hits: Vec<RemoteHit>,
        error: Option<RemoteError>,
        calls: RefCell<Vec<RemoteQuery>>,
    }

    impl StubRemote {
        fn returning(hits: Vec<RemoteHit>) -> Self {
            Self {
                hits,
                error: None,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                hits: Vec::new(),
                error: Some(RemoteError::new(message)),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl RemoteSearch for StubRemote {
        fn search(&self, query: &RemoteQuery) -> Result<Vec<RemoteHit>, RemoteError> {
            self.calls.borrow_mut().push(query.clone());
            match &self.error {
                Some(error) => Err(error.clone()),
                None => Ok(self.hits.clone()),
            }
        }
    }

    fn index_with_text(fixture: &Fixture, entries: &[(&str, usize, &str, &str)]) -> HybridIndex {
        let mut index = HybridIndex::new();
        for (path, start, name, text) in entries {
            index
                .upsert_unit(
                    &fixture.symbol(path, *start, name),
                    fixture.workspace_acl(),
                    text,
                    None,
                )
                .expect("no vector supplied");
        }
        index
    }

    fn keys(candidates: &[RetrievalCandidate]) -> Vec<String> {
        candidates.iter().map(|c| c.unit.key()).collect()
    }

    #[test]
    fn sourced_units_keep_their_provenance_through_ranking() {
        let fixture = Fixture::new();
        let unit = fixture.symbol("src/retrieve.rs", 40, "HybridIndex");
        let provenance = Provenance::new(
            DocumentId::new(fixture.workspace, "src/retrieve.rs"),
            SourceFingerprint::of(b"pub struct HybridIndex;"),
            SourceRange {
                start_byte: 0,
                end_byte: 24,
            },
        )
        .with_provider("codel.rust.tree-sitter");
        let mut index = HybridIndex::new();
        index
            .upsert_sourced_unit(
                &unit,
                fixture.workspace_acl(),
                Some(provenance.clone()),
                "hybrid index retrieval",
                None,
            )
            .expect("no vector supplied");

        let candidates = index.search_local(
            &fixture.access,
            &RetrievalQuery::new("hybrid index"),
            RetrievalConfig::default(),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provenance.as_ref(), Some(&provenance));
    }

    #[test]
    fn unsourced_units_report_no_provenance() {
        let fixture = Fixture::new();
        let unit = fixture.symbol("src/retrieve.rs", 40, "HybridIndex");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &unit,
                fixture.workspace_acl(),
                "durable memory about hybrid index",
                None,
            )
            .expect("no vector supplied");

        let candidates = index.search_local(
            &fixture.access,
            &RetrievalQuery::new("hybrid index"),
            RetrievalConfig::default(),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provenance, None);
    }

    #[test]
    fn lexical_ranking_is_deterministic_and_access_filtered() {
        let fixture = Fixture::new();
        let visible = fixture.symbol("src/retrieval.rs", 0, "HybridRetriever");
        let other = fixture.symbol("src/other.rs", 0, "HybridParser");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &visible,
                fixture.workspace_acl(),
                "hybrid retriever fuses lexical and dense signals",
                None,
            )
            .unwrap();
        index
            .upsert_unit(
                &other,
                fixture.workspace_acl(),
                "hybrid parser reads modules",
                None,
            )
            .unwrap();

        let query = RetrievalQuery::new("hybrid retriever");
        let first = index.search_local(&fixture.access, &query, RetrievalConfig::default());
        let second = index.search_local(&fixture.access, &query, RetrievalConfig::default());
        assert_eq!(first, second, "ranking is deterministic for one index");
        assert_eq!(keys(&first), vec![visible.key(), other.key()]);
        assert_eq!(first[0].lexical_rank, Some(1));
        assert_eq!(first[0].tier, RetrievalTier::Local);
        assert_eq!(
            first[0].matched_terms,
            vec!["hybrid".to_owned(), "retriever".to_owned()]
        );

        // A private unit owned by somebody else in the same workspace is
        // invisible, and takes no rank from the remaining units.
        let mut foreign = HybridIndex::new();
        index
            .upsert_unit(
                &other,
                fixture.foreign_acl(),
                "hybrid retriever fuses lexical and dense signals",
                None,
            )
            .unwrap();
        foreign
            .upsert_unit(&visible, fixture.workspace_acl(), "hybrid retriever", None)
            .unwrap();
        let hidden = foreign.search_local(&fixture.access, &query, RetrievalConfig::default());
        assert_eq!(keys(&hidden), vec![visible.key()]);
        assert_eq!(
            hidden[0].lexical_rank,
            Some(1),
            "the visible unit keeps rank 1"
        );
    }

    #[test]
    fn upsert_replaces_postings_and_narrows_visibility_in_one_step() {
        let fixture = Fixture::new();
        let unit = fixture.symbol("src/index.rs", 0, "SourceIndex");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(&unit, fixture.workspace_acl(), "legacy scanner", None)
            .unwrap();
        let generation = index.generation();
        assert_eq!(index.unit_count(), 1);

        index
            .upsert_unit(&unit, fixture.private_acl(), "tree-sitter parser", None)
            .unwrap();
        assert!(index.generation() > generation, "generation advances");
        assert_eq!(index.unit_count(), 1, "replacing never duplicates a unit");

        let stale = index.search_local(
            &fixture.access,
            &RetrievalQuery::new("legacy"),
            RetrievalConfig::default(),
        );
        assert!(stale.is_empty(), "replaced text leaves no stale postings");
        let fresh = index.search_local(
            &fixture.access,
            &RetrievalQuery::new("parser"),
            RetrievalConfig::default(),
        );
        assert_eq!(keys(&fresh), vec![unit.key()]);
        assert_eq!(index.acl(&unit), Some(&fixture.private_acl()));

        assert!(index.remove_unit(&unit));
        assert!(!index.remove_unit(&unit), "removal is idempotent");
        assert!(
            index
                .search_local(
                    &fixture.access,
                    &RetrievalQuery::new("parser"),
                    RetrievalConfig::default()
                )
                .is_empty()
        );
        assert_eq!(index.vector_count(), 0);
    }

    #[test]
    fn dense_signal_fuses_with_lexical_and_breaks_ties_by_score() {
        let fixture = Fixture::new();
        let lexical = fixture.symbol("src/text.rs", 0, "Textual");
        let semantic = fixture.symbol("src/semantic.rs", 0, "Semantic");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &lexical,
                fixture.workspace_acl(),
                "retrieval rank fusion",
                Some(&[1.0, 0.6, 0.0]),
            )
            .unwrap();
        index
            .upsert_unit(
                &semantic,
                fixture.workspace_acl(),
                "unrelated vocabulary",
                Some(&[1.0, 0.0, 0.0]),
            )
            .unwrap();

        let query = RetrievalQuery::new("retrieval rank fusion").with_vector([1.0f32, 0.0, 0.0]);
        let outcome = index.retrieve(
            &fixture.access,
            &query,
            RetrievalConfig {
                min_local_hits: 1,
                ..RetrievalConfig::default()
            },
            None,
        );
        assert_eq!(outcome.coverage, RetrievalCoverage::LocalOnly);
        assert_eq!(outcome.remote, RemoteStatus::NotConsulted);
        let lexical_candidate = outcome
            .candidates
            .iter()
            .find(|candidate| candidate.unit == lexical)
            .expect("lexical hit present");
        assert_eq!(lexical_candidate.lexical_rank, Some(1));
        assert_eq!(
            lexical_candidate.vector_rank,
            Some(2),
            "the less aligned embedding ranks second"
        );
        let semantic_candidate = outcome
            .candidates
            .iter()
            .find(|candidate| candidate.unit == semantic)
            .expect("vector-only hit present");
        assert_eq!(semantic_candidate.lexical_rank, None);
        assert_eq!(semantic_candidate.vector_rank, Some(1));
        // Lexical + vector outranks vector-only.
        assert_eq!(outcome.keys()[0], lexical.key());
        assert!(outcome.candidates[0].score > outcome.candidates[1].score);
    }

    #[test]
    fn a_query_without_embedding_still_ranks_lexically() {
        let fixture = Fixture::new();
        let unit = fixture.symbol("src/lex.rs", 0, "LexOnly");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &unit,
                fixture.workspace_acl(),
                "hybrid search",
                Some(&[0.0, 1.0]),
            )
            .unwrap();
        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("hybrid"),
            RetrievalConfig::default(),
            None,
        );
        assert_eq!(keys(&outcome.candidates), vec![unit.key()]);
        assert_eq!(outcome.candidates[0].vector_rank, None);
        assert_eq!(index.vector_count(), 1, "the stored embedding survives");
    }

    #[test]
    fn exact_declared_name_outranks_a_body_mention() {
        let fixture = Fixture::new();
        let declaration = fixture.symbol("src/a.rs", 0, "HybridIndex");
        let mention = fixture.symbol("src/b.rs", 0, "mentions_everything");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &mention,
                fixture.workspace_acl(),
                "HybridIndex HybridIndex HybridIndex the type used here",
                None,
            )
            .unwrap();
        // The declaration's body never mentions its own name.
        index
            .upsert_unit(
                &declaration,
                fixture.workspace_acl(),
                "bm25 vectors symbols acl generation",
                None,
            )
            .unwrap();

        let query = RetrievalQuery::new("hybrid index");
        let outcome = index.retrieve(
            &fixture.access,
            &query,
            RetrievalConfig {
                min_local_hits: 1,
                ..RetrievalConfig::default()
            },
            None,
        );
        assert_eq!(outcome.keys()[0], declaration.key());
        let top = &outcome.candidates[0];
        assert!(top.exact_name);
        assert_eq!(
            top.matched_terms,
            vec!["hybrid".to_owned(), "index".to_owned()]
        );
        assert_eq!(outcome.coverage, RetrievalCoverage::LocalOnly);
    }

    #[test]
    fn kind_filter_excludes_other_units_from_every_signal() {
        let fixture = Fixture::new();
        let symbol = fixture.symbol("src/a.rs", 0, "HybridSearch");
        let chunk = UnitRef::Chunk(crate::model::ChunkId::new(
            DocumentId::new(fixture.workspace, "src/a.rs"),
            0,
        ));
        let mut index = HybridIndex::new();
        index
            .upsert_unit(
                &symbol,
                fixture.workspace_acl(),
                "hybrid search engine",
                Some(&[1.0, 0.0]),
            )
            .unwrap();
        index
            .upsert_unit(
                &chunk,
                fixture.workspace_acl(),
                "hybrid search engine",
                Some(&[1.0, 0.0]),
            )
            .unwrap();

        let query = RetrievalQuery::new("hybrid search")
            .with_kind(UnitKind::Symbol)
            .with_vector([1.0f32, 0.0]);
        let outcome = index.retrieve(
            &fixture.access,
            &query,
            RetrievalConfig {
                min_local_hits: 1,
                ..RetrievalConfig::default()
            },
            None,
        );
        assert!(query.is_kind_filtered());
        assert_eq!(outcome.keys(), vec![symbol.key()]);
    }

    #[test]
    fn a_satisfied_local_ranking_never_calls_the_remote_tier() {
        let fixture = Fixture::new();
        let mut index = index_with_text(
            &fixture,
            &[
                ("src/a.rs", 0, "Alpha", "zvec dense index"),
                ("src/b.rs", 0, "Beta", "zvec quantization"),
                ("src/c.rs", 0, "Gamma", "zvec recall"),
            ],
        );
        let unit = fixture.symbol("src/d.rs", 0, "Delta");
        index
            .upsert_unit(&unit, fixture.workspace_acl(), "zvec search", None)
            .unwrap();
        let remote = StubRemote::returning(Vec::new());
        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("zvec"),
            RetrievalConfig::default(),
            Some(&remote),
        );
        assert_eq!(outcome.coverage, RetrievalCoverage::LocalOnly);
        assert_eq!(remote.calls(), 0);
        assert_eq!(outcome.candidates.len(), 4);
        assert_eq!(outcome.local_hits, 0, "no escalation happened");
    }

    #[test]
    fn insufficient_local_coverage_escalates_and_keeps_local_first() {
        let fixture = Fixture::new();
        let local = fixture.symbol("src/local.rs", 0, "LocalHit");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(&local, fixture.workspace_acl(), "bm25 ranking", None)
            .unwrap();
        let remote_unit = fixture.symbol("src/remote.rs", 0, "RemoteHit");
        let remote = StubRemote::returning(vec![RemoteHit {
            unit: remote_unit.clone(),
            score: 1.0,
        }]);

        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("bm25 ranking"),
            RetrievalConfig::default(),
            Some(&remote),
        );
        assert_eq!(remote.calls(), 1);
        assert_eq!(outcome.local_hits, 1);
        assert_eq!(outcome.coverage, RetrievalCoverage::EscalatedRemote);
        assert!(!outcome.coverage.is_complete());
        assert_eq!(outcome.remote, RemoteStatus::Consulted { hits: 1 });
        assert_eq!(
            outcome.keys()[0],
            local.key(),
            "local rank 1 wins the fusion"
        );
        assert_eq!(outcome.candidates[0].tier, RetrievalTier::Local);
        assert_eq!(outcome.candidates[1].tier, RetrievalTier::Remote);
        assert_eq!(outcome.candidates[1].remote_rank, Some(1));
        // Remote weight is half of local, so its rank-1 contribution is smaller.
        assert!(outcome.candidates[0].score > outcome.candidates[1].score);
    }

    #[test]
    fn remote_hits_are_rechecked_against_local_acls() {
        let fixture = Fixture::new();
        let hidden = fixture.symbol("src/secret.rs", 0, "Secret");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(&hidden, fixture.foreign_acl(), "unrelated", None)
            .unwrap();
        let remote = StubRemote::returning(vec![RemoteHit {
            unit: hidden.clone(),
            score: 1.0,
        }]);
        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("bm25"),
            RetrievalConfig::default(),
            Some(&remote),
        );
        assert_eq!(outcome.remote, RemoteStatus::Consulted { hits: 0 });
        assert!(outcome.is_empty(), "an unauthorised remote hit is dropped");
        assert_eq!(outcome.local_hits, 0);
    }

    #[test]
    fn a_failing_remote_tier_preserves_local_results() {
        let fixture = Fixture::new();
        let local = fixture.symbol("src/local.rs", 0, "Local");
        let mut index = HybridIndex::new();
        index
            .upsert_unit(&local, fixture.workspace_acl(), "bm25 ranking", None)
            .unwrap();
        let remote = StubRemote::failing("503 from remote index");
        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("bm25"),
            RetrievalConfig::default(),
            Some(&remote),
        );
        assert_eq!(outcome.coverage, RetrievalCoverage::EscalationFailed);
        assert_eq!(
            outcome.remote,
            RemoteStatus::Failed {
                message: "503 from remote index".to_owned()
            }
        );
        assert_eq!(outcome.keys(), vec![local.key()]);
        assert_eq!(
            RemoteStatus::Failed {
                message: "x".into()
            }
            .as_str(),
            "failed"
        );
    }

    #[test]
    fn escalation_never_and_missing_remote_are_reported_distinctly() {
        let fixture = Fixture::new();
        let index = HybridIndex::new();
        let remote = StubRemote::returning(Vec::new());
        let never = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("bm25"),
            RetrievalConfig {
                escalation: Escalation::Never,
                ..RetrievalConfig::default()
            },
            Some(&remote),
        );
        assert_eq!(never.coverage, RetrievalCoverage::LocalOnly);
        assert_eq!(remote.calls(), 0);

        let unavailable = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("bm25"),
            RetrievalConfig::default(),
            None,
        );
        assert_eq!(
            unavailable.coverage,
            RetrievalCoverage::EscalationUnavailable
        );
        assert_eq!(unavailable.remote, RemoteStatus::Unavailable);
        assert_eq!(RetrievalCoverage::LocalOnly.as_str(), "local-only");
        assert_eq!(Escalation::Never.as_str(), "never");
        assert_eq!(RetrievalTier::Remote.as_str(), "remote");
    }

    #[test]
    fn query_limit_overrides_configuration_and_truncates() {
        let fixture = Fixture::new();
        let mut index = HybridIndex::new();
        for (start, name) in [(0, "One"), (10, "Two"), (20, "Three")] {
            let unit = fixture.symbol("src/many.rs", start, name);
            index
                .upsert_unit(
                    &unit,
                    fixture.workspace_acl(),
                    "hybrid fusion ranking",
                    None,
                )
                .unwrap();
        }
        let outcome = index.retrieve(
            &fixture.access,
            &RetrievalQuery::new("hybrid fusion").with_limit(2),
            RetrievalConfig::default(),
            None,
        );
        assert_eq!(outcome.candidates.len(), 2);
        assert_eq!(outcome.coverage, RetrievalCoverage::LocalOnly);
        assert_eq!(outcome.candidates[0].lexical_rank, Some(1));
        assert_eq!(outcome.candidates[1].lexical_rank, Some(2));
    }

    #[test]
    fn vectors_are_validated_and_normalized() {
        let mut vectors = VectorIndex::new();
        assert_eq!(vectors.dimension(), None);
        assert!(vectors.is_empty());
        assert_eq!(vectors.upsert("a", &[]), Err(VectorError::Empty));
        assert_eq!(
            vectors.upsert("a", &[f32::NAN, 1.0]),
            Err(VectorError::NotFinite)
        );
        assert_eq!(vectors.upsert("a", &[0.0, 0.0]), Err(VectorError::Empty));
        assert_eq!(vectors.upsert("a", &[3.0, 0.0]), Ok(()));
        assert_eq!(vectors.dimension(), Some(2));
        assert_eq!(
            vectors.upsert("b", &[1.0, 0.0, 0.0]),
            Err(VectorError::Dimension {
                expected: 2,
                actual: 3
            })
        );

        let hits = vectors.search(&[4.0, 0.0], |_| true);
        assert_eq!(hits.len(), 1);
        assert!(
            (hits[0].1 - 1.0).abs() < f32::EPSILON,
            "cosine of parallel vectors is 1"
        );
        assert!(vectors.search(&[1.0, 0.0, 0.0], |_| true).is_empty());
        assert!(vectors.search(&[f32::INFINITY, 0.0], |_| true).is_empty());
        assert!(vectors.search(&[0.0, 0.0], |_| true).is_empty());
        assert!(vectors.search(&[1.0, 0.0], |_| false).is_empty());

        assert!(vectors.remove("a"));
        assert!(!vectors.remove("a"));
        assert_eq!(vectors.dimension(), None, "emptying resets the dimension");
    }

    #[test]
    fn rrf_contribution_decreases_with_rank_and_is_guarded() {
        let first = rrf_contribution(1.0, DEFAULT_RRF_K, 1);
        let second = rrf_contribution(1.0, DEFAULT_RRF_K, 2);
        let third = rrf_contribution(1.0, DEFAULT_RRF_K, 3);
        assert!(first > second && second > third);
        assert!((first - 1.0 / 61.0).abs() < 1e-6);
        assert!(
            third > first / 2.0,
            "rank decay is sublinear: one over k plus rank flattens with rank"
        );
        assert_eq!(rrf_contribution(1.0, 0.0, 1), 0.0);
        assert_eq!(rrf_contribution(0.0, DEFAULT_RRF_K, 1), 0.0);
        assert_eq!(rrf_contribution(1.0, DEFAULT_RRF_K, 0), 0.0);
    }

    #[test]
    fn configuration_is_normalized_against_hostile_values() {
        let config = RetrievalConfig {
            limit: 0,
            min_local_hits: 9,
            rrf_k: f32::NAN,
            lexical_weight: -1.0,
            vector_weight: f32::INFINITY,
            remote_weight: -0.0,
            escalation: Escalation::Never,
        }
        .normalized();
        assert_eq!(config.limit, 1);
        assert_eq!(config.min_local_hits, 1);
        assert_eq!(config.rrf_k, DEFAULT_RRF_K);
        assert_eq!(config.lexical_weight, DEFAULT_LEXICAL_WEIGHT);
        assert_eq!(config.vector_weight, DEFAULT_VECTOR_WEIGHT);
        assert_eq!(config.remote_weight, -0.0, "zero weight is legal");
        assert_eq!(config.escalation, Escalation::Never);
        assert!(
            EXACT_NAME_BONUS > rrf_contribution(1.0, DEFAULT_RRF_K, 1),
            "an exact identifier match outranks one rank-zero lexical hit"
        );
    }
}
