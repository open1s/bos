use async_trait::async_trait;
use std::sync::Arc;

use super::{Tool, ToolDescription, ToolError};

/// A wrapper that converts an async function into a Tool implementation.
///
/// The function must accept `&serde_json::Value` as input and return `Result<serde_json::Value, ToolError>`.
#[allow(clippy::type_complexity)]
pub struct FunctionTool {
    name: String,
    description: ToolDescription,
    schema: serde_json::Value,
    func: Arc<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync>,
}

impl FunctionTool {
    /// Create a new FunctionTool from an async function.
    ///
    /// The function will receive arguments as JSON and must return a JSON-serializable result
    pub fn new<F>(name: &str, description: &str, schema: serde_json::Value, func: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            description: ToolDescription {
                short: description.to_string(),
                parameters: "JSON object with function parameters".to_string(),
            },
            schema,
            func: Arc::new(func),
        }
    }

    /// Create a FunctionTool with automatic schema generation for simple numeric functions.
    ///
    /// This helper creates a schema for functions expecting up to 5 numeric parameters (a, b, c, d, e).
    pub fn numeric<F>(name: &str, description: &str, num_params: usize, func: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        let params = ['a', 'b', 'c', 'd', 'e'];
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for param_name in params.iter().take(num_params) {
            properties.insert(
                param_name.to_string(),
                serde_json::json!({
                    "type": "number",
                    "description": format!("Parameter {}", param_name)
                }),
            );
            required.push(param_name.to_string());
        }

        let schema = serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        });

        Self::new(name, description, schema, func)
    }
}

#[async_trait]
impl Tool for FunctionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        self.description.clone()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (self.func)(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_tool_basic() {
        let tool = FunctionTool::new(
            "echo",
            "Echo the input",
            serde_json::json!({"type": "object", "properties": {"message": {"type": "string"}}}),
            |args: &serde_json::Value| Ok(args.clone()),
        );

        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description().short, "Echo the input");
    }

    #[test]
    fn test_function_tool_numeric() {
        let tool =
            FunctionTool::numeric("add", "Add two numbers", 2, |args: &serde_json::Value| {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
                Ok(serde_json::json!(a + b))
            });

        assert_eq!(tool.name(), "add");
        let schema = tool.json_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["a"]["type"] == "number");
        assert_eq!(schema["required"], serde_json::json!(["a", "b"]));
    }

    #[tokio::test]
    async fn test_function_tool_execute() {
        let tool = FunctionTool::numeric(
            "multiply",
            "Multiply two numbers",
            2,
            |args: &serde_json::Value| {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
                Ok(serde_json::json!(a * b))
            },
        );

        let args = serde_json::json!({"a": 3.0, "b": 4.0});
        let result = tool.execute(&args).await.unwrap();
        assert_eq!(result, serde_json::json!(12.0));
    }
}
