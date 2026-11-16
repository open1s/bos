//! Deterministic, explainable hybrid retrieval (issue #345).
//!
//! No vector database, no native dependency: Unicode-normalized tokens with
//! CJK character n-grams, BM25 term-frequency ranking, scope/kind/metadata
//! filtering BEFORE ranking, kind/recency/verification features, per-hit score
//! explanations, and a token-budgeted top-K. Every step is deterministic for a
//! given corpus and clock, so recall is reproducible and auditable.
//!
//! The legacy [`super::MemoryStore::search`] ranking stays untouched for the
//! binding fixtures; this module is the new `recall` seam the agent prompt
//! path uses.

use serde::{Deserialize, Serialize};

use super::record::{MemoryKind, MemoryRecord, MemoryScope};
use super::{now_millis, MetadataFilter};
use crate::agent::context::estimate_tokens;

/// BM25 term-frequency saturation parameter (documented default).
const K1: f64 = 1.2;
/// BM25 length-normalization parameter (documented default).
const B: f64 = 0.75;

/// One recall request: bounds, filters and the clock.
#[derive(Debug, Clone)]
pub struct RecallRequest {
    /// Maximum number of hits.
    pub limit: usize,
    /// Optional token budget over the hit contents (documented estimator).
    pub token_budget: Option<usize>,
    /// Optional exact metadata filter, applied before ranking.
    pub filter: Option<MetadataFilter>,
    /// The viewer's scope; records outside it are filtered before ranking.
    pub viewer: Option<MemoryScope>,
    /// Optional kind allowlist, applied before ranking.
    pub kinds: Option<Vec<MemoryKind>>,
    /// The clock for expiry and recency; 0 means the current wall clock.
    pub now_ms: u64,
}

impl Default for RecallRequest {
    fn default() -> Self {
        Self {
            limit: 5,
            token_budget: None,
            filter: None,
            viewer: None,
            kinds: None,
            now_ms: 0,
        }
    }
}

/// One named contribution to a hit's score, for explainable diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreFeature {
    /// The feature name (bm25, kind, verified, recency).
    pub name: &'static str,
    /// Its numeric contribution.
    pub value: f64,
}

/// One recalled record with its explanation.
#[derive(Debug, Clone)]
pub struct RecallHit {
    /// The legacy item view (compatibility).
    pub item: super::MemoryItem,
    /// The full record, carrying scope and provenance.
    pub record: MemoryRecord,
    /// The final score used for ordering.
    pub score: f64,
    /// The score's named contributions.
    pub features: Vec<ScoreFeature>,
    /// The estimated tokens of the content (documented estimator).
    pub token_estimate: usize,
}

/// The outcome of a recall: hits plus a skip reason when nothing is returned.
#[derive(Debug, Clone, Default)]
pub struct RecallOutcome {
    /// The ranked hits, best first.
    pub hits: Vec<RecallHit>,
    /// Why recall returned nothing, when it did.
    pub skip: Option<String>,
    /// Ranked candidates dropped because the token budget could not fit them
    /// (issue #345): the omission is recorded, not silent.
    pub truncated: usize,
}

/// Whether a character is a CJK ideograph, kana or hangul syllable.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'   // CJK extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK unified ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
        | '\u{3040}'..='\u{30FF}' // hiragana + katakana
        | '\u{AC00}'..='\u{D7AF}' // hangul syllables
    )
}

/// Fold fullwidth ASCII to its plain form so ＡＢＣ matches abc.
fn fold_width(c: char) -> char {
    if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
        char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
    } else if c == '\u{3000}' {
        ' '
    } else {
        c
    }
}

/// Split a CJK run into character unigrams and bigrams.
///
/// Continuous CJK text has no word boundaries; unigrams let a single-character
/// query match, bigrams let multi-character terms match with far fewer false
/// positives than unigrams alone. Both are emitted, deterministically.
fn flush_cjk(run: &mut String, tokens: &mut Vec<String>) {
    let chars: Vec<char> = run.chars().collect();
    for c in &chars {
        tokens.push(c.to_string());
    }
    for window in chars.windows(2) {
        tokens.push(window.iter().collect());
    }
    run.clear();
}

fn flush_word(word: &mut String, tokens: &mut Vec<String>) {
    if !word.is_empty() {
        tokens.push(std::mem::take(word));
    }
}

/// Unicode-aware tokenization: lowercase, width-folded alphanumeric words plus
/// CJK unigrams/bigrams. Deterministic and dependency-free.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut cjk = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = fold_width(c);
        if is_cjk(c) {
            flush_word(&mut word, &mut tokens);
            cjk.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut tokens);
            word.push(c);
        } else {
            flush_word(&mut word, &mut tokens);
            flush_cjk(&mut cjk, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk, &mut tokens);
    tokens
}

/// Normalized form for deduplication keys: lowercase, width-folded, whitespace
/// collapsed. Two candidates with the same normalized content are duplicates.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = fold_width(c);
        if c.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

/// BM25 scores of `docs` for the distinct `query_tokens`.
fn bm25(query_tokens: &[String], docs: &[Vec<String>]) -> Vec<f64> {
    let n = docs.len() as f64;
    if n == 0.0 {
        return Vec::new();
    }
    let avgdl = docs.iter().map(Vec::len).sum::<usize>() as f64 / n;
    let mut scores = vec![0.0f64; docs.len()];
    for token in query_tokens {
        let df = docs
            .iter()
            .filter(|doc| doc.iter().any(|t| t == token))
            .count() as f64;
        if df == 0.0 {
            continue;
        }
        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
        for (index, doc) in docs.iter().enumerate() {
            let tf = doc.iter().filter(|t| *t == token).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let dl = doc.len() as f64;
            let norm = tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * dl / avgdl.max(1.0)));
            scores[index] += idf * norm;
        }
    }
    scores
}

/// The kind feature: durable knowledge outranks raw exchanges at equal BM25.
fn kind_boost(kind: MemoryKind) -> f64 {
    match kind {
        MemoryKind::Decision | MemoryKind::Fact => 0.15,
        MemoryKind::Procedure | MemoryKind::Lesson => 0.10,
        MemoryKind::Preference | MemoryKind::TaskState => 0.05,
        MemoryKind::Note | MemoryKind::Exchange => 0.0,
    }
}

/// Rank live records for a query: filter first, BM25 + features second,
/// deterministic tie-breaks third, token budget last.
#[must_use]
pub fn recall(records: &[MemoryRecord], query: &str, request: &RecallRequest) -> RecallOutcome {
    if request.limit == 0 {
        return RecallOutcome {
            hits: Vec::new(),
            skip: Some("recall limit is zero".to_string()),
            truncated: 0,
        };
    }
    let now = if request.now_ms == 0 {
        now_millis()
    } else {
        request.now_ms
    };
    // Scope, kind, metadata and lifecycle filtering happen BEFORE ranking, so
    // no out-of-scope content is ever scored or exposed (issues #340/#345).
    let candidates: Vec<&MemoryRecord> = records
        .iter()
        .filter(|record| {
            record.is_live(now)
                && request
                    .viewer
                    .as_ref()
                    .is_none_or(|viewer| viewer.permits(&record.scope))
                && request
                    .kinds
                    .as_ref()
                    .is_none_or(|kinds| kinds.contains(&record.kind))
                && request
                    .filter
                    .as_ref()
                    .is_none_or(|filter| filter.matches(&record.to_item()))
        })
        .collect();
    if candidates.is_empty() {
        return RecallOutcome {
            hits: Vec::new(),
            skip: Some("no live records in scope".to_string()),
            truncated: 0,
        };
    }

    let mut query_tokens = tokenize(query);
    query_tokens.sort();
    query_tokens.dedup();

    let mut scored: Vec<(f64, Vec<ScoreFeature>, &MemoryRecord)> = Vec::new();
    if query_tokens.is_empty() {
        // A blank query means "most recent", the legacy contract.
        for record in &candidates {
            scored.push((
                0.0,
                vec![ScoreFeature {
                    name: "recency",
                    value: 0.0,
                }],
                record,
            ));
        }
    } else {
        let docs: Vec<Vec<String>> = candidates
            .iter()
            .map(|record| tokenize(&record.content))
            .collect();
        let base = bm25(&query_tokens, &docs);
        for (index, record) in candidates.iter().enumerate() {
            let bm25_score = base[index];
            if bm25_score <= 0.0 {
                continue;
            }
            let kind = kind_boost(record.kind);
            let verified = if record.confidence.verified {
                0.10
            } else {
                0.0
            };
            // A bounded freshness bonus: at most 0.05, halving every 30 days.
            let age_days = now.saturating_sub(record.created_at_ms) as f64 / 86_400_000.0;
            let recency = 0.05 * (-age_days / 30.0).exp();
            let final_score = bm25_score * (1.0 + kind + verified) + recency;
            scored.push((
                final_score,
                vec![
                    ScoreFeature {
                        name: "bm25",
                        value: bm25_score,
                    },
                    ScoreFeature {
                        name: "kind",
                        value: bm25_score * kind,
                    },
                    ScoreFeature {
                        name: "verified",
                        value: bm25_score * verified,
                    },
                    ScoreFeature {
                        name: "recency",
                        value: recency,
                    },
                ],
                record,
            ));
        }
    }

    // Deterministic order: score desc, creation desc, id asc.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.2.created_at_ms.cmp(&a.2.created_at_ms))
            .then(a.2.id.cmp(&b.2.id))
    });

    let mut hits = Vec::new();
    let mut spent = 0usize;
    let mut truncated = 0usize;
    for (score, features, record) in scored {
        if hits.len() >= request.limit {
            break;
        }
        let token_estimate = estimate_tokens(&record.content);
        if let Some(budget) = request.token_budget {
            if spent + token_estimate > budget {
                truncated += 1;
                continue;
            }
            spent += token_estimate;
        }
        hits.push(RecallHit {
            item: record.to_item(),
            record: (*record).clone(),
            score,
            features,
            token_estimate,
        });
    }
    let skip = if hits.is_empty() {
        Some(
            if query_tokens.is_empty() {
                "no live records in scope"
            } else if request.token_budget == Some(0) {
                "token budget exhausted"
            } else {
                "no matching items"
            }
            .to_string(),
        )
    } else {
        None
    };
    RecallOutcome {
        hits,
        skip,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::record::{Confidence, ConfidenceLevel, Provenance};

    fn record(id: &str, content: &str, created: u64) -> MemoryRecord {
        let mut r = MemoryRecord::new(
            MemoryKind::Note,
            MemoryScope::workspace("w"),
            content,
            Provenance::default(),
        );
        r.id = id.to_string();
        r.created_at_ms = created;
        r.updated_at_ms = created;
        r
    }

    #[test]
    fn continuous_chinese_matches_by_ngrams() {
        let records = vec![
            record("a", "项目的部署流程先用 cargo 构建，再运行测试。", 1),
            record("b", "The deployment pipeline builds with cargo.", 2),
        ];
        let outcome = recall(&records, "部署流程", &RecallRequest::default());
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].record.id, "a");
        // The legacy alphanumeric tokenizer cannot match this query at all:
        // the whole sentence is one token. The n-gram tokenizer can.
        assert!(outcome.hits[0]
            .features
            .iter()
            .any(|f| f.name == "bm25" && f.value > 0.0));
    }

    #[test]
    fn mixed_language_queries_match_both_scripts() {
        let records = vec![record("a", "使用tokio运行时统一并发模型", 1)];
        let outcome = recall(&records, "tokio 运行时", &RecallRequest::default());
        assert_eq!(outcome.hits.len(), 1);
    }

    #[test]
    fn fullwidth_and_case_variants_match() {
        let records = vec![record("a", "Use TOKIO for async IO", 1)];
        let outcome = recall(&records, "ｔｏｋｉｏ", &RecallRequest::default());
        assert_eq!(outcome.hits.len(), 1);
    }

    #[test]
    fn bm25_prefers_term_frequency_over_a_single_mention() {
        let records = vec![
            record("once", "rust memory store", 2),
            record("twice", "rust rust memory", 1),
        ];
        let outcome = recall(&records, "rust", &RecallRequest::default());
        assert_eq!(outcome.hits[0].record.id, "twice");
    }

    #[test]
    fn scope_filtering_happens_before_ranking() {
        let mut foreign = record("foreign", "部署流程 secret", 10);
        foreign.scope = MemoryScope::workspace("other");
        let mine = record("mine", "部署流程 public", 1);
        let request = RecallRequest {
            viewer: Some(MemoryScope::workspace("w")),
            ..Default::default()
        };
        let outcome = recall(&[foreign, mine], "部署流程", &request);
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].record.id, "mine");
    }

    #[test]
    fn expired_and_tombstoned_records_never_recall() {
        let mut expired = record("expired", "部署流程", 1);
        expired.expires_at_ms = Some(50);
        let mut dead = record("dead", "部署流程", 2);
        dead.status = crate::memory::Lifecycle::Tombstoned;
        let live = record("live", "部署流程", 3);
        let request = RecallRequest {
            now_ms: 100,
            ..Default::default()
        };
        let outcome = recall(&[expired, dead, live], "部署流程", &request);
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].record.id, "live");
    }

    #[test]
    fn kind_and_verification_features_are_explainable() {
        let mut decision = record("decision", "use the kernel for commits", 1);
        decision.kind = MemoryKind::Decision;
        decision.confidence = Confidence {
            level: ConfidenceLevel::High,
            verified: true,
        };
        let note = record("note", "use the kernel for commits", 2);
        let outcome = recall(
            &[note, decision],
            "kernel commits",
            &RecallRequest::default(),
        );
        assert_eq!(outcome.hits[0].record.id, "decision");
        let features = &outcome.hits[0].features;
        assert!(features.iter().any(|f| f.name == "kind" && f.value > 0.0));
        assert!(features
            .iter()
            .any(|f| f.name == "verified" && f.value > 0.0));
    }

    #[test]
    fn token_budget_bounds_the_result_set() {
        let records = vec![
            record("a", "alpha beta gamma", 3),
            record("b", "alpha beta delta", 2),
            record("c", "alpha beta epsilon", 1),
        ];
        let request = RecallRequest {
            limit: 10,
            token_budget: Some(8), // each content estimates 4 tokens (16 ASCII chars)
            ..Default::default()
        };
        let outcome = recall(&records, "alpha beta", &request);
        assert_eq!(outcome.hits.len(), 2);
        let spent: usize = outcome.hits.iter().map(|h| h.token_estimate).sum();
        assert!(spent <= 8);
    }

    #[test]
    fn ordering_is_deterministic_for_equal_scores() {
        let records = vec![
            record("b", "same text here", 1),
            record("a", "same text here", 1),
        ];
        let first = recall(&records, "same text", &RecallRequest::default());
        let second = recall(&records, "same text", &RecallRequest::default());
        assert_eq!(first.hits[0].record.id, second.hits[0].record.id);
        assert_eq!(first.hits[0].record.id, "a", "id ascending breaks the tie");
    }

    #[test]
    fn blank_query_returns_the_most_recent() {
        let records = vec![record("old", "x", 1), record("new", "y", 2)];
        let outcome = recall(&records, "  ", &RecallRequest::default());
        assert_eq!(outcome.hits[0].record.id, "new");
    }

    #[test]
    fn normalize_collapses_width_case_and_whitespace() {
        assert_eq!(normalize("  ＡＢＣ   def "), "abc def");
        assert_eq!(normalize("记忆  存储"), "记忆 存储");
    }

    /// The versioned benchmark corpus (issue #345): Chinese, English and mixed
    /// queries must retrieve their relevant documents, measurably and fast.
    #[test]
    fn benchmark_corpus_v1_meets_recall_and_latency_targets() {
        let fixture = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/memory_recall_benchmark_v1.json"
        ))
        .expect("benchmark fixture");
        let corpus: serde_json::Value = serde_json::from_str(&fixture).unwrap();
        assert_eq!(corpus["version"], 1);
        let scope = MemoryScope::workspace("bench");
        let records: Vec<MemoryRecord> = corpus["documents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|doc| {
                let mut r = record(
                    doc["id"].as_str().unwrap(),
                    doc["content"].as_str().unwrap(),
                    doc["created"].as_u64().unwrap(),
                );
                r.scope = scope.clone();
                r
            })
            .collect();
        let queries = corpus["queries"].as_array().unwrap();

        let mut latencies = Vec::new();
        let mut recall_sum = 0.0;
        let mut precision_sum = 0.0;
        for _ in 0..30 {
            let started = std::time::Instant::now();
            for query in queries {
                let text = query["query"].as_str().unwrap();
                let relevant: Vec<&str> = query["relevant"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                let outcome = recall(
                    &records,
                    text,
                    &RecallRequest {
                        limit: 3,
                        now_ms: 1_000_000,
                        ..Default::default()
                    },
                );
                let top: Vec<&str> = outcome.hits.iter().map(|h| h.record.id.as_str()).collect();
                let found = relevant.iter().filter(|id| top.contains(id)).count();
                recall_sum += found as f64 / relevant.len() as f64;
                precision_sum += found as f64 / top.len().max(1) as f64;
            }
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let n = (queries.len() * 30) as f64;
        let recall_at_3 = recall_sum / n;
        let precision_at_3 = precision_sum / n;
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[latencies.len() * 95 / 100];
        assert!(
            recall_at_3 >= 0.95,
            "Recall@3 too low on the multilingual corpus: {recall_at_3}"
        );
        assert!(
            precision_at_3 >= 0.4,
            "Precision@3 too low on the multilingual corpus: {precision_at_3}"
        );
        assert!(p50 < 100.0, "P50 latency over the corpus: {p50}ms");
        assert!(p95 < 250.0, "P95 latency over the corpus: {p95}ms");
    }
}
