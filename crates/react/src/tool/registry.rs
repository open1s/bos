use super::descriptor::ToolDefinition;
use super::error::ToolError;
use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;

/// A synchronous tool: a named function the model can call.
pub trait Tool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;
    /// Human-readable description.
    fn description(&self) -> String;
    /// Category used for grouping.
    fn category(&self) -> String {
        "builtin".to_string()
    }
    /// Run the tool with JSON input.
    fn run(&self, input: &Value) -> Result<Value, ToolError>;
    /// JSON schema for the tool arguments.
    fn json_schema(&self) -> Value {
        serde_json::json!({})
    }
    /// Render this tool as an OpenAI tool definition.
    fn to_openai_definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
    }
    /// Whether this tool is a skill.
    fn is_skill(&self) -> bool {
        false
    }
    /// Whether a running call can be cancelled.
    fn is_cancelable(&self) -> bool {
        false
    }
    /// Cancel the call with the given id, if running.
    fn cancel(&self, _call_id: &str) {}
}

#[async_trait]
/// An asynchronous tool: a named async function the model can call.
pub trait AsyncTool: Send + Sync {
    /// Unique tool name.
    fn name(&self) -> &str;
    /// Human-readable description.
    fn description(&self) -> String;
    /// Category used for grouping.
    fn category(&self) -> String {
        "async".to_string()
    }
    /// Run the tool with JSON input.
    async fn run(&self, input: &Value) -> Result<Value, ToolError>;
    /// JSON schema for the tool arguments.
    fn json_schema(&self) -> Value {
        serde_json::json!({})
    }
    /// Render this tool as an OpenAI tool definition.
    fn to_openai_definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
    }
    /// Whether this tool is a skill.
    fn is_skill(&self) -> bool {
        false
    }
    /// Whether the tool can stream partial output.
    fn supports_streaming(&self) -> bool {
        false
    }
    /// Whether a running call can be cancelled.
    fn is_cancelable(&self) -> bool {
        false
    }
    /// Cancel the call with the given id, if running.
    fn cancel(&self, _call_id: &str) {}
    /// Run the tool and stream partial output.
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

/// A tool of either the sync or async flavor.
pub enum ToolVariant {
    /// A synchronous tool.
    Sync(Box<dyn Tool>),
    /// An asynchronous tool.
    Async(Box<dyn AsyncTool>),
}

impl ToolVariant {
    /// Run the wrapped tool with JSON input.
    pub async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        match self {
            Self::Sync(tool) => tool.run(input),
            Self::Async(tool) => tool.run(input).await,
        }
    }

    /// Tool name.
    pub fn name(&self) -> &str {
        match self {
            Self::Sync(tool) => tool.name(),
            Self::Async(tool) => tool.name(),
        }
    }

    /// Tool description.
    pub fn description(&self) -> String {
        match self {
            Self::Sync(tool) => tool.description(),
            Self::Async(tool) => tool.description(),
        }
    }

    /// Render the wrapped tool as an OpenAI tool definition.
    pub fn to_openai_definition(&self) -> ToolDefinition {
        match self {
            Self::Sync(tool) => tool.to_openai_definition(),
            Self::Async(tool) => tool.to_openai_definition(),
        }
    }

    /// Cancel the call with `call_id`.
    pub fn cancel(&self, call_id: &str) {
        match self {
            Self::Sync(tool) => tool.cancel(call_id),
            Self::Async(tool) => tool.cancel(call_id),
        }
    }

    /// Whether the wrapped tool can be cancelled.
    pub fn is_cancelable(&self) -> bool {
        match self {
            Self::Sync(tool) => tool.is_cancelable(),
            Self::Async(tool) => tool.is_cancelable(),
        }
    }
}

/// A synchronous tool backed by a closure.
pub struct FnTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// The closure that implements the tool.
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
    /// Category used for grouping.
    fn category(&self) -> String {
        "builtin".to_string()
    }
}

/// A thread-safe registry of tools by name.
pub struct ToolRegistry {
    tools: DashMap<String, ToolVariant>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ToolRegistry {{ tools: {} }}", self.tools.len())
    }
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
        }
    }

    /// Register a tool, replacing any tool with the same name.
    pub fn register(&self, t: ToolVariant) {
        self.tools.insert(t.name().to_string(), t);
    }

    /// Alias for [`ToolRegistry::register`].
    pub fn insert(&self, t: ToolVariant) {
        self.tools.insert(t.name().to_string(), t);
    }

    /// Wrap and register a synchronous tool.
    pub fn register_sync(&self, t: Box<dyn Tool>) {
        self.register(ToolVariant::Sync(t));
    }

    /// Wrap and register an asynchronous tool.
    pub fn register_async(&self, t: Box<dyn AsyncTool>) {
        self.register(ToolVariant::Async(t));
    }

    /// Every registered tool as an OpenAI definition.
    pub fn to_openai_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|entry| entry.value().to_openai_definition())
            .collect()
    }

    /// Call a tool by name with JSON input.
    pub async fn call(&self, name: &str, input: &Value) -> Result<Value, ToolError> {
        if let Some(tool) = self.tools.get(name) {
            tool.run(input).await
        } else {
            Err(ToolError::NotFound(name.to_string()))
        }
    }

    /// Borrow a tool by name.
    pub fn get(&self, name: &str) -> Option<dashmap::mapref::one::Ref<'_, String, ToolVariant>> {
        self.tools.get(name)
    }

    /// Names of every registered tool.
    pub fn values(&self) -> Vec<String> {
        self.tools.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Iterate over the registered tools.
    pub fn iter(&self) -> dashmap::iter::Iter<'_, String, ToolVariant> {
        self.tools.iter()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
