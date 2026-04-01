use crate::error::{ConfigError, ConfigResult};
use crate::types::{ConfigFormat, ConfigMergeStrategy, ConfigMetadata, ConfigSource};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

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

    /// Discover config files from standard locations.
    ///
    /// Searches in priority order (later overwrites earlier):
    /// 1. `/etc/bos/conf`
    /// 2. `~/.bos/conf`
    /// 3. `~/.config/bos/conf`
    /// 4. `./bos/conf` (current working directory)
    ///
    /// Supports `.toml`, `.yaml`, `.yml`, `.json` files.
    /// Skips directories that don't exist — no error.
    pub fn discover(mut self) -> Self {
        self.discover_locations();
        self
    }

// Mutable builder methods for Python bindings
pub fn discover_mut(&mut self) -> &mut Self {
    self.discover_locations();
    self
}

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

    fn discover_locations(&mut self) {
        let dirs = [
            "/etc/bos/conf",
            "~/.bos/conf",
            "~/.config/bos/conf",
            "./bos/conf",
        ];

        for dir in &dirs {
            let expanded = shellexpand::tilde(dir);
            let path = Path::new(expanded.as_ref());
            if path.exists() && path.is_dir() {
                debug!("发现配置目录: {}", expanded);
                self.sources
                    .push(ConfigSource::Directory(expanded.into_owned()));
            } else {
                debug!("跳过不存在的配置目录: {}", expanded);
            }
        }

        self.cached_config = None;
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
            ConfigSource::File(path) => {
                let res = self.load_file_sync(path)?;
                if let Some(format) = ConfigFormat::from_path(path) {
                    metadata.format = Some(format);
                }
                res
            }
            ConfigSource::Directory(dir) => {
                metadata.format = None;
                self.load_directory_sync(dir, metadata)?
            }
            ConfigSource::Inline(value) => ("inline".to_string(), value.clone()),
            ConfigSource::Custom(custom) => {
                metadata.format = None;
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

        let content = match fs::read_to_string(path_obj) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read config file '{}': {}", path, e);
                return Err(ConfigError::LoadError(anyhow::anyhow!("Failed to read {}: {}", path, e)));
            }
        };
        
        let value = match Self::parse_content(&content, format) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Failed to parse config file '{}': {}", path, e);
                return Err(e);
            }
        };

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
                if !path.is_file() {
                    return false;
                }
                if let Some(path_str) = path.to_str() {
                    ConfigFormat::from_path(path_str).is_some()
                } else {
                    false
                }
            })
            .collect();

        files.sort_by_key(|e| e.path());

        for entry in files {
            let path = entry.path();
            let path_str = match path.to_str() {
                Some(s) => s,
                None => {
                    debug!("跳过无法转换为 UTF-8 的文件路径: {:?}", path);
                    continue;
                }
            };
            match self.load_file_sync(path_str) {
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
            ConfigSource::File(path) => {
                let res = self.load_file(path).await?;
                if let Some(format) = ConfigFormat::from_path(path) {
                    metadata.format = Some(format);
                }
                res
            }
            ConfigSource::Directory(dir) => {
                metadata.format = None;
                self.load_directory(dir, metadata).await?
            }
            ConfigSource::Inline(value) => ("inline".to_string(), value.clone()),
            ConfigSource::Custom(custom) => {
                metadata.format = None;
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

        let content = match tokio::fs::read_to_string(path_obj).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to read config file '{}': {}", path, e);
                return Err(ConfigError::LoadError(anyhow::anyhow!("Failed to read {}: {}", path, e)));
            }
        };
        
        let value = match Self::parse_content(&content, format) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Failed to parse config file '{}': {}", path, e);
                return Err(e);
            }
        };

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
            let path_str = match path.to_str() {
                Some(s) => s,
                None => {
                    debug!("跳过无法转换为 UTF-8 的文件路径: {:?}", path);
                    continue;
                }
            };
            match self.load_file(path_str).await {
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

    #[tokio::test]
    async fn test_load_config() {
        let mut loader = crate::loader::ConfigLoader::new();
        let config = loader.load().await.unwrap();
        info!("Loaded config: {:#?}", config);
    }

    #[tokio::test]
    async fn test_discover_add_existing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bos").join("config");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.toml"), r#"key = "a""#).unwrap();

        let mut loader = ConfigLoader::new()
            .discover()
            .add_source(ConfigSource::Directory(dir.to_string_lossy().to_string()));
        loader.load().await.unwrap();
        let config = loader.get().unwrap();
        assert_eq!(config.get("key").unwrap(), "a");
    }

    #[tokio::test]
    async fn test_discover_override() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let override_dir = tmp.path().join("override");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(base.join("config.toml"), r#"x = 1
y = 1"#).unwrap();
        std::fs::write(override_dir.join("config.toml"), r#"y = 2"#).unwrap();

        let mut loader = ConfigLoader::new();
        loader
            .add_directory_mut(base.to_string_lossy().to_string())
            .unwrap();
        loader
            .add_directory_mut(override_dir.to_string_lossy().to_string())
            .unwrap();
        loader.load().await.unwrap();
        let config = loader.get().unwrap();
        assert_eq!(config.get("x").unwrap(), 1);
        assert_eq!(config.get("y").unwrap(), 2);
    }

    #[test]
    fn test_discover_skips_nonexistent_dirs() {
        let loader = ConfigLoader::new();
        let loader = loader.discover();
        let sources = loader.sources();

        for src in sources {
            match src {
                ConfigSource::Directory(dir) => {
                    assert!(Path::new(dir).exists(), "discover should only add existing dirs");
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_discover_loads_home_config() {
        let home = std::env::var("HOME").ok();
        if home.is_none() {
            return;
        }

        let home_conf = PathBuf::from(home.unwrap()).join(".bos/conf");
        if !home_conf.exists() || !home_conf.is_dir() {
            return;
        }

        let mut loader = ConfigLoader::new()
            .discover();
        let config = loader.load().await.unwrap();

        assert!(!config.as_object().unwrap().is_empty());
        assert_eq!(
            config.get("general").and_then(|v| v.get("name")).and_then(|v| v.as_str()),
            Some("brainos")
        );
    }
}
