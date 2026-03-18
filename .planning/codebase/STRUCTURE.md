# STRUCTURE.md

## Project Root

```
/Users/gaosg/Projects/bos/
├── .git/               # Git repository
├── .gitignore
├── .idea/              # IntelliJ IDEA config
├── .jj/                # Jujutsu (VCS) config
├── .planning/          # GSD planning directory
├── Cargo.lock
├── Cargo.toml           # Workspace manifest
├── crates/              # Rust crates
├── pytest.ini           # Python test config
└── target/             # Build output
```

## Crate Structure

```
crates/
├── bus/                 # Zenoh messaging wrapper
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs       # Entry point
│       ├── error.rs     # ZenohError
│       ├── publisher.rs # PublisherWrapper
│       ├── subscriber.rs# SubscriberWrapper<T>
│       ├── session.rs   # SessionManager
│       ├── query.rs     # QueryWrapper
│       ├── queryable.rs # QueryableWrapper
│       └── python.rs    # PyO3 bindings
│
└── config/              # Configuration loader
    ├── Cargo.toml
    └── src/
        ├── lib.rs       # Entry point
        ├── error.rs     # ConfigError
        ├── loader.rs    # ConfigLoader (main logic)
        ├── types.rs     # Types & enums
        └── python.rs    # PyO3 bindings
```

## Key File Locations

### Core Entry Points

| File | Purpose |
|------|---------|
| `crates/bus/src/lib.rs` | Bus crate public API |
| `crates/config/src/lib.rs` | Config crate public API |
| `Cargo.toml` | Workspace definition |

### Error Handling

| File | Error Type |
|------|------------|
| `crates/bus/src/error.rs` | `ZenohError` |
| `crates/config/src/error.rs` | `ConfigError` |

### Main Implementation Files

| File | Lines | Purpose |
|------|-------|---------|
| `crates/config/src/loader.rs` | ~1085 | ConfigLoader with all merge strategies |
| `crates/bus/src/session.rs` | ~206 | SessionManager with builder |
| `crates/bus/src/publisher.rs` | ~108 | Cached publisher wrapper |
| `crates/bus/src/query.rs` | ~297 | Query/queryable wrappers |

## Naming Conventions

### Rust Naming

| Pattern | Example |
|---------|---------|
| Structs | `PublisherWrapper`, `ConfigLoader` |
| Enums | `ZenohError`, `ConfigMergeStrategy` |
| Traits | `CustomConfigSource` |
| Builder methods | `with_strategy()`, `add_file()` |
| Async methods | `connect()`, `publish()`, `recv()` |
| Init methods | `init(&mut self, session)` |
| Boolean getters | `is_initialized()`, `is_connected()` |

### Module Organization

- `lib.rs` - Public API (re-exports)
- `error.rs` - Error types only
- `types.rs` - Type definitions, enums, structs
- `loader.rs`, `session.rs`, etc. - Feature modules

## Testing Structure

Inline tests within source files:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_foo() { ... }
    
    #[tokio::test]
    async fn test_bar() { ... }
}
```

### Test File Locations

| Module | Test Location |
|--------|---------------|
| `loader.rs` | Lines 650-1085 |
| `types.rs` | Lines 121-304 |
| `subscriber.rs` | Lines 96-122 |
| `query.rs` | Lines 111-297 |

## Python Bindings

Both crates have optional Python support via PyO3:

```
crates/bus/src/python.rs    # When feature "python-extension" enabled
crates/config/src/python.rs # When feature "python" enabled
```
