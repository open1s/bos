//! Config source discovery, parsing, and merging.
//!
//! [`ConfigLoader`] collects [`ConfigSource`]s and merges them according to a
//! [`ConfigMergeStrategy`].

use crate::error::{ConfigError, ConfigResult};
use crate::types::{ConfigFormat, ConfigMergeStrategy, ConfigMetadata, ConfigSource};
use log::{debug, info, warn};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Collects configuration sources and merges them into one JSON document.
#[derive(Clone)]
pub struct ConfigLoader {
    sources: Vec<ConfigSource>,
    strategy: ConfigMergeStrategy,
    metadata: Option<ConfigMetadata>,
    cached_config: Option<serde_json::Value>,
}

impl ConfigLoader {
    /// Create an empty loader with the default merge strategy.
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
            strategy: ConfigMergeStrategy::default(),
            metadata: None,
            cached_config: None,
        }
    }

    /// Set the merge strategy.
    pub fn with_strategy(mut self, strategy: ConfigMergeStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Append a configuration source.
    pub fn add_source(mut self, source: ConfigSource) -> Self {
        self.sources.push(source);
        self.cached_config = None;
        self
    }

    /// Append a single config file.
    pub fn add_file(mut self, path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_string_lossy().to_string();
        self.sources.push(ConfigSource::File(path));
        self.cached_config = None;
        self
    }

    /// Append several config files.
    pub fn add_files(mut self, paths: Vec<PathBuf>) -> Self {
        for path in paths {
            self.sources
                .push(ConfigSource::File(path.to_string_lossy().to_string()));
        }
        self.cached_config = None;
        self
    }

    /// Append a directory of config files, erroring if it does not exist.
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

    /// Append an already-parsed JSON value.
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
    /// Mutable variant of [`ConfigLoader::discover`] for the bindings.
    pub fn discover_mut(&mut self) -> &mut Self {
        self.discover_locations();
        self
    }

    /// Mutable variant of [`ConfigLoader::add_file`].
    pub fn add_file_mut(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path = path.as_ref().to_string_lossy().to_string();
        self.sources.push(ConfigSource::File(path));
        self.cached_config = None;
        self
    }

    /// Mutable variant of [`ConfigLoader::add_files`].
    pub fn add_files_mut(&mut self, paths: Vec<PathBuf>) -> &mut Self {
        for path in paths {
            self.sources
                .push(ConfigSource::File(path.to_string_lossy().to_string()));
        }
        self.cached_config = None;
        self
    }

    /// Mutable variant of [`ConfigLoader::add_directory`].
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

    /// Mutable variant of [`ConfigLoader::add_inline`].
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
                debug!("discovered config directory: {}", expanded);
                self.sources
                    .push(ConfigSource::Directory(expanded.into_owned()));
            } else {
                debug!("skipping missing config directory: {}", expanded);
            }
        }

        self.cached_config = None;
    }

    /// Load, merge, and cache the sources; repeated calls reuse the cache.
    pub async fn load(&mut self) -> ConfigResult<&serde_json::Value> {
        if let Some(ref cached) = self.cached_config {
            debug!("using cached config");
            return Ok(cached);
        }

        info!("loading config, strategy: {}", self.strategy.name());
        debug!("config source count: {}", self.sources.len());

        let mut metadata = ConfigMetadata::new(self.strategy);

        if self.sources.is_empty() {
            warn!("no config sources specified, returning empty config");
            let empty = serde_json::Value::Object(serde_json::Map::new());
            self.cached_config = Some(empty.clone());
            self.metadata = Some(metadata);
            return Ok(self.cached_config.as_ref().unwrap());
        }

        // The sync strategies are the single implementation; the async entry
        // point delegates to them so the two cannot drift.
        match self.strategy {
            ConfigMergeStrategy::First => {
                self.load_first_sync(&mut metadata)?;
            }
            ConfigMergeStrategy::Override => {
                self.load_override_sync(&mut metadata)?;
            }
            ConfigMergeStrategy::DeepMerge => {
                self.load_deep_merge_sync(&mut metadata)?;
            }
            ConfigMergeStrategy::Accumulate => {
                self.load_accumulate_sync(&mut metadata)?;
            }
        }

        self.cached_config.as_ref().ok_or_else(|| {
            ConfigError::LoadError(anyhow::anyhow!(
                "config load finished but produced no cached value"
            ))
        })
    }

    /// Load and deserialize the merged config into `T`.
    pub async fn load_typed<T>(&mut self) -> ConfigResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = self.load().await?;
        let config: T = serde_json::from_value(value.clone()).map_err(ConfigError::Json)?;
        Ok(config)
    }

    /// The last merged value, if a load has run.
    pub fn get(&self) -> Option<&serde_json::Value> {
        self.cached_config.as_ref()
    }

    /// Metadata describing the last load.
    pub fn metadata(&self) -> Option<&ConfigMetadata> {
        self.metadata.as_ref()
    }

    /// The configured sources, in order.
    pub fn sources(&self) -> &[ConfigSource] {
        &self.sources
    }

    /// The active merge strategy.
    pub fn strategy(&self) -> ConfigMergeStrategy {
        self.strategy
    }

    /// Drop the cached value and metadata without touching the sources.
    pub fn reset(&mut self) {
        self.cached_config = None;
        self.metadata = None;
    }

    /// Drop the cache and load again.
    pub async fn reload(&mut self) -> ConfigResult<&serde_json::Value> {
        self.cached_config = None;
        self.metadata = None;
        self.load().await
    }

    /// Synchronous equivalent of [`ConfigLoader::load`].
    pub fn load_sync(&mut self) -> ConfigResult<serde_json::Value> {
        if let Some(ref cached) = self.cached_config {
            return Ok(cached.clone());
        }

        info!("loading config (sync), strategy: {}", self.strategy.name());
        debug!("config source count: {}", self.sources.len());

        let mut metadata = ConfigMetadata::new(self.strategy);

        if self.sources.is_empty() {
            warn!("no config sources specified, returning empty config");
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
                    debug!("failed to load config source: {:#}, trying next", e);
                    continue;
                }
            }
        }
        Err(ConfigError::LoadError(anyhow::anyhow!(
            "all config sources failed to load"
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
                    debug!("failed to load config source: {:#}, trying next", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "all config sources failed to load"
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
                    debug!("failed to load config source: {:#}, skipping", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "all config sources failed to load"
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
                    debug!("failed to load config source: {:#}, skipping", e);
                    continue;
                }
            }
        }

        if !has_value {
            return Err(ConfigError::LoadError(anyhow::anyhow!(
                "all config sources failed to load"
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
                return Err(ConfigError::LoadError(anyhow::anyhow!(
                    "Failed to read {}: {}",
                    path,
                    e
                )));
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
                    debug!("skipping path that is not valid UTF-8: {:?}", path);
                    continue;
                }
            };
            match self.load_file_sync(path_str) {
                Ok((_, value)) => {
                    merged = Self::deep_merge_json(merged, value);
                }
                Err(e) => {
                    debug!("skipping file {:?}: {:#}", path, e);
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
        std::fs::write(
            base.join("config.toml"),
            r#"x = 1
y = 1"#,
        )
        .unwrap();
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
            if let ConfigSource::Directory(dir) = src {
                assert!(
                    Path::new(dir).exists(),
                    "discover should only add existing dirs"
                );
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

        let mut loader = ConfigLoader::new().discover();
        // Discovery must find the home directory, but its contents are the
        // user's own config, so assert structure rather than specific values.
        assert!(!loader.sources().is_empty());
        let config = loader.load().await.unwrap();
        assert!(!config.as_object().unwrap().is_empty());
    }
}
