use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SkillContent {
    pub metadata: SkillMetadata,
    pub instructions: String,
    pub references: Vec<ReferenceFile>,
}

#[derive(Debug, Clone)]
pub struct ReferenceFile {
    pub name: String,
    pub path: PathBuf,
}
