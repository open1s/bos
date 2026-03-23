pub mod roles;

use std::sync::Arc;
use async_trait::async_trait;
use agent::{Tool, ToolDescription, ToolError};
use agent::tools::FunctionTool;
use serde_json::{json, Value};

pub struct AddTool;

impl AddTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Add two numbers".to_string(),
            parameters: "a: number, b: number".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number", "description": "First number"},
                "b": {"type": "number", "description": "Second number"}
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
        let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
        Ok(json!(a + b))
    }
}

pub struct MultiplyTool;

impl MultiplyTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MultiplyTool {
    fn name(&self) -> &str {
        "multiply"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Multiply two numbers".to_string(),
            parameters: "a: number, b: number".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number", "description": "First number"},
                "b": {"type": "number", "description": "Second number"}
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
        let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
        Ok(json!(a * b))
    }
}

pub fn create_add_function_tool() -> Arc<dyn Tool> {
    Arc::new(FunctionTool::numeric(
        "add_fn",
        "Add two numbers using FunctionTool helper",
        2,
        |args: &Value| {
            let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
            let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
            Ok(json!(a + b))
        },
    ))
}

pub fn create_echo_function_tool() -> Arc<dyn Tool> {
    let schema = json!({
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "Message to echo back"
            },
            "repeat": {
                "type": "integer",
                "description": "Number of times to repeat",
                "default": 1
            }
        },
        "required": ["message"]
    });

    Arc::new(FunctionTool::new(
        "echo",
        "Echo a message back",
        schema,
        |args: &Value| {
            let message = args["message"].as_str().ok_or_else(|| ToolError::ExecutionFailed("message required".to_string()))?;
            let repeat = args["repeat"].as_u64().unwrap_or(1) as usize;
            let repeated = message.repeat(repeat);
            Ok(json!(repeated))
        },
    ))
}

pub fn get_local_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(AddTool::new()),
        Arc::new(MultiplyTool::new()),
        create_add_function_tool(),
        create_echo_function_tool(),
    ]
}