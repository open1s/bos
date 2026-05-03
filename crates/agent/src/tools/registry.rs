use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

use super::{Tool, ToolError};
use react::tool::registry::AsyncTool;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    async_tools: HashMap<String, Arc<dyn AsyncTool>>,
    schema_cache: DashMap<String, serde_json::Value>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            async_tools: self.async_tools.clone(),
            schema_cache: DashMap::new(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            async_tools: HashMap::new(),
            schema_cache: DashMap::new(),
        }
    }

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

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn get_async(&self, name: &str) -> Option<Arc<dyn AsyncTool>> {
        self.async_tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.extend(self.async_tools.keys().cloned());
        names
    }

    pub fn async_tool_names(&self) -> Vec<String> {
        self.async_tools.keys().cloned().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn Tool>)> {
        self.tools.iter()
    }

    /// Find an async tool by exact name or suffix match
    fn find_async_tool(&self, name: &str) -> Option<Arc<dyn AsyncTool>> {
        // Try exact match first
        if let Some(tool) = self.async_tools.get(name) {
            return Some(tool.clone());
        }
        // Try suffix match - key might be "mcp_hello/add" when name is "hello/add"
        // Check if key ends with name directly or with "_name"
        for (async_name, async_tool) in self.async_tools.iter() {
            if async_name.ends_with(name) || async_name.ends_with(&format!("_{}", name)) {
                return Some(async_tool.clone());
            }
        }
        None
    }

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

            return rx.recv().map_err(|e| ToolError::Failed(format!("Channel error: {}", e)))?;
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
