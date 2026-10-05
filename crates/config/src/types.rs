//! Configuration value types: formats, merge strategies, sources, metadata.

/// Supported configuration file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    /// TOML (`.toml`).
    Toml,
    /// YAML (`.yaml`, `.yml`).
    Yaml,
    /// JSON (`.json`).
    Json,
}

impl ConfigFormat {
    /// Infer the format from a file extension.
    pub fn from_path(path: &str) -> Option<Self> {
        let ext = std::path::Path::new(path)
            .extension()?
            .to_str()?
            .to_lowercase();

        match ext.as_str() {
            "toml" => Some(ConfigFormat::Toml),
            "yaml" | "yml" => Some(ConfigFormat::Yaml),
            "json" => Some(ConfigFormat::Json),
            _ => None,
        }
    }

    /// Human-readable format name.
    pub fn name(&self) -> &'static str {
        match self {
            ConfigFormat::Toml => "TOML",
            ConfigFormat::Yaml => "YAML",
            ConfigFormat::Json => "JSON",
        }
    }
}

/// How multiple configuration sources are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigMergeStrategy {
    /// Later values overwrite earlier ones, one level deep.
    #[default]
    Override,
    /// Recurse into nested objects and merge key by key.
    DeepMerge,
    /// Use the first source that loads successfully.
    First,
    /// Concatenate arrays; overwrite every other value.
    Accumulate,
}

impl ConfigMergeStrategy {
    /// Stable identifier used in logs and config values.
    pub fn name(&self) -> &'static str {
        match self {
            ConfigMergeStrategy::Override => "override",
            ConfigMergeStrategy::DeepMerge => "deep_merge",
            ConfigMergeStrategy::First => "first",
            ConfigMergeStrategy::Accumulate => "accumulate",
        }
    }
}

use std::sync::Arc;

/// A place configuration is read from.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// A single config file.
    File(String),
    /// A directory whose config files are merged in filename order.
    Directory(String),
    /// An already-parsed JSON value.
    Inline(serde_json::Value),
    /// A user-provided provider.
    Custom(Arc<dyn CustomConfigSource>),
}

impl ConfigSource {
    /// Wrap a custom provider as a source.
    pub fn custom(source: Arc<dyn CustomConfigSource>) -> Self {
        ConfigSource::Custom(source)
    }
}

impl ConfigSource {
    /// A single config file.
    pub fn file(path: impl Into<String>) -> Self {
        ConfigSource::File(path.into())
    }

    /// A directory of config files.
    pub fn directory(path: impl Into<String>) -> Self {
        ConfigSource::Directory(path.into())
    }

    /// An already-parsed JSON value.
    pub fn inline(value: serde_json::Value) -> Self {
        ConfigSource::Inline(value)
    }
}

/// A user-provided configuration source.
pub trait CustomConfigSource: std::fmt::Debug + Send + Sync {
    /// Load this source into a JSON value.
    fn load(&self) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>;
}

/// What the last load read and how it merged it.
#[derive(Debug, Clone)]
pub struct ConfigMetadata {
    /// Names of the sources that contributed, in load order.
    pub sources: Vec<String>,
    /// Format of the last file read, if a single format applied.
    pub format: Option<ConfigFormat>,
    /// The merge strategy that was used.
    pub strategy: ConfigMergeStrategy,
    /// When the load finished.
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

impl ConfigMetadata {
    /// Empty metadata for `strategy`, stamped with the current time.
    pub fn new(strategy: ConfigMergeStrategy) -> Self {
        Self {
            sources: Vec::new(),
            format: None,
            strategy,
            loaded_at: chrono::Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== ConfigFormat Tests ==========

    #[test]
    fn test_config_format_from_path_toml() {
        assert_eq!(
            ConfigFormat::from_path("config.toml"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(
            ConfigFormat::from_path("/path/to/config.toml"),
            Some(ConfigFormat::Toml)
        );
    }

    #[test]
    fn test_config_format_from_path_yaml() {
        assert_eq!(
            ConfigFormat::from_path("config.yaml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::from_path("config.yml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::from_path("/path/to/settings.yaml"),
            Some(ConfigFormat::Yaml)
        );
    }

    #[test]
    fn test_config_format_from_path_json() {
        assert_eq!(
            ConfigFormat::from_path("config.json"),
            Some(ConfigFormat::Json)
        );
        assert_eq!(
            ConfigFormat::from_path("/path/to/data.json"),
            Some(ConfigFormat::Json)
        );
    }

    #[test]
    fn test_config_format_from_path_unknown() {
        assert_eq!(ConfigFormat::from_path("config.txt"), None);
        assert_eq!(ConfigFormat::from_path("config"), None);
        assert_eq!(ConfigFormat::from_path(""), None);
    }

    #[test]
    fn test_config_format_from_path_case_insensitive() {
        assert_eq!(
            ConfigFormat::from_path("CONFIG.TOML"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(
            ConfigFormat::from_path("Config.Yaml"),
            Some(ConfigFormat::Yaml)
        );
        assert_eq!(
            ConfigFormat::from_path("CONFIG.JSON"),
            Some(ConfigFormat::Json)
        );
    }

    #[test]
    fn test_config_format_name() {
        assert_eq!(ConfigFormat::Toml.name(), "TOML");
        assert_eq!(ConfigFormat::Yaml.name(), "YAML");
        assert_eq!(ConfigFormat::Json.name(), "JSON");
    }

    // ========== ConfigMergeStrategy Tests ==========

    #[test]
    fn test_config_merge_strategy_name() {
        assert_eq!(ConfigMergeStrategy::Override.name(), "override");
        assert_eq!(ConfigMergeStrategy::DeepMerge.name(), "deep_merge");
        assert_eq!(ConfigMergeStrategy::First.name(), "first");
        assert_eq!(ConfigMergeStrategy::Accumulate.name(), "accumulate");
    }

    #[test]
    fn test_config_merge_strategy_default() {
        assert_eq!(
            ConfigMergeStrategy::default(),
            ConfigMergeStrategy::Override
        );
    }

    // ========== ConfigSource Tests ==========

    #[test]
    fn test_config_source_file() {
        let source = ConfigSource::file("config.toml");
        assert!(matches!(source, ConfigSource::File(s) if s == "config.toml"));
    }

    #[test]
    fn test_config_source_directory() {
        let source = ConfigSource::directory("/etc/app");
        assert!(matches!(source, ConfigSource::Directory(s) if s == "/etc/app"));
    }

    #[test]
    fn test_config_source_inline() {
        let json = serde_json::json!({"key": "value"});
        let source = ConfigSource::inline(json.clone());
        assert!(matches!(source, ConfigSource::Inline(v) if v == json));
    }

    #[test]
    fn test_config_source_clone_file() {
        let source = ConfigSource::file("test.toml");
        let cloned = source.clone();
        assert!(matches!(cloned, ConfigSource::File(s) if s == "test.toml"));
    }

    #[test]
    fn test_config_source_clone_directory() {
        let source = ConfigSource::directory("/path/to/config");
        let cloned = source.clone();
        assert!(matches!(cloned, ConfigSource::Directory(s) if s == "/path/to/config"));
    }

    #[test]
    fn test_config_source_clone_inline() {
        let json = serde_json::json!({"nested": {"key": 123}});
        let source = ConfigSource::inline(json.clone());
        let cloned = source.clone();
        assert!(matches!(cloned, ConfigSource::Inline(v) if v == json));
    }

    #[test]
    fn test_config_source_clone_custom() {
        #[derive(Debug)]
        struct MockCustomSource;
        impl CustomConfigSource for MockCustomSource {
            fn load(&self) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
                Ok(serde_json::json!({}))
            }
        }
        let source = ConfigSource::custom(Arc::new(MockCustomSource));
        let cloned = source.clone();
        assert!(matches!(cloned, ConfigSource::Custom(_)));
    }

    // ========== ConfigMetadata Tests ==========

    #[test]
    fn test_config_metadata_new() {
        let metadata = ConfigMetadata::new(ConfigMergeStrategy::DeepMerge);
        assert!(metadata.sources.is_empty());
        assert!(metadata.format.is_none());
        assert_eq!(metadata.strategy, ConfigMergeStrategy::DeepMerge);
        assert!(metadata.loaded_at <= chrono::Utc::now());
    }

    #[test]
    fn test_config_metadata_with_strategy() {
        for strategy in [
            ConfigMergeStrategy::Override,
            ConfigMergeStrategy::DeepMerge,
            ConfigMergeStrategy::First,
            ConfigMergeStrategy::Accumulate,
        ] {
            let metadata = ConfigMetadata::new(strategy);
            assert_eq!(metadata.strategy, strategy);
        }
    }

    #[test]
    fn test_config_metadata_clone() {
        let metadata = ConfigMetadata::new(ConfigMergeStrategy::Override);
        let cloned = metadata.clone();
        assert_eq!(cloned.sources, metadata.sources);
        assert_eq!(cloned.format, metadata.format);
        assert_eq!(cloned.strategy, metadata.strategy);
    }
}
