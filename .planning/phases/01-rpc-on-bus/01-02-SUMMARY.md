# Plan 01-02 Summary: RpcService + RpcDiscovery

**Phase:** 01-rpc-on-bus  
**Plan:** 02  
**Status:** ✅ Completed  
**Date:** 2026-03-19

---

## What Was Built

RpcService (typed RPC server) and RpcDiscovery (peer-to-peer service discovery) with full integration test coverage.

### Core Components

1. **RpcService** (`crates/bus/src/rpc/service.rs`)
   - `RpcHandler` trait: `async fn handle(&self, method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError>`
   - `RpcServiceBuilder`: fluent builder for `service_name` + optional `topic_prefix`
   - `RpcServiceUninit::init()`: creates QueryableWrapper with async handler, declares queryable
   - `RpcService::announce()`: publishes `DiscoveryInfo` to `rpc/services/{name}`
   - `RpcService::into_task()`: spawns background task for query handling
   - `RpcResponseEnvelope`: `{ status: "ok"|"err", ok: Option<Vec<u8>>, err: Option<RpcErrBody> }`

2. **RpcDiscovery** (`crates/bus/src/rpc/discovery.rs`)
   - `RpcDiscovery::announce()`: publishes DiscoveryInfo on `rpc/services/{name}`
   - `RpcDiscovery::discover().query()`: subscribes to `rpc/services/{name}` and collects within timeout
   - `DiscoveryQueryBuilder`: fluent builder with `.session()` and `.timeout()`
   - `DiscoveryInfo`: `{ service_name: String }`

3. **QueryableWrapper async handler support** (`crates/bus/src/queryable.rs`)
   - `with_handler()` now accepts async closures directly
   - Handler type: `Box<dyn Fn(Q) -> Pin<Box<dyn Future<Output=Result<R, ZenohError>> + Send>> + Send + Sync>`

4. **RpcClient updated** (`crates/bus/src/rpc/client.rs`)
   - Topic format: `rpc/{service}` (service-level, not method-level)
   - `call()` / `call_all()`: wraps payload in `RpcRequest { method, payload }` then encodes
   - `extract_single()`: decodes `RpcResponseEnvelope`, extracts inner value via `serde_json::from_slice`

### Integration Tests

- `test_rpc_full_cycle`: discovery → announce → call (echo + add) → response
- `test_discovery_pubsub`: pub/sub discovery info exchange
- `test_discovery_query`: discovery query with timeout
- `test_zenos_pubsub_same_session`: zenoh pub/sub baseline

---

## Key Design Decisions

### Discovery: Pub/Sub over Queryable

**Original plan**: Queryable-based discovery on `/rpc/discover/{service_name}`

**What we built**: Pub/sub-based discovery on `rpc/services/{service_name}`

**Reason**: Queryable-based discovery had a race condition — subscriber needed to register BEFORE the service published, but the plan test setup had them in the wrong order. Pub/sub is more robust: the subscriber just needs to be listening before the service announces.

### Response Envelope: Vec\<u8\> over serde_json::Value

**Original plan**: `ok: Option<serde_json::Value>`

**What we built**: `ok: Option<Vec<u8>>` with double-encoding

**Reason**: `serde_json::Value` caused serialization errors with `#[serde(tag = "status")]`. Instead, the handler returns raw bytes, the service encodes them directly into `ok`, and the client decodes the inner value with `serde_json::from_slice`.

### Client Payload Serialization

**Change**: Client now does `serde_json::to_vec(&payload)` before setting `RpcRequest.payload`, matching what the handler expects to decode.

---

## Files Modified

| File | Changes |
|------|---------|
| `crates/bus/src/rpc/service.rs` | RpcService, RpcHandler, RpcServiceBuilder, RpcRequest, RpcResponseEnvelope, RpcErrBody |
| `crates/bus/src/rpc/discovery.rs` | RpcDiscovery, DiscoveryInfo, DiscoveryQueryBuilder |
| `crates/bus/src/rpc/client.rs` | Topic format (service-level), RpcRequest wrapping, RpcResponseEnvelope decoding |
| `crates/bus/src/queryable.rs` | Async handler support via `with_handler()` |
| `crates/bus/src/rpc/mod.rs` | Module exports, RpcService/RpcDiscovery re-exports, integration tests |
| `crates/bus/src/lib.rs` | Public exports for RpcService, RpcDiscovery, RpcHandler |
| `Cargo.toml` (workspace + bus) | Added `bincode`, `async-channel`, `async-trait` |

---

## Verification Results

- `cargo check -p bus` → **PASS** (0 errors)
- `cargo test -p bus` → **37 passed, 0 failed**
- All acceptance criteria from plan tasks verified

---

## What Was NOT Built (Deviations from Original Plan)

1. **Queryable-based discovery** → Changed to pub/sub (see above)
2. **RpcDiscovery with QueryableWrapper responding to queries** → Simplified to publisher-only announce + subscriber-based discover
3. **Original `RpcResponseEnvelope` with `serde_json::Value`** → Changed to `Vec<u8>` (see above)
4. **RpcService storing Arc\<dyn RpcHandler\>** → Handler is wrapped in async closure, stored as boxed asyncFn
5. **Discovery response body** → Simplified to just `{ service_name }` instead of `{ topic_prefix, service_name }`
