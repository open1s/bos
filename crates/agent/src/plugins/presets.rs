//! Declarative presets: load a preset from JSON or TOML and validate it.
//!
//! Validation is field-oriented: every problem is a [`PresetError`](crate::plugins::presets::PresetError) carrying the
//! dotted path of the offending field (for example `name`, `max_steps`, or
//! `tools[1]`) so a caller can point at exactly what to fix. All problems in a
//! document are reported together rather than stopping at the first.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One validation problem, tied to the field that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetError {
    /// Dotted path of the offending field, e.g. `tools[1]` or `max_steps`.
    pub path: String,
    /// What is wrong with that field.
    pub message: String,
}

impl PresetError {
    /// Create an error for `path` with `message`.
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Errors raised while loading a preset from a path.
#[derive(Debug)]
pub enum PresetLoadError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The document was not valid JSON or TOML.
    Parse(String),
    /// The document parsed but failed validation.
    Invalid(Vec<PresetError>),
}

impl std::fmt::Display for PresetLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetLoadError::Io(err) => write!(f, "preset I/O error: {err}"),
            PresetLoadError::Parse(err) => write!(f, "preset parse error: {err}"),
            PresetLoadError::Invalid(errors) => {
                let joined = errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(f, "invalid preset: {joined}")
            }
        }
    }
}

impl std::error::Error for PresetLoadError {}

/// A validated declarative preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    /// Preset name; required and non-empty.
    pub name: String,
    /// Tool names the preset enables; required (may be empty).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Optional model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Optional cap on agent steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u64>,
}

impl Preset {
    /// Field names accepted at the top level.
    pub const ALLOWED_KEYS: [&'static str; 5] =
        ["name", "tools", "model", "system_prompt", "max_steps"];

    /// Load a preset from `path`, choosing JSON or TOML by file extension.
    pub fn load(path: &Path) -> Result<Preset, PresetLoadError> {
        let text = std::fs::read_to_string(path).map_err(PresetLoadError::Io)?;
        let is_toml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("toml"))
            .unwrap_or(false);
        if is_toml {
            Self::from_toml_str(&text)
        } else {
            Self::from_json_str(&text)
        }
    }

    /// Parse and validate a JSON preset document.
    pub fn from_json_str(text: &str) -> Result<Preset, PresetLoadError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| PresetLoadError::Parse(e.to_string()))?;
        Self::from_value(&value).map_err(PresetLoadError::Invalid)
    }

    /// Parse and validate a TOML preset document.
    pub fn from_toml_str(text: &str) -> Result<Preset, PresetLoadError> {
        let parsed: toml::Value =
            toml::from_str(text).map_err(|e| PresetLoadError::Parse(e.to_string()))?;
        let value =
            serde_json::to_value(parsed).map_err(|e| PresetLoadError::Parse(e.to_string()))?;
        Self::from_value(&value).map_err(PresetLoadError::Invalid)
    }

    /// Validate an already-parsed JSON document.
    ///
    /// Returns every problem found, in document order.
    pub fn from_value(value: &serde_json::Value) -> Result<Preset, Vec<PresetError>> {
        let Some(object) = value.as_object() else {
            return Err(vec![PresetError::new(
                "",
                format!("expected an object, got {}", json_type(value)),
            )]);
        };

        let mut errors = Vec::new();

        for key in object.keys() {
            if !Self::ALLOWED_KEYS.contains(&key.as_str()) {
                errors.push(PresetError::new(key, "unknown field"));
            }
        }

        let name = match object.get("name") {
            None => {
                errors.push(PresetError::new("name", "missing required field"));
                String::new()
            }
            Some(value) => match value.as_str() {
                Some(name) if !name.trim().is_empty() => name.to_string(),
                Some(_) => {
                    errors.push(PresetError::new("name", "must not be empty"));
                    String::new()
                }
                None => {
                    errors.push(PresetError::new(
                        "name",
                        format!("expected a string, got {}", json_type(value)),
                    ));
                    String::new()
                }
            },
        };

        let tools = match object.get("tools") {
            None => {
                errors.push(PresetError::new("tools", "missing required field"));
                Vec::new()
            }
            Some(value) => match value.as_array() {
                Some(items) => {
                    let mut tools = Vec::with_capacity(items.len());
                    for (index, item) in items.iter().enumerate() {
                        match item.as_str() {
                            Some(tool) => tools.push(tool.to_string()),
                            None => errors.push(PresetError::new(
                                format!("tools[{index}]"),
                                format!("expected a string, got {}", json_type(item)),
                            )),
                        }
                    }
                    tools
                }
                None => {
                    errors.push(PresetError::new(
                        "tools",
                        format!("expected an array, got {}", json_type(value)),
                    ));
                    Vec::new()
                }
            },
        };

        let model = optional_string(object, "model", &mut errors);
        let system_prompt = optional_string(object, "system_prompt", &mut errors);

        let max_steps = match object.get("max_steps") {
            None => None,
            Some(value) => match value.as_u64() {
                Some(steps) => Some(steps),
                None => {
                    errors.push(PresetError::new(
                        "max_steps",
                        format!("expected a non-negative integer, got {}", json_type(value)),
                    ));
                    None
                }
            },
        };

        if errors.is_empty() {
            Ok(Preset {
                name,
                tools,
                model,
                system_prompt,
                max_steps,
            })
        } else {
            Err(errors)
        }
    }
}

fn optional_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    errors: &mut Vec<PresetError>,
) -> Option<String> {
    match object.get(key) {
        None => None,
        Some(value) => match value.as_str() {
            Some(text) => Some(text.to_string()),
            None => {
                errors.push(PresetError::new(
                    key,
                    format!("expected a string, got {}", json_type(value)),
                ));
                None
            }
        },
    }
}

fn json_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_preset_parses() {
        let text = r#"{
            "name": "research",
            "tools": ["web_search", "read_file"],
            "model": "gpt-4o",
            "system_prompt": "Be precise.",
            "max_steps": 12
        }"#;

        let preset = Preset::from_json_str(text).expect("valid preset");
        assert_eq!(preset.name, "research");
        assert_eq!(preset.tools, vec!["web_search", "read_file"]);
        assert_eq!(preset.model.as_deref(), Some("gpt-4o"));
        assert_eq!(preset.system_prompt.as_deref(), Some("Be precise."));
        assert_eq!(preset.max_steps, Some(12));
    }

    #[test]
    fn minimal_preset_needs_only_name_and_tools() {
        let preset = Preset::from_json_str(r#"{"name": "min", "tools": []}"#).unwrap();
        assert_eq!(preset.name, "min");
        assert!(preset.tools.is_empty());
        assert!(preset.model.is_none());
        assert!(preset.max_steps.is_none());
    }

    #[test]
    fn unknown_key_names_the_field() {
        let errors = Preset::from_json_str(r#"{"name": "x", "tools": [], "wat": true}"#)
            .expect_err("unknown key");

        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "wat");
        assert_eq!(errors[0].message, "unknown field");
        assert!(errors[0].to_string().contains("wat"));
    }

    #[test]
    fn missing_required_field_names_the_field() {
        let errors = Preset::from_json_str(r#"{"tools": []}"#).expect_err("missing name");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "name");
        assert_eq!(errors[0].message, "missing required field");
    }

    #[test]
    fn missing_tools_field_is_reported() {
        let errors = Preset::from_json_str(r#"{"name": "x"}"#).expect_err("missing tools");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors[0].path, "tools");
        assert_eq!(errors[0].message, "missing required field");
    }

    #[test]
    fn wrong_type_names_the_field() {
        let text = r#"{"name": "x", "tools": ["ok"], "max_steps": "many"}"#;
        let errors = Preset::from_json_str(text).expect_err("bad type");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "max_steps");
        assert!(errors[0]
            .message
            .contains("expected a non-negative integer"));
        assert!(errors[0].message.contains("string"));
    }

    #[test]
    fn wrong_type_inside_array_names_the_index() {
        let text = r#"{"name": "x", "tools": ["ok", 7]}"#;
        let errors = Preset::from_json_str(text).expect_err("bad element");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "tools[1]");
        assert!(errors[0].message.contains("expected a string"));
    }

    #[test]
    fn all_problems_are_reported_together() {
        let text = r#"{"name": 5, "tools": "nope", "extra": 1, "model": 9}"#;
        let errors = Preset::from_json_str(text).expect_err("many problems");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        let paths: Vec<&str> = errors.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"name"), "{paths:?}");
        assert!(paths.contains(&"tools"), "{paths:?}");
        assert!(paths.contains(&"extra"), "{paths:?}");
        assert!(paths.contains(&"model"), "{paths:?}");
    }

    #[test]
    fn non_object_document_is_rejected() {
        let errors = Preset::from_json_str("[1, 2, 3]").expect_err("array");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("expected an object"));
        assert!(errors[0].message.contains("array"));
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let err = Preset::from_json_str("{not json").expect_err("parse error");
        assert!(matches!(err, PresetLoadError::Parse(_)));
    }

    #[test]
    fn valid_toml_preset_parses() {
        let text = r#"
            name = "toml-preset"
            tools = ["a", "b"]
            max_steps = 3
        "#;
        let preset = Preset::from_toml_str(text).expect("valid toml");
        assert_eq!(preset.name, "toml-preset");
        assert_eq!(preset.tools, vec!["a", "b"]);
        assert_eq!(preset.max_steps, Some(3));
    }

    #[test]
    fn invalid_toml_field_is_named() {
        let text = r#"
            name = "x"
            tools = ["a"]
            bogus = 1
        "#;
        let errors = Preset::from_toml_str(text).expect_err("unknown key");
        let PresetLoadError::Invalid(errors) = errors else {
            panic!("expected validation errors");
        };
        assert_eq!(errors[0].path, "bogus");
    }

    #[test]
    fn load_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("bos-preset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let json_path = dir.join("preset.json");
        let toml_path = dir.join("preset.toml");
        std::fs::write(&json_path, r#"{"name": "j", "tools": ["t"]}"#).unwrap();
        std::fs::write(&toml_path, "name = \"t\"\ntools = [\"t\"]\n").unwrap();

        assert_eq!(Preset::load(&json_path).unwrap().name, "j");
        assert_eq!(Preset::load(&toml_path).unwrap().name, "t");

        std::fs::remove_file(&json_path).ok();
        std::fs::remove_file(&toml_path).ok();
    }
}
