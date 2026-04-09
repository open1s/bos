use crate::tools::policy::{BoxedPolicy, PolicyContext};
use serde_json;
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::{Arc, Weak};

use super::{Tool, ToolError};
use crate::tools::{async_trait, ToolDescription};
use dashmap::DashMap;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    tool_name_index: HashMap<String, Vec<Weak<dyn Tool>>>,
    namespaced_tool_index: HashMap<String, HashMap<String, Weak<dyn Tool>>>,
    namespace_index: HashMap<String, Vec<String>>,
    namespaces: Vec<String>,
    schema_cache: DashMap<String, Arc<serde_json::Value>>, // Arc 共享，零拷贝
    openai_format_cache: RwLock<Arc<Vec<serde_json::Value>>>,
    policies: std::collections::HashMap<String, BoxedPolicy>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            tool_name_index: self.tool_name_index.clone(),
            namespaced_tool_index: self.namespaced_tool_index.clone(),
            namespace_index: self.namespace_index.clone(),
            namespaces: self.namespaces.clone(),
            schema_cache: DashMap::new(),
            openai_format_cache: RwLock::new(Arc::new(Vec::new())),
            policies: self.policies.clone(),
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tool_names: Vec<_> = self.tools.keys().collect();
        f.debug_struct("ToolRegistry")
            .field("tools_count", &tool_names.len())
            .field("tool_names", &tool_names)
            .field("namespaces", &self.namespaces)
            .field("namespace_count", &self.namespace_index.len())
            .finish_non_exhaustive()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            tool_name_index: HashMap::new(),
            namespaced_tool_index: HashMap::new(),
            namespace_index: HashMap::new(),
            namespaces: Vec::new(),
            schema_cache: DashMap::new(),
            openai_format_cache: RwLock::new(Arc::new(Vec::new())),
            policies: std::collections::HashMap::new(),
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

        let weak_tool = Arc::downgrade(&tool);
        self.tool_name_index
            .entry(name.clone())
            .or_default()
            .push(weak_tool);

        let schema = tool.cached_schema();
        self.schema_cache.insert(name.clone(), schema);
        self.push_openai_format_entry(tool.as_ref());

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
                namespace, tool_name
            )));
        }

        // Create a wrapper tool with the namespaced name
        let wrapped = Arc::new(NamespacedTool::new(tool, namespaced_name.clone()));
        let weak_wrapped = Arc::downgrade(&wrapped);
        self.tool_name_index
            .entry(tool_name.clone())
            .or_default()
            .push(weak_wrapped.clone());
        self.namespaced_tool_index
            .entry(namespace.to_string())
            .or_default()
            .insert(tool_name.clone(), weak_wrapped);

        let namespace_entry = self
            .namespace_index
            .entry(namespace.to_string())
            .or_default();
        if namespace_entry.is_empty() {
            self.namespaces.push(namespace.to_string());
        }
        namespace_entry.push(namespaced_name.clone());

        let schema = wrapped.cached_schema();
        self.schema_cache.insert(namespaced_name.clone(), schema);
        self.push_openai_format_entry(wrapped.as_ref());

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
    pub fn get_from_namespace(&self, name: &str, namespace: &str) -> Option<Arc<dyn Tool>> {
        self.namespaced_tool_index
            .get(namespace)
            .and_then(|tools| tools.get(name))
            .and_then(Weak::upgrade)
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect::<Vec<_>>()
    }

    /// Iterate over all registered tools.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn Tool>)> {
        self.tools.iter()
    }

    /// List tools from a specific namespace.
    pub fn list_namespace(&self, namespace: &str) -> Vec<String> {
        self.namespace_index
            .get(namespace)
            .cloned()
            .unwrap_or_default()
    }

    /// List all available namespaces.
    pub fn list_namespaces(&self) -> Vec<String> {
        self.namespaces.clone()
    }

    pub async fn execute(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        // Patch D: policy check before tool execution
        if let Some(policy) = self.policies.get(name) {
            if !policy.is_allowed(name, &PolicyContext::new()) {
                return Err(ToolError::ExecutionFailed("policy_denied".to_string()));
            }
        }

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

        super::validate_args(schema.as_ref(), args)?;
        tool.execute(args).await
    }

    // Patch D: policy-enabled configure (bind a policy to a tool)
    pub fn configure_policy(&mut self, tool_name: &str, policy: BoxedPolicy) {
        self.policies.insert(tool_name.to_string(), policy);
    }

    // Patch D skeleton: policy-enabled execute (no-op policy hook)
    pub async fn execute_with_policy(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        // Future: consult per-tool policy here
        // Currently, delegate to existing execute path for compatibility
        self.execute(name, args).await
    }

    /// Execute multiple tools in parallel for batch operations
    /// Returns results in the same order as input requests
    pub async fn execute_batch(
        &self,
        requests: Vec<(String, serde_json::Value)>,
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
                            let schema = if let Some(cached) =
                                registry.schema_cache.get(&name).map(|r| r.clone())
                            {
                                cached
                            } else {
                                let schema = tool.cached_schema();
                                registry.schema_cache.insert(name.clone(), schema.clone());
                                schema
                            };
                            if let Err(e) = super::validate_args(schema.as_ref(), &args) {
                                Err(e)
                            } else {
                                tool.execute(&args).await
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
        self.to_openai_format_shared().as_ref().clone()
    }

    pub fn to_openai_format_shared(&self) -> Arc<Vec<serde_json::Value>> {
        self.openai_format_cache
            .read()
            .expect("openai_format_cache poisoned")
            .clone()
    }

    fn push_openai_format_entry(&self, tool: &dyn Tool) {
        let mut cache = self
            .openai_format_cache
            .write()
            .expect("openai_format_cache poisoned");
        Arc::make_mut(&mut cache).push(Self::build_openai_format_entry(tool));
    }

    fn build_openai_format_entry(tool: &dyn Tool) -> serde_json::Value {
        let desc = tool.description();
        serde_json::json!({
            "type": "function",
            "function": {
                "name": tool.name(),
                "description": desc.short,
                "parameters": (*tool.cached_schema()).clone()
            }
        })
    }
}

/// A wrapper tool that prefixes its name with a namespace.
struct NamespacedTool {
    inner: Arc<dyn Tool>,
    namespaced_name: String,
}

impl NamespacedTool {
    fn new(tool: Arc<dyn Tool>, namespaced_name: String) -> Self {
        Self {
            inner: tool,
            namespaced_name,
        }
    }
}

#[async_trait]
impl Tool for NamespacedTool {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn description(&self) -> ToolDescription {
        self.inner.description()
    }

    fn json_schema(&self) -> serde_json::Value {
        self.inner.json_schema()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
    use super::ToolDescription;
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

        async fn execute(&self, _args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
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
        let args = serde_json::json!({});
        let result = registry.execute("dummy", &args).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!("executed"));
    }

    #[tokio::test]
    async fn test_not_found() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({});
        let result = registry.execute("nonexistent", &args).await;
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

    #[test]
    fn test_namespaced_tool_uses_namespaced_name_in_openai_format() {
        let mut registry = ToolRegistry::new();
        registry
            .register_with_namespace(Arc::new(DummyTool), "calculator")
            .unwrap();

        let format = registry.to_openai_format();

        assert_eq!(format.len(), 1);
        assert_eq!(format[0]["function"]["name"], "calculator/dummy");
    }

    #[test]
    fn test_openai_format_cache_stays_in_sync_after_registration() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool)).unwrap();
        assert_eq!(registry.to_openai_format().len(), 1);

        registry
            .register_with_namespace(Arc::new(DummyTool), "calculator")
            .unwrap();

        let format = registry.to_openai_format();
        assert_eq!(format.len(), 2);
        assert!(format
            .iter()
            .any(|entry| entry["function"]["name"] == "dummy"));
        assert!(format
            .iter()
            .any(|entry| entry["function"]["name"] == "calculator/dummy"));
    }

    #[tokio::test]
    async fn test_namespace_registration() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool);

        // Register with namespace
        registry
            .register_with_namespace(tool.clone(), "calculator")
            .unwrap();

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
        registry
            .register_from_skill("my-skill", vec![tool])
            .unwrap();

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
        let args = serde_json::json!({});
        let result = registry.execute("calc/dummy", &args).await;
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

            async fn execute(
                &self,
                _args: &serde_json::Value,
            ) -> Result<serde_json::Value, ToolError> {
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
            let _ = registry.execute(&tool_name, &args).await;
        }
        let seq_duration = start_seq.elapsed();

        let start_batch = std::time::Instant::now();
        let _ = registry.execute_batch(requests).await;
        let batch_duration = start_batch.elapsed();

        println!("Sequential execution: {:?}", seq_duration);
        println!("Batch execution: {:?}", batch_duration);
        println!(
            "Speedup: {:.2}x",
            seq_duration.as_nanos() as f64 / batch_duration.as_nanos() as f64
        );

        assert!(
            batch_duration < seq_duration,
            "Batch execution should be faster"
        );
    }
}
