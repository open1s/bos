# CONCERNS.md

## Technical Debt

### Incomplete Python Bindings

Both crates have `python.rs` modules that are feature-gated but may not be fully implemented:

```rust
#[cfg(feature = "python")]
pub mod python;

#[cfg(feature = "python-extension")]
pub mod python;
```

**Status**: Modules declared but content unknown. May need implementation.

### Mixed Language Documentation

The `config` crate has Chinese comments mixed with English:

```rust
/// 配置文件格式支持
pub enum ConfigFormat { ... }

/// 配置合并策略
pub enum ConfigMergeStrategy { ... }
```

**Recommendation**: Standardize on English documentation for consistency.

## Performance Considerations

### Publisher Caching

The `PublisherWrapper` caches Zenoh publisher declaration:

```rust
pub struct PublisherWrapper {
    publisher: Option<CachedPublisher>,  // Cached declaration
}
```

This is correctly implemented for high-performance scenarios.

### Session Management

Uses `Arc<RwLock<Option<Arc<Session>>>>` pattern for thread-safe session access:

```rust
pub struct SessionManager {
    session: Arc<RwLock<Option<Arc<Session>>>>,
}
```

**Note**: Multiple clones share the same underlying session via `Arc`.

## Potential Issues

### Timeout Configuration

Hardcoded timeout in `session.rs`:

```rust
"accept_timeout": 100  // ms - Fast accept timeout
```

This is hardcoded rather than configurable. Consider making it adjustable.

### Clone Semantics

Publisher clone behavior is explicit but potentially confusing:

```rust
impl Clone for PublisherWrapper {
    fn clone(&self) -> Self {
        self.clone_without_session()  // Drops session!
    }
}
```

Users may expect `clone()` to preserve the session. Recommend clearer naming or documentation.

### Error Messages

Some error messages are in Chinese:

```rust
info!("开始加载配置，策略: {}", self.strategy.name());
debug!("配置源数量: {}", self.sources.len());
warn!("未指定任何配置源，返回空配置");
```

Consider using English for consistency with Rust ecosystem.

## Missing Features

### No Tokio Console Support

The codebase uses `tracing` but doesn't include `tracing-subscriber` with console/tokio-console support.

### No Connection Pooling

Each `SessionManager` manages a single session. No pooling for high-throughput scenarios.

### No Retry Logic

Failed connections or operations don't retry automatically. Applications must implement their own retry logic.

## Security Considerations

### No Authentication

Zenoh configuration has no authentication options exposed.

### No TLS Configuration

Transport security not configurable.

### Secrets in Config

Config files may contain sensitive data. No built-in secret management.

## Testing Gaps

### No Property-Based Tests

Tests use concrete examples but no property-based testing (e.g., `proptest`).

### No Benchmark Tests

No `#[bench]` tests for performance-critical paths.

### Integration Tests Require Running Zenoh

Integration tests (`test_query_wrapper_integration`) require a running Zenoh instance or scout mode.

## Documentation Gaps

### No Examples Directory

No `examples/` folder with runnable example code.

### API Documentation

Limited `rustdoc` comments on public API.

### README

No top-level README explaining the project purpose.

## Stability

### Early Stage

Version `0.1.0` indicates early development:

```toml
[workspace.package]
version = "0.1.0"
```

### Limited Production Usage

No indication of production deployments or battle-testing.

## Maintenance

### Jujutsu VCS

Project uses both Git (`.git/`) and Jujutsu (`.jj/`) for version control. Ensure team is aligned on VCS strategy.
