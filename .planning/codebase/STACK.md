# BrainOS Technology Stack

## Languages & Runtime

- **Rust** (Edition 2021) - Primary language
- **Python** (via PyO3) - Optional Python bindings for config and bus crates

## Core Dependencies

### Async Runtime
- **tokio** 1.40 - Async runtime with full features
- **futures** 0.3 - Future utilities
- **tokio-stream** 0.1 - Stream utilities
- **async-stream** 0.3 - Async stream macros
- **async-channel** 2.5 - Async channels

### Serialization & Configuration
- **serde** 1.0 - Serialization framework
- **serde_json** 1.0 - JSON support
- **toml** 0.8 - TOML configuration
- **serde_yaml** 0.9 - YAML configuration
- **rkyv** 0.8 - Zero-copy serialization

### Error Handling
- **anyhow** 1.0 - Error context
- **thiserror** 2.0 - Error derive macros

### Logging & Tracing
- **tracing** 0.1 - Structured diagnostics
- **tracing-subscriber** 0.3 - Tracing subscribers
- **flexi_logger** - File-based logging (logging crate)
- **log** 0.4 - Logging facade

### Networking & Communication
- **zenoh** 1.8 - Distributed messaging
- **reqwest** 0.12 - HTTP client with JSON support

### Utilities
- **uuid** 1.0 - UUID generation (v4)
- **chrono** 0.4 - Date/time with serde support
- **fastrand** 2.0 - Random number generation
- **regex** 1.12 - Regular expressions
- **glob** 0.3 - File pattern matching
- **urlencoding** 2.1 - URL encoding/decoding

### Python Integration (Optional)
- **pyo3** 0.28 - Python bindings
- **pyo3-async-runtimes** 0.28 - Async runtime integration

### Development & Testing
- **criterion** 0.5 - Benchmarking with HTML reports
- **pprof** 0.13 - Profiling with flamegraph support
- **tempfile** 3.0 - Temporary file handling

## Build Configuration

### Release Profile
- `opt-level = 3` - Maximum optimization
- `lto = true` - Link-time optimization
- `codegen-units = 1` - Single codegen unit for better optimization

### Benchmark Profile
- Inherits release settings
- `debug = true` - Debug symbols for profiling

## Workspace Structure

```
workspace/
├── crates/
│   ├── config/    - Configuration management
│   ├── bus/       - Zenoh communication wrapper
│   ├── agent/     - Core agent infrastructure
│   └── logging/   - Logging utilities
└── Cargo.toml     - Workspace configuration
```

## Key Features

- **Async-first**: All crates use tokio for async operations
- **Zero-copy serialization**: rkyv for efficient data transfer
- **Distributed messaging**: Zenoh for inter-service communication
- **Python interoperability**: Optional PyO3 bindings
- **Structured logging**: Tracing-based diagnostics
- **Performance optimized**: Aggressive release profile settings
