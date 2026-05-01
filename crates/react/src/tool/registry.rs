use super::descriptor::ToolDefinition;
use super::error::ToolError;
use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;

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

#[async_trait]
pub trait AsyncTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn category(&self) -> String {
        "async".to_string()
    }
    async fn run(&self, input: &Value) -> Result<Value, ToolError>;
    fn json_schema(&self) -> Value {
        serde_json::json!({})
    }
    fn to_openai_definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
    }
    fn is_skill(&self) -> bool {
        false
    }
    fn supports_streaming(&self) -> bool {
        false
    }
    async fn run_streaming(
        &self,
        input: &Value,
    ) -> Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<String, ToolError>> + Send>>,
        ToolError,
    > {
        let _ = input;
        Err(ToolError::Failed("Streaming not supported".to_string()))
    }
}

pub enum ToolVariant {
    Sync(Box<dyn Tool>),
    Async(Box<dyn AsyncTool>),
}

impl ToolVariant {
    pub async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        match self {
            Self::Sync(tool) => tool.run(input),
            Self::Async(tool) => tool.run(input).await,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Sync(tool) => tool.name(),
            Self::Async(tool) => tool.name(),
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Sync(tool) => tool.description(),
            Self::Async(tool) => tool.description(),
        }
    }

    pub fn to_openai_definition(&self) -> ToolDefinition {
        match self {
            Self::Sync(tool) => tool.to_openai_definition(),
            Self::Async(tool) => tool.to_openai_definition(),
        }
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
    tools: DashMap<String, ToolVariant>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ToolRegistry {{ tools: {} }}", self.tools.len())
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
        }
    }

    pub fn register(&self, t: ToolVariant) {
        self.tools.insert(t.name().to_string(), t);
    }

    pub fn insert(&self, t: ToolVariant) {
        self.tools.insert(t.name().to_string(), t);
    }

    pub fn register_sync(&self, t: Box<dyn Tool>) {
        self.register(ToolVariant::Sync(t));
    }

    pub fn register_async(&self, t: Box<dyn AsyncTool>) {
        self.register(ToolVariant::Async(t));
    }

    pub fn to_openai_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|entry| entry.value().to_openai_definition())
            .collect()
    }

    pub async fn call(&self, name: &str, input: &Value) -> Result<Value, ToolError> {
        if let Some(tool) = self.tools.get(name) {
            tool.run(input).await
        } else {
            Err(ToolError::NotFound(name.to_string()))
        }
    }

    pub fn get(&self, name: &str) -> Option<dashmap::mapref::one::Ref<'_, String, ToolVariant>> {
        self.tools.get(name)
    }

    pub fn values(&self) -> Vec<String> {
        self.tools.iter().map(|entry| entry.key().clone()).collect()
    }

    pub fn iter(&self) -> dashmap::iter::Iter<'_, String, ToolVariant> {
        self.tools.iter()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
