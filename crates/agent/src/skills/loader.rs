use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::metadata::{ReferenceFile, SkillCategory, SkillContent, SkillMetadata, SkillVersion};
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

    pub fn discover(&mut self) -> Result<Vec<SkillMetadata>, SkillError> {
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

        Ok(self.discovered.values().cloned().collect())
    }

    pub fn load(&self, name: &str) -> Result<SkillContent, SkillError> {
        let meta = self
            .discovered
            .get(name)
            .ok_or(SkillError::NotFound(name.to_string()))?
            .clone();

        let content = std::fs::read_to_string(&meta.path)?;
        Self::parse_skill_content(meta, &content)
    }

    pub fn list(&self) -> Vec<&SkillMetadata> {
        self.discovered.values().collect()
    }

    fn parse_metadata(path: &Path) -> Result<Option<SkillMetadata>, SkillError> {
        Self::validate_skill_path(path)?;
        let content = std::fs::read_to_string(path)?;
        let (frontmatter, _) = Self::parse_frontmatter(&content)?;

        let name = frontmatter
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SkillError::InvalidFormat("missing 'name' in frontmatter".into()))?
            .to_string();

        Self::validate_name(&name)?;

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

        let version = frontmatter
            .get("version")
            .and_then(|v| v.as_str())
            .and_then(|s| SkillVersion::parse(s).ok())
            .unwrap_or(SkillVersion::new(1, 0, 0));

        let author = frontmatter
            .get("author")
            .and_then(|v| v.as_str())
            .map(String::from);

        let tags: Vec<String> = frontmatter
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let requires: Vec<String> = frontmatter
            .get("requires")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let provides: Vec<String> = frontmatter
            .get("provides")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(SkillMetadata {
            name,
            description,
            path: path.to_path_buf(),
            category,
            version,
            author,
            tags,
            requires,
            provides,
        }))
    }

    fn parse_skill_content(meta: SkillMetadata, content: &str) -> Result<SkillContent, SkillError> {
        let _frontmatter = Self::extract_frontmatter(content)?;
        let body = Self::extract_body(content);

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

    fn validate_name(name: &str) -> Result<(), SkillError> {
        if name.len() < 1 || name.len() > 100 {
            return Err(SkillError::InvalidSkillName(
                "Skill name must be 1-100 characters".to_string(),
            ));
        }

        let valid_chars = name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');

        if !valid_chars {
            return Err(SkillError::InvalidSkillName(
                "Skill name must only contain alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            ));
        }

        Ok(())
    }

    fn validate_skill_path(path: &Path) -> Result<(), SkillError> {
        if !path.exists() {
            return Err(SkillError::NotFound(format!("{:?}", path)));
        }

        if !path.is_file() {
            return Err(SkillError::InvalidFormat(format!(
                "{:?} is not a file",
                path
            )));
        }

        Ok(())
    }

    fn validate_frontmatter(frontmatter: &serde_json::Value) -> Result<(), SkillError> {
        if !frontmatter.get("name").is_some() {
            return Err(SkillError::MissingField("name".to_string()));
        }

        if !frontmatter.get("description").is_some() {
            return Err(SkillError::MissingField("description".to_string()));
        }

        if let Some(name_val) = frontmatter.get("name") {
            if let Some(name_str) = name_val.as_str() {
                Self::validate_name(name_str)?;
            }
        }

        Ok(())
    }

    fn detect_circular_deps(
        &self,
        skill_name: &str,
        visited: &mut Vec<String>,
    ) -> Result<(), SkillError> {
        if visited.contains(&skill_name.to_string()) {
            let cycle = visited.join(" → ");
            return Err(SkillError::CircularDependency(cycle));
        }

        visited.push(skill_name.to_string());

        if let Some(metadata) = self.discovered.get(skill_name) {
            for req in &metadata.requires {
                if self.discovered.contains_key(req) {
                    self.detect_circular_deps(req, visited)?;
                }
            }
        }

        visited.pop();
        Ok(())
    }

    fn check_dependencies(&self, metadata: &SkillMetadata) -> Result<(), SkillError> {
        for dep in &metadata.requires {
            if !self.discovered.contains_key(dep) {
                return Err(SkillError::DependencyNotFound(dep.clone()));
            }
        }
        Ok(())
    }

    pub fn validate_all(&self) -> Result<(), SkillError> {
        for (name, metadata) in &self.discovered {
            self.check_dependencies(metadata)?;
            self.detect_circular_deps(name, &mut Vec::new())?;
        }

        Ok(())
    }

    fn parse_frontmatter(content: &str) -> Result<(serde_json::Value, String), SkillError> {
        let lines: Vec<&str> = content.lines().collect();

        if lines.len() < 2 {
            return Err(SkillError::InvalidFormat(
                "File too short for frontmatter".to_string(),
            ));
        }

        if !lines[0].starts_with("---") {
            return Err(SkillError::InvalidFormat(
                "Frontmatter must start with ---".to_string(),
            ));
        }

        let end_idx = lines[1..]
            .iter()
            .position(|line| line.starts_with("---"))
            .ok_or(SkillError::InvalidFormat(
                "Frontmatter must end with ---".to_string(),
            ))?;

        let frontmatter_str = lines[1..=end_idx].join("\n");
        let instructions = lines[end_idx + 1..].join("\n");

        let frontmatter: serde_json::Value = serde_yaml::from_str(&frontmatter_str)
            .map_err(|e| SkillError::ParseError(e.to_string()))?;

        Self::validate_frontmatter(&frontmatter)?;

        Ok((frontmatter, instructions))
    }

    pub fn list_by_category(&self, category: SkillCategory) -> Vec<&SkillMetadata> {
        self.discovered
            .values()
            .filter(|m| m.category == category)
            .collect()
    }

    pub fn list_by_tag(&self, tag: &str) -> Vec<&SkillMetadata> {
        self.discovered
            .values()
            .filter(|m| m.tags.contains(&tag.to_string()))
            .collect()
    }

    pub fn has_skill(&self, name: &str) -> bool {
        self.discovered.contains_key(name)
    }

    pub fn dependency_graph(&self) -> Vec<(String, Vec<String>)> {
        self.discovered
            .iter()
            .map(|(name, meta)| (name.clone(), meta.requires.clone()))
            .collect()
    }

    pub fn stats(&self) -> SkillStats {
        let mut by_category = std::collections::HashMap::new();
        let mut total_deps = 0;

        for metadata in self.discovered.values() {
            *by_category.entry(metadata.category.clone()).or_insert(0) += 1;
            total_deps += metadata.requires.len();
        }

        SkillStats {
            total_skills: self.discovered.len(),
            by_category,
            total_dependencies: total_deps,
            skills_dir: self.skills_dir.clone(),
        }
    }

    pub fn rediscover(&mut self) -> Result<Vec<SkillMetadata>, SkillError> {
        self.discovered.clear();
        self.discover()
    }
}

#[derive(Debug, Clone)]
pub struct SkillStats {
    pub total_skills: usize,
    pub by_category: std::collections::HashMap<SkillCategory, usize>,
    pub total_dependencies: usize,
    pub skills_dir: PathBuf,
}

impl std::fmt::Display for SkillStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Skills Statistics:")?;
        writeln!(f, " Total: {}", self.total_skills)?;
        writeln!(f, " Directory: {:?}", self.skills_dir)?;

        let mut sorted: Vec<_> = self.by_category.iter().collect();
        sorted.sort_by_key(|(_, c)| *c);

        for (category, count) in sorted {
            writeln!(f, " {}: {}", category.display_name(), count)?;
        }

        writeln!(f, " Total Dependencies: {}", self.total_dependencies)?;
        Ok(())
    }
}
