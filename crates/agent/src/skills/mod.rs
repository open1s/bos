pub mod injector;
pub mod loader;
pub mod metadata;
#[cfg(test)]
mod tests;

pub use injector::{InjectionFormat, InjectionOptions, InjectionStats, SkillInjector};
pub use loader::SkillLoader;
pub use metadata::{ReferenceFile, SkillCategory, SkillContent, SkillMetadata, SkillVersion};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SkillError {
    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),

    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid skill name: {0}")]
    InvalidSkillName(String),

    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),

    #[error("Dependency not found: {0}")]
    DependencyNotFound(String),

    #[error("Reference file not found: {0}")]
    ReferenceNotFound(String),

    #[error("Skill already loaded: {0}")]
    AlreadyLoaded(String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),
}

impl SkillError {
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::YamlError(_) | Self::ParseError(_))
    }

    pub fn is_missing(&self) -> bool {
        matches!(
            self,
            Self::NotFound(_) | Self::DirectoryNotFound(_) | Self::ReferenceNotFound(_)
        )
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::DirectoryNotFound(_) => "FS",
            Self::NotFound(_) => "Lookup",
            Self::InvalidFormat(_) | Self::ParseError(_) => "Parse",
            Self::Io(_) => "IO",
            Self::YamlError(_) => "YAML",
            Self::MissingField(_) => "Schema",
            Self::InvalidSkillName(_) => "Validation",
            Self::CircularDependency(_) => "Dependency",
            Self::DependencyNotFound(_) => "Dependency",
            Self::ReferenceNotFound(_) => "Reference",
            Self::AlreadyLoaded(_) => "State",
            Self::ValidationFailed(_) => "Validation",
        }
    }

    pub fn formatted(&self) -> String {
        format!("[{}] {}", self.category(), self)
    }
}
