use crate::tool::{Tool, ToolError};
use serde_json::Value;

pub struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }
    fn description(&self) -> String {
        "Perform a lightweight search over a fixed dataset".to_string()
    }

    fn category(&self) -> String {
        "search".to_string()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let q = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if q.is_empty() {
            return Err(ToolError::Failed("empty query".to_string()));
        }
        // Simple mock dataset response
        let mut results = Vec::new();
        results.push(Value::String(format!("result: {} #1", q)));
        results.push(Value::String(format!("result: {} #2", q)));
        Ok(Value::Array(results))
    }
}
