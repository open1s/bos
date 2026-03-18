# STACK.md

## Languages & Runtimes

- **Rust 2021 Edition** - Primary language
- **Python** - Via PyO3 bindings (optional feature `python`)
- **Tokio** - Async runtime

## Crates

### `crates/bus`
Zenoh communication wrapper for inter-component messaging.

| Dependency | Version | Purpose |
|------------|---------|---------|
| `zenoh` | 1.7.2 | Pub/sub messaging |
| `tokio` | 1.40 | Async runtime |
| `serde` | 1.0 | Serialization |
| `serde_json` | 1.0 | JSON codec |
| `anyhow` | 1.0 | Error handling |
| `thiserror` | 1.0 | Enum error derive |

### `crates/config`
Multi-format configuration loader.

| Dependency | Version | Purpose |
|------------|---------|---------|
| `serde` | 1.0 | Serialization |
| `serde_json` | 1.0 | JSON parsing |
| `toml` | 0.8 | TOML parsing |
| `serde_yaml` | 0.9 | YAML parsing |
| `anyhow` | 1.0 | Error handling |
| `thiserror` | 1.0 | Enum error derive |
| `tokio` | 1.40 | Async file I/O |
| `chrono` | 0.4 | Timestamp tracking |
| `pyo3` | 0.28 | Python bindings (optional) |

## Workspace Dependencies

```toml
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
serde_yaml = "0.9"
anyhow = "1.0"
thiserror = "1.0"
async-trait = "0.1"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1.0", features = ["v4"] }
futures = "0.3"
tokio-stream = "0.1"
chrono = { version = "0.4", features = ["serde"] }
fastrand = "2.0"
reqwest = { version = "0.12", features = ["json"] }
regex = "1.10"
glob = "0.3"
urlencoding = "2.1"
```

## Build Configuration

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
```

## Features

### bus crate
- `python-extension` - Enable PyO3 Python bindings

### config crate
- `python` - Enable PyO3 Python bindings with tokio runtime
