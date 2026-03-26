# BrainOS Code Conventions

## Code Style

### Rust Edition

- **Edition**: Rust 2021
- **Formatter**: rustfmt (default settings)
- **Linter**: clippy (recommended lints)

### Naming Conventions

#### Types

```rust
// Structs, Enums, Traits
pub struct AgentConfig { }
pub enum ToolError { }
pub trait Tool { }
```

#### Functions & Methods

```rust
// Functions and methods
pub fn execute_tool(&self) -> Result<Value> { }
pub async fn load_skill(&mut self) -> Result<Skill> { }
```

#### Variables

```rust
// Local variables
let tool_name = "example";
let max_retries = 3;

// Constants
const MAX_RETRIES: usize = 3;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
```

#### Modules

```rust
// Module declarations
pub mod agent;
pub mod tools;
pub mod skills;
```

### File Organization

#### Module Files

- **Public modules**: `mod.rs` in subdirectories
- **Flat modules**: `module_name.rs` in `src/`
- **Tests**: `tests.rs` or `#[cfg(test)]` modules

#### Example Structure

```
src/
├── lib.rs              # Public API
├── agent.rs            # Agent module
├── tools/
│   ├── mod.rs          # Tools public API
│   ├── registry.rs     # Tool registry
│   └── tests.rs        # Tool tests
```

## Code Patterns

### Error Handling

#### Use Result Types

```rust
// Always use Result for fallible operations
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let content = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}
```

#### Error Propagation

```rust
// Use ? operator for error propagation
pub async fn execute_tool(&self, tool: &Tool) -> Result<Value, ToolError> {
    let input = self.validate_input(tool)?;
    let result = tool.execute(input).await?;
    Ok(result)
}
```

#### Context with anyhow

```rust
// Use anyhow for error context in application code
use anyhow::{Context, Result};

pub fn run_agent() -> Result<()> {
    let config = load_config("config.toml")
        .context("Failed to load configuration")?;
    Ok(())
}
```

#### Typed Errors with thiserror

```rust
// Use thiserror for library error types
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}
```

### Async Patterns

#### Async Functions

```rust
// Use async for I/O operations
pub async fn fetch_data(url: &str) -> Result<String, reqwest::Error> {
    let response = reqwest::get(url).await?;
    let text = response.text().await?;
    Ok(text)
}
```

#### Async Traits

```rust
// Use async-trait for async trait methods
use async_trait::async_trait;

#[async_trait]
pub trait Tool: Send + Sync {
    async fn execute(&self, input: Value) -> Result<Value, ToolError>;
}
```

#### Channels

```rust
// Use async-channel for message passing
use async_channel::{bounded, Sender, Receiver};

let (tx, rx) = bounded(100);

// Sender
tx.send(message).await?;

// Receiver
while let Ok(message) = rx.recv().await {
    process(message);
}
```

### Builder Pattern

```rust
// Use builder pattern for complex construction
pub struct AgentBuilder {
    config: Option<AgentConfig>,
    llm_client: Option<LlmClient>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            config: None,
            llm_client: None,
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_llm_client(mut self, client: LlmClient) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub fn build(self) -> Result<Agent, AgentError> {
        Ok(Agent {
            config: self.config.ok_or(AgentError::MissingConfig)?,
            llm_client: self.llm_client.ok_or(AgentError::MissingLlmClient)?,
        })
    }
}
```

### Trait Objects

```rust
// Use trait objects for dynamic dispatch
pub type ToolBox = Box<dyn Tool + Send + Sync>;

pub struct ToolRegistry {
    tools: HashMap<String, ToolBox>,
}
```

### Serialization

```rust
// Use serde for serialization
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct AgentConfig {
    pub name: String,
    pub max_retries: usize,
    pub timeout: Duration,
}

// Custom serialization
#[derive(Serialize, Deserialize)]
pub struct Message {
    #[serde(with = "serde_with::json::as_string")]
    pub data: Value,
}
```

## Documentation

### Public API Documentation

```rust
/// Executes a tool with the given input.
///
/// # Arguments
///
/// * `tool` - The tool to execute
/// * `input` - The input data for the tool
///
/// # Returns
///
/// Returns the result of tool execution or an error.
///
/// # Errors
///
/// Returns an error if:
/// - The tool is not found
/// - The input is invalid
/// - Tool execution fails
///
/// # Examples
///
/// ```rust
/// let result = agent.execute_tool(&tool, input).await?;
/// ```
pub async fn execute_tool(&self, tool: &Tool, input: Value) -> Result<Value, ToolError> {
    // Implementation
}
```

### Module Documentation

```rust
//! Tool system for agent execution.
//!
//! This module provides:
//! - Tool registration and discovery
//! - Tool execution and validation
//! - Tool result handling
//!
//! # Example
//!
//! ```rust
//! use agent::tools::ToolRegistry;
//!
//! let registry = ToolRegistry::new();
//! registry.register(tool);
//! ```
```

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_execution() {
        let tool = create_test_tool();
        let result = tool.execute(input).await.unwrap();
        assert!(result.is_ok());
    }
}
```

### Integration Tests

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_workflow() {
        let agent = Agent::builder().build().unwrap();
        let result = agent.process_message(message).await.unwrap();
        assert!(result.is_success());
    }
}
```

### Test Organization

- **Unit tests**: In `#[cfg(test)]` modules
- **Integration tests**: In `tests/` directory
- **Test utilities**: In `test_utils.rs` or `common/mod.rs`

## Performance Guidelines

### Zero-Copy Serialization

```rust
// Use rkyv for zero-copy serialization
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
pub struct Message {
    pub data: Vec<u8>,
}
```

### Async I/O

```rust
// Use async I/O for network operations
pub async fn fetch_data(url: &str) -> Result<String> {
    let response = reqwest::get(url).await?;
    response.text().await
}
```

### Avoid Cloning

```rust
// Use references instead of cloning
pub fn process_data(data: &[u8]) -> Result<Vec<u8>> {
    // Process data without cloning
    Ok(data.to_vec())
}
```

## Security Guidelines

### Input Validation

```rust
// Always validate input
pub fn execute_tool(&self, input: &Value) -> Result<Value, ToolError> {
    self.validate_input(input)?;
    // Execute tool
}
```

### Error Messages

```rust
// Don't expose sensitive information in errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to load configuration")]
    LoadFailed,
    // Don't include file paths or secrets
}
```

### Secrets Management

```rust
// Never hardcode secrets
// Use environment variables or configuration
let api_key = std::env::var("API_KEY")?;
```

## Code Quality

### Clippy Lints

```bash
# Run clippy
cargo clippy --all-targets --all-features

# Fix clippy warnings
cargo clippy --fix --allow-dirty --allow-staged
```

### Rustfmt

```bash
# Format code
cargo fmt

# Check formatting
cargo fmt --check
```

### Documentation

```bash
# Generate documentation
cargo doc --all-features --no-deps

# Open documentation
cargo doc --open
```

## Common Patterns

### Option Handling

```rust
// Use ? for early returns
let value = option.ok_or(Error::NotFound)?;

// Use unwrap_or for defaults
let timeout = config.timeout.unwrap_or(DEFAULT_TIMEOUT);

// Use map for transformations
let result = option.map(|v| v * 2);
```

### Result Handling

```rust
// Use ? for error propagation
let result = operation()?;

// Use map_err for error conversion
let result = operation().map_err(|e| Error::Custom(e))?;

// Use and_then for chaining
let result = operation().and_then(|v| process(v))?;
```

### Iterator Patterns

```rust
// Use iterators for transformations
let results: Vec<_> = items
    .iter()
    .map(|item| process(item))
    .collect();

// Use filter for filtering
let filtered: Vec<_> = items
    .iter()
    .filter(|item| item.is_valid())
    .collect();
```

## Best Practices

### Use Type System

```rust
// Use newtypes for type safety
pub struct ToolName(String);

pub struct ToolId(uuid::Uuid);
```

### Prefer Composition

```rust
// Compose small functions
pub fn execute_tool(agent: &Agent, tool: &Tool) -> Result<Value> {
    let input = validate_input(tool)?;
    let result = tool.execute(input)?;
    Ok(result)
}
```

### Avoid Panics

```rust
// Use Result instead of panic
pub fn divide(a: i32, b: i32) -> Result<i32, DivisionError> {
    if b == 0 {
        return Err(DivisionError::DivisionByZero);
    }
    Ok(a / b)
}
```

### Use Send + Sync

```rust
// Make types thread-safe where appropriate
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, ToolBox>>>,
}
```
