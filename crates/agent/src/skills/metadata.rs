use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::SkillError;

/// Skill category for classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SkillCategory {
    Analysis,
    Code,
    Communication,
    Data,
    Domain, // For domain-specific skills (e.g., IoT, ML)
    Security,
    Testing,
    Utility,
    Other,
}

impl SkillCategory {
    /// Parse from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "analysis" => Self::Analysis,
            "code" => Self::Code,
            "communication" => Self::Communication,
            "data" => Self::Data,
            "domain" => Self::Domain,
            "security" => Self::Security,
            "testing" | "test" => Self::Testing,
            "utility" | "util" => Self::Utility,
            _ => Self::Other,
        }
    }

    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Code => "code",
            Self::Communication => "communication",
            Self::Data => "data",
            Self::Domain => "domain",
            Self::Security => "security",
            Self::Testing => "testing",
            Self::Utility => "utility",
            Self::Other => "other",
        }
    }

    /// Display formatted category name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Analysis => "Analysis",
            Self::Code => "Code",
            Self::Communication => "Communication",
            Self::Data => "Data",
            Self::Domain => "Domain-Specific",
            Self::Security => "Security",
            Self::Testing => "Testing",
            Self::Utility => "Utility",
            Self::Other => "Other",
        }
    }
}

/// Skill version following semantic versioning
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SkillVersion {
    /// Create a new version
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse from semantic version string (e.g., "1.2.3")
    pub fn parse(s: &str) -> Result<Self, SkillError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(SkillError::ParseError(format!(
                "Invalid version format: {}",
                s
            )));
        }

        let major = parts[0]
            .parse::<u32>()
            .map_err(|_| SkillError::ParseError(format!("Invalid major version: {}", parts[0])))?;
        let minor = parts[1]
            .parse::<u32>()
            .map_err(|_| SkillError::ParseError(format!("Invalid minor version: {}", parts[1])))?;
        let patch = parts[2]
            .parse::<u32>()
            .map_err(|_| SkillError::ParseError(format!("Invalid patch version: {}", parts[2])))?;

        Ok(Self {
            major,
            minor,
            patch,
        })
    }

    /// Display as semantic version string
    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// Compare versions for ordering
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match self.major.cmp(&other.major) {
            std::cmp::Ordering::Equal => match self.minor.cmp(&other.minor) {
                std::cmp::Ordering::Equal => self.patch.cmp(&other.patch),
                other => other,
            },
            other => other,
        }
    }
}

/// Metadata for a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub category: SkillCategory,
    pub version: SkillVersion,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub requires: Vec<String>, // Other skills this skill depends on
    pub provides: Vec<String>, // Tools or capabilities this skill provides
}

impl SkillMetadata {
    /// Create minimal metadata
    pub fn new(name: String, description: String, path: PathBuf) -> Self {
        Self {
            name,
            description,
            path,
            category: SkillCategory::Other,
            version: SkillVersion::new(1, 0, 0),
            author: None,
            tags: Vec::new(),
            requires: Vec::new(),
            provides: Vec::new(),
        }
    }

    /// Set the category
    pub fn with_category(mut self, category: SkillCategory) -> Self {
        self.category = category;
        self
    }

    /// Set the version
    pub fn with_version(mut self, version: SkillVersion) -> Self {
        self.version = version;
        self
    }

    /// Set the author
    pub fn with_author(mut self, author: String) -> Self {
        self.author = Some(author);
        self
    }

    /// Add a tag
    pub fn with_tag(mut self, tag: String) -> Self {
        self.tags.push(tag);
        self
    }

    /// Add a required skill
    pub fn with_requirement(mut self, require: String) -> Self {
        self.requires.push(require);
        self
    }

    /// Add a provided capability
    pub fn with_provides(mut self, provides: String) -> Self {
        self.provides.push(provides);
        self
    }

    /// Check if skill requires another skill
    pub fn requires_skill(&self, skill_name: &str) -> bool {
        self.requires.iter().any(|req| req == skill_name)
    }

    /// Check if skill provides a specific capability
    pub fn provides_capability(&self, capability: &str) -> bool {
        self.provides.iter().any(|prov| prov == capability)
    }

    /// Get all tags as a comma-separated string
    pub fn tags_string(&self) -> String {
        self.tags.join(", ")
    }
}

impl std::fmt::Display for SkillMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} v{} ({}) - {}",
            self.name,
            self.version.display(),
            self.category.as_str(),
            self.description
        )
    }
}

/// Content of a skill including instructions and references
#[derive(Debug, Clone)]
pub struct SkillContent {
    pub metadata: SkillMetadata,
    pub instructions: String,
    pub references: Vec<ReferenceFile>,
}

impl SkillContent {
    /// Create new content
    pub fn new(metadata: SkillMetadata, instructions: String) -> Self {
        Self {
            metadata,
            instructions,
            references: Vec::new(),
        }
    }

    /// Add a reference file
    pub fn with_reference(mut self, name: String, path: PathBuf) -> Self {
        self.references.push(ReferenceFile { name, path });
        self
    }

    /// Get the total content length (instructions + references)
    pub fn total_length(&self) -> usize {
        self.instructions.len()
            + self
                .references
                .iter()
                .map(|r| r.name.len() + r.path.to_string_lossy().len())
                .sum::<usize>()
    }

    /// Format as markdown
    pub fn to_markdown(&self) -> String {
        let mut md = format!(
            "# {}\n\n{}\n",
            self.metadata.name, self.metadata.description
        );

        if !self.metadata.tags_string().is_empty() {
            md.push_str(&format!("\n**Tags:** {}\n", self.metadata.tags_string()));
        }

        md.push_str("\n## Instructions\n\n");
        md.push_str(&self.instructions);

        if !self.references.is_empty() {
            md.push_str("\n## References\n\n");
            for ref_file in &self.references {
                md.push_str(&format!(
                    "- **{}**: {}\n",
                    ref_file.name,
                    ref_file.path.display()
                ));
            }
        }

        md
    }
}

impl std::fmt::Display for SkillContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SkillContent[{}] — {} instructions, {} references",
            self.metadata.name,
            self.instructions.len(),
            self.references.len()
        )
    }
}

/// A reference file associated with a skill
#[derive(Debug, Clone)]
pub struct ReferenceFile {
    pub name: String,
    pub path: PathBuf,
}
