use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct ToolInput {
    pub params: Value,
}

#[derive(Debug)]
pub struct ToolOutput {
    pub result: Value,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Tool execution failed: {0}")]
    Failed(String),
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn run(&self, input: &Value) -> Result<Value, ToolError>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }
    pub fn register(&mut self, t: Box<dyn Tool>) {
        self.tools.insert(t.name().to_string(), t);
    }
    pub fn call(&self, name: &str, input: &Value) -> Result<Value, ToolError> {
        if let Some(t) = self.tools.get(name) {
            t.run(input)
        } else {
            Err(ToolError::NotFound(name.to_string()))
        }
    }
}
