# CONVENTIONS.md

## Code Style

### Error Handling

Uses `thiserror` for enum errors with custom variants:

```rust
// bus crate
#[derive(Error, Debug)]
pub enum ZenohError {
    #[error("Session error: {0}")]
    Session(String),
    
    #[error("Not connected")]
    NotConnected,
}

// config crate
pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
}
```

### Async Patterns

```rust
// Init pattern
pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
    self.session = Some(session.clone());
    self.publisher = Some(session.declare_publisher(&self.topic).await?);
    Ok(())
}

// Builder pattern
pub fn builder() -> SessionManagerBuilder {
    SessionManagerBuilder::default()
}
```

### Clone Semantics

```rust
// Explicit clone behavior for clarity
impl Clone for PublisherWrapper {
    fn clone(&self) -> Self {
        self.clone_without_session()  // Explicit naming
    }
}

impl PublisherWrapper {
    pub fn clone_without_session(&self) -> Self { ... }
    
    pub async fn clone_with_session(&self, session: Arc<Session>) -> Result<Self, ZenohError> { ... }
}
```

## Module Documentation

```rust
//! BrainOS Zenoh communication wrapper
//!
//! Provides common abstractions for Zenoh-based inter-component communication.

use crate::{error::ZenohError, JsonCodec, Session};
```

## Data Structures

### Builder Pattern

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionManagerBuilder {
    mode: Option<String>,
    connect: Vec<String>,
    listen: Vec<String>,
    peer: Option<String>,
}

impl SessionManagerBuilder {
    pub fn mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }
    
    pub fn connect(mut self, endpoint: impl Into<String>) -> Self {
        self.connect.push(endpoint.into());
        self
    }
    
    pub fn build_config(self) -> ZenohConfig { ... }
}
```

### Option/Result Handling

```rust
// Prefer ? operator
pub async fn get_session(&self) -> Result<Arc<Session>, ZenohError> {
    let guard = self.session.read().await;
    guard.clone().ok_or(ZenohError::NotConnected)
}

// Timeout handling
pub async fn disconnect_with_timeout(&self, timeout: Duration) -> Result<(), ZenohError> {
    tokio::time::timeout(timeout, session.close())
        .await
        .map_err(|_| ZenohError::Timeout)?
        .map_err(ZenohError::from)?;
    Ok(())
}
```

## Serialization

Uses serde with JSON codec:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZenohConfig { ... }

impl JsonCodec {
    pub fn encode<T: serde::Serialize>(&self, value: &T) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(value)?)
    }
}
```

## Documentation Comments

Chinese comments in config crate (mixed with English):

```rust
/// 配置文件格式支持
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFormat {
    /// 深度合并：递归合并嵌套结构
    DeepMerge,
}
```

## Imports

Standard library ordering:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{error::ZenohError, Session, ZenohConfig};
```

## Testing Conventions

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_creation() {
        let wrapper = PublisherWrapper::new("test/topic");
        assert_eq!(wrapper.topic(), "test/topic");
    }
    
    #[tokio::test]
    async fn test_async_init() {
        // Setup with tokio::test
    }
}
```
