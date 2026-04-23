mod error;
pub mod loader;
pub mod types;

pub use error::{ConfigError, ConfigResult};
pub use loader::ConfigLoader;
pub use types::{ConfigFormat, ConfigMergeStrategy};

#[derive(Debug,Default, Clone)]
pub struct Section {
    config: serde_json::Value
}

impl Section {
    pub async fn init(&mut self) ->Result<(), String>{
        let mut loader = ConfigLoader::new().discover();
        if loader.sources().is_empty() {
            return Err(
                "No config sources found. Make sure ~/.bos/conf/config.toml exists.".to_string(),
            );
        }
        let result = loader
        .load()
        .await
        .map_err(|e| e.to_string())
        .cloned()
        .map(|v| v.clone());

        self.config = result?;

        Ok(())
    }

    pub fn section(&self, sec: &str) -> Option<&serde_json::Value> {
        // sec format like "llm.openai.key"
        let keys: Vec<&str> = sec.split('.').collect();
        let mut current = &self.config;

        for key in keys {
            current = current.get(key)?;
        }
        Some(current)
    }

    pub fn extract<T: serde::de::DeserializeOwned>(&self, sec: &str) -> Option<T> {
        let value = self.section(sec)?;
        serde_json::from_value(value.clone()).ok()
    }
}