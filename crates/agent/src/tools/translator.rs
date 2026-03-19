pub fn describe_schema(schema: &serde_json::Value) -> String {
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("object") => describe_object(schema),
        Some("array") => describe_array(schema),
        Some("string") => describe_string(schema),
        Some("number") | Some("integer") => describe_number(schema),
        Some("boolean") => "boolean".to_string(),
        Some("null") => "null".to_string(),
        _ => schema.to_string(),
    }
}

fn describe_object(schema: &serde_json::Value) -> String {
    let mut parts = Vec::new();

    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (name, prop) in props {
            let type_str = describe_schema(prop);
            let required = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
                .unwrap_or(false);

            let marker = if required { "" } else { "?" };
            parts.push(format!("{}{}: {}", name, marker, type_str));
        }
    }

    format!("object({})", parts.join(", "))
}

fn describe_array(schema: &serde_json::Value) -> String {
    let items = schema
        .get("items")
        .map(|i| describe_schema(i))
        .unwrap_or_else(|| "any".to_string());
    format!("array[{}]", items)
}

fn describe_string(schema: &serde_json::Value) -> String {
    if let Some(enum_vals) = schema.get("enum").and_then(|e| e.as_array()) {
        let vals: Vec<String> = enum_vals
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        format!("string(one of: {})", vals.join(", "))
    } else if let Some(desc) = schema.get("description").and_then(|d| d.as_str()) {
        format!("string - {}", desc)
    } else {
        "string".to_string()
    }
}

fn describe_number(schema: &serde_json::Value) -> String {
    let min = schema.get("minimum").and_then(|v| v.as_f64());
    let max = schema.get("maximum").and_then(|v| v.as_f64());

    match (min, max) {
        (Some(lo), Some(hi)) => format!("number({}..={})", lo, hi),
        (Some(lo), None) => format!("number({}..)", lo),
        (None, Some(hi)) => format!("number(..={})", hi),
        (None, None) => "number".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_describe_simple_string() {
        let schema = json!({"type": "string"});
        assert_eq!(describe_schema(&schema), "string");
    }

    #[test]
    fn test_describe_string_with_enum() {
        let schema = json!({"type": "string", "enum": ["a", "b", "c"]});
        assert_eq!(describe_schema(&schema), "string(one of: a, b, c)");
    }

    #[test]
    fn test_describe_number_with_range() {
        let schema = json!({"type": "number", "minimum": 0, "maximum": 100});
        assert_eq!(describe_schema(&schema), "number(0..=100)");
    }

    #[test]
    fn test_describe_array() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        assert_eq!(describe_schema(&schema), "array[string]");
    }

    #[test]
    fn test_describe_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "number"}
            },
            "required": ["name"]
        });
        let desc = describe_schema(&schema);
        assert!(desc.contains("name: string"));
        assert!(desc.contains("count?: number"));
    }
}
