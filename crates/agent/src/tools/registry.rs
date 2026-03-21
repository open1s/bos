use std::collections::HashMap;
use std::sync::{Arc, Weak};

use super::{Tool, ToolError};
use crate::tools::{ToolDescription, async_trait};
use dashmap::DashMap;
use serde_json::json;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    tool_name_index: HashMap<String, Vec<Weak<dyn Tool>>>,
    schema_cache: DashMap<String, Arc<serde_json::Value>>,  // Arc 共享，零拷贝
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            tool_name_index: HashMap::new(),
            schema_cache: DashMap::new(),
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

        let WeakTool = Arc::downgrade(&tool);
        self.tool_name_index.entry(name.clone()).or_default().push(WeakTool);

        let schema = tool.cached_schema();
        self.schema_cache.insert(name.clone(), schema);

        self.tools.insert(name, tool);
        Ok(())
    }

    /// Register a tool with a namespace prefix to avoid conflicts.
    /// Tool name will be `{namespace}/{tool_name}`.
    pub fn register_with_namespace(
        &mut self,
        tool: Arc<dyn Tool>,
        namespace: &str,
    ) -> Result<(), ToolError> {
        let tool_name = tool.name().to_string();
        let namespaced_name = format!("{}/{}", namespace, tool_name);
        
        if self.tools.contains_key(&namespaced_name) {
            return Err(ToolError::ExecutionFailed(format!(
                "duplicate tool in namespace '{}': {}",
                namespace,
                tool_name
            )));
        }

        // Create a wrapper tool with the namespaced name
        let wrapped = Arc::new(NamespacedTool::new(tool, namespace.to_string(), tool_name.to_string()));
        let WeakWrapped = Arc::downgrade(&wrapped);
        self.tool_name_index.entry(tool_name.clone()).or_default().push(WeakWrapped);
        
        let schema = wrapped.cached_schema();
        self.schema_cache.insert(namespaced_name.clone(), schema);
        
        self.tools.insert(namespaced_name, wrapped);
        Ok(())
    }

    /// Register multiple tools from a skill with automatic namespacing.
    /// All tools will be prefixed with `{skill_name}/`.
    pub fn register_from_skill(
        &mut self,
        skill_name: &str,
        tools: Vec<Arc<dyn Tool>>,
    ) -> Result<(), ToolError> {
        for tool in tools {
            self.register_with_namespace(tool, skill_name)?;
        }
        Ok(())
    }

    /// Get a tool by name (with or without namespace).
    /// If no namespace is provided, searches all namespaces using index (O(1)).
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        // First try exact match (could be namespaced or not)
        if let Some(tool) = self.tools.get(name).cloned() {
            return Some(tool);
        }

        // Fast O(1) lookup using index instead of O(n) scan
        if let Some(weak_tools) = self.tool_name_index.get(name) {
            for weak_tool in weak_tools {
                if let Some(tool) = weak_tool.upgrade() {
                    return Some(tool);
                }
            }
        }

        None
    }

    /// Get a tool from a specific namespace.
    pub fn get_from_namespace(
        &self,
        name: &str,
        namespace: &str,
    ) -> Option<Arc<dyn Tool>> {
        let namespaced_name = format!("{}/{}", namespace, name);
        self.tools.get(&namespaced_name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        let count = self.tools.len();
        self.tools.keys().cloned().collect::<Vec<_>>()
    }

    /// List tools from a specific namespace.
    pub fn list_namespace(&self, namespace: &str) -> Vec<String> {
        let namespace_prefix = format!("{}/", namespace);
        self.tools
            .keys()
            .filter(|name| name.starts_with(&namespace_prefix))
            .cloned()
            .collect()
    }

    /// List all available namespaces.
    pub fn list_namespaces(&self) -> Vec<String> {
        let mut namespaces: std::collections::HashSet<String> = std::collections::HashSet::new();
        for name in self.tools.keys() {
            if let Some(pos) = name.find('/') {
                namespaces.insert(name[..pos].to_string());
            }
        }
        namespaces.into_iter().collect()
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

        let schema = if let Some(cached) = self.schema_cache.get(name).map(|r| r.clone()) {
            cached
        } else {
            let schema = tool.cached_schema();
            self.schema_cache.insert(name.to_string(), schema.clone());
            schema
        };

        super::validate_args(schema.as_ref(), &args)?;
        tool.execute(args).await
    }

    /// Execute multiple tools in parallel for batch operations
    /// Returns results in the same order as input requests
    pub async fn execute_batch(
        &self,
        requests: Vec<(String, serde_json::Value)>
    ) -> Vec<Result<serde_json::Value, ToolError>> {
        use futures::future::join_all;

        let futures: Vec<_> = requests
            .into_iter()
            .map(|(name, args)| {
                let registry = self;
                async move {
                    let tool = registry.tools.get(&name).cloned();
                    match tool {
                        Some(tool) => {
                            let schema = if let Some(cached) = registry.schema_cache.get(&name).map(|r| r.clone()) {
                                cached
                            } else {
                                let schema = tool.cached_schema();
                                registry.schema_cache.insert(name.clone(), schema.clone());
                                schema
                            };
                            if let Err(e) = super::validate_args(schema.as_ref(), &args) {
                                Err(e)
                            } else {
                                tool.execute(args).await
                            }
                        }
                        None => Err(ToolError::NotFound(name)),
                    }
                }
            })
            .collect();

        join_all(futures).await
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
                        // 使用 cached_schema 避免重复分配
                        "parameters": (*tool.cached_schema()).clone()
                    }
                })
            })
            .collect::<Vec<_>>()
    }
}

/// A wrapper tool that prefixes its name with a namespace.
struct NamespacedTool {
    inner: Arc<dyn Tool>,
    namespace: String,
    tool_name: String,
}

impl NamespacedTool {
    fn new(tool: Arc<dyn Tool>, namespace: String, tool_name: String) -> Self {
        Self {
            inner: tool,
            namespace,
            tool_name,
        }
    }
}

#[async_trait]
impl Tool for NamespacedTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> ToolDescription {
        self.inner.description()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.inner.execute(args).await
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
    use super::ToolDescription;
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

    #[tokio::test]
    async fn test_namespace_registration() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);
        
        // Register with namespace
        registry.register_with_namespace(tool.clone(), "calculator").unwrap();
        
        // Tool should be accessible with namespaced name
        assert!(registry.get_from_namespace("dummy", "calculator").is_some());
        
        // Tool list should include namespaced name
        let tools = registry.list_namespace("calculator");
        assert!(tools.contains(&"calculator/dummy".to_string()));
        
        // List namespaces
        let namespaces = registry.list_namespaces();
        assert!(namespaces.contains(&"calculator".to_string()));
    }

    #[tokio::test]
    async fn test_namespace_no_conflict() {
        let mut registry = ToolRegistry::new();
        
        // Same tool name in different namespaces should not conflict
        let tool1 = Arc::new(DummyTool);
        let tool2 = Arc::new(DummyTool);
        
        registry.register_with_namespace(tool1, "skill1").unwrap();
        registry.register_with_namespace(tool2, "skill2").unwrap();
        
        // Both should be successfully registered
        assert!(registry.get_from_namespace("dummy", "skill1").is_some());
        assert!(registry.get_from_namespace("dummy", "skill2").is_some());
        
        // Namespace list should have both
        let namespaces = registry.list_namespaces();
        assert_eq!(namespaces.len(), 2);
    }

    #[tokio::test]
    async fn test_register_from_skill() {
        let mut registry = ToolRegistry::new();
        
        let tool = Arc::new(DummyTool);
        registry.register_from_skill("my-skill", vec![tool]).unwrap();
        
        // Tool should be registered with skill prefix
        assert!(registry.get("my-skill/dummy").is_some());
        
        // List should include namesspaced tool
        let tools = registry.list();
        assert!(tools.contains(&"my-skill/dummy".to_string()));
    }

    #[tokio::test]
    async fn test_execute_namespaced_tool() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);
        
        registry.register_with_namespace(tool, "calc").unwrap();
        
        // Execute with namespaced name
        let result = registry.execute("calc/dummy", serde_json::json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!("executed"));
    }

    #[tokio::test]
    async fn test_get_without_namespace_finds_in_namespace() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);

        registry.register_with_namespace(tool, "calc").unwrap();

        // get without namespace should still find it
        let result = registry.get("dummy");
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_batch_execution_parallel() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);
        registry.register(tool.clone()).unwrap();

        let requests = vec![
            ("dummy".to_string(), serde_json::json!({})),
            ("dummy".to_string(), serde_json::json!({})),
            ("dummy".to_string(), serde_json::json!({})),
        ];

        let results = registry.execute_batch(requests).await;

        assert_eq!(results.len(), 3);
        for result in results {
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), serde_json::json!("executed"));
        }
    }

    #[tokio::test]
    async fn test_batch_execution_mixed_success_failure() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);
        registry.register(tool.clone()).unwrap();

        let requests = vec![
            ("dummy".to_string(), serde_json::json!({})),
            ("nonexistent".to_string(), serde_json::json!({})),
            ("dummy".to_string(), serde_json::json!({})),
        ];

        let results = registry.execute_batch(requests).await;

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }

    #[tokio::test]
    async fn test_batch_execution_performance() {
        let mut registry = ToolRegistry::new();

        struct DelayTool {
            delay_ms: u64,
        }

        #[async_trait]
        impl Tool for DelayTool {
            fn name(&self) -> &str {
                "delay"
            }

            fn description(&self) -> ToolDescription {
                ToolDescription {
                    short: "Tool with artificial delay".to_string(),
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
                tokio::time::sleep(tokio::time::Duration::from_millis(self.delay_ms)).await;
                Ok(serde_json::json!("done"))
            }
        }

        let tool = Arc::new(DelayTool { delay_ms: 100 });
        registry.register(tool).unwrap();

        let tool_name = "delay".to_string();
        let args = serde_json::json!({});
        let num_calls = 5;

        let requests = vec![(tool_name.clone(), args.clone()); num_calls];

        let start_seq = std::time::Instant::now();
        for _ in 0..num_calls {
            let _ = registry.execute(&tool_name, args.clone()).await;
        }
        let seq_duration = start_seq.elapsed();

        let start_batch = std::time::Instant::now();
        let _ = registry.execute_batch(requests).await;
        let batch_duration = start_batch.elapsed();

        println!("Sequential execution: {:?}", seq_duration);
        println!("Batch execution: {:?}", batch_duration);
        println!("Speedup: {:.2}x", seq_duration.as_nanos() as f64 / batch_duration.as_nanos() as f64);

        assert!(batch_duration < seq_duration, "Batch execution should be faster");
    }
}
