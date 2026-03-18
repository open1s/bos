use std::io;
use thiserror::Error;

pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Unsupported file format: {0}")]
    UnsupportedFormat(String),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error("Merge error: {0}")]
    MergeError(String),

    #[error("Load error: {0}")]
    LoadError(#[from] anyhow::Error),

    #[error("Custom source error: {0}")]
    Custom(String),
}
