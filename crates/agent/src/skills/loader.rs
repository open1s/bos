use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::metadata::{ReferenceFile, SkillContent, SkillMetadata};
use super::SkillError;

pub struct SkillLoader {
    skills_dir: PathBuf,
    discovered: HashMap<String, SkillMetadata>,
}

impl SkillLoader {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            discovered: HashMap::new(),
        }
    }

    /// Discovery phase: Scan directory, load name + description only
    pub fn discover(&mut self) -> Result<(), SkillError> {
        if !self.skills_dir.exists() {
            return Err(SkillError::DirectoryNotFound(
                self.skills_dir.display().to_string(),
            ));
        }

        for entry in std::fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }

            let skill_file = skill_dir.join("SKILL.md");
            if skill_file.exists() {
                if let Some(meta) = Self::parse_metadata(&skill_file)? {
                    self.discovered.insert(meta.name.clone(), meta);
                }
            }
        }
        Ok(())
    }

    /// Activation phase: Load full SKILL.md content on-demand
    pub fn load(&self, name: &str) -> Result<SkillContent, SkillError> {
        let meta = self
            .discovered
            .get(name)
            .ok_or(SkillError::NotFound(name.to_string()))?;

        let content = std::fs::read_to_string(&meta.path)?;
        Self::parse_skill_content(meta.clone(), &content)
    }

    pub fn list(&self) -> Vec<&SkillMetadata> {
        self.discovered.values().collect()
    }

    fn parse_metadata(path: &Path) -> Result<Option<SkillMetadata>, SkillError> {
        let content = std::fs::read_to_string(path)?;
        let frontmatter = Self::extract_frontmatter(&content)?;

        let name = frontmatter
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SkillError::InvalidFormat("missing 'name' in frontmatter".into()))?
            .to_string();

        let description = frontmatter
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(Some(SkillMetadata {
            name,
            description,
            path: path.to_path_buf(),
        }))
    }

    fn parse_skill_content(meta: SkillMetadata, content: &str) -> Result<SkillContent, SkillError> {
        let _frontmatter = Self::extract_frontmatter(content)?;
        let body = Self::extract_body(content);

        // Parse optional references from references/ subdirectory
        let mut references = Vec::new();
        if let Some(parent) = meta.path.parent() {
            let refs_dir = parent.join("references");
            if refs_dir.exists() {
                for entry in std::fs::read_dir(&refs_dir)? {
                    let entry = entry?;
                    if entry.path().is_file() {
                        references.push(ReferenceFile {
                            name: entry.file_name().to_string_lossy().to_string(),
                            path: entry.path(),
                        });
                    }
                }
            }
        }

        Ok(SkillContent {
            metadata: meta,
            instructions: body,
            references,
        })
    }

    fn extract_frontmatter(content: &str) -> Result<serde_json::Value, SkillError> {
        let content = content.trim();
        if !content.starts_with("---") {
            return Err(SkillError::InvalidFormat(
                "Missing YAML frontmatter delimiter".into(),
            ));
        }

        let end_idx = content[3..].find("---").ok_or(SkillError::InvalidFormat(
            "Missing closing --- for frontmatter".into(),
        ))?;

        let yaml_str = &content[3..3 + end_idx];
        serde_yaml::from_str(yaml_str).map_err(|e| SkillError::ParseError(e.to_string()))
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
