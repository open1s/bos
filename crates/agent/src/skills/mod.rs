//! Skills module - simplified skill loading and injection
//!
//! Provides basic skill loading from filesystem with minimal features.

/// Metadata-only Skill catalogue: retrieval and dependency resolution.
pub mod index;

/// Per-session skill activation and skill authoring validation.
pub mod session;

/// Task-scoped Skill runtime: plan, activate, compose and observe.
pub mod runtime;

pub use index::{
    SemanticError, SemanticIndex, SkillIndex, SkillLoadError, SkillMatch, SkillPermission,
    SkillPlan, SkillPlanError, TrustError, TrustPolicy,
};
pub use runtime::{
    retrieval_metrics, Activation, ActiveSkill, ComposedSkills, RetrievalMetrics, RuntimeError,
    SkillEvent, SkillLimits, SkillRuntime,
};

pub use session::{SkillActivation, SkillAuthoringRecord, SkillValidationError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Skill category for classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[qserde::Archive]
pub enum SkillCategory {
    /// Unclassified skill.
    #[default]
    Other,
    /// Writes or edits code.
    Code,
    /// Analyses code or data.
    Analysis,
    /// Transforms data sets.
    Data,
    /// Runs tests.
    Testing,
    /// General-purpose helper.
    Utility,
}

impl SkillCategory {
    /// Map a category name to its variant, defaulting to [`SkillCategory::Other`].
    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "code" => Self::Code,
            "analysis" => Self::Analysis,
            "data" => Self::Data,
            "testing" | "test" => Self::Testing,
            "utility" | "util" => Self::Utility,
            _ => Self::Other,
        }
    }

    /// Lowercase name for this category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::Code => "code",
            Self::Analysis => "analysis",
            Self::Data => "data",
            Self::Testing => "testing",
            Self::Utility => "utility",
        }
    }
}

/// Skill version
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[qserde::Archive]
pub struct SkillVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl SkillVersion {
    /// Create a version from its parts.
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Format the version as `major.minor.patch`.
    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// Parse exactly `major.minor.patch`.
    ///
    /// A malformed declaration is an error, never a silently substituted
    /// version: a consumer that checks compatibility must see the author's
    /// value or a refusal.
    pub fn parse(text: &str) -> Result<Self, SkillMetadataError> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(SkillMetadataError::BadVersion(text.to_string()));
        }
        let numeric = |part: &str| {
            part.parse::<u32>()
                .map_err(|_| SkillMetadataError::BadVersion(text.to_string()))
        };
        Ok(Self {
            major: numeric(parts[0])?,
            minor: numeric(parts[1])?,
            patch: numeric(parts[2])?,
        })
    }

    /// This version as a semver value, for constraint checks.
    pub fn to_semver(&self) -> semver::Version {
        semver::Version::new(self.major as u64, self.minor as u64, self.patch as u64)
    }

    /// Whether this version satisfies a declared constraint.
    pub fn satisfies(&self, constraint: &str) -> bool {
        semver::VersionReq::parse(constraint)
            .map(|req| req.matches(&self.to_semver()))
            .unwrap_or(false)
    }
}

/// Where a Skill came from, for provenance and trust policy.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default, Hash,
)]
#[serde(rename_all = "snake_case")]
#[qserde::Archive]
pub enum SkillSource {
    /// A Skill the reader authored or installed locally.
    #[default]
    User,
    /// A Skill shipped with the build.
    Builtin,
    /// A Skill installed from a third-party package or registry.
    ThirdParty,
    /// A Skill discovered from a federated peer.
    Federated,
}

impl SkillSource {
    /// Map a frontmatter source name to its variant.
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "builtin" | "built-in" => Self::Builtin,
            "third-party" | "third_party" | "thirdparty" | "package" => Self::ThirdParty,
            "federated" | "federation" | "remote" => Self::Federated,
            _ => Self::User,
        }
    }

    /// The stable name recorded in metadata and policy.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Builtin => "builtin",
            Self::ThirdParty => "third-party",
            Self::Federated => "federated",
        }
    }
}

/// A Skill this Skill requires or may use, with an optional version constraint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[qserde::Archive]
pub struct SkillDependency {
    /// The required Skill's name.
    pub name: String,
    /// A semver requirement, for example ">=1.2.0".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the plan may proceed without this dependency.
    #[serde(default)]
    pub optional: bool,
}

/// Why a Skill's frontmatter could not become metadata.
#[derive(Debug, thiserror::Error)]
pub enum SkillMetadataError {
    /// The file could not be read.
    #[error("could not read skill manifest: {0}")]
    Io(#[from] std::io::Error),
    /// The manifest exceeded the bounded size.
    #[error("skill manifest is {actual} bytes; the limit is {limit}")]
    TooLarge {
        /// Configured limit in bytes.
        limit: usize,
        /// Observed size in bytes.
        actual: usize,
    },
    /// The YAML frontmatter was missing or malformed.
    #[error("invalid skill frontmatter: {0}")]
    Frontmatter(String),
    /// The name was missing, blank, too long or contained whitespace.
    #[error("invalid skill name: {0}")]
    BadName(String),
    /// The description was missing, blank or too long.
    #[error("invalid skill description: {0}")]
    BadDescription(String),
    /// The declared version was malformed.
    #[error("invalid skill version: {0}")]
    BadVersion(String),
    /// Tags were not a string list.
    #[error("invalid skill tags: {0}")]
    BadTags(String),
    /// A dependency entry was malformed.
    #[error("invalid skill dependency: {0}")]
    BadDependency(String),
    /// Conflicts were not a string list.
    #[error("invalid skill conflicts: {0}")]
    BadConflicts(String),
}

/// The largest manifest a loader will read.
pub const MAX_SKILL_BYTES: usize = 1_048_576;

/// Metadata for a skill
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[qserde::Archive]
pub struct SkillMetadata {
    /// Unique skill name.
    pub name: String,
    /// One-line description shown to the model.
    pub description: String,
    /// Path to the skill definition file.
    #[rkyv(with = qserde::rkyv::with::AsString)]
    pub path: PathBuf,
    /// Classification used when listing skills.
    pub category: SkillCategory,
    /// Skill version.
    pub version: SkillVersion,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// Whether the manifest restricts this skill to user invocation.
    ///
    /// The key is `disable-model-invocation: true` in the `SKILL.md`
    /// frontmatter - the same field Claude Code and Codex skills use to keep a
    /// skill out of model-driven selection. A host that offers a skill list
    /// reads this flag to label the entry; the reader is the only one who may
    /// choose it.
    #[serde(default)]
    pub user_only: bool,
    /// Skills this one requires (or may optionally use), with version
    /// constraints. Empty means the Skill stands alone.
    #[serde(default)]
    pub dependencies: Vec<SkillDependency>,
    /// Skill names this one declares mutually exclusive with itself.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Where the Skill came from, for provenance and trust policy.
    #[serde(default)]
    pub source: SkillSource,
    /// The publisher or owner identity, when one is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// SHA-256 of the manifest bytes this metadata was parsed from.
    ///
    /// Lazy loading re-computes it, so a file changed between discovery and
    /// activation is refused rather than serving a definition that no longer
    /// matches its index entry.
    #[serde(default)]
    pub digest: String,
}

impl SkillMetadata {
    /// Create metadata with the `Other` category, version 1.0.0, and no tags.
    pub fn new(name: String, description: String, path: PathBuf) -> Self {
        Self {
            name,
            description,
            path,
            category: SkillCategory::Other,
            version: SkillVersion::new(1, 0, 0),
            tags: Vec::new(),
            user_only: false,
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            source: SkillSource::User,
            publisher: None,
            digest: String::new(),
        }
    }
}

/// Content of a skill including instructions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[qserde::Archive]
pub struct SkillContent {
    /// Metadata discovered for the skill.
    pub metadata: SkillMetadata,
    /// Skill body, with frontmatter stripped.
    pub instructions: String,
    /// Directory containing the skill.
    #[rkyv(with = qserde::rkyv::with::AsString)]
    pub skill_dir: PathBuf,
}

/// Skill loader - discovers and loads skills from filesystem
pub struct SkillLoader {
    skills_dir: PathBuf,
    discovered: std::collections::HashMap<String, SkillMetadata>,
}

impl SkillLoader {
    /// Create a loader rooted at `skills_dir`.
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            discovered: std::collections::HashMap::new(),
        }
    }

    /// Discover skills in the skills directory.
    ///
    /// The catalogue is NAME-ORDERED, not hash-ordered. `discovered` is a map
    /// keyed by skill name, so iterating it directly returns whatever order the
    /// map's hash seed happens to produce - a different order for every loader,
    /// and therefore for every call. A caller that draws these rows or registers
    /// them per frame would reorder its list under the pointer on each redraw, so
    /// the order is pinned here once rather than left to the map.
    pub fn discover(&mut self) -> std::io::Result<Vec<SkillMetadata>> {
        if !self.skills_dir.exists() {
            return Ok(Vec::new());
        }

        for entry in std::fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }

            let skill_file = skill_dir.join("SKILL.md");
            if skill_file.exists() {
                match Self::parse_metadata_checked(&skill_file) {
                    Ok(meta) => {
                        self.discovered.insert(meta.name.clone(), meta);
                    }
                    Err(error) => {
                        // A malformed manifest is skipped as a catalogue
                        // entry, but the reason is reported rather than
                        // silently inventing metadata for it.
                        log::warn!("skipping skill {}: {error}", skill_file.display());
                    }
                }
            }
        }

        let mut discovered: Vec<SkillMetadata> = self.discovered.values().cloned().collect();
        discovered.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(discovered)
    }

    /// Load a skill by name
    pub fn load(&self, name: &str) -> Option<SkillContent> {
        let meta = self.discovered.get(name)?.clone();
        let content = std::fs::read_to_string(&meta.path).ok()?;
        let instructions = Self::extract_body(&content);
        let skill_dir = meta.path.parent()?.to_path_buf();
        Some(SkillContent {
            metadata: meta,
            instructions,
            skill_dir,
        })
    }

    /// List all discovered skills, in the same name order as [`Self::discover`].
    pub fn list(&self) -> Vec<&SkillMetadata> {
        let mut discovered: Vec<&SkillMetadata> = self.discovered.values().collect();
        discovered.sort_by(|left, right| left.name.cmp(&right.name));
        discovered
    }

    /// Parse and validate one manifest's metadata.
    ///
    /// Discovery uses this (not the infallible wrapper) so a malformed version,
    /// tag list or dependency is reported. Optional fields keep their
    /// documented defaults: version 1.0.0, no tags, no dependencies, no
    /// conflicts, source user, and model-invocable.
    pub fn parse_metadata_checked(path: &Path) -> Result<SkillMetadata, SkillMetadataError> {
        let bytes = std::fs::read(path)?;
        if bytes.len() > MAX_SKILL_BYTES {
            return Err(SkillMetadataError::TooLarge {
                limit: MAX_SKILL_BYTES,
                actual: bytes.len(),
            });
        }
        let digest = digest_bytes(&bytes);
        let content = String::from_utf8(bytes).map_err(|_| {
            SkillMetadataError::Frontmatter("manifest is not valid UTF-8".to_string())
        })?;
        let (frontmatter, _) = Self::parse_frontmatter(&content).ok_or_else(|| {
            SkillMetadataError::Frontmatter("missing YAML frontmatter".to_string())
        })?;

        let name = match frontmatter.get("name") {
            Some(serde_json::Value::String(name)) => name.trim(),
            _ => "",
        };
        if name.is_empty() {
            return Err(SkillMetadataError::BadName(
                "name is required and cannot be blank".to_string(),
            ));
        }
        if name.chars().count() > 128 || name.chars().any(char::is_whitespace) {
            return Err(SkillMetadataError::BadName(format!(
                "'{name}' must be at most 128 characters and contain no whitespace"
            )));
        }

        let description = match frontmatter.get("description") {
            None => "",
            Some(serde_json::Value::String(description)) => description.trim(),
            Some(other) => {
                return Err(SkillMetadataError::BadDescription(format!(
                    "description must be a string, found {other}"
                )))
            }
        };
        if description.is_empty() {
            return Err(SkillMetadataError::BadDescription(
                "description is required and cannot be blank".to_string(),
            ));
        }
        if description.chars().count() > 1024 {
            return Err(SkillMetadataError::BadDescription(
                "description must be at most 1024 characters".to_string(),
            ));
        }

        let category = frontmatter
            .get("category")
            .and_then(|v| v.as_str())
            .map(SkillCategory::from_name)
            .unwrap_or(SkillCategory::Other);

        let version = match frontmatter.get("version") {
            None | Some(serde_json::Value::Null) => SkillVersion::new(1, 0, 0),
            Some(serde_json::Value::String(text)) => SkillVersion::parse(text)?,
            Some(other) => {
                return Err(SkillMetadataError::BadVersion(format!(
                    "version must be a quoted major.minor.patch string, found {other}"
                )))
            }
        };

        let tags = parse_tags(frontmatter.get("tags"))?;
        let dependencies = parse_dependencies(frontmatter.get("dependencies"))?;
        let conflicts = parse_conflicts(frontmatter.get("conflicts"))?;

        // `disable-model-invocation` is the frontmatter key the skill
        // standards use to keep a skill out of model-driven selection. Only a
        // literal true counts: a missing, false or unparsable value leaves the
        // skill model-invocable, which is the convention's own default.
        let user_only = frontmatter
            .get("disable-model-invocation")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let source = frontmatter
            .get("source")
            .and_then(|v| v.as_str())
            .map(SkillSource::from_name)
            .unwrap_or(SkillSource::User);
        let publisher = frontmatter
            .get("publisher")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        Ok(SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            path: path.to_path_buf(),
            category,
            version,
            tags,
            user_only,
            dependencies,
            conflicts,
            source,
            publisher,
            digest,
        })
    }

    fn parse_frontmatter(content: &str) -> Option<(serde_json::Value, &str)> {
        let content = content.trim();
        if !content.starts_with("---") {
            return None;
        }

        let after_first = content.strip_prefix("---")?;
        let end_idx = after_first.find("---")?;
        let yaml_str = &after_first[..end_idx];
        let body = &after_first[end_idx + 3..];

        let frontmatter: serde_json::Value = serde_yaml::from_str(yaml_str).ok()?;
        Some((frontmatter, body.trim()))
    }

    pub(crate) fn extract_body(content: &str) -> String {
        let content = content.trim();
        if let Some(start) = content.find("---") {
            let after_first = &content[start + 3..];
            if let Some(end) = after_first.find("---") {
                return after_first[end + 3..].trim().to_string();
            }
        }
        content.to_string()
    }
}

/// Normalize a tag list: trim, lowercase, drop blanks, deduplicate and sort.
fn normalize_tags<I: IntoIterator<Item = String>>(tags: I) -> Vec<String> {
    let mut tags: Vec<String> = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Parse the optional `tags` frontmatter field.
fn parse_tags(value: Option<&serde_json::Value>) -> Result<Vec<String>, SkillMetadataError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => {
            let mut tags = Vec::with_capacity(items.len());
            for item in items {
                let tag = item.as_str().ok_or_else(|| {
                    SkillMetadataError::BadTags(format!("tag {item} is not a string"))
                })?;
                tags.push(tag.to_string());
            }
            Ok(normalize_tags(tags))
        }
        Some(serde_json::Value::String(text)) => {
            Ok(normalize_tags(text.split(',').map(str::to_string)))
        }
        Some(other) => Err(SkillMetadataError::BadTags(other.to_string())),
    }
}

/// Parse required/optional dependencies.
///
/// A bare string is a required dependency with no version constraint; an
/// object may add `version` and `optional`. Entries are deduplicated by name
/// and name-sorted so resolution is deterministic.
fn parse_dependencies(
    value: Option<&serde_json::Value>,
) -> Result<Vec<SkillDependency>, SkillMetadataError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items = value.as_array().ok_or_else(|| {
        SkillMetadataError::BadDependency("dependencies must be a list".to_string())
    })?;
    let mut dependencies: Vec<SkillDependency> = Vec::new();
    for item in items {
        let (name, version, optional) = match item {
            serde_json::Value::String(name) => (name.clone(), None, false),
            serde_json::Value::Object(map) => {
                let name = map
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        SkillMetadataError::BadDependency(
                            "dependency needs a non-empty name".to_string(),
                        )
                    })?
                    .to_string();
                let version = map
                    .get("version")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
                if let Some(constraint) = &version {
                    semver::VersionReq::parse(constraint).map_err(|_| {
                        SkillMetadataError::BadDependency(format!(
                            "'{name}' declares an invalid version constraint '{constraint}'"
                        ))
                    })?;
                }
                let optional = map
                    .get("optional")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                (name, version, optional)
            }
            other => {
                return Err(SkillMetadataError::BadDependency(format!(
                    "dependency {other} is neither a name nor an object"
                )))
            }
        };
        if name.is_empty() {
            return Err(SkillMetadataError::BadDependency(
                "dependency name cannot be blank".to_string(),
            ));
        }
        dependencies.push(SkillDependency {
            name,
            version,
            optional,
        });
    }
    dependencies.sort_by(|left, right| left.name.cmp(&right.name));
    dependencies.dedup_by(|left, right| left.name == right.name);
    Ok(dependencies)
}

/// Parse declared conflicts as normalized, sorted Skill names.
fn parse_conflicts(value: Option<&serde_json::Value>) -> Result<Vec<String>, SkillMetadataError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                let name = item.as_str().ok_or_else(|| {
                    SkillMetadataError::BadConflicts(format!("conflict {item} is not a string"))
                })?;
                names.push(name.trim().to_string());
            }
            names.retain(|name| !name.is_empty());
            names.sort();
            names.dedup();
            Ok(names)
        }
        Some(serde_json::Value::String(text)) => {
            let mut names: Vec<String> = text
                .split(',')
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect();
            names.sort();
            names.dedup();
            Ok(names)
        }
        Some(other) => Err(SkillMetadataError::BadConflicts(other.to_string())),
    }
}

/// SHA-256 of a manifest, lowercase hex.
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}
/// What one `load_skill` name resolved to.
#[derive(Debug, Clone, Copy)]
pub enum ModelSkillLookup<'a> {
    /// The skill may be advertised to and loaded by the model.
    Found(&'a SkillContent),
    /// The skill exists, but its manifest reserves it for the reader
    /// (`disable-model-invocation: true`).
    UserOnly(&'a SkillContent),
    /// No registered skill answers to that name.
    Unknown,
}

/// Resolve one skill name the MODEL asked to load.
///
/// The policy is the manifest's own `disable-model-invocation` flag, read into
/// [`SkillMetadata::user_only`] by the loader. A user-only skill stays
/// registered - the reader's `/name` gesture loads it - but it must not be
/// advertised to the model nor opened by its `load_skill` tool. Keeping the
/// decision here means the catalogue and the tool answer from one rule.
pub fn lookup_for_model<'a>(skills: &'a [SkillContent], name: &str) -> ModelSkillLookup<'a> {
    lookup_for_model_in(skills, &[], name)
}

/// Resolve one model-requested skill name across the build's own set and the
/// live extension overlay.
///
/// The two sets are separate because they move at different times: a package
/// can be activated or suspended while an agent is already running, and the
/// loader must read the overlay as it is when the model asks, not as it was
/// when the engine was built. The base entry wins a collision, which is the
/// same precedence the catalogue uses. The policy is unchanged: a skill whose
/// manifest reserves it for the reader is refused, never reported missing.
pub fn lookup_for_model_in<'a>(
    base: &'a [SkillContent],
    live: &'a [SkillContent],
    name: &str,
) -> ModelSkillLookup<'a> {
    match base
        .iter()
        .chain(live.iter())
        .find(|skill| skill.metadata.name == name)
    {
        Some(skill) if skill.metadata.user_only => ModelSkillLookup::UserOnly(skill),
        Some(skill) => ModelSkillLookup::Found(skill),
        None => ModelSkillLookup::Unknown,
    }
}

/// Errors raised while loading skills.
#[derive(thiserror::Error, Debug)]
pub enum SkillError {
    /// The skills directory does not exist.
    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),

    /// No skill matched the requested name.
    #[error("Skill not found: {0}")]
    NotFound(String),

    /// Reading a skill file failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A skill frontmatter block was not valid YAML.
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),
}

#[cfg(test)]
mod tests {
    use super::{
        lookup_for_model, lookup_for_model_in, ModelSkillLookup, SkillCategory, SkillContent,
        SkillLoader, SkillMetadata, SkillMetadataError, SkillSource, SkillVersion,
    };
    use std::path::PathBuf;

    fn skill(name: &str, user_only: bool) -> SkillContent {
        let mut metadata = SkillMetadata::new(
            name.to_string(),
            "a test skill".to_string(),
            PathBuf::from("/skills").join(name).join("SKILL.md"),
        );
        metadata.user_only = user_only;
        SkillContent {
            metadata,
            instructions: format!("Body of {name}."),
            skill_dir: PathBuf::from("/skills").join(name),
        }
    }

    /// The manifest decides who may open a skill: the model gets the ones it
    /// was offered, and a user-only skill is NAMED as such rather than
    /// reported missing.
    #[test]
    fn a_user_only_skill_is_not_the_models_to_open() {
        let skills = vec![skill("open", false), skill("reserved", true)];
        match lookup_for_model(&skills, "open") {
            ModelSkillLookup::Found(skill) => assert_eq!(skill.metadata.name, "open"),
            other => panic!("an ordinary skill answers the model: {other:?}"),
        }
        assert!(
            matches!(
                lookup_for_model(&skills, "reserved"),
                ModelSkillLookup::UserOnly(skill) if skill.metadata.name == "reserved"
            ),
            "a reserved skill is refused as user-only, not invented as missing"
        );
        assert!(
            matches!(lookup_for_model(&skills, "nope"), ModelSkillLookup::Unknown),
            "an unregistered name is unknown"
        );
    }

    /// The live overlay is a second namespace that moves while an engine runs:
    /// a package activated after the engine was built must answer, and the base
    /// entry wins a collision.
    #[test]
    fn the_live_overlay_answers_a_name_the_base_set_does_not() {
        let base = vec![skill("builtin", false)];
        let live = vec![skill("package", false), skill("builtin", true)];

        match lookup_for_model_in(&base, &live, "package") {
            ModelSkillLookup::Found(skill) => assert_eq!(skill.metadata.name, "package"),
            other => panic!("a package's skill answers from the overlay: {other:?}"),
        }
        match lookup_for_model_in(&base, &live, "builtin") {
            ModelSkillLookup::Found(skill) => assert!(
                !skill.metadata.user_only,
                "the base entry wins the collision, so its policy wins with it"
            ),
            other => panic!("the base set answers first: {other:?}"),
        }
        assert!(
            matches!(
                lookup_for_model_in(&base, &live, "gone"),
                ModelSkillLookup::Unknown
            ),
            "a suspended package's name is unknown, not served stale"
        );
    }

    fn write_manifest(frontmatter: &str, body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, format!("---\n{frontmatter}\n---\n{body}")).expect("write");
        (dir, path)
    }

    /// The full metadata model is populated from frontmatter, including the
    /// fields the loader used to invent (version, tags).
    #[test]
    fn complete_frontmatter_populates_version_tags_dependencies_and_provenance() {
        let frontmatter = "name: deploy\ndescription: Deploy a service safely\nversion: 2.3.4\ncategory: code\ntags: ['Ops', 'release', 'ops', '']\nsource: third-party\npublisher: acme\ndependencies:\n  - base\n  - name: audit\n    version: '>=1.2.0'\n    optional: true\nconflicts: [rollback]";
        let (dir, path) = write_manifest(frontmatter, "Body.");
        let metadata = SkillLoader::parse_metadata_checked(&path).expect("metadata");
        assert_eq!(metadata.version, SkillVersion::new(2, 3, 4));
        assert_eq!(metadata.category, SkillCategory::Code);
        assert_eq!(metadata.tags, vec!["ops", "release"]);
        assert_eq!(metadata.dependencies.len(), 2);
        assert_eq!(metadata.dependencies[0].name, "audit");
        assert_eq!(metadata.dependencies[0].version.as_deref(), Some(">=1.2.0"));
        assert!(metadata.dependencies[0].optional);
        assert_eq!(metadata.dependencies[1].name, "base");
        assert!(!metadata.dependencies[1].optional);
        assert_eq!(metadata.conflicts, vec!["rollback"]);
        assert_eq!(metadata.source, SkillSource::ThirdParty);
        assert_eq!(metadata.publisher.as_deref(), Some("acme"));
        assert_eq!(metadata.digest.len(), 64, "sha-256 hex");
        drop(dir);
    }

    /// A Skill that declares nothing optional keeps working under the
    /// documented defaults.
    #[test]
    fn omitted_optional_fields_keep_their_defaults() {
        let (dir, path) = write_manifest("name: plain\ndescription: A plain skill", "Body.");
        let metadata = SkillLoader::parse_metadata_checked(&path).expect("metadata");
        assert_eq!(metadata.version, SkillVersion::new(1, 0, 0));
        assert_eq!(metadata.tags, Vec::<String>::new());
        assert!(metadata.dependencies.is_empty());
        assert!(metadata.conflicts.is_empty());
        assert_eq!(metadata.source, SkillSource::User);
        assert_eq!(metadata.publisher, None);
        assert!(!metadata.user_only);
        drop(dir);
    }

    /// Malformed values are reported, not repaired into a different version
    /// or an empty tag list.
    #[test]
    fn malformed_metadata_is_rejected_with_its_field() {
        let cases = [
            ("name: x\ndescription: y\nversion: 1.2", "version"),
            ("name: x\ndescription: y\nversion: one.two.three", "version"),
            ("name: x\ndescription: y\ntags: {agent: true}", "tags"),
            ("name: x\ndescription: ''\n", "description"),
            ("name: has space\ndescription: y", "name"),
            (
                "name: x\ndescription: y\ndependencies:\n  - name: dep\n    version: 'not-a-range'",
                "dependency",
            ),
        ];
        for (frontmatter, field) in cases {
            let (dir, path) = write_manifest(frontmatter, "Body.");
            let error = SkillLoader::parse_metadata_checked(&path).expect_err(frontmatter);
            let text = error.to_string();
            assert!(text.contains(field), "{frontmatter}: {text}");
            drop(dir);
        }
    }

    /// The description boundary is measured in characters and does not
    /// silently truncate.
    #[test]
    fn the_description_boundary_is_a_refusal_not_a_truncation() {
        let at_limit = "é".repeat(1024);
        let (dir, path) = write_manifest(&format!("name: x\ndescription: '{at_limit}'"), "Body.");
        assert!(SkillLoader::parse_metadata_checked(&path).is_ok());
        drop(dir);

        let over_limit = "é".repeat(1025);
        let (dir, path) = write_manifest(&format!("name: x\ndescription: '{over_limit}'"), "Body.");
        assert!(matches!(
            SkillLoader::parse_metadata_checked(&path),
            Err(SkillMetadataError::BadDescription(_))
        ));
        drop(dir);
    }
}
