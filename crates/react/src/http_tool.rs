use crate::tool::{Tool, ToolError};
use serde_json::Value;

pub struct HttpTool;

impl Tool for HttpTool {
    fn name(&self) -> &str {
        "http_get"
    }
    fn description(&self) -> &str {
        "Mock HTTP GET request"
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
        // Mock payload
        Ok(Value::String(format!("GET {}", url)))
    }
}
