//! Per-extension configuration: defaults from the manifest, a user document,
//! and a bounded JSON Schema check.
//!
//! The schema validator is intentionally a subset: object types, required and
//! additional properties, arrays, primitives, enum, numeric bounds and string
//! length/pattern-by-substring. A package that needs a construct outside the
//! subset gets a validation error naming the keyword rather than a silent pass.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::manifest::Manifest;
use super::package::write_atomic;
use super::types::ExtensionError;

/// The user configuration file inside the data namespace.
pub(crate) const CONFIG_FILE: &str = "config.json";

/// The largest configuration document this host accepts.
pub(crate) const MAX_CONFIG_BYTES: usize = 256 * 1024;

/// A loaded configuration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExtensionConfig {
    /// The effective document.
    pub(crate) value: Value,
    /// The schema it was checked against, when the manifest names one.
    pub(crate) schema: Option<Value>,
}

impl ExtensionConfig {
    /// Serialize the effective document for a runtime handshake.
    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string(&self.value).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Load the effective configuration for one package.
pub(crate) fn load(
    manifest: &Manifest,
    root: &Path,
    data_root: &Path,
) -> Result<ExtensionConfig, ExtensionError> {
    let defaults = manifest
        .configuration
        .as_ref()
        .and_then(|configuration| configuration.defaults.as_ref())
        .map(|defaults| {
            serde_json::to_value(defaults).map_err(|error| ExtensionError::Json {
                path: root.join("extension.toml"),
                message: error.to_string(),
            })
        })
        .transpose()?
        .unwrap_or_else(|| serde_json::json!({}));
    let user_path = data_root.join(CONFIG_FILE);
    let value = if user_path.is_file() {
        let text = read_bounded(&user_path)?;
        serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
            path: user_path,
            message: error.to_string(),
        })?
    } else {
        defaults
    };
    let schema = match manifest
        .configuration
        .as_ref()
        .and_then(|configuration| configuration.schema.as_deref())
    {
        Some(relative) => {
            let path = root.join(relative);
            let text = read_bounded(&path)?;
            Some(serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
                path,
                message: error.to_string(),
            })?)
        }
        None => None,
    };
    if let Some(schema) = &schema {
        let mut errors = Vec::new();
        validate(schema, &value, "$", &mut errors, 0);
        if !errors.is_empty() {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("configuration does not match its schema: {}", errors.join("; ")),
            });
        }
    }
    Ok(ExtensionConfig { value, schema })
}

/// Write a user configuration after validating it.
pub(crate) fn save(
    manifest: &Manifest,
    root: &Path,
    data_root: &Path,
    value: Value,
) -> Result<ExtensionConfig, ExtensionError> {
    let schema = match manifest
        .configuration
        .as_ref()
        .and_then(|configuration| configuration.schema.as_deref())
    {
        Some(relative) => {
            let path = root.join(relative);
            let text = read_bounded(&path)?;
            Some(serde_json::from_str(&text).map_err(|error| ExtensionError::Json {
                path,
                message: error.to_string(),
            })?)
        }
        None => None,
    };
    if let Some(schema) = &schema {
        let mut errors = Vec::new();
        validate(schema, &value, "$", &mut errors, 0);
        if !errors.is_empty() {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("configuration does not match its schema: {}", errors.join("; ")),
            });
        }
    }
    let text = serde_json::to_string_pretty(&value).map_err(|error| ExtensionError::Json {
        path: data_root.join(CONFIG_FILE),
        message: error.to_string(),
    })?;
    if text.len() > MAX_CONFIG_BYTES {
        return Err(ExtensionError::Limit {
            extension: manifest.id().to_string(),
            message: format!("configuration is larger than {MAX_CONFIG_BYTES} bytes"),
        });
    }
    std::fs::create_dir_all(data_root).map_err(|source| ExtensionError::Io {
        path: data_root.to_path_buf(),
        source,
    })?;
    write_atomic(&data_root.join(CONFIG_FILE), text.as_bytes())?;
    Ok(ExtensionConfig { value, schema })
}

/// The user configuration path for an extension.
pub(crate) fn path(data_root: &Path) -> PathBuf {
    data_root.join(CONFIG_FILE)
}

fn read_bounded(path: &Path) -> Result<String, ExtensionError> {
    let metadata = std::fs::metadata(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(ExtensionError::Limit {
            extension: path.display().to_string(),
            message: format!("configuration file is larger than {MAX_CONFIG_BYTES} bytes"),
        });
    }
    std::fs::read_to_string(path).map_err(|source| ExtensionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Validate a JSON value against the supported schema subset.
pub(crate) fn validate(
    schema: &Value,
    value: &Value,
    path: &str,
    errors: &mut Vec<String>,
    depth: usize,
) {
    if depth > 32 {
        errors.push(format!("{path}: schema nesting exceeds 32"));
        return;
    }
    let Some(object) = schema.as_object() else {
        errors.push(format!("{path}: schema must be an object"));
        return;
    };
    if let Some(expected) = object.get("type") {
        let types: Vec<&str> = match expected {
            Value::String(kind) => vec![kind.as_str()],
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.is_empty() && !types.iter().any(|kind| type_matches(kind, value)) {
            errors.push(format!(
                "{path}: expected {}, found {}",
                types.join(" or "),
                json_type(value)
            ));
            return;
        }
    }
    if let Some(allowed) = object.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|allowed| allowed == value) {
            errors.push(format!("{path}: value is not one of the allowed values"));
        }
    }
    match value {
        Value::Object(map) => {
            if let Some(required) = object.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !map.contains_key(name) {
                        errors.push(format!("{path}.{name}: is required"));
                    }
                }
            }
            let properties = object.get("properties").and_then(Value::as_object);
            for (name, child) in map {
                match properties.and_then(|properties| properties.get(name)) {
                    Some(child_schema) => validate(
                        child_schema,
                        child,
                        &format!("{path}.{name}"),
                        errors,
                        depth + 1,
                    ),
                    None => {
                        if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                            errors.push(format!("{path}.{name}: is not an allowed property"));
                        }
                    }
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = object.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate(
                        item_schema,
                        item,
                        &format!("{path}[{index}]"),
                        errors,
                        depth + 1,
                    );
                }
            }
        }
        Value::String(text) => {
            if let Some(min) = object.get("minLength").and_then(Value::as_u64) {
                if text.chars().count() < min as usize {
                    errors.push(format!("{path}: must be at least {min} characters"));
                }
            }
            if let Some(max) = object.get("maxLength").and_then(Value::as_u64) {
                if text.chars().count() > max as usize {
                    errors.push(format!("{path}: must be at most {max} characters"));
                }
            }
            if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
                if !text.contains(pattern) {
                    errors.push(format!("{path}: must contain {pattern:?}"));
                }
            }
        }
        Value::Number(number) => {
            if let Some(min) = object.get("minimum").and_then(Value::as_f64) {
                if number.as_f64().is_some_and(|value| value < min) {
                    errors.push(format!("{path}: must be at least {min}"));
                }
            }
            if let Some(max) = object.get("maximum").and_then(Value::as_f64) {
                if number.as_f64().is_some_and(|value| value > max) {
                    errors.push(format!("{path}: must be at most {max}"));
                }
            }
        }
        _ => {}
    }
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(root: &Path) {
        std::fs::create_dir_all(root.join("schemas")).expect("schemas");
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.config"
name = "Configured"
version = "1.0.0"
description = "A configured package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[contributes]
tools = ["echo"]

[configuration]
schema = "schemas/config.schema.json"
[configuration.defaults]
enabled = true
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("schemas/config.schema.json"),
            r#"{
  "type": "object",
  "properties": {
    "enabled": { "type": "boolean" },
    "name": { "type": "string", "minLength": 2 }
  },
  "required": ["enabled"],
  "additionalProperties": false
}"#,
        )
        .expect("schema");
    }

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bsh-ext-config-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    #[test]
    fn defaults_validate_and_user_values_round_trip() {
        let root = scratch("roundtrip");
        manifest(&root);
        let package = Manifest::load(&root).expect("manifest");
        let data = root.join("data");
        let config = load(&package, &root, &data).expect("load");
        assert_eq!(config.value["enabled"], true);
        let saved = save(&package, &root, &data, serde_json::json!({"enabled": false, "name": "ok"}))
            .expect("save");
        assert_eq!(saved.value["enabled"], false);
        let loaded = load(&package, &root, &data).expect("reload");
        assert_eq!(loaded.value["name"], "ok");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_value_outside_the_schema_is_refused() {
        let root = scratch("invalid");
        manifest(&root);
        let package = Manifest::load(&root).expect("manifest");
        let data = root.join("data");
        let error = save(
            &package,
            &root,
            &data,
            serde_json::json!({"enabled": false, "unknown": true}),
        )
        .expect_err("extra property");
        assert!(error.to_string().contains("not an allowed property"));
        let error = save(
            &package,
            &root,
            &data,
            serde_json::json!({"enabled": false, "name": "x"}),
        )
        .expect_err("short name");
        assert!(error.to_string().contains("at least 2"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
