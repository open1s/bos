use std::collections::HashMap;
use serde_json::Value;
use super::descriptor::ToolDefinition;
use super::error::ToolError;

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn category(&self) -> String {
        "builtin".to_string()
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError>;
    fn json_schema(&self) -> Value {
        serde_json::json!({})
    }
    fn to_openai_definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
    }
    fn is_skill(&self) -> bool {
        false
    }
}

pub struct FnTool {
    pub name: String,
    pub description: String,
    pub f: Box<dyn Fn(&Value) -> Value + Send + Sync>,
}

impl Tool for FnTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        self.description.clone()
    }
    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        Ok((self.f)(input))
    }
    fn category(&self) -> String {
        "builtin".to_string()
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
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
    pub fn insert(&mut self, t: Box<dyn Tool>) {
        self.tools.insert(t.name().to_string(), t);
    }
    pub fn to_openai_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.to_openai_definition())
            .collect()
    }
    pub fn call(&self, name: &str, input: &Value) -> Result<Value, ToolError> {
        if let Some(t) = self.tools.get(name) {
            t.run(input)
        } else {
            Err(ToolError::NotFound(name.to_string()))
        }
    }
    pub fn get(&self, name: &str) -> Option<&Box<dyn Tool>> {
        self.tools.get(name)
    }
    pub fn values(&self) -> impl Iterator<Item = &Box<dyn Tool>> {
        self.tools.values()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Box<dyn Tool>)> {
        self.tools.iter()
    }
    pub fn tools(&self) -> &HashMap<String, Box<dyn Tool>> {
        &self.tools
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}