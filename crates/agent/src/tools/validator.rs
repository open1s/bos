use crate::error::ToolError;

/// Validate `args` against `schema`, returning the first mismatch.
pub fn validate_args(
    schema: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<(), ToolError> {
    let schema_type = schema
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("object");

    if schema_type == "object" {
        validate_object(schema, args)?;
    }

    Ok(())
}

fn validate_object(schema: &serde_json::Value, args: &serde_json::Value) -> Result<(), ToolError> {
    let args_obj = args.as_object().ok_or_else(|| ToolError::SchemaMismatch {
        message: "args must be an object".to_string(),
    })?;

    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for req_field in required {
            let field_name = req_field
                .as_str()
                .ok_or_else(|| ToolError::SchemaMismatch {
                    message: "invalid required field name".to_string(),
                })?;

            if !args_obj.contains_key(field_name) {
                return Err(ToolError::SchemaMismatch {
                    message: format!("missing required field: {}", field_name),
                });
            }
        }
    }

    let properties = schema.get("properties").and_then(|p| p.as_object());
    if schema.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
        for field_name in args_obj.keys() {
            if !properties.is_some_and(|props| props.contains_key(field_name)) {
                return Err(ToolError::SchemaMismatch {
                    message: format!("unknown field: {field_name}"),
                });
            }
        }
    }

    if let Some(props) = properties {
        for (field_name, field_schema) in props {
            if let Some(value) = args_obj.get(field_name) {
                validate_field(field_name, field_schema, value)?;
            }
        }
    }

    Ok(())
}

fn validate_field(
    name: &str,
    schema: &serde_json::Value,
    value: &serde_json::Value,
) -> Result<(), ToolError> {
    let expected_type = schema.get("type").and_then(|t| t.as_str()).unwrap_or("any");

    let matches = match expected_type {
        "string" => value.is_string(),
        "number" => numeric_value(value).is_some(),
        "integer" => numeric_value(value).is_some_and(|number| number.fract() == 0.0),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        "any" => true,
        _ => true,
    };

    if !matches {
        return Err(ToolError::SchemaMismatch {
            message: format!(
                "field '{}' expected type {}, got {}",
                name,
                expected_type,
                value_type(value)
            ),
        });
    }

    if let Some(values) = schema.get("enum").and_then(|v| v.as_array()) {
        if !values.contains(value) {
            return Err(ToolError::SchemaMismatch {
                message: format!("field '{name}' must be one of the declared enum values"),
            });
        }
    }

    if let Some(text) = value.as_str() {
        let length = text.chars().count();
        if let Some(minimum) = schema.get("minLength").and_then(|v| v.as_u64()) {
            if length < minimum as usize {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must contain at least {minimum} characters"),
                });
            }
        }
        if let Some(maximum) = schema.get("maxLength").and_then(|v| v.as_u64()) {
            if length > maximum as usize {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must contain at most {maximum} characters"),
                });
            }
        }
    }

    if let Some(number) = numeric_value(value) {
        if let Some(minimum) = schema.get("minimum").and_then(|v| v.as_f64()) {
            if number < minimum {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must be at least {minimum}"),
                });
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(|v| v.as_f64()) {
            if number > maximum {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must be at most {maximum}"),
                });
            }
        }
    }

    if let Some(array) = value.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(|v| v.as_u64()) {
            if array.len() < minimum as usize {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must contain at least {minimum} items"),
                });
            }
        }
        if let Some(maximum) = schema.get("maxItems").and_then(|v| v.as_u64()) {
            if array.len() > maximum as usize {
                return Err(ToolError::SchemaMismatch {
                    message: format!("field '{name}' must contain at most {maximum} items"),
                });
            }
        }
        if let Some(items_schema) = schema.get("items") {
            for (i, item) in array.iter().enumerate() {
                validate_field(&format!("{name}[{i}]"), items_schema, item)?;
            }
        }
    }

    if expected_type == "object" {
        validate_object(schema, value)?;
    }

    Ok(())
}

fn numeric_value(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().filter(|number| number.is_finite())
}

fn value_type(value: &serde_json::Value) -> &'static str {
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
    use serde_json::json;

    #[test]
    fn test_valid_simple_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "number"}
            },
            "required": ["name"]
        });
        let args = json!({"name": "test", "count": 42});
        assert!(validate_args(&schema, &args).is_ok());
    }

    #[test]
    fn test_missing_required_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let args = json!({});
        let result = validate_args(&schema, &args);
        assert!(matches!(result, Err(ToolError::SchemaMismatch { .. })));
        if let Err(ToolError::SchemaMismatch { message }) = result {
            assert!(message.contains("missing required field: name"));
        }
    }

    #[test]
    fn test_wrong_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": {"type": "number"}
            },
            "required": ["count"]
        });
        let args = json!({"count": "not a number"});
        let result = validate_args(&schema, &args);
        assert!(matches!(result, Err(ToolError::SchemaMismatch { .. })));
        if let Err(ToolError::SchemaMismatch { message }) = result {
            assert!(message.contains("expected type number"));
        }
    }

    #[test]
    fn test_nested_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "properties": {
                        "enabled": {"type": "boolean"}
                    },
                    "required": ["enabled"]
                }
            },
            "required": ["config"]
        });
        let args = json!({"config": {"enabled": true}});
        assert!(validate_args(&schema, &args).is_ok());
    }

    #[test]
    fn test_array_validation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["items"]
        });
        let args = json!({"items": ["a", "b", "c"]});
        assert!(validate_args(&schema, &args).is_ok());
    }

    #[test]
    fn test_additional_properties_are_rejected_when_disabled() {
        let schema = json!({
            "type": "object",
            "properties": { "path": {"type": "string"} },
            "required": ["path"],
            "additionalProperties": false
        });
        assert!(validate_args(&schema, &json!({"path": "src/lib.rs"})).is_ok());
        assert!(validate_args(&schema, &json!({"path": "src/lib.rs", "extra": true})).is_err());
    }

    #[test]
    fn test_enum_and_string_length_constraints() {
        let schema = json!({
            "type": "object",
            "properties": {
                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                "name": {"type": "string", "minLength": 1, "maxLength": 4}
            },
            "required": ["status", "name"]
        });
        assert!(validate_args(&schema, &json!({"status": "pending", "name": "BOS"})).is_ok());
        assert!(validate_args(&schema, &json!({"status": "done", "name": "BOS"})).is_err());
        assert!(validate_args(&schema, &json!({"status": "pending", "name": "longer"})).is_err());
    }

    #[test]
    fn test_numeric_and_array_bounds() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "minimum": 1, "maximum": 5},
                "items": {"type": "array", "items": {"type": "string"}, "maxItems": 2}
            },
            "required": ["limit", "items"]
        });
        assert!(validate_args(&schema, &json!({"limit": 3, "items": ["a", "b"]})).is_ok());
        assert!(validate_args(&schema, &json!({"limit": 6, "items": ["a"]})).is_err());
        assert!(validate_args(&schema, &json!({"limit": 2.5, "items": ["a"]})).is_err());
        assert!(validate_args(&schema, &json!({"limit": 2, "items": ["a", "b", "c"]})).is_err());
        assert!(validate_args(&schema, &json!({"limit": "3", "items": ["a"]})).is_err());
    }

    #[test]
    fn test_nested_objects_apply_additional_property_rules() {
        let schema = json!({
            "type": "object",
            "properties": {
                "options": {
                    "type": "object",
                    "properties": {"enabled": {"type": "boolean"}},
                    "additionalProperties": false
                }
            },
            "required": ["options"]
        });
        assert!(validate_args(&schema, &json!({"options": {"enabled": true}})).is_ok());
        assert!(validate_args(&schema, &json!({"options": {"enabled": true, "typo": true}})).is_err());
    }

    #[test]
    fn test_array_wrong_item_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["items"]
        });
        let args = json!({"items": ["a", 123, "c"]});
        let result = validate_args(&schema, &args);
        assert!(matches!(result, Err(ToolError::SchemaMismatch { .. })));
    }
}
