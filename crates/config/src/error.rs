use std::io;
use thiserror::Error;

/// Result alias for the config crate.
pub type ConfigResult<T> = Result<T, ConfigError>;

/// Errors produced while discovering, reading, or merging configuration.
#[derive(Error, Debug)]
pub enum ConfigError {
    /// An underlying IO failure.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// The extension did not map to a supported format.
    #[error("Unsupported file format: {0}")]
    UnsupportedFormat(String),

    /// TOML could not be parsed.
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// YAML could not be parsed.
    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    /// JSON could not be parsed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A source file does not exist.
    #[error("Config file not found: {0}")]
    NotFound(String),

    /// Two sources could not be merged.
    #[error("Merge error: {0}")]
    MergeError(String),

    /// Loading failed for a reason that does not fit the variants above.
    #[error("Load error: {0}")]
    LoadError(#[from] anyhow::Error),

    /// A [`CustomConfigSource`](crate::types::CustomConfigSource) failed.
    #[error("Custom source error: {0}")]
    Custom(String),
}
