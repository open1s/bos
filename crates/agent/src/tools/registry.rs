use std::collections::HashMap;
use std::sync::Arc;

use super::{Tool, ToolError};

#[cfg(test)]
use super::ToolDescription;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(ToolError::ExecutionFailed(format!(
                "duplicate tool: {}",
                name
            )));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        let schema = tool.json_schema();
        super::validate_args(&schema, &args)?;
        tool.execute(args).await
    }

    pub fn to_openai_format(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                let desc = tool.description();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": desc.short,
                        "parameters": tool.json_schema()
                    }
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }

        fn description(&self) -> ToolDescription {
            ToolDescription {
                short: "A dummy tool".to_string(),
                parameters: "none".to_string(),
            }
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        }

        async fn execute(&self, _args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!("executed"))
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);
        registry.register(tool).unwrap();
        assert!(registry.get("dummy").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_duplicate_registration() {
        let mut registry = ToolRegistry::new();
        let tool1 = Arc::new(DummyTool);
        let tool2 = Arc::new(DummyTool);
        registry.register(tool1).unwrap();
        let result = registry.register(tool2);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool)).unwrap();
        let result = registry.execute("dummy", serde_json::json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!("executed"));
    }

    #[tokio::test]
    async fn test_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", serde_json::json!({})).await;
        assert!(matches!(result, Err(ToolError::NotFound(_))));
    }

    #[test]
    fn test_to_openai_format() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool)).unwrap();
        let format = registry.to_openai_format();
        assert_eq!(format.len(), 1);
        assert_eq!(format[0]["type"], "function");
        assert_eq!(format[0]["function"]["name"], "dummy");
    }
}
