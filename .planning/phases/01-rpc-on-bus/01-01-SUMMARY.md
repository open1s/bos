# Plan 01-01 Summary: RPC Foundation

**Phase:** 01-rpc-on-bus  
**Plan:** 01  
**Status:** ✅ Completed  
**Date:** 2026-03-19

---

## What Was Built

This plan established the RPC (Remote Procedure Call) foundation on top of the existing Zenoh pub/sub bus. The implementation provides typed request/response communication with proper error handling and timeout support.

### Core Components

1. **Type Definitions** (`crates/bus/src/rpc/types.rs`)
   - `RpcResponse<T>`: Response envelope with `Ok(T)` and `Err { code, message }` variants
   - Helper methods: `ok()`, `err()`, `is_ok()`, `is_err()`, `into_result()`

2. **Error Types** (`crates/bus/src/rpc/error.rs`)
   - `RpcError`: Client-side errors (Timeout, NotFound, Serialization, Network)
   - `RpcServiceError`: Service-side errors carried in response
   - `From` implementations for `serde_json::Error` and `zenoh::Error`

3. **RPC Client** (`crates/bus/src/rpc/client.rs`)
   - `RpcClient`: Main client for calling remote services
   - `RpcClientBuilder`: Builder pattern with `service()`, `method()`, `timeout()`
   - `call()`: Returns single response, errors if multiple
   - `call_all()`: Returns all responses from multiple services
   - Clone semantics: clones drop session (matches QueryWrapper pattern)

---

## Key Design Decisions

### Topic Naming Convention
- Pattern: `/rpc/{service}/{method}`
- Example: `/rpc/calculator/add`

### Response Handling
- **Single response mode**: `call()` returns first `RpcResponse::Ok`, errors on multiple
- **Multi-response mode**: `call_all()` returns `Vec<T>` from all responders
- **Service errors**: Serialized in `RpcResponse::Err` with code and message

### Error Propagation
- **Client errors** (`RpcError`): Timeout, NotFound, Serialization, Network
- **Service errors** (`RpcServiceError`): Business logic errors with codes
- Clear separation between transport failures and service failures

### Builder Pattern
Following existing patterns in the codebase:
```rust
let client = RpcClient::builder()
    .service("calculator")
    .method("add")
    .timeout(Duration::from_secs(5))
    .build()?;
```

### Init Pattern
Consistent with `QueryWrapper`:
```rust
let mut client = RpcClient::new("calculator", "add");
client.init(session).await?;
```

---

## Files Created/Modified

### New Files
- `crates/bus/src/rpc/mod.rs` - Module exports and re-exports
- `crates/bus/src/rpc/error.rs` - Error type definitions
- `crates/bus/src/rpc/types.rs` - Response type definitions
- `crates/bus/src/rpc/client.rs` - RPC client implementation

### Modified Files
- `crates/bus/src/lib.rs` - Added `pub mod rpc` and re-exports

---

## Verification Results

### Acceptance Criteria
- ✅ `pub enum RpcError` defined with Timeout, NotFound, Serialization, Network variants
- ✅ `pub enum RpcResponse<T>` defined with Ok/Err variants
- ✅ `pub struct RpcClient` with `call()` and `call_all()` methods
- ✅ `pub struct RpcClientBuilder` with `service()`, `method()`, `timeout()` chaining
- ✅ `From<serde_json::Error>` and `From<zenoh::Error>` implemented
- ✅ Clone semantics: cloned RpcClient drops session
- ✅ All types re-exported from crate root

### Unit Tests
- `test_rpc_client_builder` - Builder pattern with all options
- `test_rpc_client_new` - Direct constructor
- `test_rpc_client_builder_missing_service` - Error on missing service
- `test_rpc_client_builder_missing_method` - Error on missing method
- `test_rpc_client_clone` - Clone behavior verification

---

## Next Steps

This foundation enables:
1. Service-side RPC handlers (RpcService)
2. Service discovery protocol
3. Typed service client generation
4. Integration with existing bus components

---

## Commits

1. `e73f47b` - Task 1: Create RPC module structure and type definitions
2. `8cd53a5` - Task 2: Implement RpcClient with builder, timeout, single/multi response
3. `68e3ef8` - Task 3: Wire RPC module into lib.rs exports
