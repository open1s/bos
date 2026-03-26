use crate::error::ToolError;

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

    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
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
        "number" | "integer" => {
            value.is_number() || (value.is_string() && can_parse_as_number(value))
        }
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

    if expected_type == "object" {
        validate_object(schema, value)?;
    }

    if expected_type == "array" {
        if let Some(items_schema) = schema.get("items") {
            if let Some(arr) = value.as_array() {
                for (i, item) in arr.iter().enumerate() {
                    validate_field(&format!("{}[{}]", name, i), items_schema, item)?;
                }
            }
        }
    }

    Ok(())
}

fn can_parse_as_number(value: &serde_json::Value) -> bool {
    if let Some(s) = value.as_str() {
        s.parse::<f64>().is_ok()
    } else {
        false
    }
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
