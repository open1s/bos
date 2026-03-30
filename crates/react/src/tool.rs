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

// Lightweight wrapper to allow function closures as tools
pub struct FnTool {
    pub name: String,
    pub f: Box<dyn Fn(&Value) -> Value + Send + Sync>,
}

impl Tool for FnTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        "closure tool".to_string()
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        Ok((self.f)(input))
    }
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
    fn description(&self) -> String;
    fn run(&self, input: &Value) -> Result<Value, ToolError>;
}

pub struct ToolRegistry {
    pub tools: HashMap<String, Box<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ToolRegistry {{ tools: {} }}", self.tools.len())
    }
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
    // Allow direct insertion from tests or external callers
    pub fn insert(&mut self, t: Box<dyn Tool>) {
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

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
