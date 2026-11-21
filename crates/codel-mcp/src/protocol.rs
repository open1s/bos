//! MCP JSON-RPC envelopes and the tool catalogue.

use serde_json::{Value, json};

/// Protocol version this server answers with.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Successful JSON-RPC response.
pub fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// JSON-RPC error response.
pub fn error(id: Value, code: i64, message: impl AsRef<str>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.as_ref()},
    })
}

/// The tools this server exposes.
pub fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "codel_search",
                "description": "START HERE: find declarations by name/path, or run a Tree-sitter structural query with mode=ast. Example: {\"query\":\"quick_sort\"}.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Name/path words, or a Tree-sitter query when mode=ast."},
                        "mode": {"type": "string", "enum": ["name", "ast"], "default": "name"},
                        "path": {"type": "string", "description": "Workspace-relative file for mode=ast."},
                        "captures": {"type": "array", "items": {"type": "string"}},
                        "max": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
                        "token_budget": {"type": "integer", "minimum": 1, "maximum": 32000, "default": 2000},
                        "root": {"type": "string", "description": "Optional root path; omit for active."},
                        "workspace": {"type": "string", "description": "Workspace id, label, or root; omit for active."},
                        "roots": {"type": "array", "items": {"type": "string"}},
                        "all_roots": {"type": "boolean", "default": false}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "codel_inspect",
                "description": "After codel_search, inspect one node. view=explain (default) returns edges/context; tree, ast, and compare are alternatives.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "view": {"type": "string", "enum": ["explain", "tree", "ast", "compare"], "default": "explain"},
                        "node": {"type": "string"},
                        "path": {"type": "string", "description": "Workspace-relative file for view=ast."},
                        "left": {"type": "string"},
                        "right": {"type": "string"},
                        "depth": {"type": "integer", "minimum": 0, "maximum": 16, "default": 4},
                        "root": {"type": "string", "description": "Optional root path; omit for active."},
                        "workspace": {"type": "string", "description": "Workspace id, label, or root; omit for active."}
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "codel_analyze",
                "description": "Graph analysis over an existing node or changed files. analysis=affected (default), path, impact, cycles, dead_code, god_nodes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "analysis": {"type": "string", "enum": ["affected", "path", "impact", "cycles", "dead_code", "god_nodes"], "default": "affected"},
                        "node": {"type": "string"},
                        "from": {"type": "string"},
                        "to": {"type": "string"},
                        "changed": {"type": "array", "items": {"type": "string"}},
                        "diff": {"type": "string"},
                        "kinds": {"type": "array", "items": {"type": "string"}},
                        "max_depth": {"type": "integer", "minimum": 1, "maximum": 12},
                        "max_nodes": {"type": "integer", "minimum": 1, "maximum": 1000},
                        "max": {"type": "integer", "minimum": 1, "maximum": 500},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10},
                        "token_budget": {"type": "integer", "minimum": 1, "maximum": 32000, "default": 2000},
                        "root": {"type": "string", "description": "Optional root path; omit for active."},
                        "workspace": {"type": "string", "description": "Workspace id, label, or root; omit for active."}
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "codel_workspace",
                "description": "Workspace lifecycle: help, status, list, set, use, refresh, SCIP import, benchmark. Call action=help first if unsure how to start.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["help", "status", "identity", "list", "set", "use", "refresh", "scip_import", "benchmark"], "default": "status"},
                        "root": {"type": "string", "description": "Optional root path; omit for active."},
                        "workspace": {"type": "string", "description": "Workspace id, label, or root; omit for active."},
                        "index": {"type": "string", "description": "SCIP file or directory for action=scip_import."}
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "codel_export",
                "description": "Export the graph as Markdown, JSON, wiki, Obsidian, HTML, SVG, GraphML, Cypher, or callflow.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "format": {"type": "string"},
                        "node": {"type": "string"},
                        "root": {"type": "string", "description": "Optional root path; omit for active."},
                        "workspace": {"type": "string", "description": "Workspace id, label, or root; omit for active."},
                        "token_budget": {"type": "integer", "minimum": 1, "maximum": 32000, "default": 2000}
                    },
                    "additionalProperties": false
                }
            }
        ]
    })
}
