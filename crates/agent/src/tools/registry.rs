use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{Tool, ToolError};
use react::tool::registry::AsyncTool;

/// The most async tools one synchronous adapter may run at once.
const MAX_CONCURRENT_ASYNC_TOOLS: usize = 8;

/// How long a synchronous caller waits for one of those slots.
const ASYNC_ADMISSION_WAIT: Duration = Duration::from_millis(50);

/// The default finite wait for a synchronous async-tool call.
const DEFAULT_ASYNC_TIMEOUT: Duration = Duration::from_secs(300);

/// Slots shared by clones of one registry.
#[derive(Default)]
struct AsyncSlots {
    in_use: Mutex<usize>,
    ready: Condvar,
}

impl AsyncSlots {
    fn acquire(self: &Arc<Self>, wait: Duration) -> Option<AsyncSlot> {
        let mut in_use = self
            .in_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let deadline = Instant::now() + wait;
        while *in_use >= MAX_CONCURRENT_ASYNC_TOOLS {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (guard, timeout) = self
                .ready
                .wait_timeout(in_use, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            in_use = guard;
            if timeout.timed_out() && *in_use >= MAX_CONCURRENT_ASYNC_TOOLS {
                return None;
            }
        }
        *in_use += 1;
        Some(AsyncSlot {
            slots: Arc::clone(self),
        })
    }
}

/// One held slot; dropped by the worker when the tool future ends, not by the
/// caller on timeout, so a stuck tool keeps consuming its admission slot.
struct AsyncSlot {
    slots: Arc<AsyncSlots>,
}

impl Drop for AsyncSlot {
    fn drop(&mut self) {
        if let Ok(mut in_use) = self.slots.in_use.lock() {
            *in_use = in_use.saturating_sub(1);
        }
        self.slots.ready.notify_one();
    }
}

/// A registry of synchronous and asynchronous tools, addressed by name.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    async_tools: HashMap<String, Arc<dyn AsyncTool>>,
    schema_cache: DashMap<String, serde_json::Value>,
    mcp_tool_names: HashSet<String>,
    slots: Arc<AsyncSlots>,
    async_timeout: Duration,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            async_tools: self.async_tools.clone(),
            schema_cache: DashMap::new(),
            mcp_tool_names: self.mcp_tool_names.clone(),
            slots: Arc::clone(&self.slots),
            async_timeout: self.async_timeout,
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
            slots: Arc::new(AsyncSlots::default()),
            async_timeout: DEFAULT_ASYNC_TIMEOUT,
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
    fn find_async_tool(&self, name: &str) -> Result<Option<Arc<dyn AsyncTool>>, ToolError> {
        // Exact match always wins.
        if let Some(tool) = self.async_tools.get(name) {
            return Ok(Some(tool.clone()));
        }
        // The suffix fallback is for a model naming the bare tool an MCP server
        // exposes. Two servers may expose the same bare name; picking whichever
        // the HashMap yields first would route a call by insertion order, so an
        // ambiguous suffix is refused and the candidates are listed.
        let suffix = format!("_{name}");
        let mut candidates: Vec<(&String, &Arc<dyn AsyncTool>)> = self
            .async_tools
            .iter()
            .filter(|(async_name, _)| async_name.ends_with(name) || async_name.ends_with(&suffix))
            .collect();
        match candidates.len() {
            0 => Ok(None),
            1 => Ok(Some(candidates.remove(0).1.clone())),
            _ => {
                let mut names: Vec<String> =
                    candidates.iter().map(|(name, _)| (*name).clone()).collect();
                names.sort();
                Err(ToolError::Ambiguous {
                    name: name.to_string(),
                    candidates: names,
                })
            }
        }
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
        match self.find_async_tool(name)? {
            Some(tool) => self.run_async(tool, args),
            None => Err(ToolError::NotFound(name.to_string())),
        }
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
            self.schema_cache
                .insert(registry_name.clone(), schema.clone());
            schema
        };
        super::validate_args(&schema, args)?;

        // A synchronous caller cannot await, so it runs the future on one
        // worker thread and waits for a finite time. Admission is bounded: a
        // stuck tool keeps its slot until its future actually ends, so repeated
        // timeouts cannot grow thread count without bound.
        let slot = self.slots.acquire(ASYNC_ADMISSION_WAIT).ok_or_else(|| {
            ToolError::Failed(format!(
                "async tool executor is saturated ({MAX_CONCURRENT_ASYNC_TOOLS} in flight)"
            ))
        })?;
        let args = args.clone();
        let worker_name = format!("tool-{registry_name}");
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::Builder::new()
            .name(worker_name)
            .spawn(move || {
                let _slot = slot;
                let result = match tokio::runtime::Builder::new_current_thread().build() {
                    Ok(rt) => rt.block_on(tool.run(&args)),
                    Err(e) => Err(ToolError::Failed(format!(
                        "Failed to create tool runtime: {e}"
                    ))),
                };
                let _ = tx.send(result);
            })
            .map_err(|error| {
                ToolError::Failed(format!("Failed to start async tool worker: {error}"))
            })?;

        match rx.recv_timeout(self.async_timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(ToolError::Timeout(format!(
                "async tool {registry_name} did not finish within {:?}",
                self.async_timeout
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(ToolError::Failed(
                "async tool worker exited without a result".to_string(),
            )),
        }
    }

    /// Set the finite wait a synchronous caller gives an async tool.
    ///
    /// The timeout is a wait bound, not cancellation: the worker keeps running
    /// (and keeps its admission slot) until the tool future ends. Async-native
    /// callers use the react registry and never pass through this adapter.
    pub fn with_async_timeout(mut self, timeout: Duration) -> Self {
        self.async_timeout = timeout;
        self
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

    /// An async tool that never resolves, to exercise wait bounds.
    struct Never;

    #[async_trait]
    impl AsyncTool for Never {
        fn name(&self) -> &str {
            "never"
        }

        fn description(&self) -> String {
            "never finishes".to_string()
        }

        fn json_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn run(&self, _input: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
            std::future::pending().await
        }
    }

    /// A stuck tool is waited for finitely, and each timed-out call keeps its
    /// admission slot, so a later call is refused instead of spawning more.
    #[test]
    fn a_stuck_async_tool_times_out_and_saturates() {
        let mut registry = ToolRegistry::new().with_async_timeout(Duration::from_millis(40));
        registry.register_async(Arc::new(Never)).unwrap();

        for _ in 0..MAX_CONCURRENT_ASYNC_TOOLS {
            let error = registry
                .execute("never", &serde_json::json!({}))
                .expect_err("the tool never finishes");
            assert!(
                matches!(error, ToolError::Timeout(_)),
                "the wait is finite: {error:?}"
            );
        }

        let error = registry
            .execute("never", &serde_json::json!({}))
            .expect_err("the executor is saturated");
        assert!(error.to_string().contains("saturated"), "{error}");
    }

    /// A panicking worker is reported as a failed call and releases its slot.
    #[test]
    fn a_panicking_async_tool_is_reported() {
        struct Panics;

        #[async_trait]
        impl AsyncTool for Panics {
            fn name(&self) -> &str {
                "panics"
            }

            fn description(&self) -> String {
                "panics".to_string()
            }

            fn json_schema(&self) -> serde_json::Value {
                serde_json::json!({ "type": "object" })
            }

            async fn run(
                &self,
                _input: &serde_json::Value,
            ) -> Result<serde_json::Value, ToolError> {
                panic!("tool panic");
            }
        }

        let mut registry = ToolRegistry::new().with_async_timeout(Duration::from_millis(500));
        registry.register_async(Arc::new(Panics)).unwrap();
        let error = registry
            .execute("panics", &serde_json::json!({}))
            .expect_err("the worker panicked");
        assert!(
            error.to_string().contains("exited without a result"),
            "{error}"
        );
    }
    /// A single MCP-style suffix still resolves the bare name.
    #[test]
    fn one_mcp_suffix_match_still_resolves() {
        let mut registry = ToolRegistry::new();
        registry
            .register_async(Arc::new(NamedAsync("mcp_hello_add")))
            .unwrap();
        let output = registry
            .execute("add", &serde_json::json!({}))
            .expect("one suffix match");
        assert_eq!(output["ran"], "async");
    }

    /// Two servers exposing the same bare name are an ambiguity, never an
    /// insertion-order choice, and the candidate list is sorted.
    #[test]
    fn colliding_suffixes_are_ambiguous_in_every_order() {
        for order in [
            ["mcp_files_read_file", "mcp_git_read_file"],
            ["mcp_git_read_file", "mcp_files_read_file"],
        ] {
            let mut registry = ToolRegistry::new();
            for name in order {
                registry.register_async(Arc::new(NamedAsync(name))).unwrap();
            }
            let error = registry
                .execute("read_file", &serde_json::json!({}))
                .expect_err("two candidates must not dispatch");
            match error {
                ToolError::Ambiguous { name, candidates } => {
                    assert_eq!(name, "read_file");
                    assert_eq!(
                        candidates,
                        vec![
                            "mcp_files_read_file".to_string(),
                            "mcp_git_read_file".to_string(),
                        ]
                    );
                }
                other => panic!("expected ambiguity, got {other:?}"),
            }
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
