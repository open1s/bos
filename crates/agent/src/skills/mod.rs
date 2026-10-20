//! Skills module - simplified skill loading and injection
//!
//! Provides basic skill loading from filesystem with minimal features.

/// Per-session skill activation and skill authoring validation.
pub mod session;

pub use session::{SkillActivation, SkillAuthoringRecord, SkillValidationError};

use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
}

/// Metadata for a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                if let Some(meta) = Self::parse_metadata(&skill_file) {
                    self.discovered.insert(meta.name.clone(), meta);
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

    fn parse_metadata(path: &Path) -> Option<SkillMetadata> {
        let content = std::fs::read_to_string(path).ok()?;
        let (frontmatter, _) = Self::parse_frontmatter(&content)?;

        let name = frontmatter.get("name")?.as_str()?.to_string();
        let description = frontmatter
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let category = frontmatter
            .get("category")
            .and_then(|v| v.as_str())
            .map(SkillCategory::from_name)
            .unwrap_or(SkillCategory::Other);

        // `disable-model-invocation` is the frontmatter key the skill
        // standards use to keep a skill out of model-driven selection. Only a
        // literal true counts: a missing, false or unparsable value leaves the
        // skill model-invocable, which is the convention's own default.
        let user_only = frontmatter
            .get("disable-model-invocation")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Some(SkillMetadata {
            name,
            description,
            path: path.to_path_buf(),
            category,
            version: SkillVersion::new(1, 0, 0),
            tags: Vec::new(),
            user_only,
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

    fn extract_body(content: &str) -> String {
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
