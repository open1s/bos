# Phase 01: RPC on Bus - Context

**Gathered:** 2026-03-18
**Status:** Ready for planning

<domain>
## Phase Boundary

Implement RPC (Remote Procedure Call) functionality on top of the existing Zenoh pub/sub bus in `crates/bus`. This adds request/response semantics with typed interfaces, error propagation, and service discovery.
</domain>

<decisions>
## Implementation Decisions

### Request/Response API Design

- **Strongly typed RPC** — ServiceClient<T> with typed methods, compile-time safety
- **Trait-based dispatch** — `client.call::<Method>(args)` with type-level method selection
- **Topic naming convention** — `/rpc/{service}/{method}` (flat with prefix)
- **Dedicated wrappers** — New `RpcClient` and `RpcService` wrappers, NOT extending QueryWrapper

### Response Handling

- **Single/Multiple configurable** — Both options available via API:
  - `rpc.call().await` → single T (errors if multiple)
  - `rpc.call_all().await` → Vec<T>
- **Timeout configurable** — Builder pattern: `RpcClient::builder().timeout(Duration::from_secs(5)).build()`
- **Multiple responders** — Return ALL responses from all responding services

### Error Propagation

- **Service errors** — Result envelope in response:
  ```rust
  pub enum RpcResponse<T> {
      Ok(T),
      Err(RpcServiceError { code: u32, message: String }),
  }
  ```
- **Client errors** — Dedicated `RpcError` enum:
  ```rust
  pub enum RpcError {
      Timeout,
      NotFound,      // No service responded
      Serialization(String),
      Network(String),
  }
  ```
- **Error transport** — Errors serialized in response body (JSON)

### Service Discovery

- **Discovery protocol** — Queryable on `/rpc/discover/{service_name}` returns endpoint info
- **Peer-to-peer** — Services announce themselves, no central registry
- **Auto-discover** — `client.discover::<CalculatorService>()`
  - First queries `/rpc/discover/calculator`
  - Caches endpoint for subsequent calls

### Claude's Discretion

- Exact trait definition syntax (async-trait vs native async fn in traits)
- Discovery response format/details
- Service health checking
- Connection pooling

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

- `.planning/codebase/ARCHITECTURE.md` — Existing bus patterns, wrapper structure
- `.planning/codebase/STACK.md` — Zenoh 1.7.2, serde_json, async patterns
- `crates/bus/src/query.rs` — Existing QueryWrapper/QueryableWrapper (foundation for RPC)
- `crates/bus/src/error.rs` — Error handling patterns to follow

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `QueryWrapper` — Foundation for RpcClient (sends queries)
- `QueryableWrapper` — Foundation for RpcService (handles queries)
- `JsonCodec` — Encoding/decoding for typed RPC
- `ZenohError` — Error pattern to follow

### Established Patterns
- Builder pattern for configuration (`SessionManagerBuilder`)
- Init pattern: `new()` → `init(session)` → use
- Clone semantics: clones drop session, explicit `clone_with_session()`

### Integration Points
- New RPC module: `crates/bus/src/rpc/` (new directory)
- Export from `crates/bus/src/lib.rs`
- Tests in `crates/bus/src/rpc/mod.rs` (inline tests)

</code_context>

<specifics>
## Specific Ideas

- gRPC-inspired API but simpler (no proto files, no codegen)
- Discovery inspired by Consul/DNS service discovery patterns
- Strong emphasis on compile-time type safety over runtime reflection

</specifics>

<deferred>
## Deferred Ideas

- Service versioning — future phase
- Authentication/authorization on RPC calls — future phase
- Load balancing across multiple service instances — future phase
- Circuit breaker pattern — future phase

</deferred>

---

*Phase: 01-rpc-on-bus*
*Context gathered: 2026-03-18*
