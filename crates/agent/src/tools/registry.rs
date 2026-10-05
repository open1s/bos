use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{Tool, ToolError};
use react::tool::registry::AsyncTool;

/// A registry of synchronous and asynchronous tools, addressed by name.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    async_tools: HashMap<String, Arc<dyn AsyncTool>>,
    schema_cache: DashMap<String, serde_json::Value>,
    mcp_tool_names: HashSet<String>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            async_tools: self.async_tools.clone(),
            schema_cache: DashMap::new(),
            mcp_tool_names: self.mcp_tool_names.clone(),
        }
    }
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            async_tools: HashMap::new(),
            schema_cache: DashMap::new(),
            mcp_tool_names: HashSet::new(),
        }
    }

    /// Register a synchronous tool, failing on a duplicate name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            return Err(ToolError::Failed(format!("duplicate tool: {}", name)));
        }
        let schema = tool.json_schema();
        self.schema_cache.insert(name.clone(), schema);
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Register an asynchronous tool, failing on a duplicate name.
    pub fn register_async(&mut self, tool: Arc<dyn AsyncTool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.async_tools.contains_key(&name) {
            return Err(ToolError::Failed(format!("duplicate async tool: {}", name)));
        }
        let schema = tool.json_schema();
        self.schema_cache.insert(name.clone(), schema);
        self.async_tools.insert(name, tool);
        Ok(())
    }

    /// Register a synchronous tool under `namespace_toolname`.
    pub fn register_with_namespace(
        &mut self,
        tool: Arc<dyn Tool>,
        namespace: &str,
    ) -> Result<(), ToolError> {
        let namespaced_name = format!("{}_{}", namespace, tool.name());
        let schema = tool.json_schema();
        self.schema_cache.insert(namespaced_name.clone(), schema);
        self.tools.insert(namespaced_name, tool);
        Ok(())
    }

    /// Register an asynchronous tool under `namespace_toolname`.
    pub fn register_async_with_namespace(
        &mut self,
        tool: Arc<dyn AsyncTool>,
        namespace: &str,
    ) -> Result<(), ToolError> {
        let namespaced_name = format!("{}_{}", namespace, tool.name());
        let schema = tool.json_schema();
        self.schema_cache.insert(namespaced_name.clone(), schema);
        self.async_tools.insert(namespaced_name, tool);
        Ok(())
    }

    /// Look up a synchronous tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Look up an asynchronous tool by name.
    pub fn get_async(&self, name: &str) -> Option<Arc<dyn AsyncTool>> {
        self.async_tools.get(name).cloned()
    }

    /// Names of every registered tool, sync and async.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.extend(self.async_tools.keys().cloned());
        names
    }

    /// Names of the registered asynchronous tools.
    pub fn async_tool_names(&self) -> Vec<String> {
        self.async_tools.keys().cloned().collect()
    }

    /// Iterate over the synchronous tools.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn Tool>)> {
        self.tools.iter()
    }

    /// Mark `name` as originating from MCP.
    pub fn mark_mcp_tool(&mut self, name: &str) {
        self.mcp_tool_names.insert(name.to_string());
    }

    /// Whether `name` is an MCP tool.
    pub fn is_mcp_tool(&self, name: &str) -> bool {
        self.mcp_tool_names.contains(name)
    }

    /// `{name, description}` projections of MCP tools, sync and async.
    ///
    /// MCP tools are registered as async tools, so a projection that only walked
    /// the sync map returned nothing. Both language bindings share this method so
    /// they cannot drift again.
    pub fn mcp_tool_entries(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for (name, tool) in &self.tools {
            if self.mcp_tool_names.contains(name) {
                out.push(serde_json::json!({
                    "name": name,
                    "description": tool.description(),
                }));
            }
        }
        for (name, tool) in &self.async_tools {
            if self.mcp_tool_names.contains(name) {
                out.push(serde_json::json!({
                    "name": name,
                    "description": tool.description(),
                }));
            }
        }
        out
    }

    /// MCP resources exposed under `namespace`, sync and async.
    pub fn mcp_resource_entries(&self, namespace: &str) -> Vec<serde_json::Value> {
        let prefix = format!("{}_", namespace);
        let mut out = Vec::new();
        for (name, tool) in &self.tools {
            if name.starts_with(&prefix) {
                out.push(serde_json::json!({
                    "name": name,
                    "description": tool.description(),
                }));
            }
        }
        for (name, tool) in &self.async_tools {
            if name.starts_with(&prefix) {
                out.push(serde_json::json!({
                    "name": name,
                    "description": tool.description(),
                }));
            }
        }
        out
    }

    /// `{name, description}` projections for MCP prompts.
    ///
    /// Prompts are not yet registered into the tool registry (see the MCP
    /// client), so this mirrors the MCP-marked set. It exists so both bindings
    /// resolve to one definition instead of diverging heuristics.
    pub fn mcp_prompt_entries(&self) -> Vec<serde_json::Value> {
        self.mcp_tool_entries()
    }

    /// Names of MCP prompts; see `mcp_prompt_entries`.
    pub fn mcp_prompt_names(&self) -> Vec<String> {
        self.mcp_tool_names.iter().cloned().collect()
    }

    /// Find an async tool by exact name or suffix match
    fn find_async_tool(&self, name: &str) -> Option<Arc<dyn AsyncTool>> {
        // Try exact match first
        if let Some(tool) = self.async_tools.get(name) {
            return Some(tool.clone());
        }
        // Try suffix match - key might be "mcp_hello_add" when name is "hello_add"
        // Check if key ends with name directly or with "_name"
        for (async_name, async_tool) in self.async_tools.iter() {
            if async_name.ends_with(name) || async_name.ends_with(&format!("_{}", name)) {
                return Some(async_tool.clone());
            }
        }
        None
    }

    /// Execute a tool by name, validating arguments against its schema.
    pub fn execute(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        // First try async tools via suffix matching
        if let Some(tool) = self.find_async_tool(name) {
            let tool = tool.clone();
            let args = args.clone();

            let (tx, rx) = std::sync::mpsc::channel();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("Failed to create runtime");
                let result = rt.block_on(tool.run(&args));
                let _ = tx.send(result);
            });

            return rx
                .recv()
                .map_err(|e| ToolError::Failed(format!("Channel error: {}", e)))?;
        }

        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        let schema = if let Some(cached) = self.schema_cache.get(name).map(|r| r.clone()) {
            cached
        } else {
            let schema = tool.json_schema();
            self.schema_cache.insert(name.to_string(), schema.clone());
            schema
        };

        super::validate_args(&schema, args)?;
        tool.run(args)
    }

    /// Render the tools as OpenAI function-calling definitions.
    pub fn to_openai_format(&self) -> Vec<serde_json::Value> {
        let mut result: Vec<serde_json::Value> = self
            .tools
            .values()
            .map(|tool| {
                let desc = tool.description();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": desc,
                        "parameters": tool.json_schema()
                    }
                })
            })
            .collect();

        result.extend(self.async_tools.values().map(|tool| {
            let desc = tool.description();
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name(),
                    "description": desc,
                    "parameters": tool.json_schema()
                }
            })
        }));

        result
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod mcp_projection_tests {
    use super::*;
    use async_trait::async_trait;

    struct DummyAsync {
        name: String,
    }

    #[async_trait]
    impl AsyncTool for DummyAsync {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> String {
            format!("{} description", self.name)
        }
        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn run(&self, _input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!({}))
        }
    }

    fn registry(name: &str, mcp: bool) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register_async(Arc::new(DummyAsync { name: name.into() }))
            .unwrap();
        if mcp {
            r.mark_mcp_tool(name);
        }
        r
    }

    #[test]
    fn mcp_tool_entries_include_async_tools() {
        let entries = registry("ns_echo", true).mcp_tool_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "ns_echo");
    }

    #[test]
    fn mcp_resource_entries_filter_by_namespace() {
        let r = registry("ns_echo", true);
        assert_eq!(r.mcp_resource_entries("ns").len(), 1);
        assert!(r.mcp_resource_entries("other").is_empty());
    }

    #[test]
    fn non_mcp_tools_are_excluded() {
        assert!(registry("plain", false).mcp_tool_entries().is_empty());
    }
}
