//! Minimal MCP stdio server over the codel candidate graph.
//!
//! The server speaks newline-delimited JSON-RPC, exposes five tools, and indexes
//! one workspace lazily on the first tool call. Workspace identity comes from
//! the host context; the root comes from host context or MCP roots and is always
//! re-canonicalized by WorkspaceDescriptor.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use codel::{
    AffectedOptions, AnswerBudget, CodeGraph, DEFAULT_ANSWER_TOKENS, EdgeKind, NodeId, PathOptions,
    SourceIndexStatus, WorkspaceId,
};
use serde_json::{Value, json};

use crate::protocol::{PROTOCOL_VERSION, error, success, tool_definitions};
use crate::workspace::{
    HostContext, WorkspaceGraph, load_workspace, parse_host_context, roots_from_value,
};

/// One MCP server over a generic writer.
pub struct McpServer<W: Write> {
    writer: W,
    initialized: bool,
    roots_capable: bool,
    roots_request_id: Option<u64>,
    roots: Vec<PathBuf>,
    roots_error: Option<String>,
    host: Option<HostContext>,
    workspaces: BTreeMap<PathBuf, HostContext>,
    workspace: Option<WorkspaceGraph>,
    scoped_graphs: BTreeMap<PathBuf, WorkspaceGraph>,
    scip_index: Option<PathBuf>,
    next_request_id: u64,
    allow_roots: bool,
    host_explicit: bool,
    identity_source: &'static str,
    watch_enabled: bool,
    dirty: Arc<AtomicBool>,
    watcher: Option<RecommendedWatcher>,
}

impl<W: Write> McpServer<W> {
    /// Create an uninitialized server.
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            initialized: false,
            roots_capable: false,
            roots_request_id: None,
            roots: Vec::new(),
            roots_error: None,
            host: None,
            workspaces: BTreeMap::new(),
            workspace: None,
            scoped_graphs: BTreeMap::new(),
            scip_index: None,
            next_request_id: 1,
            allow_roots: true,
            host_explicit: false,
            identity_source: "fallback",
            watch_enabled: false,
            dirty: Arc::new(AtomicBool::new(false)),
            watcher: None,
        }
    }

    /// Enable filesystem watching for the selected workspace root.
    pub fn enable_watch(&mut self) {
        self.watch_enabled = true;
        if let Some(root) = self.host.as_ref().and_then(|host| host.root.clone()) {
            self.start_watcher(&root);
        }
    }

    fn start_watcher(&mut self, root: &std::path::Path) {
        if self.watcher.is_some() {
            return;
        }
        let dirty = Arc::clone(&self.dirty);
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if result.is_ok() {
                    dirty.store(true, Ordering::SeqCst);
                }
            },
            notify::Config::default(),
        );
        let Ok(mut watcher) = watcher else {
            return;
        };
        if watcher.watch(root, RecursiveMode::Recursive).is_err() {
            return;
        }
        self.watcher = Some(watcher);
    }

    /// Disable the server-to-client roots/list request.
    ///
    /// HTTP transport cannot push that request as a standalone response, so the
    /// HTTP launcher requires a host context root instead.
    pub fn disable_roots(&mut self) {
        self.allow_roots = false;
        self.roots_capable = false;
    }

    /// Drain bytes written by the last handled message.
    ///
    /// Only available for the buffered writer used by the HTTP transport.
    pub fn take_output(&mut self) -> Vec<u8>
    where
        W: Default + Into<Vec<u8>>,
    {
        let output: W = std::mem::take(&mut self.writer);
        output.into()
    }

    /// Seed host context from the process environment.
    ///
    /// The MCP initialize host context overrides this. This is for standalone
    /// launches where the client cannot send codel metadata.
    pub fn set_host(&mut self, host: HostContext) {
        self.host_explicit = true;
        self.identity_source = "host";
        if let Some(root) = host.root.as_deref() {
            self.workspaces.insert(canonical_key(root), host.clone());
        }
        if self.watch_enabled
            && let Some(root) = host.root.clone()
        {
            self.start_watcher(&root);
        }
        self.host = Some(host);
        self.workspace = None;
    }

    /// Write one JSON-RPC message.
    pub fn send(&mut self, message: Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.writer, &message)
            .map_err(|error| format!("serialize response: {error}"))?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| format!("write response: {error}"))?;
        self.writer
            .flush()
            .map_err(|error| format!("flush response: {error}"))
    }

    /// Handle one decoded JSON-RPC message.
    pub fn handle(&mut self, message: Value) -> Result<(), String> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if message.get("result").is_some() || message.get("error").is_some() {
                self.handle_response(&message);
            }
            return Ok(());
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "initialize" => self.initialize(id, &params),
            "notifications/initialized" => {
                self.initialized = true;
                if self.roots_capable && self.roots.is_empty() {
                    self.request_roots()?;
                }
                Ok(())
            }
            "notifications/roots/list_changed" => {
                self.roots.clear();
                self.workspace = None;
                if self.roots_capable {
                    self.request_roots()?;
                }
                Ok(())
            }
            "tools/list" => self.send(success(id, tool_definitions())),
            "tools/call" => {
                if !self.initialized {
                    return self.send(error(id, -32002, "server is not initialized"));
                }
                let result = self.dispatch_tool(&params);
                self.send(success(id, result))
            }
            "ping" => self.send(success(id, json!({}))),
            _ if id.is_null() => Ok(()),
            other => self.send(error(id, -32601, format!("unknown method: {other}"))),
        }
    }

    fn initialize(&mut self, id: Value, params: &Value) -> Result<(), String> {
        match parse_host_context(params) {
            Ok(Some(host)) => {
                if let Some(root) = host.root.as_deref() {
                    self.workspaces.insert(canonical_key(root), host.clone());
                }
                self.host = Some(host);
                self.host_explicit = true;
                self.identity_source = "host";
                self.workspace = None;
            }
            Ok(None) => {}
            Err(message) => return self.send(error(id, -32602, message)),
        }
        self.roots_capable = self.allow_roots
            && params
                .get("capabilities")
                .and_then(|capabilities| capabilities.get("roots"))
                .is_some();
        self.send(success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "codel-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": "Start with codel_search {query}. Every response includes workspace {workspaceId,label,root}; omit root/workspace to use the active workspace, or pass a workspace id from codel_workspace action=list. Then use codel_inspect {view: explain|tree|ast|compare, node}. Use codel_analyze for affected/path/impact/cycles/dead_code/god_nodes. Use codel_workspace {action: help} for examples, or action=refresh after edits.",
            }),
        ))
    }

    fn handle_response(&mut self, message: &Value) {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        if self.roots_request_id != Some(id) {
            return;
        }
        self.roots_request_id = None;
        if let Some(result) = message.get("result") {
            match roots_from_value(result) {
                Ok(roots) => {
                    self.roots = roots;
                    self.roots_error = None;
                    self.workspace = None;
                }
                Err(error) => self.roots_error = Some(error),
            }
        } else if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("roots/list failed");
            self.roots_error = Some(detail.to_owned());
        }
    }

    fn request_roots(&mut self) -> Result<(), String> {
        if self.roots_request_id.is_some() {
            return Ok(());
        }
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.roots_request_id = Some(id);
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "roots/list",
            "params": {},
        }))
    }

    fn dispatch_tool(&mut self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.call_tool(name, &arguments) {
            Ok(value) => tool_result(value, false),
            Err(message) => tool_result(json!({"error": message}), true),
        }
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let mut result = self.call_tool_inner(name, arguments)?;
        if let (Some(object), Ok(workspace)) = (result.as_object_mut(), self.workspace_identity()) {
            object.entry("workspace").or_insert_with(|| workspace);
        }
        Ok(result)
    }

    fn call_tool_inner(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        match name {
            "codel_search" => {
                let mode = arguments
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("name");
                if mode != "ast" {
                    return self.call_tool("codel_query", arguments);
                }
                let path = required_str(arguments, "path")?;
                let path = crate::ast::safe_relative(path)
                    .ok_or_else(|| "path must be workspace-relative".to_owned())?;
                let query = required_str(arguments, "query")?;
                let captures = arguments
                    .get("captures")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let max = optional_usize(arguments, "max", 50)?.clamp(1, 500);
                let workspace = self.workspace_arg(arguments)?;
                crate::ast::query(
                    workspace.descriptor.canonical_root(),
                    &path,
                    query,
                    &captures,
                    max,
                )
            }
            "codel_inspect" => {
                let view = arguments
                    .get("view")
                    .and_then(Value::as_str)
                    .unwrap_or("explain");
                let legacy = match view {
                    "tree" => "codel_tree",
                    "ast" => "codel_ast",
                    "compare" => "codel_compare",
                    _ => "codel_explain",
                };
                self.call_tool(legacy, arguments)
            }
            "codel_analyze" => {
                let analysis = arguments
                    .get("analysis")
                    .and_then(Value::as_str)
                    .unwrap_or("affected");
                let legacy = match analysis {
                    "path" => "codel_path",
                    "impact" => "codel_impact",
                    "cycles" => "codel_cycles",
                    "dead_code" => "codel_dead_code",
                    "god_nodes" => "codel_god_nodes",
                    _ => "codel_affected",
                };
                self.call_tool(legacy, arguments)
            }
            "codel_workspace" => {
                let action = arguments
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("status");
                if action == "help" {
                    return Ok(tool_help());
                }
                if action == "identity" {
                    return self.workspace_identity();
                }
                let legacy = match action {
                    "list" => "codel_workspaces",
                    "set" => "codel_set_workspace",
                    "use" => "codel_use_workspace",
                    "refresh" => "codel_refresh",
                    "scip_import" => "codel_scip_import",
                    "benchmark" => "codel_benchmark",
                    _ => "codel_status",
                };
                self.call_tool(legacy, arguments)
            }
            "codel_query" => {
                let query = required_str(arguments, "query")?;
                let limit = optional_usize(arguments, "limit", 20)?;
                let token_budget =
                    optional_usize(arguments, "token_budget", DEFAULT_ANSWER_TOKENS)?;
                if arguments.get("roots").is_some()
                    || arguments
                        .get("all_roots")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    return self.query_many(query, limit, token_budget, arguments);
                }
                let workspace = self.workspace_arg(arguments)?;
                let answer =
                    workspace
                        .graph
                        .query_budgeted(query, limit, AnswerBudget::new(token_budget));
                let root = workspace.descriptor.canonical_root();
                let hits = answer
                    .value
                    .iter()
                    .map(|hit| {
                        let mut value = json!(hit);
                        if let Some(context) = crate::context::enrich(&hit.node, root) {
                            value["context"] = context;
                        }
                        if let Some(ty) = workspace.symbol_types.get(hit.node.id.as_str()) {
                            value["type"] = json!(ty);
                        }
                        value
                    })
                    .collect::<Vec<_>>();
                Ok(json!({
                    "workspaceId": workspace.graph.workspace_id().to_string(),
                    "root": root.display().to_string(),
                    "hits": hits,
                    "estimatedTokens": answer.estimated_tokens,
                    "truncated": answer.truncated,
                }))
            }
            "codel_explain" => {
                let node = required_str(arguments, "node")?;
                let limit = optional_usize(arguments, "limit", 50)?;
                let token_budget =
                    optional_usize(arguments, "token_budget", DEFAULT_ANSWER_TOKENS)?;
                let workspace = self.workspace_arg(arguments)?;
                let id = resolve_node(&workspace.graph, node)?;
                let answer = workspace
                    .graph
                    .explain_budgeted(&id, limit, AnswerBudget::new(token_budget))
                    .ok_or_else(|| format!("node disappeared: {id}"))?;
                let mut explanation = json!(answer.value);
                if let Some(context) = crate::context::enrich(
                    &answer.value.node,
                    workspace.descriptor.canonical_root(),
                ) {
                    explanation["context"] = context;
                }
                if let Some(ty) = workspace.symbol_types.get(answer.value.node.id.as_str()) {
                    explanation["type"] = json!(ty);
                }
                Ok(json!({
                    "explanation": explanation,
                    "estimatedTokens": answer.estimated_tokens,
                    "truncated": answer.truncated,
                }))
            }
            "codel_impact" => {
                let mut changed = arguments
                    .get("changed")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if let Some(diff) = arguments.get("diff").and_then(Value::as_str) {
                    for line in diff.lines() {
                        let path = line
                            .strip_prefix("+++ b/")
                            .or_else(|| line.strip_prefix("--- a/"));
                        if let Some(path) = path
                            && path != "/dev/null"
                        {
                            changed.push(path.to_owned());
                        }
                    }
                }
                changed.sort();
                changed.dedup();
                if changed.is_empty() {
                    return Err("provide changed paths or a unified diff".to_owned());
                }
                let max_depth = optional_usize(arguments, "max_depth", 3)?.clamp(1, 12);
                let max_nodes = optional_usize(arguments, "max_nodes", 200)?.clamp(1, 1000);
                let workspace = self.workspace_arg(arguments)?;
                Ok(crate::diagnostics::impact(
                    &workspace.graph,
                    &changed,
                    max_depth,
                    max_nodes,
                ))
            }
            "codel_cycles" => {
                let max = optional_usize(arguments, "max", 20)?.clamp(1, 100);
                let kinds = parse_kinds(arguments)?.unwrap_or_else(|| {
                    vec![EdgeKind::Calls, EdgeKind::Imports, EdgeKind::ReferencesType]
                });
                let workspace = self.workspace_arg(arguments)?;
                Ok(crate::diagnostics::cycles(&workspace.graph, &kinds, max))
            }
            "codel_dead_code" => {
                let max = optional_usize(arguments, "max", 50)?.clamp(1, 500);
                let workspace = self.workspace_arg(arguments)?;
                Ok(crate::diagnostics::dead_code(&workspace.graph, max))
            }
            "codel_compare" => {
                let left = required_str(arguments, "left")?;
                let right = required_str(arguments, "right")?;
                let workspace = self.workspace_arg(arguments)?;
                let left = resolve_node(&workspace.graph, left)?;
                let right = resolve_node(&workspace.graph, right)?;
                let root = workspace.descriptor.canonical_root();
                let side = |id: &NodeId| {
                    let explanation = workspace.graph.explain(id, 512)?;
                    let mut value = json!(explanation.node);
                    if let Some(context) = crate::context::enrich(&explanation.node, root) {
                        value["context"] = context;
                    }
                    if let Some(ty) = workspace.symbol_types.get(explanation.node.id.as_str()) {
                        value["type"] = json!(ty);
                    }
                    value["incoming"] = json!(explanation.incoming.len());
                    value["outgoing"] = json!(explanation.outgoing.len());
                    Some(value)
                };
                Ok(json!({
                    "left": side(&left).ok_or_else(|| "left node disappeared".to_owned())?,
                    "right": side(&right).ok_or_else(|| "right node disappeared".to_owned())?,
                }))
            }
            "codel_query_ast" => {
                let path = required_str(arguments, "path")?;
                let path = crate::ast::safe_relative(path)
                    .ok_or_else(|| "path must be workspace-relative".to_owned())?;
                let query = required_str(arguments, "query")?;
                let captures = arguments
                    .get("captures")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let max = optional_usize(arguments, "max", 50)?.clamp(1, 500);
                let workspace = self.workspace_arg(arguments)?;
                crate::ast::query(
                    workspace.descriptor.canonical_root(),
                    &path,
                    query,
                    &captures,
                    max,
                )
            }
            "codel_ast" => {
                let path = required_str(arguments, "path")?;
                let path = crate::ast::safe_relative(path)
                    .ok_or_else(|| "path must be workspace-relative".to_owned())?;
                let depth = optional_usize(arguments, "depth", 8)?.min(16);
                let workspace = self.workspace_arg(arguments)?;
                crate::ast::parse(workspace.descriptor.canonical_root(), &path, depth)
            }
            "codel_tree" => {
                let node = required_str(arguments, "node")?;
                let depth = optional_usize(arguments, "depth", 4)?.clamp(1, 8);
                let workspace = self.workspace_arg(arguments)?;
                let id = resolve_node(&workspace.graph, node)?;
                let tree = containment_tree(
                    &workspace.graph,
                    &id,
                    depth,
                    workspace.descriptor.canonical_root(),
                );
                Ok(json!({"tree": tree}))
            }
            "codel_path" => {
                let from = required_str(arguments, "from")?;
                let to = required_str(arguments, "to")?;
                let max_depth = optional_usize(arguments, "max_depth", 8)?;
                let token_budget =
                    optional_usize(arguments, "token_budget", DEFAULT_ANSWER_TOKENS)?;
                let undirected = arguments
                    .get("undirected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let workspace = self.workspace_arg(arguments)?;
                let from = resolve_node(&workspace.graph, from)?;
                let to = resolve_node(&workspace.graph, to)?;
                Ok(
                    match workspace.graph.path_budgeted(
                        &from,
                        &to,
                        PathOptions {
                            max_depth,
                            undirected,
                        },
                        AnswerBudget::new(token_budget),
                    ) {
                        Some(answer) => json!({
                            "found": true,
                            "path": answer.value,
                            "estimatedTokens": answer.estimated_tokens,
                            "truncated": answer.truncated,
                        }),
                        None => json!({"found": false}),
                    },
                )
            }
            "codel_affected" => {
                let node = required_str(arguments, "node")?;
                let max_depth = optional_usize(arguments, "max_depth", 2)?;
                let token_budget =
                    optional_usize(arguments, "token_budget", DEFAULT_ANSWER_TOKENS)?;
                let kinds = parse_kinds(arguments)?;
                let workspace = self.workspace_arg(arguments)?;
                let id = resolve_node(&workspace.graph, node)?;
                let answer = workspace
                    .graph
                    .affected_budgeted(
                        &id,
                        AffectedOptions { max_depth, kinds },
                        AnswerBudget::new(token_budget),
                    )
                    .ok_or_else(|| format!("node disappeared: {id}"))?;
                Ok(json!({
                    "root": answer.value.root,
                    "nodes": answer.value.nodes,
                    "edges": answer.value.edges,
                    "truncated": answer.truncated,
                    "estimatedTokens": answer.estimated_tokens,
                }))
            }
            "codel_export" => {
                let format = arguments
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("markdown");
                let title = arguments
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("codel graph");
                let workspace = self.workspace_arg(arguments)?;
                if format == "json" {
                    return Ok(json!({
                        "format": format,
                        "content": workspace.graph.to_json().map_err(|error| error.to_string())?,
                    }));
                }
                let documents = workspace
                    .graph
                    .export(format, title)
                    .ok_or_else(|| format!("unknown export format: {format}"))?;
                if documents.len() == 1 {
                    let document = documents.into_iter().next().expect("one document");
                    Ok(json!({
                        "format": format,
                        "path": document.path,
                        "content": document.content,
                    }))
                } else {
                    Ok(json!({"format": format, "documents": documents}))
                }
            }
            "codel_benchmark" => {
                let workspace = self.workspace_arg(arguments)?;
                Ok(json!({
                    "rawBytes": workspace.raw_bytes,
                    "benchmark": workspace.graph.benchmark(workspace.raw_bytes),
                }))
            }
            "codel_scip_import" => {
                let index = required_str(arguments, "index")?;
                let workspace = self.workspace_arg(arguments)?;
                let root = workspace.descriptor.canonical_root().to_path_buf();
                let index = if std::path::Path::new(index).is_absolute() {
                    std::path::PathBuf::from(index)
                } else {
                    root.join(index)
                };
                let references = codel::load_scip_index(&index, &root)?;
                let external = codel::load_external_symbols(&index)?;
                let external_references = codel::load_external_references(&index, &root)?;
                let marker = root.join(".codel/scip-index");
                if let Some(parent) = marker.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&marker, index.display().to_string());
                self.scip_index = Some(index.clone());
                self.workspace = None;
                Ok(json!({
                    "index": index.display().to_string(),
                    "count": references.len(),
                    "externalSymbolCount": external.len(),
                    "externalSymbols": external,
                    "externalReferenceCount": external_references.len(),
                    "externalReferences": external_references,
                    "references": references,
                    "reload": true,
                }))
            }
            "codel_god_nodes" => {
                let limit = optional_usize(arguments, "limit", 10)?;
                let token_budget =
                    optional_usize(arguments, "token_budget", DEFAULT_ANSWER_TOKENS)?;
                let workspace = self.workspace_arg(arguments)?;
                let answer = workspace
                    .graph
                    .god_nodes_budgeted(limit, AnswerBudget::new(token_budget));
                Ok(json!({
                    "nodes": answer.value,
                    "estimatedTokens": answer.estimated_tokens,
                    "truncated": answer.truncated,
                }))
            }
            "codel_use_workspace" => {
                let root = required_str(arguments, "root")?;
                self.use_workspace(root)
            }
            "codel_workspaces" => self.list_workspaces(),
            "codel_set_workspace" => {
                let root = required_str(arguments, "root")?;
                self.set_workspace(root)
            }
            "codel_refresh" => {
                let previous = self.workspace.as_ref().map(|workspace| {
                    (
                        workspace
                            .graph
                            .nodes()
                            .map(|node| node.id.clone())
                            .collect::<std::collections::BTreeSet<_>>(),
                        workspace.graph.edges().len(),
                    )
                });
                self.dirty.store(false, Ordering::SeqCst);
                if let Some(root) = arguments
                    .get("root")
                    .or_else(|| arguments.get("workspace"))
                    .and_then(Value::as_str)
                {
                    let canonical = canonical_root(root)?;
                    self.scoped_graphs.remove(&canonical);
                    let active = self
                        .host
                        .as_ref()
                        .and_then(|host| host.root.as_deref())
                        .map(canonical_key);
                    if active.as_ref() == Some(&canonical) {
                        self.workspace = None;
                    }
                } else {
                    self.workspace = None;
                    self.scoped_graphs.clear();
                }
                let workspace = self.workspace_arg(arguments)?;
                let coverage = workspace.graph.coverage();
                let current = workspace
                    .graph
                    .nodes()
                    .map(|node| node.id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let (added, removed, nodes_delta) = if let Some((previous, _)) = &previous {
                    let added = current.difference(previous).count();
                    let removed = previous.difference(&current).count();
                    (
                        added,
                        removed,
                        current.len() as isize - previous.len() as isize,
                    )
                } else {
                    (current.len(), 0, current.len() as isize)
                };
                let edges_delta = previous
                    .as_ref()
                    .map_or(workspace.graph.edges().len() as isize, |(_, edges)| {
                        workspace.graph.edges().len() as isize - *edges as isize
                    });
                Ok(json!({
                    "refreshed": true,
                    "mode": "full",
                    "addedNodes": added,
                    "removedNodes": removed,
                    "nodeDelta": nodes_delta,
                    "edgeDelta": edges_delta,
                    "workspaceId": workspace.graph.workspace_id().to_string(),
                    "root": workspace.descriptor.canonical_root().display().to_string(),
                    "indexStatus": status_label(workspace.status),
                    "source": if workspace.loaded_from_store { "store" } else { "index" },
                    "coverage": coverage,
                    "nodes": workspace.graph.nodes().count(),
                    "edges": workspace.graph.edges().len(),
                    "parsedFiles": workspace.parsed_files,
                    "unsupportedFiles": workspace.unsupported_files,
                    "recoveredFiles": workspace.recovered_files,
                    "failedFiles": workspace.failed_files,
                }))
            }
            "codel_status" => {
                let workspace = self.workspace_arg(arguments)?;
                let coverage = workspace.graph.coverage();
                Ok(json!({
                    "workspaceId": workspace.graph.workspace_id().to_string(),
                    "root": workspace.descriptor.canonical_root().display().to_string(),
                    "indexStatus": status_label(workspace.status),
                    "source": if workspace.loaded_from_store { "store" } else { "index" },
                    "coverage": coverage,
                    "nodes": workspace.graph.nodes().count(),
                    "edges": workspace.graph.edges().len(),
                    "parsedFiles": workspace.parsed_files,
                    "unsupportedFiles": workspace.unsupported_files,
                    "recoveredFiles": workspace.recovered_files,
                    "failedFiles": workspace.failed_files,
                }))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }

    fn query_many(
        &mut self,
        query: &str,
        limit: usize,
        token_budget: usize,
        arguments: &Value,
    ) -> Result<Value, String> {
        let mut roots: Vec<String> = if let Some(values) = arguments.get("roots") {
            values
                .as_array()
                .ok_or_else(|| "roots must be an array of paths".to_owned())?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "roots entries must be strings".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            self.workspaces
                .keys()
                .map(|path| path.display().to_string())
                .collect()
        };
        if roots.is_empty()
            && let Some(root) = self.host.as_ref().and_then(|host| host.root.as_deref())
        {
            roots.push(root.display().to_string());
        }
        let mut seen = BTreeMap::new();
        roots.retain(|root| {
            let key = canonical_key(Path::new(root));
            seen.insert(key, ()).is_none()
        });
        if roots.is_empty() {
            return Err("no workspaces to query".to_owned());
        }

        let per_root_budget = (token_budget / roots.len()).max(1);
        let mut all_hits = Vec::new();
        let mut estimated = 0usize;
        let mut truncated = false;
        let mut summaries = Vec::new();
        for root in &roots {
            let selector = json!({"root": root});
            let workspace = self.workspace_arg(&selector)?;
            let answer =
                workspace
                    .graph
                    .query_budgeted(query, limit, AnswerBudget::new(per_root_budget));
            let workspace_id = workspace.graph.workspace_id().to_string();
            let canonical_root = workspace.descriptor.canonical_root().display().to_string();
            let mut hits = serde_json::to_value(&answer.value)
                .map_err(|error| format!("serialize query hits: {error}"))?;
            if let Some(items) = hits.as_array_mut() {
                for hit in items {
                    hit["root"] = json!(canonical_root);
                    hit["workspaceId"] = json!(workspace_id);
                    all_hits.push(hit.clone());
                }
            }
            estimated += answer.estimated_tokens;
            truncated |= answer.truncated;
            summaries.push(json!({
                "workspaceId": workspace_id,
                "root": canonical_root,
                "hits": answer.value.len(),
                "estimatedTokens": answer.estimated_tokens,
            }));
        }
        if all_hits.len() > limit {
            all_hits.truncate(limit);
            truncated = true;
        }
        Ok(json!({
            "scope": "multi",
            "roots": summaries,
            "hits": all_hits,
            "estimatedTokens": estimated,
            "truncated": truncated,
        }))
    }

    fn workspace_arg(&mut self, arguments: &Value) -> Result<&WorkspaceGraph, String> {
        let Some(selector) = arguments
            .get("root")
            .or_else(|| arguments.get("workspace"))
            .and_then(Value::as_str)
        else {
            return self.workspace();
        };
        if self
            .host
            .as_ref()
            .is_some_and(|host| host.workspace_id.to_string() == selector)
        {
            return self.workspace();
        }
        if let Some((canonical, host)) = self
            .workspaces
            .iter()
            .find(|(_, host)| host.workspace_id.to_string() == selector)
            .map(|(root, host)| (root.clone(), host.clone()))
        {
            let graph = load_workspace(&host, &canonical, self.scip_index.as_deref())?;
            self.scoped_graphs.insert(canonical.clone(), graph);
            return self
                .scoped_graphs
                .get(&canonical)
                .ok_or_else(|| "scoped workspace is unavailable".to_owned());
        }
        let canonical = canonical_root(selector)?;
        let active = self
            .host
            .as_ref()
            .and_then(|host| host.root.as_deref())
            .map(canonical_key);
        if active.as_ref() == Some(&canonical) {
            return self.workspace();
        }
        let host = if let Some(host) = self.workspaces.get(&canonical) {
            host.clone()
        } else {
            let host = implicit_host(&canonical)?;
            self.workspaces.insert(canonical.clone(), host.clone());
            host
        };
        // Explicit roots are reloaded so a cached graph cannot bleed across
        // edits made while another root was active.
        let graph = load_workspace(&host, &canonical, self.scip_index.as_deref())?;
        self.scoped_graphs.insert(canonical.clone(), graph);
        self.scoped_graphs
            .get(&canonical)
            .ok_or_else(|| "scoped workspace is unavailable".to_owned())
    }

    fn workspace(&mut self) -> Result<&WorkspaceGraph, String> {
        if self.dirty.swap(false, Ordering::SeqCst) {
            self.workspace = None;
        }
        if self.workspace.is_none() {
            let host = self.ensure_host()?;
            let root = self.resolve_root(&host)?;
            if self.watch_enabled {
                self.start_watcher(&root);
            }
            self.workspace = Some(load_workspace(&host, &root, self.scip_index.as_deref())?);
        }
        self.workspace
            .as_ref()
            .ok_or_else(|| "workspace is unavailable".to_owned())
    }

    fn set_workspace(&mut self, root: &str) -> Result<Value, String> {
        let canonical = canonical_root(root)?;
        let host = implicit_host(&canonical)?;
        self.workspaces.insert(canonical.clone(), host.clone());
        if !self
            .roots
            .iter()
            .any(|candidate| canonical_key(candidate) == canonical)
        {
            self.roots.push(canonical.clone());
        }
        self.roots_error = None;
        self.activate(host, "tool");
        self.workspace_identity()
    }

    fn use_workspace(&mut self, root: &str) -> Result<Value, String> {
        let canonical = canonical_root(root)?;
        let host = if let Some(host) = self.workspaces.get(&canonical) {
            host.clone()
        } else if self
            .roots
            .iter()
            .any(|candidate| canonical_key(candidate) == canonical)
        {
            let host = implicit_host(&canonical)?;
            self.workspaces.insert(canonical.clone(), host.clone());
            host
        } else {
            return Err(format!(
                "unknown workspace root {}; call codel_set_workspace first",
                canonical.display()
            ));
        };
        self.activate(host, "tool");
        self.workspace_identity()
    }

    fn list_workspaces(&mut self) -> Result<Value, String> {
        if let Some(host) = self.host.clone()
            && let Some(root) = host.root.as_deref()
        {
            self.workspaces.entry(canonical_key(root)).or_insert(host);
        }
        let roots = self.roots.clone();
        for root in roots {
            let key = canonical_key(&root);
            if !self.workspaces.contains_key(&key)
                && let Ok(host) = implicit_host(&key)
            {
                self.workspaces.insert(key, host);
            }
        }
        let active = self
            .host
            .as_ref()
            .and_then(|host| host.root.as_deref())
            .map(|root| canonical_key(root).display().to_string());
        let workspaces: Vec<Value> = self
            .workspaces
            .iter()
            .map(|(root, host)| {
                json!({
                    "workspaceId": host.workspace_id.to_string(),
                    "root": root.display().to_string(),
                    "storeRoot": host.store_root.as_ref().map(|path| path.display().to_string()),
                    "active": active.as_deref() == Some(root.to_string_lossy().as_ref()),
                })
            })
            .collect();
        Ok(json!({"active": active, "workspaces": workspaces}))
    }

    fn activate(&mut self, host: HostContext, source: &'static str) {
        let root = host.root.clone();
        self.host = Some(host);
        self.host_explicit = true;
        self.identity_source = source;
        self.workspace = None;
        self.watcher = None;
        if self.watch_enabled
            && let Some(root) = root
        {
            self.start_watcher(&root);
        }
    }

    fn workspace_identity(&mut self) -> Result<Value, String> {
        let explicit = self.host_explicit;
        let host = self.ensure_host()?;
        let root = self.resolve_root(&host)?;
        let root_source = if self.identity_source == "tool" {
            "tool"
        } else if explicit {
            "host"
        } else if std::env::var_os("CODEL_ROOT").is_some() {
            "env"
        } else if self.roots.is_empty() {
            "cwd"
        } else {
            "mcp-roots"
        };
        Ok(json!({
            "workspaceId": host.workspace_id.to_string(),
            "label": root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| root.to_str().unwrap_or("workspace")),
            "root": root.display().to_string(),
            "storeRoot": host.store_root.as_ref().map(|path| path.display().to_string()),
            "source": self.identity_source,
            "rootSource": root_source,
        }))
    }

    fn ensure_host(&mut self) -> Result<HostContext, String> {
        if let Some(host) = &self.host {
            return Ok(host.clone());
        }
        let root = if let Some(root) = std::env::var_os("CODEL_ROOT") {
            PathBuf::from(root)
        } else {
            match self.roots.as_slice() {
                [] => {
                    let cwd = std::env::current_dir()
                        .map_err(|error| format!("cannot determine workspace root: {error}"))?;
                    eprintln!(
                        "codel-mcp: no host root or MCP roots; using process cwd {} (set CODEL_ROOT or send MCP roots)",
                        cwd.display()
                    );
                    cwd
                }
                [root, ..] => root.clone(),
            }
        };
        let host = implicit_host(&root)?;
        self.host = Some(host.clone());
        Ok(host)
    }

    fn resolve_root(&self, host: &HostContext) -> Result<PathBuf, String> {
        if let Some(root) = &host.root {
            if !self.roots.is_empty() {
                let canonical = std::fs::canonicalize(root)
                    .map_err(|error| format!("host root is not readable: {error}"))?;
                let listed = self.roots.iter().any(|candidate| {
                    std::fs::canonicalize(candidate).is_ok_and(|path| path == canonical)
                });
                if !listed {
                    return Err("host root is not among the MCP roots".to_owned());
                }
            }
            return Ok(root.clone());
        }
        if let Some(root) = std::env::var_os("CODEL_ROOT") {
            return Ok(PathBuf::from(root));
        }
        match self.roots.as_slice() {
            [] => Err(match &self.roots_error {
                Some(error) => format!("no workspace root; MCP roots failed: {error}"),
                None => "no workspace root; supply host context root, CODEL_ROOT, or MCP roots"
                    .to_owned(),
            }),
            [root] => Ok(root.clone()),
            _ => Err("multiple MCP roots; host context must select one".to_owned()),
        }
    }
}

/// Build a host context when the client did not send one.
///
/// The workspace id lives inside the worktree at .codel/workspace-id. It is
/// generated once, then reused across restarts. This keeps distinct worktrees
/// isolated without deriving identity from a path alone. An explicit host
/// context always wins.
fn canonical_root(root: &str) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(root)
        .map_err(|error| format!("workspace root is not readable: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!(
            "workspace root is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn canonical_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn implicit_host(root: &Path) -> Result<HostContext, String> {
    let workspace_id = implicit_workspace_id(root)?;
    let store_root = std::env::var_os("CODEL_STORE_ROOT")
        .map(PathBuf::from)
        .or_else(|| Some(root.join(".codel").join("store")));
    Ok(HostContext {
        workspace_id,
        root: Some(root.to_path_buf()),
        store_root,
    })
}

fn implicit_workspace_id(root: &Path) -> Result<WorkspaceId, String> {
    let marker = root.join(".codel").join("workspace-id");
    if let Ok(text) = std::fs::read_to_string(&marker) {
        return text
            .trim()
            .parse()
            .map_err(|error| format!("invalid {}: {error}", marker.display()));
    }
    let workspace_id = WorkspaceId::new();
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, workspace_id.to_string());
    Ok(workspace_id)
}

fn required_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument '{key}'"))
}

fn optional_usize(arguments: &Value, key: &str, default: usize) -> Result<usize, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|value| value.min(usize::MAX as u64) as usize)
            .ok_or_else(|| format!("argument '{key}' must be a non-negative integer")),
    }
}

fn containment_tree(graph: &CodeGraph, id: &NodeId, depth: usize, root: &Path) -> Option<Value> {
    let explanation = graph.explain(id, 512)?;
    let mut value = json!(explanation.node);
    if depth == 0 {
        return Some(value);
    }
    if let Some(context) = crate::context::enrich(&explanation.node, root) {
        value["context"] = context;
    }
    let children = explanation
        .outgoing
        .iter()
        .filter(|neighbor| neighbor.edge.kind == EdgeKind::Contains)
        .filter_map(|neighbor| containment_tree(graph, &neighbor.node.id, depth - 1, root))
        .collect::<Vec<_>>();
    value["children"] = Value::Array(children);
    Some(value)
}

fn tool_help() -> Value {
    json!({
        "start": "If you are unsure, call codel_workspace with action=help.",
        "workflows": [
            {
                "goal": "Find a function or file",
                "call": {"name": "codel_search", "arguments": {"query": "quick_sort"}}
            },
            {
                "goal": "See how a symbol is implemented and used",
                "call": {"name": "codel_inspect", "arguments": {"view": "explain", "node": "quick_sort"}}
            },
            {
                "goal": "See containment/tree structure",
                "call": {"name": "codel_inspect", "arguments": {"view": "tree", "node": "quick_sort", "depth": 3}}
            },
            {
                "goal": "Find a structural pattern",
                "call": {"name": "codel_search", "arguments": {"mode": "ast", "path": "src/lib.rs", "query": "(function_item name: (identifier) @name)"}}
            },
            {
                "goal": "Find callers/dependents",
                "call": {"name": "codel_analyze", "arguments": {"analysis": "affected", "node": "quick_sort"}}
            },
            {
                "goal": "Assess impact of changed files",
                "call": {"name": "codel_analyze", "arguments": {"analysis": "impact", "changed": ["src/sort.rs"]}}
            },
            {
                "goal": "Refresh after edits",
                "call": {"name": "codel_workspace", "arguments": {"action": "refresh"}}
            },
            {
                "goal": "Import SCIP",
                "call": {"name": "codel_workspace", "arguments": {"action": "scip_import", "index": "index.scip"}}
            }
        ]
    })
}

fn parse_kinds(arguments: &Value) -> Result<Option<Vec<EdgeKind>>, String> {
    let Some(value) = arguments.get("kinds") else {
        return Ok(None);
    };
    let array = value
        .as_array()
        .ok_or_else(|| "argument 'kinds' must be an array".to_owned())?;
    let mut kinds = Vec::with_capacity(array.len());
    for item in array {
        let name = item
            .as_str()
            .ok_or_else(|| "every kind must be a string".to_owned())?;
        kinds.push(match name {
            "contains" => EdgeKind::Contains,
            "calls" => EdgeKind::Calls,
            "imports" => EdgeKind::Imports,
            "inherits" => EdgeKind::Inherits,
            "implements" => EdgeKind::Implements,
            "references_type" => EdgeKind::ReferencesType,
            "accesses" => EdgeKind::Accesses,
            "writes" => EdgeKind::Writes,
            "scip" => EdgeKind::Scip,
            other => return Err(format!("unknown edge kind: {other}")),
        });
    }
    Ok(Some(kinds))
}

fn resolve_node(graph: &CodeGraph, value: &str) -> Result<NodeId, String> {
    if let Some(node) = graph.nodes().find(|node| node.id.as_str() == value) {
        return Ok(node.id.clone());
    }
    let hits = graph.query(value, 2);
    match hits.as_slice() {
        [] => Err(format!("no node matches '{value}'")),
        [hit] => Ok(hit.node.id.clone()),
        _ => Err(format!(
            "'{value}' is ambiguous: {}",
            hits.iter()
                .map(|hit| format!("{} ({})", hit.node.name, hit.node.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn status_label(status: Option<SourceIndexStatus>) -> &'static str {
    match status {
        None => "store",
        Some(SourceIndexStatus::Complete) => "complete",
        Some(SourceIndexStatus::Partial) => "partial",
        Some(SourceIndexStatus::Cancelled) => "cancelled",
        Some(SourceIndexStatus::Failed) => "failed",
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_owned());
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use codel::WorkspaceId;
    use serde_json::json;
    use tempfile::tempdir;

    fn last_value(server: &McpServer<Vec<u8>>) -> Value {
        let text = String::from_utf8_lossy(&server.writer);
        let line = text.lines().last().expect("a response line");
        serde_json::from_str(line).expect("valid response JSON")
    }

    fn initialize(
        server: &mut McpServer<Vec<u8>>,
        workspace_id: Option<&str>,
        root: Option<&str>,
        store_root: Option<&str>,
    ) {
        let mut meta = json!({});
        if let Some(workspace_id) = workspace_id {
            meta["workspaceId"] = json!(workspace_id);
        }
        if let Some(root) = root {
            meta["root"] = json!(root);
        }
        if let Some(store_root) = store_root {
            meta["storeRoot"] = json!(store_root);
        }
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "_meta": {"codel": meta},
                },
            }))
            .expect("initialize");
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }))
            .expect("initialized");
    }

    fn tool_text(response: &Value) -> Value {
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool text");
        serde_json::from_str(text).expect("tool JSON")
    }

    #[test]
    fn initialize_and_list_tools() {
        let mut server = McpServer::new(Vec::new());
        initialize(&mut server, None, None, None);
        let response = last_value(&server);
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(
            response["result"]["instructions"]
                .as_str()
                .is_some_and(|text| text.contains("codel_search")),
            "{response}"
        );

        server
            .handle(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .expect("tools/list");
        let response = last_value(&server);
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"codel_search"));
        assert!(names.contains(&"codel_inspect"));
        assert!(names.contains(&"codel_analyze"));
        assert!(names.contains(&"codel_workspace"));
        assert!(names.contains(&"codel_export"));
    }

    #[test]
    fn indexes_a_workspace_and_answers_graph_queries() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() { beta(); }\nfn beta() {}\n",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "beta"}},
            }))
            .expect("query");
        let query = tool_text(&last_value(&server));
        assert_eq!(query["workspaceId"], workspace_id.to_string());
        assert_eq!(query["hits"][0]["node"]["name"], "beta");
        assert!(
            query["hits"][0]["context"]["snippet"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("fn beta")),
            "{query}"
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_affected", "arguments": {"node": "beta"}},
            }))
            .expect("affected");
        let affected = tool_text(&last_value(&server));
        assert!(
            affected["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["node"]["name"] == "alpha" && node["depth"] == 1)
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "codel_tree",
                    "arguments": {"node": "alpha", "depth": 2}
                },
            }))
            .expect("tree");
        let tree = tool_text(&last_value(&server));
        assert_eq!(tree["tree"]["name"], "alpha", "{tree}");
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "codel_ast",
                    "arguments": {"path": "src/lib.rs", "depth": 3}
                },
            }))
            .expect("ast");
        let ast = tool_text(&last_value(&server));
        assert!(
            ast["sexp"]
                .as_str()
                .is_some_and(|sexp| sexp.contains("function_item")),
            "{ast}"
        );
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "codel_compare",
                    "arguments": {"left": "alpha", "right": "beta"}
                },
            }))
            .expect("compare");
        let compare = tool_text(&last_value(&server));
        assert_eq!(compare["left"]["name"], "alpha");
        assert_eq!(compare["right"]["name"], "beta");

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": {
                    "name": "codel_query_ast",
                    "arguments": {
                        "path": "src/lib.rs",
                        "query": "(function_item name: (identifier) @name)"
                    }
                },
            }))
            .expect("ast query");
        let ast_query = tool_text(&last_value(&server));
        assert!(
            ast_query["matches"]
                .as_array()
                .is_some_and(|matches| !matches.is_empty()),
            "{ast_query}"
        );
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": {"name": "codel_cycles", "arguments": {}},
            }))
            .expect("cycles");
        assert_eq!(tool_text(&last_value(&server))["count"], 0);
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": {"name": "codel_dead_code", "arguments": {}},
            }))
            .expect("dead code");
        let dead = tool_text(&last_value(&server));
        assert!(
            dead["candidates"]
                .as_array()
                .is_some_and(|candidates| candidates.iter().any(|node| node["name"] == "alpha")),
            "{dead}"
        );
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": {
                    "name": "codel_impact",
                    "arguments": {"changed": ["src/lib.rs"]}
                },
            }))
            .expect("impact");
        let impact = tool_text(&last_value(&server));
        assert!(
            impact["changedNodes"]
                .as_array()
                .is_some_and(|nodes| !nodes.is_empty()),
            "{impact}"
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "codel_path", "arguments": {"from": "alpha", "to": "beta"}},
            }))
            .expect("path");
        let path = tool_text(&last_value(&server));
        assert_eq!(path["found"], true);
        assert_eq!(path["path"]["edges"][0]["class"], "inferred");

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {"name": "codel_status", "arguments": {}},
            }))
            .expect("status");
        let status = tool_text(&last_value(&server));
        assert_eq!(status["workspaceId"], workspace_id.to_string());
        assert!(status["nodes"].as_u64().unwrap_or(0) >= 2);
    }

    #[test]
    fn unknown_tools_return_a_tool_error() {
        let mut server = McpServer::new(Vec::new());
        initialize(&mut server, None, None, None);
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "nope", "arguments": {}},
            }))
            .expect("unknown tool");
        let response = last_value(&server);
        assert_eq!(response["result"]["isError"], true);
    }
    #[test]
    fn loads_graph_from_store_after_restart() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() { beta(); }
fn beta() {}
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let store_directory = tempdir().expect("store dir");
        let store_text = store_directory.path().display().to_string();
        let root_text = directory.path().display().to_string();

        let mut first = McpServer::new(Vec::new());
        initialize(
            &mut first,
            Some(&workspace_id.to_string()),
            Some(&root_text),
            Some(&store_text),
        );
        first
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "beta"}},
            }))
            .expect("first query");
        let query = tool_text(&last_value(&first));
        assert_eq!(query["hits"][0]["node"]["name"], "beta");

        let mut second = McpServer::new(Vec::new());
        initialize(
            &mut second,
            Some(&workspace_id.to_string()),
            Some(&root_text),
            Some(&store_text),
        );

        second
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_status", "arguments": {}},
            }))
            .expect("status");
        let status = tool_text(&last_value(&second));
        assert_eq!(status["source"], "store");

        second
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "gamma"}},
            }))
            .expect("query");
        let query = tool_text(&last_value(&second));
        assert_eq!(query["hits"].as_array().expect("hits").len(), 0);

        fs::write(directory.path().join("src/extra.rs"), b"fn gamma() {}\n").expect("new source");
        let mut third = McpServer::new(Vec::new());
        initialize(
            &mut third,
            Some(&workspace_id.to_string()),
            Some(&root_text),
            Some(&store_text),
        );
        third
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "codel_status", "arguments": {}},
            }))
            .expect("refresh status");
        let status = tool_text(&last_value(&third));
        assert_eq!(status["source"], "index");
        third
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "gamma"}},
            }))
            .expect("refresh query");
        let query = tool_text(&last_value(&third));
        assert!(
            query["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .any(|hit| hit["node"]["name"] == "gamma")
        );
    }

    #[test]
    fn indexes_typescript_workspace() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/app.ts"),
            b"class Service { run() { helper(); } }
function helper() {}
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "helper"}},
            }))
            .expect("query");
        let query = tool_text(&last_value(&server));
        assert!(
            query["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .any(|hit| hit["node"]["name"] == "helper")
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_affected", "arguments": {"node": "helper"}},
            }))
            .expect("affected");
        let affected = tool_text(&last_value(&server));
        assert!(
            affected["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["node"]["name"] == "run")
        );
    }
    #[test]
    fn type_references_create_graph_edges() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/app.ts"),
            b"class Bar {}
class Foo { value: Bar; }
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_affected", "arguments": {"node": "Bar", "kinds": ["references_type", "contains"]}},
            }))
            .expect("affected");
        let affected = tool_text(&last_value(&server));
        assert!(
            affected["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["node"]["name"] == "value"),
            "affected: {affected}"
        );
    }

    #[test]
    fn indexes_python_workspace() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/app.py"),
            b"class Base:
    pass
class Child(Base):
    def run(self):
        helper()
def helper():
    pass
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "helper"}},
            }))
            .expect("query");
        let query = tool_text(&last_value(&server));
        assert!(
            query["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .any(|hit| hit["node"]["name"] == "helper")
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_affected", "arguments": {"node": "Base"}},
            }))
            .expect("affected");
        let affected = tool_text(&last_value(&server));
        assert!(
            affected["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["node"]["name"] == "Child")
        );
    }

    #[test]
    fn exports_reports_and_benchmarks_the_graph() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() { beta(); }
fn beta() {}
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_export", "arguments": {"format": "markdown"}},
            }))
            .expect("export");
        let export = tool_text(&last_value(&server));
        assert!(
            export["content"]
                .as_str()
                .unwrap_or_default()
                .contains("God nodes")
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_benchmark", "arguments": {}},
            }))
            .expect("benchmark");
        let benchmark = tool_text(&last_value(&server));
        assert!(benchmark["rawBytes"].as_u64().unwrap_or(0) > 0);
        assert!(benchmark["benchmark"]["graphBytes"].as_u64().unwrap_or(0) > 0);
    }
    #[test]
    fn implicit_host_reuses_the_worktree_marker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = implicit_host(directory.path()).expect("first host");
        let second = implicit_host(directory.path()).expect("second host");
        assert_eq!(first.workspace_id, second.workspace_id);
        assert!(first.root.as_deref() == Some(directory.path()));
        assert_eq!(
            first.store_root.as_deref(),
            Some(directory.path().join(".codel").join("store").as_path())
        );
        assert!(
            directory
                .path()
                .join(".codel")
                .join("workspace-id")
                .is_file()
        );
    }
    #[test]
    fn set_workspace_switches_root_and_creates_marker() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let root = directory.path().to_str().expect("utf8");
        let value = server.set_workspace(root).expect("switch");
        assert_eq!(value["source"], "tool");
        assert_eq!(value["rootSource"], "tool");
        assert_eq!(
            value["root"].as_str(),
            Some(
                std::fs::canonicalize(directory.path())
                    .expect("canonical")
                    .to_str()
                    .expect("utf8")
            )
        );
        assert!(
            directory
                .path()
                .join(".codel")
                .join("workspace-id")
                .is_file()
        );
    }
    #[test]
    fn keeps_multiple_workspaces_and_switches_between_them() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let one = server
            .set_workspace(first.path().to_str().expect("utf8"))
            .expect("first workspace");
        let two = server
            .set_workspace(second.path().to_str().expect("utf8"))
            .expect("second workspace");
        assert_ne!(one["workspaceId"], two["workspaceId"]);

        let listed = server.list_workspaces().expect("list");
        assert_eq!(listed["workspaces"].as_array().expect("array").len(), 2);
        assert_eq!(listed["active"], two["root"]);

        let switched = server
            .use_workspace(first.path().to_str().expect("utf8"))
            .expect("switch back");
        assert_eq!(switched["workspaceId"], one["workspaceId"]);
        assert_eq!(switched["source"], "tool");
    }
    #[test]
    fn query_root_argument_is_isolated_from_the_active_workspace() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        std::fs::write(
            first.path().join("alpha.rs"),
            "fn alpha() {}
",
        )
        .expect("alpha");
        std::fs::write(
            second.path().join("beta.rs"),
            "fn beta() {}
",
        )
        .expect("beta");
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let first_root = first.path().to_str().expect("utf8");
        let second_root = second.path().to_str().expect("utf8");
        server.set_workspace(second_root).expect("active");

        let alpha = server
            .call_tool(
                "codel_query",
                &json!({"query": "alpha", "root": first_root}),
            )
            .expect("alpha query");
        assert!(
            alpha["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .any(|hit| hit["node"]["name"] == "alpha")
        );
        let beta = server
            .call_tool("codel_query", &json!({"query": "beta"}))
            .expect("beta query");
        assert!(
            beta["hits"]
                .as_array()
                .expect("hits")
                .iter()
                .any(|hit| hit["node"]["name"] == "beta")
        );
    }
    #[test]
    fn multi_root_query_tags_each_hit_with_its_workspace() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        std::fs::write(
            first.path().join("alpha.rs"),
            "fn alpha() {}
",
        )
        .expect("alpha");
        std::fs::write(
            second.path().join("beta.rs"),
            "fn alpha() {}
fn beta() {}
",
        )
        .expect("beta");
        let mut server = McpServer::new(Vec::new());
        server.disable_roots();
        let first_root = first.path().to_str().expect("utf8");
        let second_root = second.path().to_str().expect("utf8");
        server.set_workspace(first_root).expect("first");
        server.set_workspace(second_root).expect("second");

        let result = server
            .call_tool(
                "codel_query",
                &json!({
                    "query": "alpha",
                    "roots": [first_root, second_root],
                    "limit": 10
                }),
            )
            .expect("multi query");
        assert_eq!(result["scope"], "multi");
        let hits = result["hits"].as_array().expect("hits");
        assert!(hits.len() >= 2);
        let first_canonical = std::fs::canonicalize(first_root).expect("canonical");
        let second_canonical = std::fs::canonicalize(second_root).expect("canonical");
        assert!(
            hits.iter()
                .any(|hit| hit["root"] == first_canonical.to_str().expect("utf8"))
        );
        assert!(
            hits.iter()
                .any(|hit| hit["root"] == second_canonical.to_str().expect("utf8"))
        );
        assert!(hits.iter().all(|hit| hit["workspaceId"].is_string()));
    }
    #[test]
    fn scip_import_adds_graph_edges_end_to_end() {
        use scip::types::{Document, Index, Occurrence, SingleLineRange, SymbolRole};

        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        let source = "class A { /** doc */ int value; int run() { return value; } }
";
        fs::write(directory.path().join("src/A.java"), source).expect("source");
        let definition_start = source.find("value").expect("definition");
        let reference_start = source.rfind("value").expect("reference");

        let mut definition = Occurrence::new();
        definition.symbol = "scip java . . value".to_owned();
        definition.symbol_roles = SymbolRole::Definition as i32;
        let mut range = SingleLineRange::new();
        range.line = 0;
        range.start_character = definition_start as i32;
        range.end_character = (definition_start + 5) as i32;
        definition.set_single_line_range(range);

        let mut reference = Occurrence::new();
        reference.symbol = definition.symbol.clone();
        reference.symbol_roles = SymbolRole::ReadAccess as i32;
        let mut range = SingleLineRange::new();
        range.line = 0;
        range.start_character = reference_start as i32;
        range.end_character = (reference_start + 5) as i32;
        reference.set_single_line_range(range);

        let mut document = Document::new();
        document.relative_path = "src/A.java".to_owned();
        document.text = source.to_owned();
        document.occurrences = vec![definition, reference];
        let mut index = Index::new();
        index.documents.push(document);
        let index_path = directory.path().join("index.scip");
        scip::write_message_to_file(&index_path, index).expect("index");

        let workspace_id = WorkspaceId::new();
        let store = tempdir().expect("store");
        let store_root = store.path().display().to_string();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            Some(&store_root),
        );
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "codel_scip_import",
                    "arguments": {"index": index_path.display().to_string()}
                },
            }))
            .expect("import");
        assert_eq!(tool_text(&last_value(&server))["count"], 1);

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "codel_affected",
                    "arguments": {"node": "value", "kinds": ["accesses"]}
                },
            }))
            .expect("affected");
        let affected = tool_text(&last_value(&server));
        assert!(
            affected["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["node"]["name"] == "run"),
            "{affected}"
        );

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "codel_explain",
                    "arguments": {"node": "value"}
                },
            }))
            .expect("explain");
        let explanation = tool_text(&last_value(&server));
        assert!(
            explanation["explanation"]["context"]["snippet"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("int value")),
            "{explanation}"
        );
        assert!(
            explanation["explanation"]["context"]["documentation"]
                .as_str()
                .is_some_and(|text| text.contains("doc")),
            "{explanation}"
        );
        assert!(
            explanation["explanation"]["context"]["metrics"]["lines"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
        assert_eq!(explanation["explanation"]["type"], "int");
        let mut restored = McpServer::new(Vec::new());
        initialize(
            &mut restored,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            Some(&store_root),
        );
        restored
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "codel_explain",
                    "arguments": {"node": "value"}
                },
            }))
            .expect("restored explain");
        let restored_explanation = tool_text(&last_value(&restored));
        assert_eq!(restored_explanation["explanation"]["type"], "int");
        assert!(
            restored_explanation["explanation"]["incoming"]
                .as_array()
                .expect("incoming")
                .iter()
                .any(|edge| edge["edge"]["kind"] == "accesses"),
            "{restored_explanation}"
        );
    }
    #[test]
    fn refresh_reindexes_changed_files() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() {}
",
        )
        .expect("source");
        let workspace_id = WorkspaceId::new();
        let mut server = McpServer::new(Vec::new());
        initialize(
            &mut server,
            Some(&workspace_id.to_string()),
            Some(&directory.path().display().to_string()),
            None,
        );
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "alpha"}},
            }))
            .expect("query");
        assert_eq!(
            tool_text(&last_value(&server))["hits"][0]["node"]["name"],
            "alpha"
        );

        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() {}
fn gamma() {}
",
        )
        .expect("updated source");
        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "codel_refresh", "arguments": {}},
            }))
            .expect("refresh");
        assert_eq!(tool_text(&last_value(&server))["refreshed"], true);

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {"name": "codel_query", "arguments": {"query": "gamma"}},
            }))
            .expect("query after refresh");
        assert_eq!(
            tool_text(&last_value(&server))["hits"][0]["node"]["name"],
            "gamma"
        );
    }
    #[test]
    fn consolidated_tools_route_to_legacy_handlers() {
        let directory = tempdir().expect("temp dir");
        fs::create_dir_all(directory.path().join("src")).expect("src dir");
        fs::write(
            directory.path().join("src/lib.rs"),
            b"fn alpha() { beta(); }
fn beta() {}
",
        )
        .expect("source");
        let mut server = McpServer::new(Vec::new());
        let workspace_id = WorkspaceId::new().to_string();
        initialize(
            &mut server,
            Some(&workspace_id),
            Some(&directory.path().display().to_string()),
            None,
        );

        for (id, name, arguments) in [
            (2, "codel_search", json!({"query": "beta"})),
            (
                3,
                "codel_inspect",
                json!({"view": "explain", "node": "beta"}),
            ),
            (
                4,
                "codel_analyze",
                json!({"analysis": "affected", "node": "beta"}),
            ),
            (5, "codel_workspace", json!({"action": "status"})),
            (6, "codel_workspace", json!({"action": "help"})),
        ] {
            server
                .handle(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": {"name": name, "arguments": arguments},
                }))
                .expect("call");
            assert!(
                last_value(&server)["result"].is_object(),
                "{name}: {}",
                last_value(&server)
            );
        }

        server
            .handle(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "codel_search",
                    "arguments": {"query": "beta", "workspace": workspace_id}
                },
            }))
            .expect("workspace selector");
        let selected = tool_text(&last_value(&server));
        assert_eq!(selected["workspace"]["workspaceId"], workspace_id);
    }
}
