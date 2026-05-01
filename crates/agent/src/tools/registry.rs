use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

use super::{Tool, ToolError};

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    schema_cache: DashMap<String, serde_json::Value>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            schema_cache: DashMap::new(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
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

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<dyn Tool>)> {
        self.tools.iter()
    }

    pub fn execute(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
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
        self.tools
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
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
