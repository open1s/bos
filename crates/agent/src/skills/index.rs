//! Metadata-only Skill catalogue: retrieval, dependency resolution and lazy
//! instruction loading.
//!
//! Discovery operates on [SkillMetadata], never on instruction bodies. A
//! selected Skill is resolved through [SkillIndex::load], which opens exactly
//! one manifest, verifies its digest against the indexed entry and returns the
//! body. This is the boundary issue #276 asks for: a catalogue can rank every
//! Skill without retaining any of their instructions.

use super::{
    digest_bytes, SkillContent, SkillDependency, SkillLoader, SkillMetadata, SkillMetadataError,
    SkillSource, MAX_SKILL_BYTES,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// A catalogue of Skill metadata, keyed by name.
#[derive(Debug, Clone, Default)]
pub struct SkillIndex {
    entries: BTreeMap<String, SkillMetadata>,
}

impl SkillIndex {
    /// Build an index from already-parsed metadata.
    pub fn from_metadata(entries: impl IntoIterator<Item = SkillMetadata>) -> Self {
        let mut by_name = BTreeMap::new();
        for entry in entries {
            by_name.insert(entry.name.clone(), entry);
        }
        Self { entries: by_name }
    }

    /// Discover a directory's Skills into a metadata-only index.
    pub fn from_dir(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let mut loader = SkillLoader::new(dir.into());
        let metadata = loader.discover()?;
        Ok(Self::from_metadata(metadata))
    }

    /// Add or replace one metadata entry.
    pub fn insert(&mut self, metadata: SkillMetadata) {
        self.entries.insert(metadata.name.clone(), metadata);
    }

    /// Remove one entry, returning it when it was present.
    pub fn remove(&mut self, name: &str) -> Option<SkillMetadata> {
        self.entries.remove(name)
    }

    /// One entry by exact name.
    pub fn get(&self, name: &str) -> Option<&SkillMetadata> {
        self.entries.get(name)
    }

    /// Every entry, name-ordered.
    pub fn entries(&self) -> impl Iterator<Item = &SkillMetadata> {
        self.entries.values()
    }

    /// Every name, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    /// Number of catalogued Skills.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalogue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve one Skill's instruction body, verifies it still matches the
    /// indexed identity, and returns it.
    pub fn load(&self, name: &str) -> Result<SkillContent, SkillLoadError> {
        let metadata = self
            .entries
            .get(name)
            .ok_or_else(|| SkillLoadError::NotFound(name.to_string()))?;
        let bytes = std::fs::read(&metadata.path).map_err(|source| SkillLoadError::Io {
            name: name.to_string(),
            source,
        })?;
        if bytes.len() > MAX_SKILL_BYTES {
            return Err(SkillLoadError::TooLarge {
                name: name.to_string(),
                limit: MAX_SKILL_BYTES,
                actual: bytes.len(),
            });
        }
        let actual = digest_bytes(&bytes);
        if !metadata.digest.is_empty() && actual != metadata.digest {
            return Err(SkillLoadError::Stale {
                name: name.to_string(),
                expected: metadata.digest.clone(),
                actual,
            });
        }
        let content = String::from_utf8(bytes).map_err(|_| SkillLoadError::Manifest {
            name: name.to_string(),
            source: SkillMetadataError::Frontmatter("manifest is not valid UTF-8".to_string()),
        })?;
        let skill_dir = metadata
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Ok(SkillContent {
            metadata: metadata.clone(),
            instructions: SkillLoader::extract_body(&content),
            skill_dir,
        })
    }

    /// Rank metadata-only matches for a query.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SkillMatch> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<SkillMatch> = self
            .entries
            .values()
            .map(|metadata| SkillMatch {
                metadata: metadata.clone(),
                score: score_metadata(query, metadata),
                semantic: None,
            })
            .filter(|matched| matched.score > 0)
            .collect();
        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.metadata.name.cmp(&right.metadata.name))
        });
        matches.truncate(limit);
        matches
    }

    /// Hybrid retrieval: lexical candidates plus an optional semantic stage.
    ///
    /// The semantic stage can only ADD candidates; lexical scoring remains the
    /// baseline and the fallback. A provider error, a malformed vector or a
    /// timeout returns the lexical ranking unchanged.
    pub fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
        semantic: Option<&dyn SemanticIndex>,
    ) -> Vec<SkillMatch> {
        let lexical = self.search(query, usize::MAX);
        let Some(provider) = semantic else {
            let mut lexical = lexical;
            lexical.truncate(limit);
            return lexical;
        };
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut texts = Vec::with_capacity(self.len() + 1);
        texts.push(query.to_string());
        for metadata in self.entries.values() {
            texts.push(embedding_text(metadata));
        }
        let vectors = match provider.embed(&texts) {
            Ok(vectors) if vectors.len() == texts.len() => vectors,
            _ => {
                let mut lexical = lexical;
                lexical.truncate(limit);
                return lexical;
            }
        };
        let Some(query_vector) = vectors.first() else {
            return Vec::new();
        };
        if query_vector.is_empty() {
            let mut lexical = lexical;
            lexical.truncate(limit);
            return lexical;
        }

        let mut by_name: HashMap<String, SkillMatch> = lexical
            .into_iter()
            .map(|matched| (matched.metadata.name.clone(), matched))
            .collect();
        for (offset, metadata) in self.entries.values().enumerate() {
            let Some(vector) = vectors.get(offset + 1) else {
                continue;
            };
            if vector.len() != query_vector.len() {
                continue;
            }
            let similarity = cosine(query_vector, vector);
            let matched = by_name
                .entry(metadata.name.clone())
                .or_insert_with(|| SkillMatch {
                    metadata: metadata.clone(),
                    score: 0,
                    semantic: None,
                });
            matched.semantic = Some(similarity);
        }

        let mut matches: Vec<SkillMatch> = by_name.into_values().collect();
        // Documented weights: the lexical score is the primary signal, and a
        // semantic similarity contributes at most 30 points, so an exact name
        // match (120) is never displaced by a merely related embedding.
        matches.sort_by(|left, right| {
            let left_score = left.score as f64 + f64::from(left.semantic.unwrap_or(0.0)) * 30.0;
            let right_score = right.score as f64 + f64::from(right.semantic.unwrap_or(0.0)) * 30.0;
            right_score
                .partial_cmp(&left_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.metadata.name.cmp(&right.metadata.name))
        });
        matches.truncate(limit);
        matches
    }

    /// Resolve a dependency-first activation plan from the requested roots.
    ///
    /// The plan is deterministic: roots and dependencies are visited in name
    /// order, duplicate edges are collapsed, and a missing required dependency,
    /// an incompatible version or a declared conflict is an error before
    /// anything is activated. An optional missing dependency is omitted with a
    /// recorded reason.
    pub fn resolve(&self, roots: &[String]) -> Result<SkillPlan, SkillPlanError> {
        let mut selected: BTreeSet<String> = BTreeSet::new();
        let mut omitted: Vec<OmittedDependency> = Vec::new();
        let mut order: Vec<String> = Vec::new();
        let mut state: HashMap<String, Visit> = HashMap::new();
        let mut path: Vec<String> = Vec::new();

        let mut roots: Vec<String> = roots.to_vec();
        roots.sort();
        roots.dedup();

        for root in &roots {
            if self.get(root).is_none() {
                return Err(SkillPlanError::UnknownRoot(root.clone()));
            }
            self.visit(root, None, &mut state, &mut path, &mut order, &mut omitted)?;
        }

        selected.extend(order.iter().cloned());
        for name in &order {
            let Some(entry) = self.get(name) else {
                continue;
            };
            for conflict in &entry.conflicts {
                if selected.contains(conflict) {
                    return Err(SkillPlanError::Conflict {
                        left: name.clone(),
                        right: conflict.clone(),
                    });
                }
            }
        }

        Ok(SkillPlan {
            roots,
            order,
            omitted,
        })
    }

    fn visit(
        &self,
        name: &str,
        owner: Option<&str>,
        state: &mut HashMap<String, Visit>,
        path: &mut Vec<String>,
        order: &mut Vec<String>,
        omitted: &mut Vec<OmittedDependency>,
    ) -> Result<(), SkillPlanError> {
        match state.get(name) {
            Some(Visit::Done) => return Ok(()),
            Some(Visit::Visiting) => {
                let mut cycle = path.clone();
                cycle.push(name.to_string());
                return Err(SkillPlanError::Cycle(cycle));
            }
            None => {}
        }
        let Some(entry) = self.get(name) else {
            // The caller checks roots; a dependency that is absent reaches
            // here through the owner's edge.
            return Err(SkillPlanError::Missing {
                owner: owner.unwrap_or("").to_string(),
                name: name.to_string(),
            });
        };
        state.insert(name.to_string(), Visit::Visiting);
        path.push(name.to_string());
        for dependency in &entry.dependencies {
            if let Some(target) = self.dependency_target(name, dependency, omitted)? {
                self.visit(&target, Some(name), state, path, order, omitted)?;
            }
        }
        path.pop();
        state.insert(name.to_string(), Visit::Done);
        order.push(name.to_string());
        Ok(())
    }

    /// Decide one dependency edge: required present, optional omitted, or a
    /// version refusal. Returns the target name to visit.
    fn dependency_target(
        &self,
        owner: &str,
        dependency: &SkillDependency,
        omitted: &mut Vec<OmittedDependency>,
    ) -> Result<Option<String>, SkillPlanError> {
        let Some(target) = self.get(&dependency.name) else {
            if dependency.optional {
                omitted.push(OmittedDependency {
                    owner: owner.to_string(),
                    name: dependency.name.clone(),
                    reason: "not installed".to_string(),
                });
                return Ok(None);
            }
            return Err(SkillPlanError::Missing {
                owner: owner.to_string(),
                name: dependency.name.clone(),
            });
        };
        if let Some(constraint) = &dependency.version {
            if !target.version.satisfies(constraint) {
                if dependency.optional {
                    omitted.push(OmittedDependency {
                        owner: owner.to_string(),
                        name: dependency.name.clone(),
                        reason: format!(
                            "version {} does not satisfy {constraint}",
                            target.version.display()
                        ),
                    });
                    return Ok(None);
                }
                return Err(SkillPlanError::Incompatible {
                    owner: owner.to_string(),
                    name: dependency.name.clone(),
                    constraint: constraint.clone(),
                    actual: target.version.display(),
                });
            }
        }
        Ok(Some(dependency.name.clone()))
    }
}

/// One Skill descriptor discovered from a federated peer or registry.
///
/// Discovery carries metadata only: a remote instruction body is never part of
/// this value, and accepting the descriptor does not authorize a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkill {
    /// Metadata advertised by the peer.
    pub metadata: SkillMetadata,
    /// Optional expiry, in epoch milliseconds. None never expires.
    pub expires_at_ms: Option<u64>,
    /// The peer or registry that advertised it.
    pub peer: String,
}

/// What merging one remote descriptor did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteAdmission {
    /// The descriptor joined the catalogue.
    Accepted,
    /// The descriptor had expired.
    Stale,
    /// An identical descriptor was already present.
    Duplicate,
    /// A local Skill owns this name.
    ShadowedLocal,
}

/// The outcome of one remote merge, in input order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteMergeReport {
    /// One entry per input descriptor, in the same order.
    pub admissions: Vec<(String, RemoteAdmission)>,
}

impl SkillIndex {
    /// Merge remote metadata deterministically.
    ///
    /// Local entries always win a name collision. Expired descriptors are not
    /// presented as current. A duplicate (same name, digest and version) is
    /// admitted once. A newer remote revision replaces an older remote one;
    /// equal versions keep the lexicographically earlier peer, so the result
    /// does not depend on arrival order.
    pub fn merge_remote(
        &mut self,
        remote: impl IntoIterator<Item = RemoteSkill>,
        now_ms: u64,
    ) -> RemoteMergeReport {
        let mut report = RemoteMergeReport::default();
        for candidate in remote {
            let name = candidate.metadata.name.clone();
            if candidate
                .expires_at_ms
                .is_some_and(|expiry| expiry <= now_ms)
            {
                report.admissions.push((name, RemoteAdmission::Stale));
                continue;
            }
            match self.entries.get(&name) {
                Some(existing) if existing.source != SkillSource::Federated => {
                    report
                        .admissions
                        .push((name, RemoteAdmission::ShadowedLocal));
                }
                Some(existing) if existing.digest == candidate.metadata.digest => {
                    report.admissions.push((name, RemoteAdmission::Duplicate));
                }
                Some(existing) => {
                    let newer = candidate.metadata.version > existing.version;
                    let same_version = candidate.metadata.version == existing.version;
                    let earlier_peer = same_version
                        && candidate
                            .peer
                            .cmp(&existing.publisher.clone().unwrap_or_default())
                            .is_lt();
                    if !newer && !earlier_peer {
                        report.admissions.push((name, RemoteAdmission::Duplicate));
                        continue;
                    }
                    let mut metadata = candidate.metadata;
                    metadata.source = SkillSource::Federated;
                    if metadata.publisher.is_none() {
                        metadata.publisher = Some(candidate.peer);
                    }
                    self.entries.insert(name.clone(), metadata);
                    report.admissions.push((name, RemoteAdmission::Accepted));
                }
                None => {
                    let mut metadata = candidate.metadata;
                    metadata.source = SkillSource::Federated;
                    if metadata.publisher.is_none() {
                        metadata.publisher = Some(candidate.peer);
                    }
                    self.entries.insert(name.clone(), metadata);
                    report.admissions.push((name, RemoteAdmission::Accepted));
                }
            }
        }
        report
    }
}

/// One ranked Skill result, metadata only.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMatch {
    /// The catalogue entry that matched.
    pub metadata: SkillMetadata,
    /// Lexical score, primary signal.
    pub score: i64,
    /// Cosine similarity when a semantic stage supplied one.
    pub semantic: Option<f32>,
}

/// A provider-neutral embedding stage.
///
/// The Skill subsystem never depends on a vendor SDK or a vector database: a
/// host may implement this over any model, and returning an error (or a
/// malformed vector) makes retrieval fall back to lexical ranking.
pub trait SemanticIndex {
    /// Embed each text, returning one vector per input in the same order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SemanticError>;
}

/// Why a semantic stage could not contribute.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("semantic retrieval unavailable: {0}")]
pub struct SemanticError(pub String);

/// What a dependency-first plan selected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillPlan {
    /// The requested roots, sorted.
    pub roots: Vec<String>,
    /// Dependency-first activation order.
    pub order: Vec<String>,
    /// Optional dependencies that were deliberately skipped.
    pub omitted: Vec<OmittedDependency>,
}

/// One optional dependency the plan left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmittedDependency {
    /// The Skill whose edge was optional.
    pub owner: String,
    /// The absent or incompatible dependency.
    pub name: String,
    /// Why it was omitted.
    pub reason: String,
}

/// Why a plan could not be produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillPlanError {
    /// A requested root is not in the catalogue.
    #[error("skill '{0}' is not in the catalogue")]
    UnknownRoot(String),
    /// A required dependency is absent.
    #[error("skill '{owner}' requires '{name}', which is not installed")]
    Missing {
        /// The Skill that declared the edge.
        owner: String,
        /// The absent dependency.
        name: String,
    },
    /// A dependency's version does not satisfy the declared constraint.
    #[error("skill '{owner}' requires '{name}' {constraint}, found {actual}")]
    Incompatible {
        /// The Skill that declared the edge.
        owner: String,
        /// The dependency.
        name: String,
        /// The declared constraint.
        constraint: String,
        /// The installed version.
        actual: String,
    },
    /// The dependency graph contains a cycle.
    #[error("skill dependency cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    /// Two selected Skills declare each other mutually exclusive.
    #[error("skills '{left}' and '{right}' conflict")]
    Conflict {
        /// One side of the conflict.
        left: String,
        /// The other side.
        right: String,
    },
}

/// Why a selected Skill's body could not be read.
#[derive(Debug, thiserror::Error)]
pub enum SkillLoadError {
    /// The catalogue has no such entry.
    #[error("skill '{0}' is not in the catalogue")]
    NotFound(String),
    /// The manifest could not be read.
    #[error("could not read skill '{name}': {source}")]
    Io {
        /// The Skill name.
        name: String,
        /// The filesystem error.
        source: std::io::Error,
    },
    /// The manifest grew past the loader's bound.
    #[error("skill '{name}' is {actual} bytes; the limit is {limit}")]
    TooLarge {
        /// The Skill name.
        name: String,
        /// The configured limit.
        limit: usize,
        /// The observed size.
        actual: usize,
    },
    /// The manifest no longer matches the indexed digest.
    #[error("skill '{name}' changed after discovery (expected {expected}, found {actual})")]
    Stale {
        /// The Skill name.
        name: String,
        /// Digest recorded at discovery.
        expected: String,
        /// Digest observed at load.
        actual: String,
    },
    /// Re-parsing the manifest failed.
    #[error("skill '{name}' could not be parsed: {source}")]
    Manifest {
        /// The Skill name.
        name: String,
        /// The parse error.
        source: SkillMetadataError,
    },
}

/// Whether a Skill's source is allowed, and for whom.
#[derive(Debug, Clone)]
pub struct TrustPolicy {
    /// Sources the host accepts.
    pub allowed_sources: BTreeSet<SkillSource>,
    /// Whether an installed third-party or federated Skill must name its
    /// publisher before its instructions may enter model context.
    pub require_publisher: bool,
}

impl Default for TrustPolicy {
    fn default() -> Self {
        Self {
            // Local and shipped Skills are trusted by default. Third-party and
            // federated sources are opt-in, which is the conservative default
            // for instruction supply chains.
            allowed_sources: BTreeSet::from([SkillSource::User, SkillSource::Builtin]),
            require_publisher: true,
        }
    }
}

/// Who is asking to use a Skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillPermission {
    /// Model-driven selection through find_skill/load_skill.
    Model,
    /// The reader's explicit /name gesture.
    User,
}

impl TrustPolicy {
    /// Check provenance and invocation policy before a Skill is activated.
    pub fn authorize(
        &self,
        metadata: &SkillMetadata,
        permission: SkillPermission,
    ) -> Result<(), TrustError> {
        if permission == SkillPermission::Model && metadata.user_only {
            return Err(TrustError::UserOnly(metadata.name.clone()));
        }
        if !self.allowed_sources.contains(&metadata.source) {
            return Err(TrustError::SourceDenied {
                name: metadata.name.clone(),
                origin: metadata.source,
            });
        }
        if self.require_publisher
            && matches!(
                metadata.source,
                SkillSource::ThirdParty | SkillSource::Federated
            )
            && metadata.publisher.is_none()
        {
            return Err(TrustError::PublisherRequired(metadata.name.clone()));
        }
        Ok(())
    }
}

/// Why a Skill was refused by trust policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    /// A user-only Skill cannot be model-selected.
    #[error("skill '{0}' is reserved for user invocation")]
    UserOnly(String),
    /// The host policy does not allow this source.
    #[error("skill '{name}' comes from a denied source ({origin})", origin = .origin.as_str())]
    SourceDenied {
        /// The Skill name.
        name: String,
        /// The denied source.
        origin: SkillSource,
    },
    /// A third-party or federated Skill did not declare its publisher.
    #[error("skill '{0}' must declare a publisher before activation")]
    PublisherRequired(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Visiting,
    Done,
}

/// Tokenize a query or document for lexical matching.
///
/// Tokens are lowercased, split on non-alphanumeric characters, and single
/// ASCII characters are dropped as noise. A non-ASCII run (for example a
/// Chinese phrase with no spaces) contributes its consecutive character
/// bigrams, which is a deliberate, documented lexical approximation - not a
/// substitute for the optional semantic stage.
pub fn terms(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for word in lowered.split(|character: char| !character.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let length = word.chars().count();
        if length == 1 && word.is_ascii() {
            continue;
        }
        out.push(word.to_string());
        if !word.is_ascii() && length >= 2 {
            let chars: Vec<char> = word.chars().collect();
            for window in chars.windows(2) {
                out.push(window.iter().collect());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Lexical relevance of one Skill to a query.
///
/// The weights are part of the contract: exact name 120, name prefix 60, name
/// term 25, tag 18, category 12, description 6. A name match therefore always
/// outranks a weak metadata match.
pub fn score_metadata(query: &str, metadata: &SkillMetadata) -> i64 {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return 0;
    }
    let name = metadata.name.to_lowercase();
    let description = metadata.description.to_lowercase();
    let category = metadata.category.as_str();
    let tags: Vec<String> = metadata.tags.iter().map(|tag| tag.to_lowercase()).collect();
    let mut score = 0i64;
    if name == normalized {
        score += 120;
    } else if name.starts_with(&normalized) {
        score += 60;
    }
    for term in terms(query) {
        if name.contains(&term) {
            score += 25;
        }
        if tags.iter().any(|tag| tag == &term) {
            score += 18;
        } else if tags.iter().any(|tag| tag.contains(&term)) {
            score += 10;
        }
        if category.contains(&term) {
            score += 12;
        }
        if description.contains(&term) {
            score += 6;
        }
    }
    score
}

/// The metadata fields an embedding provider may see.
///
/// Instructions, paths, publishers and digests are deliberately excluded.
fn embedding_text(metadata: &SkillMetadata) -> String {
    format!(
        "{} {} {} {}",
        metadata.name,
        metadata.description,
        metadata.category.as_str(),
        metadata.tags.join(" ")
    )
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let left_norm: f32 = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm: f32 = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm * right_norm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillCategory, SkillDependency, SkillVersion};

    fn metadata(name: &str, description: &str) -> SkillMetadata {
        SkillMetadata::new(
            name.to_string(),
            description.to_string(),
            PathBuf::from("/skills").join(name).join("SKILL.md"),
        )
    }

    struct FakeSemantic {
        fail: bool,
    }

    impl SemanticIndex for FakeSemantic {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SemanticError> {
            if self.fail {
                return Err(SemanticError("provider offline".to_string()));
            }
            Ok(texts
                .iter()
                .map(|text| {
                    let shipping = ["ship", "deploy", "release"]
                        .iter()
                        .any(|word| text.contains(word));
                    if shipping {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }
    }

    #[test]
    fn retrieval_prefers_an_exact_name_then_tags_then_description() {
        let mut named = metadata("git-commit", "Write a commit message");
        named.category = SkillCategory::Code;
        let mut tagged = metadata("release", "Publish a build");
        tagged.tags = vec!["git".to_string(), "commit".to_string()];
        let described = metadata("notes", "This text mentions git commit in passing");
        let index = SkillIndex::from_metadata([named, tagged, described]);

        let matches = index.search("git commit", 3);
        assert_eq!(matches[0].metadata.name, "git-commit");
        assert!(
            matches[1].metadata.name == "release",
            "tags outrank a weak description match: {matches:?}"
        );
        assert_eq!(matches[2].metadata.name, "notes");
        assert!(matches[0].score > matches[1].score);
    }

    #[test]
    fn a_unicode_query_matches_a_phrase_without_spaces() {
        let index = SkillIndex::from_metadata([metadata("deploy", "部署服务到集群并检查健康状态")]);
        let matches = index.search("部署", 3);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].score > 0);
        assert!(
            index.search("健康", 3).len() == 1,
            "a bigram inside the phrase also finds it"
        );
    }

    #[test]
    fn hybrid_retrieval_adds_semantic_candidates_and_falls_back_on_error() {
        let index = SkillIndex::from_metadata([
            metadata("release-deploy", "Publish a build"),
            metadata("cooking", "Bake bread"),
        ]);
        let matches = index.hybrid_search("ship it", 3, Some(&FakeSemantic { fail: false }));
        assert_eq!(matches[0].metadata.name, "release-deploy");
        assert!(matches[0].semantic.unwrap_or(0.0) > 0.5);

        let lexical = index.search("ship it", 3);
        let fallback = index.hybrid_search("ship it", 3, Some(&FakeSemantic { fail: true }));
        assert_eq!(
            fallback.len(),
            lexical.len(),
            "a failed provider contributes nothing extra"
        );
        assert!(fallback.is_empty(), "no lexical match for that wording");
    }

    fn dep(name: &str, version: Option<&str>, optional: bool) -> SkillDependency {
        SkillDependency {
            name: name.to_string(),
            version: version.map(str::to_string),
            optional,
        }
    }

    #[test]
    fn resolution_is_dependency_first_deterministic_and_records_optional_omissions() {
        let mut base = metadata("base", "base skill");
        base.version = SkillVersion::new(1, 2, 0);
        let mut audit = metadata("audit", "audit skill");
        audit.version = SkillVersion::new(1, 3, 0);
        let mut app = metadata("app", "app skill");
        app.dependencies = vec![
            dep("audit", Some(">=1.2.0"), false),
            dep("base", None, false),
            dep("missing-tool", None, true),
        ];
        let index = SkillIndex::from_metadata([base, app, audit]);

        let plan = index.resolve(&["app".to_string()]).expect("plan");
        assert_eq!(plan.order, vec!["audit", "base", "app"]);
        assert_eq!(plan.omitted.len(), 1);
        assert_eq!(plan.omitted[0].name, "missing-tool");
        assert_eq!(plan.omitted[0].owner, "app");
        assert_eq!(plan.roots, vec!["app"]);
    }

    #[test]
    fn a_missing_required_dependency_is_refused() {
        let mut app = metadata("app", "app skill");
        app.dependencies = vec![dep("absent", None, false)];
        let index = SkillIndex::from_metadata([app]);
        assert_eq!(
            index.resolve(&["app".to_string()]),
            Err(SkillPlanError::Missing {
                owner: "app".to_string(),
                name: "absent".to_string(),
            })
        );
    }

    #[test]
    fn an_incompatible_version_is_refused() {
        let base = metadata("base", "base");
        let mut app = metadata("app", "app");
        app.dependencies = vec![dep("base", Some(">=2.0.0"), false)];
        let index = SkillIndex::from_metadata([base, app]);
        assert!(matches!(
            index.resolve(&["app".to_string()]),
            Err(SkillPlanError::Incompatible { .. })
        ));
    }

    #[test]
    fn cycles_are_detected_before_activation() {
        let mut a = metadata("a", "a");
        a.dependencies = vec![dep("b", None, false)];
        let mut b = metadata("b", "b");
        b.dependencies = vec![dep("a", None, false)];
        let index = SkillIndex::from_metadata([a, b]);
        assert!(matches!(
            index.resolve(&["a".to_string()]),
            Err(SkillPlanError::Cycle(_))
        ));
    }

    #[test]
    fn declared_conflicts_are_refused() {
        let base = metadata("base", "base");
        let mut app = metadata("app", "app");
        app.dependencies = vec![dep("base", None, false)];
        app.conflicts = vec!["base".to_string()];
        let index = SkillIndex::from_metadata([base, app]);
        assert_eq!(
            index.resolve(&["app".to_string()]),
            Err(SkillPlanError::Conflict {
                left: "app".to_string(),
                right: "base".to_string(),
            })
        );
    }

    #[test]
    fn loading_reads_only_the_selected_manifest_and_refuses_stale_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (name, body) in [("one", "ONE"), ("two", "TWO")] {
            let skill_dir = dir.path().join(name);
            std::fs::create_dir_all(&skill_dir).expect("dir");
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}"),
            )
            .expect("write");
        }
        let index = SkillIndex::from_dir(dir.path()).expect("index");
        assert_eq!(index.len(), 2, "discovery keeps metadata, not instructions");
        let loaded = index.load("one").expect("load one");
        assert_eq!(loaded.instructions, "ONE");
        assert!(!loaded.instructions.contains("TWO"));

        // A manifest edited between discovery and activation is refused: the
        // index entry's digest no longer matches the file.
        std::fs::write(
            dir.path().join("two").join("SKILL.md"),
            "---\nname: two\ndescription: two skill\n---\nCHANGED",
        )
        .expect("rewrite");
        assert!(matches!(
            index.load("two"),
            Err(SkillLoadError::Stale { .. })
        ));
    }

    #[test]
    fn trust_policy_gates_sources_user_only_and_publisher() {
        let mut third_party = metadata("pkg", "package skill");
        third_party.source = SkillSource::ThirdParty;
        let mut reserved = metadata("mine", "reserved");
        reserved.user_only = true;
        let local = metadata("local", "local");
        let policy = TrustPolicy::default();

        assert_eq!(
            policy.authorize(&reserved, SkillPermission::Model),
            Err(TrustError::UserOnly("mine".to_string()))
        );
        assert!(policy.authorize(&reserved, SkillPermission::User).is_ok());
        assert!(matches!(
            policy.authorize(&third_party, SkillPermission::Model),
            Err(TrustError::SourceDenied { .. })
        ));
        assert!(policy.authorize(&local, SkillPermission::Model).is_ok());

        third_party.publisher = Some("acme".to_string());
        let mut widened = TrustPolicy::default();
        widened.allowed_sources.insert(SkillSource::ThirdParty);
        assert!(widened
            .authorize(&third_party, SkillPermission::Model)
            .is_ok());
    }

    fn remote(name: &str, version: (u32, u32, u32), digest: &str, peer: &str) -> RemoteSkill {
        let mut metadata = metadata(name, "remote skill");
        metadata.version = SkillVersion::new(version.0, version.1, version.2);
        metadata.digest = digest.to_string();
        metadata.source = SkillSource::Federated;
        RemoteSkill {
            metadata,
            expires_at_ms: None,
            peer: peer.to_string(),
        }
    }

    /// A small representative corpus with a measured baseline. The numbers are
    /// asserted, not just printed, so a ranking change that regresses recall
    /// fails the suite; run with --nocapture to read the report.
    #[test]
    fn the_retrieval_corpus_reports_precision_recall_and_mrr() {
        let mut commit = metadata("git-commit", "Write a clear commit message");
        commit.tags = vec!["git".to_string(), "commit".to_string()];
        let mut security = metadata("sec-review", "Review a change for security");
        security.tags = vec!["security".to_string(), "review".to_string()];
        let mut deploy = metadata("release", "Publish a build");
        deploy.tags = vec!["deploy".to_string(), "ship".to_string()];
        let docs = metadata("docs", "Write documentation");
        let tests = metadata("test-runner", "Run the test suite");
        let data = metadata("data-load", "Load a CSV into a table");
        let index = SkillIndex::from_metadata([commit, security, deploy, docs, tests, data]);

        let cases: [(&str, &str); 4] = [
            ("commit message", "git-commit"),
            ("security review", "sec-review"),
            ("ship the build", "release"),
            ("run tests", "test-runner"),
        ];
        let mut precision = 0.0;
        let mut recall = 0.0;
        let mut mrr = 0.0;
        for (query, expected) in cases {
            let ranked: Vec<String> = index
                .search(query, 3)
                .into_iter()
                .map(|matched| matched.metadata.name)
                .collect();
            let relevant = BTreeSet::from([expected.to_string()]);
            let metrics = super::super::runtime::retrieval_metrics(&ranked, &relevant, 3);
            eprintln!(
                "query={query:?} ranked={ranked:?} P@3={:.2} R@3={:.2} RR={:.2}",
                metrics.precision_at_k, metrics.recall_at_k, metrics.reciprocal_rank
            );
            assert!(metrics.recall_at_k > 0.0, "missed {expected} for {query:?}");
            precision += metrics.precision_at_k;
            recall += metrics.recall_at_k;
            mrr += metrics.reciprocal_rank;
        }
        let count = cases.len() as f64;
        eprintln!(
            "macro P@3={:.2} R@3={:.2} MRR={:.2}",
            precision / count,
            recall / count,
            mrr / count
        );
        // One relevant Skill per query means P@3 has a 1/3 ceiling; the
        // assertion is that every query still lands its result in the top 3.
        assert!(precision / count >= 0.32, "precision regressed");
        assert!(recall / count >= 0.99, "recall regressed");
        assert!(mrr / count >= 0.8, "rank regressed");
    }

    #[test]
    fn remote_metadata_is_governed_expiring_and_deterministic() {
        let mut index = SkillIndex::from_metadata([metadata("local", "local skill")]);
        let mut expired = remote("gone", (1, 0, 0), "d0", "peer-a");
        expired.expires_at_ms = Some(100);

        let report = index.merge_remote(
            vec![
                remote("local", (9, 0, 0), "d1", "peer-a"),
                expired,
                remote("new", (1, 0, 0), "d2", "peer-a"),
                remote("new", (1, 0, 0), "d2", "peer-a"),
            ],
            200,
        );
        assert_eq!(
            report.admissions,
            vec![
                ("local".to_string(), RemoteAdmission::ShadowedLocal),
                ("gone".to_string(), RemoteAdmission::Stale),
                ("new".to_string(), RemoteAdmission::Accepted),
                ("new".to_string(), RemoteAdmission::Duplicate),
            ]
        );
        assert_eq!(
            index.get("new").map(|entry| entry.source),
            Some(SkillSource::Federated)
        );
        assert_eq!(
            index
                .get("new")
                .and_then(|entry| entry.publisher.as_deref()),
            Some("peer-a")
        );

        // A newer revision replaces an older remote one.
        let report = index.merge_remote(vec![remote("new", (2, 0, 0), "d3", "peer-b")], 200);
        assert_eq!(
            report.admissions,
            vec![("new".to_string(), RemoteAdmission::Accepted)]
        );
        assert_eq!(index.get("new").map(|entry| entry.version.major), Some(2));

        // Equal versions keep the lexicographically earlier peer regardless of
        // arrival order.
        let mut first = SkillIndex::from_metadata([]);
        first.merge_remote(vec![remote("tie", (1, 0, 0), "d4", "peer-b")], 0);
        first.merge_remote(vec![remote("tie", (1, 0, 0), "d5", "peer-a")], 0);
        assert_eq!(
            first
                .get("tie")
                .and_then(|entry| entry.publisher.as_deref()),
            Some("peer-a")
        );
    }
}
