# TESTING.md

## Test Framework

- **Rust**: Built-in `#[test]`, `#[tokio::test]`, `tempfile`
- **Python**: pytest (see `pytest.ini`)

## Test Structure

Tests are **inline** within source files using `#[cfg(test)]` modules.

### Example: Inline Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sync_function() {
        assert_eq!(2 + 2, 4);
    }
    
    #[tokio::test]
    async fn test_async_function() {
        let result = async_function().await;
        assert!(result.is_ok());
    }
}
```

## Test Coverage by Module

### `crates/config/src/loader.rs` (650-1085)

**Unit Tests:**
- `test_config_format_from_path()` - Format detection
- `test_deep_merge_json()` - JSON merging logic
- `test_loader_*` - Loader functionality (15+ tests)

**Async Tests:**
- `test_loader_load_single_file()` - TOML loading
- `test_loader_load_yaml_file()` - YAML loading
- `test_loader_load_json_file()` - JSON loading
- `test_loader_reload()` - Cache invalidation
- `test_loader_override_strategy()` - Merge behavior
- `test_loader_first_strategy()` - First-wins behavior
- `test_loader_accumulate_strategy()` - Array accumulation
- `test_loader_mixed_sources()` - Multiple sources
- `test_loader_load_typed()` - Type deserialization
- `test_loader_invalid_*` - Error cases

### `crates/config/src/types.rs` (121-304)

**Format Tests:**
- `test_config_format_from_path_toml()`
- `test_config_format_from_path_yaml()`
- `test_config_format_from_path_json()`
- `test_config_format_from_path_unknown()`
- `test_config_format_from_path_case_insensitive()`

**Strategy Tests:**
- `test_config_merge_strategy_name()`
- `test_config_merge_strategy_default()`

**Source Tests:**
- `test_config_source_file()`
- `test_config_source_directory()`
- `test_config_source_inline()`
- `test_config_source_clone_*`
- `test_config_source_clone_custom_panics()`

**Metadata Tests:**
- `test_config_metadata_new()`
- `test_config_metadata_with_strategy()`
- `test_config_metadata_clone()`

### `crates/bus/src/subscriber.rs` (96-122)

- `test_subscriber_creation()`
- `test_subscriber_clone()`
- `test_subscriber_recv_timeout_before_init()` (async)

### `crates/bus/src/query.rs` (111-297)

- `test_query_wrapper_new()`
- `test_query_wrapper_clone()`
- `test_query_wrapper_init()` (async, multi_thread)
- `test_query_wrapper_query()` (async)
- `test_query_wrapper_query_with_timeout()` (async)
- `test_query_wrapper_integration()` (async) - Full pub/sub test
- `test_query_wrapper_empty_payload()` (async)
- `test_query_wrapper_large_payload()` (async)
- `test_query_wrapper_timeout_behavior()` (async)
- `test_query_wrapper_not_connected_error()` (async)
- `test_query_wrapper_clone_behavior()` (async)

## Mocking Strategy

No external mocking framework used. Tests use:
- Real Zenoh sessions for integration tests
- `tempfile` for config file tests
- Direct assertion on behavior

## Running Tests

```bash
# All tests
cargo test

# Specific crate
cargo test -p bus
cargo test -p config

# With output
cargo test -- --nocapture

# Specific test
cargo test test_loader_override_strategy
```

## Integration Testing

The `test_query_wrapper_integration()` test demonstrates full pub/sub:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_query_wrapper_integration() {
    // 1. Create session
    // 2. Create QueryableWrapper with handler
    // 3. Spawn task
    // 4. Create QueryWrapper
    // 5. Query and verify response
}
```

## Test Utilities

- `tempfile::tempdir()` - Temporary directories for config files
- `serde_json::json!()` - Inline JSON construction
- `tokio::time::Duration` - Timeout control
