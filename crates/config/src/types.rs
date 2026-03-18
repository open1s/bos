/// 配置文件格式支持
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Yaml,
    Json,
}

impl ConfigFormat {
    /// 从文件路径推断格式
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

    /// 获取格式名称
    pub fn name(&self) -> &'static str {
        match self {
            ConfigFormat::Toml => "TOML",
            ConfigFormat::Yaml => "YAML",
            ConfigFormat::Json => "JSON",
        }
    }
}

/// 配置合并策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigMergeStrategy {
    /// 覆盖：后面的配置完全覆盖前面的
    #[default]
    Override,
    /// 深度合并：递归合并嵌套结构
    DeepMerge,
    /// 首个：只使用第一个有效的配置
    First,
    /// 累加：数组类型累加，其他覆盖
    Accumulate,
}

impl ConfigMergeStrategy {
    pub fn name(&self) -> &'static str {
        match self {
            ConfigMergeStrategy::Override => "override",
            ConfigMergeStrategy::DeepMerge => "deep_merge",
            ConfigMergeStrategy::First => "first",
            ConfigMergeStrategy::Accumulate => "accumulate",
        }
    }
}

/// 配置源
#[derive(Debug)]
pub enum ConfigSource {
    File(String),
    Directory(String),
    Inline(serde_json::Value),
    Custom(Box<dyn CustomConfigSource + Send + Sync>),
}

impl Clone for ConfigSource {
    fn clone(&self) -> Self {
        match self {
            ConfigSource::File(s) => ConfigSource::File(s.clone()),
            ConfigSource::Directory(s) => ConfigSource::Directory(s.clone()),
            ConfigSource::Inline(v) => ConfigSource::Inline(v.clone()),
            ConfigSource::Custom(_) => {
                panic!("Cannot clone Custom config source. Use a different approach.");
            }
        }
    }
}

impl ConfigSource {
    pub fn file(path: impl Into<String>) -> Self {
        ConfigSource::File(path.into())
    }

    pub fn directory(path: impl Into<String>) -> Self {
        ConfigSource::Directory(path.into())
    }

    pub fn inline(value: serde_json::Value) -> Self {
        ConfigSource::Inline(value)
    }
}

/// 自定义配置源 trait
pub trait CustomConfigSource: std::fmt::Debug {
    fn load(&self) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>;
}

/// 配置加载元数据
#[derive(Debug, Clone)]
pub struct ConfigMetadata {
    pub sources: Vec<String>,
    pub format: Option<ConfigFormat>,
    pub strategy: ConfigMergeStrategy,
    pub loaded_at: chrono::DateTime<chrono::Utc>,
}

impl ConfigMetadata {
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
    #[should_panic(expected = "Cannot clone Custom config source")]
    fn test_config_source_clone_custom_panics() {
        #[derive(Debug)]
        struct MockCustomSource;
        impl CustomConfigSource for MockCustomSource {
            fn load(&self) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
                Ok(serde_json::json!({}))
            }
        }
        let source = ConfigSource::Custom(Box::new(MockCustomSource));
        let _ = source.clone();
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
