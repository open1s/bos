use crate::tool::{Tool, ToolError};
use serde_json::Value;

pub struct TextTool;

impl Tool for TextTool {
    fn name(&self) -> &str {
        "text_transform"
    }
    fn description(&self) -> &str {
        "Transform text (uppercase/lowercase)"
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let op = input.get("op").and_then(|v| v.as_str()).unwrap_or("upper");
        let out = match op {
            "upper" => text.to_uppercase(),
            "lower" => text.to_lowercase(),
            _ => text.to_string(),
        };
        Ok(Value::String(out))
    }
}
