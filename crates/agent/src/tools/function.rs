use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use react::tool::registry::AsyncTool;
use react::tool::{Tool, ToolError};

/// Boxed future returned by an [`AsyncFunctionTool`] closure.
type BoxedToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send + 'a>>;

/// A wrapper that converts a synchronous function into a [`Tool`].
///
/// The function must accept `&serde_json::Value` as input and return `Result<serde_json::Value, ToolError>`.
#[allow(clippy::type_complexity)]
pub struct FunctionTool {
    name: String,
    description: String,
    schema: serde_json::Value,
    func: Arc<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync>,
    skill: bool,
    category: String,
}

impl FunctionTool {
    /// Create a new `FunctionTool` from a synchronous function.
    ///
    /// The function receives arguments as JSON and must return a
    /// JSON-serializable result without awaiting. Use
    /// [`AsyncFunctionTool`] when the body needs to await I/O.
    pub fn new<F>(name: &str, description: &str, schema: serde_json::Value, func: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            func: Arc::new(func),
            skill: false,
            category: "general".to_string(),
        }
    }

    /// Create a tool marked as a skill, with category `skill`.
    pub fn skill<F>(name: &str, description: &str, schema: serde_json::Value, func: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            func: Arc::new(func),
            skill: true,
            category: "skill".to_string(),
        }
    }

    /// Override the tool category.
    pub fn with_category(mut self, category: &str) -> Self {
        self.category = category.to_string();
        self
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

impl Tool for FunctionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    fn run(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (self.func)(args)
    }

    fn is_skill(&self) -> bool {
        self.skill
    }

    fn category(&self) -> String {
        self.category.clone()
    }
}

/// A wrapper that converts an async closure into an [`AsyncTool`].
///
/// Unlike [`FunctionTool`], the closure may await I/O. It receives
/// arguments as JSON and returns a boxed future resolving to a
/// JSON-serializable result, so the future may borrow the arguments.
///
/// ```
/// use agent::tools::{AsyncFunctionTool, AsyncTool};
/// use react::tool::ToolError;
/// use serde_json::json;
///
/// let tool = AsyncFunctionTool::new(
///     "double",
///     "Double a number",
///     json!({"type": "object", "properties": {"n": {"type": "number"}}}),
///     |args| {
///         Box::pin(async move {
///             let n = args["n"].as_f64().unwrap_or_default();
///             Ok(json!(n * 2.0))
///         })
///     },
/// );
/// assert_eq!(tool.name(), "double");
/// ```
pub struct AsyncFunctionTool {
    name: String,
    description: String,
    schema: serde_json::Value,
    #[allow(clippy::type_complexity)]
    func: Arc<dyn for<'a> Fn(&'a serde_json::Value) -> BoxedToolFuture<'a> + Send + Sync>,
    skill: bool,
    category: String,
}

impl AsyncFunctionTool {
    /// Create a tool from an async closure.
    pub fn new<F>(name: &str, description: &str, schema: serde_json::Value, func: F) -> Self
    where
        F: for<'a> Fn(&'a serde_json::Value) -> BoxedToolFuture<'a> + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            func: Arc::new(func),
            skill: false,
            category: "general".to_string(),
        }
    }

    /// Create a tool from an async closure that takes owned arguments.
    ///
    /// [`new`](Self::new) accepts a closure that *borrows* its arguments
    /// and returns a future tied to that borrow, which forces the caller to
    /// satisfy a `for<'a>` bound. When the closure moves its arguments into
    /// the future instead, `from_fn` states that directly:
    ///
    /// ```
    /// use agent::tools::{AsyncFunctionTool, AsyncTool};
    /// use serde_json::json;
    ///
    /// let tool = AsyncFunctionTool::from_fn(
    ///     "double",
    ///     "Double a number",
    ///     json!({"type": "object", "properties": {"n": {"type": "number"}}}),
    ///     |args| async move {
    ///         Ok(json!(args["n"].as_f64().unwrap_or_default() * 2.0))
    ///     },
    /// );
    /// assert_eq!(tool.name(), "double");
    /// ```
    pub fn from_fn<F, Fut>(
        name: &str,
        description: &str,
        schema: serde_json::Value,
        func: F,
    ) -> Self
    where
        F: Fn(serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, ToolError>> + Send + 'static,
    {
        Self::new(
            name,
            description,
            schema,
            move |args: &serde_json::Value| -> BoxedToolFuture<'_> { Box::pin(func(args.clone())) },
        )
    }

    /// Create a tool marked as a skill, with category `skill`.
    pub fn skill<F>(name: &str, description: &str, schema: serde_json::Value, func: F) -> Self
    where
        F: for<'a> Fn(&'a serde_json::Value) -> BoxedToolFuture<'a> + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            func: Arc::new(func),
            skill: true,
            category: "skill".to_string(),
        }
    }

    /// Override the tool category.
    pub fn with_category(mut self, category: &str) -> Self {
        self.category = category.to_string();
        self
    }

    /// Run the tool with JSON input.
    pub async fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (self.func)(input).await
    }
}

#[async_trait]
impl AsyncTool for AsyncFunctionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn run(&self, input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        (self.func)(input).await
    }

    fn is_skill(&self) -> bool {
        self.skill
    }

    fn category(&self) -> String {
        self.category.clone()
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
        assert_eq!(tool.description(), "Echo the input");
    }

    #[test]
    fn test_function_tool_numeric() {
        let tool =
            FunctionTool::numeric("add", "Add two numbers", 2, |args: &serde_json::Value| {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::Failed("a required".to_string()))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::Failed("b required".to_string()))?;
                Ok(serde_json::json!(a + b))
            });

        assert_eq!(tool.name(), "add");
        let schema = tool.json_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["a"]["type"] == "number");
        assert_eq!(schema["required"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn test_function_tool_execute() {
        let tool = FunctionTool::numeric(
            "multiply",
            "Multiply two numbers",
            2,
            |args: &serde_json::Value| {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::Failed("a required".to_string()))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::Failed("b required".to_string()))?;
                Ok(serde_json::json!(a * b))
            },
        );

        let args = serde_json::json!({"a": 3.0, "b": 4.0});
        let result = tool.run(&args).unwrap();
        assert_eq!(result, serde_json::json!(12.0));
    }
}
