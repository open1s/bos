# Phase 4: Unified Tool Demo - Context

**Gathered:** 2026-03-23
**Status:** Ready for planning

<domain>
## Phase Boundary

Create a comprehensive demo showing how to define and call tools from multiple sources (local, RPC, function, A2A) in the bos framework. The demo uses a single binary with `--role` flag to switch between coordinator and provider behaviors.

</domain>

<decisions>
## Implementation Decisions

### Local Tools
- **Pattern**: Full `Tool` trait implementation
- **Rationale**: For complex tools with state, show proper implementation pattern
- **Example**: `AddTool` struct implementing `Tool` trait with `name()`, `description()`, `json_schema()`, `execute()`

### RPC Tools
- **Pattern**: Service discovery integration
- **Rationale**: Dynamic discovery of RPC services, not hardcoded endpoints
- **Implementation**: Use `ZenohRpcDiscovery` to find RPC services on the bus automatically

### Function Tools
- **Pattern**: Both simple and complex examples
- **Simple**: `FunctionTool::numeric()` for add/multiply operations
- **Complex**: `FunctionTool::new()` with custom JSON schema for more complex functions

### A2A Tools
- **Pattern**: Full A2A workflow
- **Components**: Discovery + delegation + response handling
- **Implementation**: Use `A2AToolClient` and `A2AToolDiscovery`

### Demo Architecture
- **Structure**: Single binary with `--role` flag
- **Roles**: Coordinator (discovers and calls tools) vs Provider (exposes tools)
- **Shared code**: Single `src/lib.rs` with inline modules

### Discovery & Registration
- **Method**: `UnifiedToolRegistry` with auto-discovery
- **Sources**: Local, ZenohRpc, MCP, A2A — all auto-discovered
- **Namespace**: Tools automatically namespaced to prevent conflicts (e.g., `rpc/bob/add`)

### Error Handling
- **Pattern**: Unified error wrapper
- **Information**: Show error source (which tool type) and error message
- **Implementation**: Wrap all tool errors in common type with source context

### Claude's Discretion
- Specific tool names and schemas
- Number of example tools per type
- Startup sequence and timing
- Use `tokio::spawn` for concurrent tool discovery/registration (not blocking on arguments)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Tool System Core
- `crates/agent/src/tools/mod.rs` — Tool trait definition and exports
- `crates/agent/src/tools/function.rs` — FunctionTool wrapper patterns
- `crates/agent/src/tools/registry.rs` — ToolRegistry with namespace support

### Discovery System
- `crates/agent/src/tools/discovery/mod.rs` — ToolDiscovery trait and implementations
- `crates/agent/src/tools/discovery/zenoh_rpc.rs` — ZenohRpcDiscovery
- `crates/agent/src/tools/discovery/a2a.rs` — A2AToolDiscovery

### Unified Registry
- `crates/agent/src/tools/unified_registry.rs` — UnifiedToolRegistry aggregating all sources

### RPC & A2A Clients
- `crates/agent/src/tools/bus_client.rs` — BusToolClient for RPC calls
- `crates/agent/src/tools/a2a_client.rs` — A2AToolClient for agent delegation

### Agent Integration
- `crates/agent/src/agent/mod.rs` — Agent with tool registration methods

### Examples
- `crates/agent/tests/integration/mod.rs` — Integration tests with tool examples

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `FunctionTool` — Already exists in `tools/function.rs` with `numeric()` helper
- `UnifiedToolRegistry` — Already aggregates Local, ZenohRpc, MCP, A2A discovery
- `BusToolClient` — Ready-to-use RPC tool wrapper
- `A2AToolClient` — Ready-to-use A2A tool wrapper

### Established Patterns
- Tools implement `async_trait Tool` with `name()`, `description()`, `json_schema()`, `execute()`
- Tools wrapped in `Arc<dyn Tool>` for thread-safe sharing
- Registry uses namespace prefix `source/tool_name` format
- Discovery sources implement `ToolDiscovery` trait

### Integration Points
- Agent has `register_tool()`, `register_function()` methods
- AgentBuilder supports `with_tool()` for declarative tool config
- Tools can be loaded from TOML config via `[[tools]]` sections

</code_context>

<specifics>
## Specific Ideas

- Single binary that spawns both coordinator and provider roles as async tasks using `tokio::spawn`
- Use `UnifiedToolRegistry::new().with_zenoh_session(session).add_discovery_source(...)`
- Show tools from multiple sources in a single agent's tool list
- Unified error type shows which source (local/RPC/MCP/A2A) the error came from
- Both roles run simultaneously in the same binary for easier testing

</specifics>

<deferred>
## Deferred Ideas

- MCP tool integration (defer to later — show A2A focus for this demo)
- Multiple provider agents (keep simple: one binary switches roles)
- Complex skill system integration (focus on tool calling, not skill composition)

</deferred>

---

*Phase: 04-unified-tool-demo*
*Context gathered: 2026-03-23*