use crate::error::{ConfigError, ConfigResult};
use crate::types::{ConfigFormat, ConfigMergeStrategy, ConfigMetadata, ConfigSource};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct ConfigLoader {
    sources: Vec<ConfigSource>,
    strategy: ConfigMergeStrategy,
    metadata: Option<ConfigMetadata>,
    cached_config: Option<serde_json::Value>,
}

impl ConfigLoader {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            strategy: ConfigMergeStrategy::default(),
            metadata: None,
            cached_config: None,
        }
    }

    pub fn with_strategy(mut self, strategy: ConfigMergeStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    pub fn add_source(mut self, source: ConfigSource) -> Self {
        self.sources.push(source);
        self.cached_config = None;
        self
    }

    pub fn add_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_string_lossy().to_string();
        self.sources.push(ConfigSource::File(path));
        self.cached_config = None;
        self
    }

    pub fn add_files(mut self, paths: Vec<PathBuf>) -> Self {
        for path in paths {
            self.sources
                .push(ConfigSource::File(path.to_string_lossy().to_string()));
        }
        self.cached_config = None;
        self
    }

    pub fn add_directory(mut self, path: impl AsRef<Path>) -> ConfigResult<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_string_lossy().to_string()));
        }
        self.sources
            .push(ConfigSource::Directory(path.to_string_lossy().to_string()));
        self.cached_config = None;
        Ok(self)
    }

    pub fn add_inline(mut self, value: serde_json::Value) -> Self {
        self.sources.push(ConfigSource::Inline(value));
        self.cached_config = None;
        self
    }

    // Mutable builder methods for Python bindings
    pub fn add_file_mut(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path = path.as_ref().to_string_lossy().to_string();
        self.sources.push(ConfigSource::File(path));
        self.cached_config = None;
        self
    }

    pub fn add_files_mut(&mut self, paths: Vec<PathBuf>) -> &mut Self {
        for path in paths {
            self.sources
                .push(ConfigSource::File(path.to_string_lossy().to_string()));
        }
        self.cached_config = None;
        self
    }

    pub fn add_directory_mut(&mut self, path: impl AsRef<Path>) -> ConfigResult<&mut Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_string_lossy().to_string()));
        }
        self.sources
            .push(ConfigSource::Directory(path.to_string_lossy().to_string()));
        self.cached_config = None;
        Ok(self)
    }

    pub fn add_inline_mut(&mut self, value: serde_json::Value) -> &mut Self {
        self.sources.push(ConfigSource::Inline(value));
        self.cached_config = None;
        self
    }

    pub async fn load(&mut self) -> ConfigResult<&serde_json::Value> {
        if let Some(ref cached) = self.cached_config {
            debug!("使用缓存的配置");
            return Ok(cached);
        }

        info!("开始加载配置，策略: {}", self.strategy.name());
        debug!("配置源数量: {}", self.sources.len());

        let mut metadata = ConfigMetadata::new(self.strategy);

        if self.sources.is_empty() {
            warn!("未指定任何配置源，返回空配置");
            let empty = serde_json::Value::Object(serde_json::Map::new());
            self.cached_config = Some(empty.clone());
            self.metadata = Some(metadata);
            return Ok(self.cached_config.as_ref().unwrap());
        }

        match self.strategy {
            ConfigMergeStrategy::First => self.load_first(&mut metadata).await,
            ConfigMergeStrategy::Override => self.load_override(&mut metadata).await,
            ConfigMergeStrategy::DeepMerge => self.load_deep_merge(&mut metadata).await,
            ConfigMergeStrategy::Accumulate => self.load_accumulate(&mut metadata).await,
        }
    }

    pub async fn load_typed<T>(&mut self) -> ConfigResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = self.load().await?;
        let config: T = serde_json::from_value(value.clone()).map_err(ConfigError::Json)?;
        Ok(config)
    }

    pub fn get(&self) -> Option<&serde_json::Value> {
        self.cached_config.as_ref()
    }

    pub fn metadata(&self) -> Option<&ConfigMetadata> {
        self.metadata.as_ref()
    }

    pub fn sources(&self) -> &[ConfigSource] {
        &self.sources
    }

    pub fn strategy(&self) -> ConfigMergeStrategy {
        self.strategy
    }

    pub fn reset(&mut self) {
        self.cached_config = None;
        self.metadata = None;
    }

    pub async fn reload(&mut self) -> ConfigResult<&serde_json::Value> {
        self.cached_config = None;
        self.metadata = None;
        self.load().await
    }

    pub fn load_sync(&mut self) -> ConfigResult<serde_json::Value> {
        if let Some(ref cached) = self.cached_config {
            return Ok(cached.clone());
        }

        info!("开始加载配置（同步），策略: {}", self.strategy.name());
        debug!("配置源数量: {}", self.sources.len());

        let mut metadata = ConfigMetadata::new(self.strategy);

        if self.sources.is_empty() {
            warn!("未指定任何配置源，返回空配置");
            let empty = serde_json::Value::Object(serde_json::Map::new());
            self.cached_config = Some(empty.clone());
            self.metadata = Some(metadata);
            return Ok(empty);
        }

        match self.strategy {
            ConfigMergeStrategy::First => self.load_first_sync(&mut metadata),
            ConfigMergeStrategy::Override => self.load_override_sync(&mut metadata),
            ConfigMergeStrategy::DeepMerge => self.load_deep_merge_sync(&mut metadata),
            ConfigMergeStrategy::Accumulate => self.load_accumulate_sync(&mut metadata),
        }
    }

    fn load_first_sync(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<serde_json::Value> {
        for source in &self.sources {
            match self.load_source_sync(source, metadata) {
                Ok(v) => {
                    self.cached_config = Some(v.clone());
                    self.metadata = Some(metadata.clone());
                    return Ok(v);
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 尝试下一个", e);
                    continue;
                }
            }
        }
        Err(ConfigError::LoadError(anyhow::anyhow!(
            "所有配置源均加载失败"
        )))
    }

    fn load_override_sync(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source_sync(source, metadata) {
                Ok(value) => {
                    final_value = Self::override_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 尝试下一个", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value.clone());
        self.metadata = Some(metadata.clone());
        Ok(final_value)
    }

    fn load_deep_merge_sync(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source_sync(source, metadata) {
                Ok(value) => {
                    final_value = Self::deep_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 跳过", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value.clone());
        self.metadata = Some(metadata.clone());
        Ok(final_value)
    }

    fn load_accumulate_sync(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source_sync(source, metadata) {
                Ok(value) => {
                    final_value = Self::accumulate_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 跳过", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value.clone());
        self.metadata = Some(metadata.clone());
        Ok(final_value)
    }

    fn load_source_sync(
        &self,
        source: &ConfigSource,
        metadata: &mut ConfigMetadata,
    ) -> Result<serde_json::Value, ConfigError> {
        let (source_name, value) = match source {
            ConfigSource::File(path) => self.load_file_sync(path)?,
            ConfigSource::Directory(dir) => self.load_directory_sync(dir, metadata)?,
            ConfigSource::Inline(value) => ("inline".to_string(), value.clone()),
            ConfigSource::Custom(custom) => {
                let value = custom
                    .load()
                    .map_err(|e| ConfigError::Custom(e.to_string()))?;
                ("custom".to_string(), value)
            }
        };
        metadata.sources.push(source_name);
        Ok(value)
    }

    fn load_file_sync(&self, path: &str) -> Result<(String, serde_json::Value), ConfigError> {
        use std::fs;

        let path_obj = Path::new(path);

        if !path_obj.exists() {
            return Err(ConfigError::NotFound(path.to_string()));
        }

        let format = ConfigFormat::from_path(path)
            .ok_or_else(|| ConfigError::UnsupportedFormat(path.to_string()))?;

        let content = fs::read_to_string(path_obj)?;
        let value = Self::parse_content(&content, format)?;

        Ok((path.to_string(), value))
    }

    fn load_directory_sync(
        &self,
        dir: &str,
        metadata: &mut ConfigMetadata,
    ) -> Result<(String, serde_json::Value), ConfigError> {
        use std::fs;

        let dir_path = Path::new(dir);
        let mut merged = serde_json::Value::Object(serde_json::Map::new());

        let entries = fs::read_dir(dir_path)?;

        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let path = e.path();
                path.is_file() && ConfigFormat::from_path(path.to_str().unwrap_or("")).is_some()
            })
            .collect();

        files.sort_by_key(|e| e.path());

        for entry in files {
            let path = entry.path();
            match self.load_file_sync(path.to_str().unwrap_or("")) {
                Ok((_, value)) => {
                    merged = Self::deep_merge_json(merged, value);
                }
                Err(e) => {
                    debug!("跳过文件 {:?}: {:#}", path, e);
                    continue;
                }
            }
        }

        metadata.format = None;
        Ok((dir.to_string(), merged))
    }

    async fn load_first(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<&serde_json::Value> {
        for source in &self.sources {
            match self.load_source(source, metadata).await {
                Ok(v) => {
                    self.cached_config = Some(v);
                    self.metadata = Some(metadata.clone());
                    return Ok(self.cached_config.as_ref().unwrap());
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 尝试下一个", e);
                    continue;
                }
            }
        }
        Err(ConfigError::LoadError(anyhow::anyhow!(
            "所有配置源均加载失败"
        )))
    }

    async fn load_override(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<&serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source(source, metadata).await {
                Ok(value) => {
                    final_value = Self::override_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 尝试下一个", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value);
        self.metadata = Some(metadata.clone());
        Ok(self.cached_config.as_ref().unwrap())
    }

    async fn load_deep_merge(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<&serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source(source, metadata).await {
                Ok(value) => {
                    final_value = Self::deep_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 跳过", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value);
        self.metadata = Some(metadata.clone());
        Ok(self.cached_config.as_ref().unwrap())
    }

    async fn load_accumulate(
        &mut self,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<&serde_json::Value> {
        let mut final_value = serde_json::Value::Object(serde_json::Map::new());
        let mut has_value = false;

        for source in &self.sources {
            match self.load_source(source, metadata).await {
                Ok(value) => {
                    final_value = Self::accumulate_merge_json(final_value, value);
                    has_value = true;
                }
                Err(e) => {
                    debug!("加载配置源失败: {:#}, 跳过", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "所有配置源均加载失败"
            )));
        }

        self.cached_config = Some(final_value);
        self.metadata = Some(metadata.clone());
        Ok(self.cached_config.as_ref().unwrap())
    }

    async fn load_source(
        &self,
        source: &ConfigSource,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<serde_json::Value> {
        let (source_name, value) = match source {
            ConfigSource::File(path) => self.load_file(path).await?,
            ConfigSource::Directory(dir) => self.load_directory(dir, metadata).await?,
            ConfigSource::Inline(value) => ("inline".to_string(), value.clone()),
            ConfigSource::Custom(custom) => {
                let value = custom
                    .load()
                    .map_err(|e| ConfigError::Custom(e.to_string()))?;
                ("custom".to_string(), value)
            }
        };
        metadata.sources.push(source_name);
        Ok(value)
    }

    async fn load_file(&self, path: &str) -> ConfigResult<(String, serde_json::Value)> {
        let path_obj = Path::new(path);

        if !path_obj.exists() {
            return Err(ConfigError::NotFound(path.to_string()));
        }

        let format = ConfigFormat::from_path(path)
            .ok_or_else(|| ConfigError::UnsupportedFormat(path.to_string()))?;

        let content = tokio::fs::read_to_string(path_obj).await?;
        let value = Self::parse_content(&content, format)?;

        Ok((path.to_string(), value))
    }

    async fn load_directory(
        &self,
        dir: &str,
        metadata: &mut ConfigMetadata,
    ) -> ConfigResult<(String, serde_json::Value)> {
        let dir_path = Path::new(dir);
        let mut merged = serde_json::Value::Object(serde_json::Map::new());

        let mut entries = vec![];
        let mut read_dir = tokio::fs::read_dir(dir_path).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            entries.push(entry);
        }

        let mut files: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                let path = e.path();
                path.is_file() && ConfigFormat::from_path(path.to_str().unwrap_or("")).is_some()
            })
            .collect();

        files.sort_by_key(|e| e.path());

        for entry in files {
            let path = entry.path();
            match self.load_file(path.to_str().unwrap_or("")).await {
                Ok((_, value)) => {
                    merged = Self::deep_merge_json(merged, value);
                }
                Err(e) => {
                    debug!("跳过文件 {:?}: {:#}", path, e);
                    continue;
                }
            }
        }

        metadata.format = None;
        Ok((dir.to_string(), merged))
    }

    fn parse_content(content: &str, format: ConfigFormat) -> ConfigResult<serde_json::Value> {
        let value = match format {
            ConfigFormat::Toml => toml::from_str(content).map_err(ConfigError::TomlParse)?,
            ConfigFormat::Yaml => serde_yaml::from_str(content).map_err(ConfigError::YamlParse)?,
            ConfigFormat::Json => serde_json::from_str(content).map_err(ConfigError::Json)?,
        };
        Ok(value)
    }

    fn deep_merge_json(base: serde_json::Value, merge: serde_json::Value) -> serde_json::Value {
        match (base, merge) {
            (serde_json::Value::Object(mut base_map), serde_json::Value::Object(merge_map)) => {
                for (key, value) in merge_map {
                    if base_map.contains_key(&key) {
                        let base_value = base_map.remove(&key).unwrap();
                        base_map.insert(key, Self::deep_merge_json(base_value, value));
                    } else {
                        base_map.insert(key, value);
                    }
                }
                serde_json::Value::Object(base_map)
            }
            (_, merge) => merge,
        }
    }

    fn accumulate_merge_json(
        base: serde_json::Value,
        merge: serde_json::Value,
    ) -> serde_json::Value {
        match (base, merge) {
            (serde_json::Value::Array(mut base_arr), serde_json::Value::Array(merge_arr)) => {
                base_arr.extend(merge_arr);
                serde_json::Value::Array(base_arr)
            }
            (serde_json::Value::Object(mut base_map), serde_json::Value::Object(merge_map)) => {
                for (key, value) in merge_map {
                    if base_map.contains_key(&key) {
                        let base_value = base_map.remove(&key).unwrap();
                        base_map.insert(key, Self::accumulate_merge_json(base_value, value));
                    } else {
                        base_map.insert(key, value);
                    }
                }
                serde_json::Value::Object(base_map)
            }
            (_, merge) => merge,
        }
    }

    fn override_merge_json(base: serde_json::Value, merge: serde_json::Value) -> serde_json::Value {
        match (base, merge) {
            (serde_json::Value::Object(mut base_map), serde_json::Value::Object(merge_map)) => {
                for (key, value) in merge_map {
                    if base_map.contains_key(&key) {
                        let base_value = base_map.remove(&key).unwrap();
                        base_map.insert(key, Self::override_merge_json(base_value, value));
                    } else {
                        base_map.insert(key, value);
                    }
                }
                serde_json::Value::Object(base_map)
            }
            (_, merge) => merge,
        }
    }
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_format_from_path() {
        assert!(matches!(
            ConfigFormat::from_path("config.toml"),
            Some(ConfigFormat::Toml)
        ));
        assert!(matches!(
            ConfigFormat::from_path("config.yaml"),
            Some(ConfigFormat::Yaml)
        ));
        assert!(matches!(
            ConfigFormat::from_path("config.yml"),
            Some(ConfigFormat::Yaml)
        ));
        assert!(matches!(
            ConfigFormat::from_path("config.json"),
            Some(ConfigFormat::Json)
        ));
        assert!(ConfigFormat::from_path("config.txt").is_none());
    }

    #[test]
    fn test_deep_merge_json() {
        let base = serde_json::json!({
            "a": 1,
            "b": {
                "c": 2,
                "d": 3
            }
        });

        let merge = serde_json::json!({
            "b": {
                "c": 20,
                "e": 4
            },
            "f": 5
        });

        let result = ConfigLoader::deep_merge_json(base, merge);

        assert_eq!(result["a"], 1);
        assert_eq!(result["b"]["c"], 20);
        assert_eq!(result["b"]["d"], 3);
        assert_eq!(result["b"]["e"], 4);
        assert_eq!(result["f"], 5);
    }

    // ========== ConfigLoader Tests ==========

    #[tokio::test]
    async fn test_loader_new() {
        let loader = ConfigLoader::new();
        // Default strategy should be Override
        assert_eq!(loader.strategy, ConfigMergeStrategy::Override);
        assert!(loader.sources.is_empty());
    }

    #[tokio::test]
    async fn test_loader_with_strategy() {
        let loader = ConfigLoader::new().with_strategy(ConfigMergeStrategy::DeepMerge);
        assert_eq!(loader.strategy, ConfigMergeStrategy::DeepMerge);
    }

    #[tokio::test]
    async fn test_loader_add_single_file() {
        let loader = ConfigLoader::new().add_file("test.toml");
        assert_eq!(loader.sources.len(), 1);
        assert!(matches!(loader.sources[0], ConfigSource::File(_)));
    }

    #[tokio::test]
    async fn test_loader_add_multiple_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file1 = temp_dir.path().join("config1.toml");
        let file2 = temp_dir.path().join("config2.toml");
        std::fs::write(&file1, "").unwrap();
        std::fs::write(&file2, "").unwrap();

        let loader = ConfigLoader::new().add_files(vec![file1, file2]);
        assert_eq!(loader.sources.len(), 2);
    }

    #[tokio::test]
    async fn test_loader_add_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("config.toml"), "").unwrap();

        let loader = ConfigLoader::new().add_directory(temp_dir.path());
        assert!(loader.is_ok());
        assert!(!loader.unwrap().sources.is_empty());
    }

    #[tokio::test]
    async fn test_loader_add_directory_invalid() {
        let result = ConfigLoader::new().add_directory("/nonexistent/path");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_add_inline() {
        let json = serde_json::json!({"key": "value"});
        let loader = ConfigLoader::new().add_inline(json.clone());
        assert_eq!(loader.sources.len(), 1);
        assert!(matches!(&loader.sources[0], ConfigSource::Inline(v) if *v == json));
    }

    #[tokio::test]
    async fn test_loader_load_empty() {
        let mut loader = ConfigLoader::new();
        let config = loader.load().await.unwrap();
        assert!(config.is_object());
        assert_eq!(config.as_object().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_loader_load_single_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_content = r#"
            [database]
            host = "localhost"
            port = 5432
        "#;
        let file_path = temp_dir.path().join("config.toml");
        std::fs::write(&file_path, config_content).unwrap();

        let mut loader = ConfigLoader::new().add_file(&file_path);
        let config = loader.load().await.unwrap();

        assert_eq!(config["database"]["host"], "localhost");
        assert_eq!(config["database"]["port"], 5432);
    }

    #[tokio::test]
    async fn test_loader_load_yaml_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_content = r#"
            app:
              name: test_app
              version: 1.0.0
        "#;
        let file_path = temp_dir.path().join("config.yaml");
        std::fs::write(&file_path, config_content).unwrap();

        let mut loader = ConfigLoader::new().add_file(&file_path);
        let config = loader.load().await.unwrap();

        assert_eq!(config["app"]["name"], "test_app");
        assert_eq!(config["app"]["version"], "1.0.0");
    }

    #[tokio::test]
    async fn test_loader_load_json_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_content = r#"{"settings": {"debug": true, "timeout": 30}}"#;
        let file_path = temp_dir.path().join("config.json");
        std::fs::write(&file_path, config_content).unwrap();

        let mut loader = ConfigLoader::new().add_file(&file_path);
        let config = loader.load().await.unwrap();

        assert_eq!(config["settings"]["debug"], true);
        assert_eq!(config["settings"]["timeout"], 30);
    }

    #[tokio::test]
    async fn test_loader_reload() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("config.toml");
        std::fs::write(&file_path, "[section]\nvalue = \"original\"").unwrap();

        let mut loader = ConfigLoader::new().add_file(&file_path);
        let config1 = loader.load().await.unwrap();
        assert_eq!(config1["section"]["value"], "original");

        // Modify the file
        std::fs::write(&file_path, "[section]\nvalue = \"updated\"").unwrap();

        let config2 = loader.reload().await.unwrap();
        assert_eq!(config2["section"]["value"], "updated");
    }

    #[tokio::test]
    async fn test_loader_reload_empty() {
        let mut loader = ConfigLoader::new();
        let config = loader.reload().await.unwrap();
        assert!(config.is_object());
    }

    #[tokio::test]
    async fn test_loader_load_nonexistent_file() {
        let mut loader = ConfigLoader::new().add_file("/nonexistent/config.toml");
        let result = loader.load().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_override_strategy() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("config1.toml"),
            "[section]\nvalue = \"first\"",
        )
        .unwrap();
        std::fs::write(
            temp_dir.path().join("config2.toml"),
            "[section]\nvalue = \"second\"",
        )
        .unwrap();

        let mut loader = ConfigLoader::new()
            .with_strategy(ConfigMergeStrategy::Override)
            .add_files(vec![
                temp_dir.path().join("config1.toml"),
                temp_dir.path().join("config2.toml"),
            ]);

        let config = loader.load().await.unwrap();
        assert_eq!(config["section"]["value"], "second");
    }

    #[tokio::test]
    async fn test_loader_first_strategy() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("config1.toml"),
            "[section]\nvalue = \"first\"",
        )
        .unwrap();
        std::fs::write(
            temp_dir.path().join("config2.toml"),
            "[section]\nvalue = \"second\"",
        )
        .unwrap();

        let mut loader = ConfigLoader::new()
            .with_strategy(ConfigMergeStrategy::First)
            .add_files(vec![
                temp_dir.path().join("config1.toml"),
                temp_dir.path().join("config2.toml"),
            ]);

        let config = loader.load().await.unwrap();
        assert_eq!(config["section"]["value"], "first");
    }

    #[tokio::test]
    async fn test_loader_accumulate_strategy() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("config1.json"),
            r#"{"items": [1, 2], "name": "first"}"#,
        )
        .unwrap();
        std::fs::write(
            temp_dir.path().join("config2.json"),
            r#"{"items": [3, 4], "name": "second"}"#,
        )
        .unwrap();

        let mut loader = ConfigLoader::new()
            .with_strategy(ConfigMergeStrategy::Accumulate)
            .add_files(vec![
                temp_dir.path().join("config1.json"),
                temp_dir.path().join("config2.json"),
            ]);

        let config = loader.load().await.unwrap();
        assert_eq!(config["items"], serde_json::json!([1, 2, 3, 4]));
        assert_eq!(config["name"], "second");
    }

    #[tokio::test]
    async fn test_loader_mixed_sources() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("file.toml"),
            "[file_config]\nfrom_file = true",
        )
        .unwrap();

        let inline = serde_json::json!({"inline_config": {"from_inline": true}});

        // Use DeepMerge to preserve both sources
        let mut loader = ConfigLoader::new()
            .with_strategy(ConfigMergeStrategy::DeepMerge)
            .add_file(temp_dir.path().join("file.toml"))
            .add_inline(inline);

        let config = loader.load().await.unwrap();
        assert_eq!(config["file_config"]["from_file"], true);
        assert_eq!(config["inline_config"]["from_inline"], true);
    }

    #[tokio::test]
    async fn test_loader_load_typed() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        struct AppConfig {
            database: DatabaseConfig,
        }

        #[derive(Deserialize, Debug)]
        struct DatabaseConfig {
            host: String,
            port: u16,
        }

        let temp_dir = tempfile::tempdir().unwrap();
        let config_content = r#"
            [database]
            host = "db.example.com"
            port = 5432
        "#;
        let file_path = temp_dir.path().join("config.toml");
        std::fs::write(&file_path, config_content).unwrap();

        let mut loader = ConfigLoader::new().add_file(&file_path);
        let typed: AppConfig = loader.load_typed().await.unwrap();

        assert_eq!(typed.database.host, "db.example.com");
        assert_eq!(typed.database.port, 5432);
    }

    #[tokio::test]
    async fn test_loader_load_typed_error() {
        #[derive(Deserialize, Debug)]
        struct BadConfig {
            _required_field: String,
        }

        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("config.toml"),
            "[bad_config]\nmissing = \"value\"",
        )
        .unwrap();

        let mut loader = ConfigLoader::new().add_file(temp_dir.path().join("config.toml"));
        let result: Result<BadConfig, _> = loader.load_typed().await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_invalid_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("invalid.toml"),
            "invalid: toml: content: [",
        )
        .unwrap();

        let mut loader = ConfigLoader::new().add_file(temp_dir.path().join("invalid.toml"));
        let result = loader.load().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_invalid_yaml() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("invalid.yaml"),
            "invalid: yaml: content: [",
        )
        .unwrap();

        let mut loader = ConfigLoader::new().add_file(temp_dir.path().join("invalid.yaml"));
        let result = loader.load().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_invalid_json() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(temp_dir.path().join("invalid.json"), "{invalid json").unwrap();

        let mut loader = ConfigLoader::new().add_file(temp_dir.path().join("invalid.json"));
        let result = loader.load().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_deep_merge_arrays() {
        // Arrays should be replaced, not merged
        let base = serde_json::json!({"items": [1, 2]});
        let merge = serde_json::json!({"items": [3, 4]});

        let result = ConfigLoader::deep_merge_json(base, merge);
        assert_eq!(result["items"], serde_json::json!([3, 4]));
    }

    #[test]
    fn test_deep_merge_nested_objects() {
        let base = serde_json::json!({
            "outer": {
                "inner1": "value1",
                "inner2": "value2"
            }
        });
        let merge = serde_json::json!({
            "outer": {
                "inner1": "updated",
                "inner3": "value3"
            }
        });

        let result = ConfigLoader::deep_merge_json(base, merge);
        assert_eq!(result["outer"]["inner1"], "updated");
        assert_eq!(result["outer"]["inner2"], "value2");
        assert_eq!(result["outer"]["inner3"], "value3");
    }

    #[test]
    fn test_deep_merge_null_values() {
        let base = serde_json::json!({"key": "value"});
        let merge = serde_json::json!({"key": serde_json::Value::Null});

        let result = ConfigLoader::deep_merge_json(base, merge);
        assert_eq!(result["key"], serde_json::Value::Null);
    }

    #[test]
    fn test_deep_merge_primitives() {
        let base = serde_json::json!("old");
        let merge = serde_json::json!("new");

        let result = ConfigLoader::deep_merge_json(base, merge);
        assert_eq!(result, "new");
    }
}
