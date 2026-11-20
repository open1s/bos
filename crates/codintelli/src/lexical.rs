//! BM25 lexical index over unit tokens.
//!
//! The index is keyed by the stable [`UnitRef::key`](crate::model::UnitRef)
//! string, so a hit can be mapped back to the unit table without a second
//! identity scheme. Text is tokenized by
//! [`tokenize`](crate::model::tokenize), which splits identifiers on
//! non-alphanumerics and camel-case boundaries.
//!
//! Scoring is Okapi BM25 over per-unit term frequencies. Nothing here
//! executes workspace code, calls an external service, or stores the text it
//! indexes beyond the postings it derives.

use std::collections::BTreeMap;

use crate::model::tokenize;

/// Default BM25 term-frequency saturation.
pub const DEFAULT_BM25_K1: f32 = 1.2;

/// Default BM25 length-normalization.
pub const DEFAULT_BM25_B: f32 = 0.75;

/// One ranked lexical hit.
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalHit {
    /// Unit key that matched.
    pub key: String,
    /// BM25 score.
    pub score: f32,
    /// Distinct query terms that occurred in the unit, in query order.
    pub matched_terms: Vec<String>,
}

/// Inverted index with per-unit term frequencies and lengths.
#[derive(Clone, Debug)]
pub struct LexicalIndex {
    postings: BTreeMap<String, BTreeMap<String, u32>>,
    lengths: BTreeMap<String, u32>,
    total_length: u64,
    k1: f32,
    b: f32,
}

impl Default for LexicalIndex {
    fn default() -> Self {
        Self::new(DEFAULT_BM25_K1, DEFAULT_BM25_B)
    }
}

impl LexicalIndex {
    /// Empty index with explicit BM25 parameters.
    #[must_use]
    pub fn new(k1: f32, b: f32) -> Self {
        Self {
            postings: BTreeMap::new(),
            lengths: BTreeMap::new(),
            total_length: 0,
            k1,
            b,
        }
    }

    /// Number of indexed units.
    #[must_use]
    pub fn unit_count(&self) -> usize {
        self.lengths.len()
    }

    /// Number of distinct terms.
    #[must_use]
    pub fn term_count(&self) -> usize {
        self.postings.len()
    }

    /// Average indexed unit length in tokens.
    #[must_use]
    pub fn average_length(&self) -> f32 {
        if self.lengths.is_empty() {
            0.0
        } else {
            self.total_length as f32 / self.lengths.len() as f32
        }
    }

    /// Whether `key` is indexed.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.lengths.contains_key(key)
    }

    /// Terms of one indexed unit, with their frequencies.
    #[must_use]
    pub fn terms(&self, key: &str) -> Vec<(&str, u32)> {
        let mut terms = Vec::new();
        for (term, docs) in &self.postings {
            if let Some(tf) = docs.get(key) {
                terms.push((term.as_str(), *tf));
            }
        }
        terms
    }

    /// Replace the indexed text of `key`.
    ///
    /// Re-indexing the same key removes its previous postings first, so an
    /// incremental update never leaves stale terms behind.
    pub fn upsert(&mut self, key: &str, text: &str) {
        self.remove(key);
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return;
        }
        let mut frequencies: BTreeMap<String, u32> = BTreeMap::new();
        for token in tokens {
            *frequencies.entry(token).or_insert(0) += 1;
        }
        let length: u32 = frequencies.values().sum();
        for (term, tf) in frequencies {
            self.postings.entry(term).or_default().insert(key.to_owned(), tf);
        }
        self.lengths.insert(key.to_owned(), length);
        self.total_length = self.total_length.saturating_add(u64::from(length));
    }

    /// Drop `key` and all of its postings.
    pub fn remove(&mut self, key: &str) {
        let Some(length) = self.lengths.remove(key) else {
            return;
        };
        self.total_length = self.total_length.saturating_sub(u64::from(length));
        let mut emptied = Vec::new();
        for (term, docs) in &mut self.postings {
            docs.remove(key);
            if docs.is_empty() {
                emptied.push(term.clone());
            }
        }
        for term in emptied {
            self.postings.remove(&term);
        }
    }

    /// Inverse document frequency of one term.
    #[must_use]
    pub fn idf(&self, term: &str) -> f32 {
        let documents = self.lengths.len() as f32;
        let matching = self.postings.get(term).map_or(0, BTreeMap::len) as f32;
        if documents == 0.0 {
            return 0.0;
        }
        (1.0 + (documents - matching + 0.5) / (matching + 0.5)).ln()
    }

    /// Rank every unit matching at least one query term.
    ///
    /// Ties are broken by unit key, so the ranking is deterministic for a
    /// given index generation.
    #[must_use]
    pub fn search(&self, text: &str) -> Vec<LexicalHit> {
        let query_terms: Vec<String> = tokenize(text);
        if query_terms.is_empty() || self.lengths.is_empty() {
            return Vec::new();
        }

        let average = self.average_length().max(f32::EPSILON);
        let mut scores: BTreeMap<&str, (f32, Vec<String>)> = BTreeMap::new();
        let mut seen_terms: BTreeMap<&str, ()> = BTreeMap::new();

        for term in &query_terms {
            if seen_terms.insert(term.as_str(), ()).is_some() {
                continue;
            }
            let Some(docs) = self.postings.get(term) else {
                continue;
            };
            let idf = self.idf(term);
            for (key, tf) in docs {
                let length = self.lengths.get(key).copied().unwrap_or(0) as f32;
                let tf = *tf as f32;
                let numerator = tf * (self.k1 + 1.0);
                let denominator = tf + self.k1 * (1.0 - self.b + self.b * length / average);
                let contribution = idf * numerator / denominator.max(f32::EPSILON);
                let entry = scores.entry(key.as_str()).or_insert((0.0, Vec::new()));
                entry.0 += contribution;
                entry.1.push(term.clone());
            }
        }

        let mut hits: Vec<LexicalHit> = scores
            .into_iter()
            .map(|(key, (score, matched_terms))| LexicalHit {
                key: key.to_owned(),
                score,
                matched_terms,
            })
            .collect();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.key.cmp(&right.key))
        });
        hits
    }

    /// Units sharing the most terms with `text`, used as a related-units
    /// signal when a query has too few direct hits.
    #[must_use]
    pub fn related(&self, text: &str, exclude: &str, limit: usize) -> Vec<LexicalHit> {
        let mut hits: Vec<LexicalHit> = self
            .search(text)
            .into_iter()
            .filter(|hit| hit.key != exclude)
            .collect();
        hits.truncate(limit);
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> LexicalIndex {
        let mut index = LexicalIndex::default();
        index.upsert("a", "fn build_index() { hybrid_retriever }");
        index.upsert("b", "fn hybrid_search() { hybrid retriever }");
        index.upsert("c", "fn unrelated() { parse modules }");
        index
    }

    #[test]
    fn ranks_units_that_mention_more_query_terms_first() {
        let hits = index().search("hybrid retriever");
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.key != "c"));
        // `b` mentions `hybrid` twice and `retriever` once; `a` mentions
        // `hybrid` and `retriever` once each, and `b` is shorter per token
        // because `a` repeats `build`/`index`.
        assert_eq!(hits[0].key, "b");
        assert_eq!(hits[0].matched_terms, vec!["hybrid", "retriever"]);
    }

    #[test]
    fn reindexing_replaces_previous_postings() {
        let mut index = index();
        index.upsert("a", "fn completely_different_symbol()");
        assert!(index.search("build_index").iter().all(|hit| hit.key != "a"));
        assert!(
            index
                .search("completely different symbol")
                .iter()
                .any(|hit| hit.key == "a")
        );
        assert_eq!(index.unit_count(), 3);
    }

    #[test]
    fn removal_drops_empty_postings_and_keeps_lengths_consistent() {
        let mut index = index();
        index.remove("c");
        assert_eq!(index.unit_count(), 2);
        assert!(index.search("modules").is_empty());
        assert!(index.average_length() > 0.0);
    }

    #[test]
    fn unknown_terms_produce_no_hits() {
        assert!(index().search("nonexistent").is_empty());
        assert!(LexicalIndex::default().search("anything").is_empty());
    }

    #[test]
    fn camel_and_snake_spellings_match_the_same_token() {
        let mut index = LexicalIndex::default();
        index.upsert("a", "pub struct HybridRetriever;");
        assert_eq!(index.search("hybrid_retriever")[0].key, "a");
    }

    #[test]
    fn scores_are_deterministic_for_equal_inputs() {
        let index = index();
        let first = index.search("hybrid");
        let second = index.search("hybrid");
        assert_eq!(first, second);
        assert_eq!(
            index.related("hybrid", "b", 2).len(),
            1,
            "the excluded unit is filtered out"
        );
        assert_eq!(index.terms("b").len(), 4);
        assert!(index.contains("b"));
    }
}
