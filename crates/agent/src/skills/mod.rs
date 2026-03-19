pub mod injector;
pub mod loader;
pub mod metadata;
#[cfg(test)]
mod tests;

pub use injector::SkillInjector;
pub use loader::SkillLoader;
pub use metadata::{ReferenceFile, SkillContent, SkillMetadata};

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
}
