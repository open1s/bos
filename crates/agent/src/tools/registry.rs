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
    ///
    /// The name is unique across BOTH maps: one `schema_cache` entry backs both
    /// kinds, so a sync and an async tool sharing a name would validate one
    /// through the other's schema and only one of them could ever be reached.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) || self.async_tools.contains_key(&name) {
            return Err(ToolError::Failed(format!("duplicate tool: {}", name)));
        }
        let schema = tool.json_schema();
        self.schema_cache.insert(name.clone(), schema);
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Register an asynchronous tool, failing on a duplicate name.
    ///
    /// The name is unique across BOTH maps, for the reason [Self::register]
    /// gives.
    pub fn register_async(&mut self, tool: Arc<dyn AsyncTool>) -> Result<(), ToolError> {
        let name = tool.name().to_string();
        if self.async_tools.contains_key(&name) || self.tools.contains_key(&name) {
            return Err(ToolError::Failed(format!("duplicate async tool: {}", name)));
        }
        let schema = tool.json_schema();
        self.schema_cache.insert(name.clone(), schema);
        self.async_tools.insert(name, tool);
        Ok(())
    }

    /// Remove an asynchronous tool and all of its derived registry metadata.
    ///
    /// The caller is responsible for invalidating any cached execution engine
    /// that captured a prior registry snapshot.
    pub fn remove_async(&mut self, name: &str) -> bool {
        let removed = self.async_tools.remove(name).is_some();
        if removed {
            self.schema_cache.remove(name);
            self.mcp_tool_names.remove(name);
        }
        removed
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

    /// Find an async tool by exact name, else by an MCP `{namespace}_` suffix.
    ///
    /// [Self::execute] consults this only after both exact registries miss, so
    /// the suffix can never take a call away from a sync tool with that exact
    /// name.
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
    ///
    /// Resolution is EXACT name first, in both registries, and only then the
    /// MCP suffix fallback. A suffix match must never shadow a tool the caller
    /// named exactly: an MCP server registers its tools as
    /// `{namespace}_{tool_name}`, so a server tool called `read_file` used to
    /// answer the model's `read_file` call and run the server's tool instead of
    /// the built-in one. The fallback exists for the opposite direction only,
    /// where the model names the bare tool an MCP server exposes.
    ///
    /// Async tools are driven to completion on a dedicated thread with its own
    /// current-thread runtime, because this entry point is synchronous while
    /// the tool's future may await I/O. Their arguments are validated before
    /// dispatch, exactly like a synchronous tool's.
    pub fn execute(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        if let Some(tool) = self.async_tools.get(name).cloned() {
            return self.run_async(tool, args);
        }
        if let Some(tool) = self.tools.get(name).cloned() {
            return self.run_sync(name, tool, args);
        }
        // The suffix match is the MCP bare-name fallback, so it is consulted
        // only after both exact maps missed.
        if let Some(tool) = self.find_async_tool(name) {
            return self.run_async(tool, args);
        }
        Err(ToolError::NotFound(name.to_string()))
    }

    /// Validate one async tool's arguments and run it to completion.
    fn run_async(
        &self,
        tool: Arc<dyn AsyncTool>,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let registry_name = tool.name().to_string();
        let schema = if let Some(cached) = self.schema_cache.get(&registry_name).map(|r| r.clone())
        {
            cached
        } else {
            let schema = tool.json_schema();
            self.schema_cache.insert(registry_name, schema.clone());
            schema
        };
        super::validate_args(&schema, args)?;

        let args = args.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = match tokio::runtime::Builder::new_current_thread().build() {
                Ok(rt) => rt.block_on(tool.run(&args)),
                Err(e) => Err(ToolError::Failed(format!(
                    "Failed to create tool runtime: {e}"
                ))),
            };
            let _ = tx.send(result);
        });

        rx.recv()
            .map_err(|e| ToolError::Failed(format!("Async tool thread failed: {e}")))?
    }

    /// Validate one synchronous tool's arguments and run it.
    fn run_sync(
        &self,
        name: &str,
        tool: Arc<dyn Tool>,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
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

    struct StrictAsync;

    #[async_trait]
    impl AsyncTool for StrictAsync {
        fn name(&self) -> &str {
            "strict_exec"
        }
        fn description(&self) -> String {
            "requires a value".to_string()
        }
        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"]
            })
        }
        async fn run(&self, _input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!({ "ran": true }))
        }
    }

    #[test]
    fn async_tool_arguments_are_validated_before_dispatch() {
        let mut registry = ToolRegistry::new();
        registry.register_async(Arc::new(StrictAsync)).unwrap();

        let error = registry
            .execute("strict_exec", &serde_json::json!({}))
            .expect_err("missing required argument");
        assert!(
            matches!(error, ToolError::SchemaMismatch { .. }),
            "{error:?}"
        );

        let value = registry
            .execute("strict_exec", &serde_json::json!({"value": "x"}))
            .expect("valid call runs");
        assert_eq!(value["ran"], true);
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

    #[test]
    fn removing_async_tool_clears_schema_and_mcp_metadata() {
        let mut registry = registry("ns_echo", true);
        assert_eq!(registry.mcp_tool_entries().len(), 1);
        assert!(registry.remove_async("ns_echo"));
        assert!(registry.get_async("ns_echo").is_none());
        assert!(registry.mcp_tool_entries().is_empty());
        assert!(!registry.remove_async("ns_echo"));
    }

    /// A synchronous tool that answers with its own name, so a test can tell
    /// which of two same-shaped tools actually ran.
    struct SyncTool(&'static str);

    impl Tool for SyncTool {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> String {
            format!("{} (sync)", self.0)
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        fn run(&self, _input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!({ "ran": "sync" }))
        }
    }

    /// An async tool that answers with its own name.
    struct NamedAsync(&'static str);

    #[async_trait]
    impl AsyncTool for NamedAsync {
        fn name(&self) -> &str {
            self.0
        }

        fn description(&self) -> String {
            format!("{} (async)", self.0)
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn run(&self, _input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            Ok(serde_json::json!({ "ran": "async" }))
        }
    }

    /// A call by an exact tool name must reach that tool. An MCP server's
    /// `{namespace}_{tool}` registration used to win the call through the suffix
    /// match, so a server tool named `read_file` silently replaced the built-in
    /// file reader.
    #[test]
    fn an_exact_sync_name_beats_an_mcp_suffix_match() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(SyncTool("read_file"))).unwrap();
        r.register_async(Arc::new(NamedAsync("srv_read_file")))
            .unwrap();

        let exact = r.execute("read_file", &serde_json::json!({})).unwrap();
        assert_eq!(exact["ran"], "sync", "the built-in tool keeps its name");

        let namespaced = r.execute("srv_read_file", &serde_json::json!({})).unwrap();
        assert_eq!(
            namespaced["ran"], "async",
            "the namespaced tool is reachable"
        );
    }

    /// The suffix fallback still answers a bare tool name, which is what it is
    /// for: the model names the tool an MCP server exposes without the
    /// namespace prefix.
    #[test]
    fn a_bare_name_falls_back_to_the_namespaced_async_tool() {
        let mut r = ToolRegistry::new();
        r.register_async(Arc::new(NamedAsync("ns_echo"))).unwrap();
        let value = r.execute("echo", &serde_json::json!({})).unwrap();
        assert_eq!(value["ran"], "async");
    }

    /// One name identifies one tool: the registries are checked together, so a
    /// sync and an async tool cannot share a name (and with it a schema cache
    /// entry).
    #[test]
    fn a_name_is_unique_across_both_registries() {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(SyncTool("dup"))).unwrap();
        assert!(
            r.register_async(Arc::new(NamedAsync("dup"))).is_err(),
            "async registration must see the sync name"
        );

        let mut r = ToolRegistry::new();
        r.register_async(Arc::new(NamedAsync("dup"))).unwrap();
        assert!(
            r.register(Arc::new(SyncTool("dup"))).is_err(),
            "sync registration must see the async name"
        );
    }
}
