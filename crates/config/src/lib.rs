//! Configuration loading for BrainOS: discovery, parsing, and merging.
#![warn(missing_docs)]

mod error;
pub mod loader;
pub mod types;

pub use error::{ConfigError, ConfigResult};
pub use loader::ConfigLoader;
pub use types::{ConfigFormat, ConfigMergeStrategy};

/// A dotted-path view over discovered configuration.
#[derive(Debug, Default, Clone)]
pub struct Section {
    config: serde_json::Value,
}

impl Section {
    /// Load the discovered configuration, erroring if there are no sources.
    pub async fn init(&mut self) -> Result<(), String> {
        let mut loader = ConfigLoader::new().discover();
        if loader.sources().is_empty() {
            return Err(
                "No config sources found. Make sure ~/.bos/conf/config.toml exists.".to_string(),
            );
        }
        let result = loader.load().await.map_err(|e| e.to_string()).cloned();

        self.config = result?;

        Ok(())
    }

    /// Look up a dotted path such as `llm.openai.key`.
    pub fn section(&self, sec: &str) -> Option<&serde_json::Value> {
        // sec format like "llm.openai.key"
        let keys: Vec<&str> = sec.split('.').collect();
        let mut current = &self.config;

        for key in keys {
            current = current.get(key)?;
        }
        Some(current)
    }

    /// Deserialize the value at a dotted path into `T`.
    pub fn extract<T: serde::de::DeserializeOwned>(&self, sec: &str) -> Option<T> {
        let value = self.section(sec)?;
        serde_json::from_value(value.clone()).ok()
    }
}
