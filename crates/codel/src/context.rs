//! Bounded, source-verified context assembly over retrieval candidates.
//!
//! Retrieval ranks units; it does not read source. This module turns a ranked
//! candidate list into the exact excerpts an agent may quote, under one byte
//! budget and one segment budget. Every excerpt is checked against the
//! fingerprint recorded in its [`Provenance`], so a stale index can never yield
//! context that claims to be current source.
//!
//! The engine performs no I/O itself: the host supplies a [`SourceReader`],
//! normally over its retained workspace capability. A candidate without
//! provenance, a source that no longer matches its fingerprint, a range outside
//! the snapshot, and budget exhaustion are reported as explicit omissions
//! rather than silently dropped.
//!
//! Candidates are assumed to be already access-filtered by retrieval. This
//! module does not re-check ACLs; it verifies source identity and enforces the
//! byte budget only.
//!
//! # Budget
//!
//! [`ContextBudget`] is a byte budget, not a token budget: bytes are stable
//! across tokenizers and providers. A host that prices tokens projects the
//! returned excerpts after assembly.

use std::collections::BTreeMap;

use crate::model::{DocumentId, Provenance, UnitRef};
use crate::retrieve::RetrievalCandidate;
use crate::syntax::SourceRange;

/// Default excerpt budget: 32 KiB of source across all segments.
pub const DEFAULT_MAX_CONTEXT_BYTES: usize = 32 * 1024;

/// Default segment budget: eight excerpts per bundle.
pub const DEFAULT_MAX_CONTEXT_SEGMENTS: usize = 8;

/// Hard bounds on one assembled context bundle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextBudget {
    /// Maximum source bytes across every included segment.
    pub max_bytes: usize,
    /// Maximum number of included segments.
    pub max_segments: usize,
}

impl ContextBudget {
    /// Budget with explicit bounds.
    #[must_use]
    pub const fn new(max_bytes: usize, max_segments: usize) -> Self {
        Self {
            max_bytes,
            max_segments,
        }
    }

    /// Clamp both bounds to at least one, so a caller cannot request a bundle
    /// that can never contain an excerpt.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            max_bytes: if self.max_bytes == 0 {
                1
            } else {
                self.max_bytes
            },
            max_segments: if self.max_segments == 0 {
                1
            } else {
                self.max_segments
            },
        }
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_CONTEXT_BYTES,
            max_segments: DEFAULT_MAX_CONTEXT_SEGMENTS,
        }
    }
}

/// A host source read failed.
///
/// The message is host-supplied and must not carry source text or secrets; it
/// is reported in an omission for diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("source unavailable: {message}")]
pub struct SourceError {
    /// Host-supplied description of the failure.
    pub message: String,
}

impl SourceError {
    /// Build an error from a description.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Host seam that reads one workspace document by identity.
///
/// Implementations must return the exact bytes of the document identified by
/// `document`; the assembler verifies them against the candidate's fingerprint.
/// A host implements this over its retained, confined workspace capability, so
/// the engine never learns an absolute path.
pub trait SourceReader {
    /// Read the current bytes of one document.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError`] when the document is unavailable or unreadable.
    fn read(&self, document: &DocumentId) -> Result<Vec<u8>, SourceError>;
}

/// Why a candidate contributed no excerpt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OmissionReason {
    /// The candidate carried no source provenance (for example a memory unit
    /// or a remote-only hit).
    NoProvenance,
    /// The host reader failed for the candidate's document.
    SourceUnavailable,
    /// The source bytes no longer match the fingerprint the index recorded.
    FingerprintMismatch,
    /// The provenance range was inverted or extended past the source.
    RangeOutOfBounds,
    /// The requested range was empty.
    EmptyRange,
    /// The included range did not end on a UTF-8 boundary.
    InvalidUtf8,
    /// The byte or segment budget was exhausted before this candidate.
    BudgetExhausted,
}

impl OmissionReason {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoProvenance => "no-provenance",
            Self::SourceUnavailable => "source-unavailable",
            Self::FingerprintMismatch => "fingerprint-mismatch",
            Self::RangeOutOfBounds => "range-out-of-bounds",
            Self::EmptyRange => "empty-range",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::BudgetExhausted => "budget-exhausted",
        }
    }
}

/// One candidate that contributed no excerpt, with an explicit reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextOmission {
    /// Ranked unit that was omitted.
    pub unit: UnitRef,
    /// Why it was omitted.
    pub reason: OmissionReason,
    /// Host diagnostics for [`OmissionReason::SourceUnavailable`], when any.
    pub detail: Option<String>,
}

/// One verified source excerpt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextSegment {
    /// Ranked unit the excerpt was assembled for.
    pub unit: UnitRef,
    /// Source edge the excerpt was verified against.
    pub provenance: Provenance,
    /// Exact byte range included; equal to the provenance range unless
    /// `truncated` is set.
    pub range: SourceRange,
    /// Excerpt text, exactly the bytes in `range`.
    pub text: String,
    /// Whether the budget clipped the provenance range.
    pub truncated: bool,
}

/// Result of one bounded assembly pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextBundle {
    /// Verified excerpts, in candidate rank order.
    pub segments: Vec<ContextSegment>,
    /// Candidates that contributed nothing, in candidate rank order.
    pub omitted: Vec<ContextOmission>,
    /// Total excerpt bytes in `segments`.
    pub bytes: usize,
}

impl ContextBundle {
    /// Whether the bundle contains no excerpt.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Keys of the included units, in rank order.
    #[must_use]
    pub fn unit_keys(&self) -> Vec<String> {
        self.segments
            .iter()
            .map(|segment| segment.unit.key())
            .collect()
    }
}

/// Assemble a bounded context bundle from ranked candidates.
///
/// Candidates are processed in the order supplied. Each source document is
/// read at most once per call. A candidate is included only when its provenance
/// fingerprint matches the bytes the reader returns and its range lies inside
/// them; otherwise it is recorded in [`ContextBundle::omitted`].
///
/// Once either budget is exhausted, every remaining candidate is omitted with
/// [`OmissionReason::BudgetExhausted`]; the function never exceeds a bound.
#[must_use]
pub fn assemble_context(
    reader: &dyn SourceReader,
    candidates: &[RetrievalCandidate],
    budget: ContextBudget,
) -> ContextBundle {
    let budget = budget.normalized();
    let mut bundle = ContextBundle::default();
    let mut sources: BTreeMap<DocumentId, Result<Vec<u8>, SourceError>> = BTreeMap::new();

    for candidate in candidates {
        let unit = candidate.unit.clone();
        if bundle.segments.len() >= budget.max_segments || bundle.bytes >= budget.max_bytes {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::BudgetExhausted));
            continue;
        }
        let Some(provenance) = candidate.provenance.clone() else {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::NoProvenance));
            continue;
        };

        let document = provenance.document.clone();
        let source = sources
            .entry(document.clone())
            .or_insert_with(|| reader.read(&document));
        let bytes = match source {
            Ok(bytes) => bytes.as_slice(),
            Err(error) => {
                bundle.omitted.push(ContextOmission {
                    unit,
                    reason: OmissionReason::SourceUnavailable,
                    detail: Some(error.message.clone()),
                });
                continue;
            }
        };

        if !provenance.fingerprint.matches(bytes) {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::FingerprintMismatch));
            continue;
        }

        let range = provenance.range;
        if range.start_byte > range.end_byte || range.end_byte > bytes.len() {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::RangeOutOfBounds));
            continue;
        }
        if range.start_byte == range.end_byte {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::EmptyRange));
            continue;
        }

        let remaining = budget.max_bytes.saturating_sub(bundle.bytes);
        let wanted_end = range
            .end_byte
            .min(range.start_byte.saturating_add(remaining));
        let end = floor_char_boundary(bytes, range.start_byte, wanted_end);
        if end == range.start_byte {
            bundle
                .omitted
                .push(omission(unit, OmissionReason::BudgetExhausted));
            continue;
        }

        let text = match std::str::from_utf8(&bytes[range.start_byte..end]) {
            Ok(text) => text.to_owned(),
            Err(_) => {
                bundle
                    .omitted
                    .push(omission(unit, OmissionReason::InvalidUtf8));
                continue;
            }
        };

        bundle.bytes += text.len();
        bundle.segments.push(ContextSegment {
            unit,
            provenance,
            range: SourceRange {
                start_byte: range.start_byte,
                end_byte: end,
            },
            text,
            truncated: end < range.end_byte,
        });
    }

    bundle
}

fn omission(unit: UnitRef, reason: OmissionReason) -> ContextOmission {
    ContextOmission {
        unit,
        reason,
        detail: None,
    }
}

/// Largest boundary at or below `end` that does not split a UTF-8 sequence.
fn floor_char_boundary(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::WorkspaceId;
    use crate::evidence::SourceFingerprint;
    use crate::retrieve::{RetrievalCandidate, RetrievalTier};

    struct MapReader(BTreeMap<DocumentId, Vec<u8>>);

    impl SourceReader for MapReader {
        fn read(&self, document: &DocumentId) -> Result<Vec<u8>, SourceError> {
            self.0
                .get(document)
                .cloned()
                .ok_or_else(|| SourceError::new("document missing from fixture"))
        }
    }

    fn document(path: &str) -> DocumentId {
        DocumentId::new(WorkspaceId::new(), path)
    }

    fn provenance(document: &DocumentId, source: &[u8], start: usize, end: usize) -> Provenance {
        Provenance::new(
            document.clone(),
            SourceFingerprint::of(source),
            SourceRange {
                start_byte: start,
                end_byte: end,
            },
        )
    }

    fn candidate(unit: UnitRef, provenance: Option<Provenance>) -> RetrievalCandidate {
        RetrievalCandidate {
            unit,
            score: 1.0,
            tier: RetrievalTier::Local,
            lexical_rank: Some(1),
            vector_rank: None,
            remote_rank: None,
            exact_name: false,
            matched_terms: Vec::new(),
            provenance,
        }
    }

    fn fixture(source: &[u8]) -> (DocumentId, MapReader) {
        let document = document("src/lib.rs");
        let mut sources = BTreeMap::new();
        sources.insert(document.clone(), source.to_vec());
        (document, MapReader(sources))
    }

    #[test]
    fn returns_a_verified_excerpt_and_marks_truncation() {
        let source = b"fn alpha() {}
fn beta() {}
";
        let (document, reader) = fixture(source);
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(unit, Some(provenance(&document, source, 0, 14)))];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::new(8, 8));

        assert_eq!(bundle.segments.len(), 1);
        assert_eq!(bundle.segments[0].text, "fn alpha");
        assert_eq!(
            bundle.segments[0].range,
            SourceRange {
                start_byte: 0,
                end_byte: 8,
            }
        );
        assert!(bundle.segments[0].truncated);
        assert_eq!(bundle.bytes, 8);
    }

    #[test]
    fn an_exact_range_is_not_marked_truncated() {
        let source = b"fn alpha() {}
fn beta() {}
";
        let (document, reader) = fixture(source);
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(unit, Some(provenance(&document, source, 0, 14)))];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::new(1024, 8));

        assert_eq!(bundle.segments[0].text, "fn alpha() {}\n");
        assert!(!bundle.segments[0].truncated);
        assert_eq!(bundle.bytes, 14);
    }

    #[test]
    fn a_changed_source_is_rejected_not_quoted() {
        let source = b"fn alpha() {}
";
        let (document, reader) = fixture(
            b"fn changed() {}
",
        );
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(unit, Some(provenance(&document, source, 0, 14)))];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::default());

        assert!(bundle.is_empty());
        assert_eq!(bundle.omitted.len(), 1);
        assert_eq!(
            bundle.omitted[0].reason,
            OmissionReason::FingerprintMismatch
        );
    }

    #[test]
    fn a_candidate_without_provenance_is_omitted() {
        let source = b"fn alpha() {}
";
        let (document, reader) = fixture(source);
        let candidates = vec![candidate(UnitRef::Document(document), None)];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::default());

        assert!(bundle.is_empty());
        assert_eq!(bundle.omitted[0].reason, OmissionReason::NoProvenance);
    }

    #[test]
    fn a_range_past_the_source_is_omitted() {
        let source = b"fn alpha() {}
";
        let (document, reader) = fixture(source);
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(
            unit,
            Some(provenance(&document, source, 0, source.len() + 1)),
        )];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::default());

        assert!(bundle.is_empty());
        assert_eq!(bundle.omitted[0].reason, OmissionReason::RangeOutOfBounds);
    }

    #[test]
    fn the_segment_budget_omits_later_candidates() {
        let source = b"fn alpha() {}
fn beta() {}
";
        let (document, reader) = fixture(source);
        let first = UnitRef::Document(document.clone());
        let second = UnitRef::Document(DocumentId::new(document.workspace_id, "src/other.rs"));
        let candidates = vec![
            candidate(first, Some(provenance(&document, source, 0, 14))),
            candidate(second, Some(provenance(&document, source, 15, 28))),
        ];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::new(1024, 1));

        assert_eq!(bundle.segments.len(), 1);
        assert_eq!(bundle.omitted.len(), 1);
        assert_eq!(bundle.omitted[0].reason, OmissionReason::BudgetExhausted);
    }

    #[test]
    fn the_byte_budget_is_shared_across_candidates() {
        let source = b"fn alpha() {}
fn beta() {}
";
        let (document, reader) = fixture(source);
        let first = UnitRef::Document(document.clone());
        let second = UnitRef::Document(DocumentId::new(document.workspace_id, "src/other.rs"));
        let candidates = vec![
            candidate(first, Some(provenance(&document, source, 0, 14))),
            candidate(second, Some(provenance(&document, source, 15, 28))),
        ];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::new(8, 8));

        assert_eq!(bundle.segments.len(), 1);
        assert_eq!(bundle.bytes, 8);
        assert_eq!(bundle.omitted[0].reason, OmissionReason::BudgetExhausted);
    }

    #[test]
    fn a_multibyte_character_is_never_split() {
        let source = "éx".as_bytes();
        let (document, reader) = fixture(source);
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(
            unit,
            Some(provenance(&document, source, 0, source.len())),
        )];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::new(1, 8));

        assert!(bundle.is_empty());
        assert_eq!(bundle.omitted[0].reason, OmissionReason::BudgetExhausted);
    }

    #[test]
    fn a_missing_document_reports_the_host_detail() {
        let source = b"fn alpha() {}
";
        let document = document("src/lib.rs");
        let reader = MapReader(BTreeMap::new());
        let unit = UnitRef::Document(document.clone());
        let candidates = vec![candidate(unit, Some(provenance(&document, source, 0, 14)))];

        let bundle = assemble_context(&reader, &candidates, ContextBudget::default());

        assert_eq!(bundle.omitted[0].reason, OmissionReason::SourceUnavailable);
        assert_eq!(
            bundle.omitted[0].detail.as_deref(),
            Some("document missing from fixture")
        );
    }
}
