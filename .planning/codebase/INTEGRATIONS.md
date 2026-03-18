# INTEGRATIONS.md

## Zenoh Messaging

The `bus` crate provides a wrapper around **Zenoh** (version 1.7.2) for distributed pub/sub communication.

### Configuration

```rust
pub struct ZenohConfig {
    pub mode: String,        // "peer" or "client"
    pub connect: Vec<String>, // Connection endpoints
    pub listen: Vec<String>, // Listen endpoints
    pub peer: Option<String>,
}
```

### Default Configuration

```rust
ZenohConfig {
    mode: "peer".to_string(),
    connect: vec![],
    listen: vec![],
    peer: None,
}
```

### Session Management

```rust
// Builder pattern
let manager = SessionManager::builder()
    .mode("peer")
    .connect("tcp/localhost:7447")
    .connect_many(vec!["tcp/host1:7447".into(), "tcp/host2:7447".into()])
    .build_and_connect()
    .await?;
```

## Configuration File Formats

The `config` crate supports loading from multiple formats:

| Format | Extension | Parser |
|--------|-----------|--------|
| TOML | `.toml` | `toml` crate |
| YAML | `.yaml`, `.yml` | `serde_yaml` |
| JSON | `.json` | `serde_json` |

### Config Sources

```rust
enum ConfigSource {
    File(String),      // Single file path
    Directory(String), // Directory with multiple config files
    Inline(Value),     // Inline JSON value
    Custom(Box<dyn CustomConfigSource>), // Custom source
}
```

### Config Merge Strategies

```rust
enum ConfigMergeStrategy {
    Override,    // Later sources override earlier
    DeepMerge,   // Recursive merge for nested objects
    First,      // Use first successful source
    Accumulate,  // Arrays accumulate, objects merge
}
```

## Python Integration

Both crates support Python bindings via PyO3:

### config crate
```toml
[features]
python = ["pyo3", "pyo3-async-runtimes"]
```

### bus crate
```toml
[features]
python-extension = ["pyo3", "pyo3-async-runtimes"]
```

## Serialization

Uses JSON (`serde_json`) for all message encoding:

```rust
pub struct JsonCodec;

impl JsonCodec {
    pub fn encode<T: serde::Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }
    
    pub fn decode<T: serde::de::DeserializeOwned>(&self, data: &[u8]) -> anyhow::Result<T> {
        Ok(serde_json::from_slice(data)?)
    }
}

pub static DEFAULT_CODEC: JsonCodec = JsonCodec;
```
