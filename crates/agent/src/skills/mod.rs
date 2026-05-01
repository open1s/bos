//! Skills module - simplified skill loading and injection
//!
//! Provides basic skill loading from filesystem with minimal features.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Skill category for classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[qserde::Archive]
pub enum SkillCategory {
    #[default]
    Other,
    Code,
    Analysis,
    Data,
    Testing,
    Utility,
}

impl SkillCategory {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "code" => Self::Code,
            "analysis" => Self::Analysis,
            "data" => Self::Data,
            "testing" | "test" => Self::Testing,
            "utility" | "util" => Self::Utility,
            _ => Self::Other,
        }
    }

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
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SkillVersion {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Metadata for a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
#[qserde::Archive]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[rkyv(with = qserde::rkyv::with::AsString)]
    pub path: PathBuf,
    pub category: SkillCategory,
    pub version: SkillVersion,
    pub tags: Vec<String>,
}

impl SkillMetadata {
    pub fn new(name: String, description: String, path: PathBuf) -> Self {
        Self {
            name,
            description,
            path,
            category: SkillCategory::Other,
            version: SkillVersion::new(1, 0, 0),
            tags: Vec::new(),
        }
    }
}

/// Content of a skill including instructions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[qserde::Archive]
pub struct SkillContent {
    pub metadata: SkillMetadata,
    pub instructions: String,
}

/// Skill loader - discovers and loads skills from filesystem
pub struct SkillLoader {
    skills_dir: PathBuf,
    discovered: std::collections::HashMap<String, SkillMetadata>,
}

impl SkillLoader {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            discovered: std::collections::HashMap::new(),
        }
    }

    /// Discover skills in the skills directory
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

        Ok(self.discovered.values().cloned().collect())
    }

    /// Load a skill by name
    pub fn load(&self, name: &str) -> Option<SkillContent> {
        let meta = self.discovered.get(name)?.clone();
        let content = std::fs::read_to_string(&meta.path).ok()?;
        let instructions = Self::extract_body(&content);
        Some(SkillContent {
            metadata: meta,
            instructions,
        })
    }

    /// List all discovered skills
    pub fn list(&self) -> Vec<&SkillMetadata> {
        self.discovered.values().collect()
    }

    /// Check if a skill exists
    pub fn has_skill(&self, name: &str) -> bool {
        self.discovered.contains_key(name)
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
            .map(SkillCategory::from_str)
            .unwrap_or(SkillCategory::Other);

        Some(SkillMetadata {
            name,
            description,
            path: path.to_path_buf(),
            category,
            version: SkillVersion::new(1, 0, 0),
            tags: Vec::new(),
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

/// Skill injector - formats skills for system prompt injection
pub struct SkillInjector {
    compact: bool,
}

#[derive(Debug, Clone)]
pub struct InjectionOptions {
    pub compact: bool,
}

impl InjectionOptions {
    pub fn compact() -> Self {
        Self { compact: true }
    }
}

impl SkillInjector {
    pub fn new() -> Self {
        Self { compact: false }
    }

    pub fn with_options(options: InjectionOptions) -> Self {
        Self {
            compact: options.compact,
        }
    }

    pub fn inject_available(&self, skills: &[SkillMetadata]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        if self.compact {
            let mut xml = String::from("<available_skills>\n");
            for skill in skills {
                xml.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
            }
            xml.push_str("</available_skills>");
            xml
        } else {
            let mut xml = String::from("<available_skills>\n");
            for skill in skills {
                xml.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
            }
            xml.push_str("</available_skills>");
            xml
        }
    }
}

impl Default for SkillInjector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(thiserror::Error, Debug)]
pub enum SkillError {
    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),

    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),
}
