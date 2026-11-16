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
        let mut current = &self.config;

        for key in sec.split('.') {
            current = current.get(key)?;
        }
        Some(current)
    }

    /// Deserialize the value at a dotted path into `T`.
    pub fn extract<T: serde::de::DeserializeOwned>(&self, sec: &str) -> Option<T> {
        let value = self.section(sec)?;
        // Deserialize straight from the borrowed value instead of cloning the
        // whole subtree first.
        T::deserialize(value).ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Llm {
        model: String,
    }

    #[test]
    fn test_section_and_extract_without_cloning_the_subtree() {
        let section = Section {
            config: serde_json::json!({
                "llm": { "model": "m", "nested": { "key": 1 } },
            }),
        };

        assert_eq!(
            section.section("llm.model").and_then(|v| v.as_str()),
            Some("m")
        );
        assert!(section.section("llm.missing").is_none());
        assert!(section.section("missing").is_none());

        let llm: Llm = section.extract("llm").expect("llm section should extract");
        assert_eq!(
            llm,
            Llm {
                model: "m".to_string()
            }
        );
        assert_eq!(
            section
                .extract::<serde_json::Value>("llm.nested")
                .and_then(|v| v.get("key").and_then(|k| k.as_i64())),
            Some(1)
        );
    }
}
