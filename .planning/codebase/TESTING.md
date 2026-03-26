# BrainOS Testing Strategy

## Testing Framework

### Primary Framework

- **Rust Built-in**: `#[test]` and `#[tokio::test]` attributes
- **Async Testing**: `tokio::test` for async test cases
- **Benchmarking**: `criterion` for performance benchmarks

### Test Organization

```
crates/
├── agent/
│   └── src/
│       ├── skills/
│       │   └── tests.rs        # Skill tests
│       └── mcp/
│           └── tests.rs        # MCP tests
├── config/
│   └── src/
│       └── lib.rs              # Config tests
└── logging/
    └── src/
        └── lib.rs              # Logging tests
```

## Test Types

### Unit Tests

**Purpose**: Test individual functions and methods in isolation

**Location**: In `#[cfg(test)]` modules within source files

**Example**:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_validation() {
        let tool = create_test_tool();
        let result = tool.validate_input(&valid_input);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tool_execution() {
        let tool = create_test_tool();
        let result = tool.execute(input).await.unwrap();
        assert_eq!(result, expected_output);
    }
}
```

### Integration Tests

**Purpose**: Test interactions between components

**Location**: In `tests/` directory at crate root

**Example**:

```rust
// tests/integration_test.rs
use agent::Agent;
use config::ConfigLoader;

#[tokio::test]
async fn test_agent_workflow() {
    let config = ConfigLoader::load("config.toml").unwrap();
    let agent = Agent::builder()
        .with_config(config)
        .build()
        .unwrap();

    let result = agent.process_message(message).await.unwrap();
    assert!(result.is_success());
}
```

### Async Tests

**Purpose**: Test async functionality

**Attribute**: `#[tokio::test]`

**Example**:

```rust
#[tokio::test]
async fn test_async_tool_execution() {
    let tool = create_async_tool();
    let result = tool.execute(input).await.unwrap();
    assert!(result.is_ok());
}
```

### Benchmark Tests

**Purpose**: Measure performance characteristics

**Framework**: `criterion`

**Location**: In `benches/` directory

**Example**:

```rust
// benches/tool_execution.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_tool_execution(c: &mut Criterion) {
    let tool = create_test_tool();
    let input = create_test_input();

    c.bench_function("tool_execution", |b| {
        b.iter(|| {
            tool.execute(black_box(input.clone())).await.unwrap()
        })
    });
}

criterion_group!(benches, bench_tool_execution);
criterion_main!(benches);
```

## Test Patterns

### Test Fixtures

**Purpose**: Reusable test data and utilities

**Pattern**:

```rust
#[cfg(test)]
mod fixtures {
    use super::*;

    pub fn create_test_tool() -> Tool {
        Tool::new("test_tool", "Test tool for testing")
    }

    pub fn create_test_input() -> Value {
        json!({"key": "value"})
    }

    pub fn create_test_agent() -> Agent {
        Agent::builder()
            .with_config(create_test_config())
            .build()
            .unwrap()
    }
}
```

### Mock Objects

**Purpose**: Isolate dependencies in tests

**Pattern**:

```rust
#[cfg(test)]
mod mocks {
    use super::*;

    pub struct MockLlmClient {
        responses: Vec<String>,
    }

    impl MockLlmClient {
        pub fn new(responses: Vec<String>) -> Self {
            Self { responses }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, prompt: &str) -> Result<String, LlmError> {
            Ok(self.responses[0].clone())
        }
    }
}
```

### Test Helpers

**Purpose**: Common test utilities

**Pattern**:

```rust
#[cfg(test)]
mod test_utils {
    use super::*;

    pub async fn setup_test_agent() -> Agent {
        let config = create_test_config();
        Agent::builder()
            .with_config(config)
            .build()
            .unwrap()
    }

    pub fn assert_success<T>(result: Result<T>) -> T {
        result.unwrap_or_else(|e| panic!("Expected success, got error: {:?}", e))
    }

    pub fn assert_error<T, E>(result: Result<T, E>) -> E
    where
        E: std::fmt::Debug,
    {
        result.unwrap_err()
    }
}
```

## Test Coverage

### Coverage Goals

- **Core functionality**: >80% coverage
- **Error paths**: >70% coverage
- **Public API**: 100% coverage

### Coverage Tools

```bash
# Install tarpaulin for coverage
cargo install cargo-tarpaulin

# Generate coverage report
cargo tarpaulin --out Html

# Generate coverage for specific crate
cargo tarpaulin -p agent --out Html
```

## Test Execution

### Run All Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run tests in release mode
cargo test --release
```

### Run Specific Tests

```bash
# Run specific test
cargo test test_tool_validation

# Run tests in specific module
cargo test agent::tools::tests

# Run tests matching pattern
cargo test tool_*
```

### Run Async Tests

```bash
# Run async tests
cargo test --test '*'

# Run specific async test
cargo test test_async_tool_execution
```

### Run Benchmarks

```bash
# Run benchmarks
cargo bench

# Run specific benchmark
cargo bench tool_execution

# Run benchmarks with output
cargo bench -- --output-format bencher
```

## Test Best Practices

### Test Naming

```rust
// Use descriptive test names
#[test]
fn test_tool_execution_with_valid_input_succeeds() {
    // Test implementation
}

#[test]
fn test_tool_execution_with_invalid_input_fails() {
    // Test implementation
}
```

### Test Structure

```rust
#[test]
fn test_feature() {
    // Arrange
    let input = create_test_input();
    let expected = create_expected_output();

    // Act
    let result = function_under_test(input);

    // Assert
    assert_eq!(result, expected);
}
```

### Test Isolation

```rust
// Each test should be independent
#[test]
fn test_feature_1() {
    let state = create_test_state();
    // Test feature 1
}

#[test]
fn test_feature_2() {
    let state = create_test_state();
    // Test feature 2 (independent of feature 1)
}
```

### Error Testing

```rust
#[test]
fn test_error_handling() {
    let result = operation_that_fails();
    assert!(result.is_err());

    match result {
        Err(Error::SpecificVariant) => {
            // Assert specific error variant
        }
        Err(e) => panic!("Unexpected error: {:?}", e),
        _ => panic!("Expected error"),
    }
}
```

## Mocking Strategies

### Dependency Injection

```rust
// Use trait objects for mocking
pub struct Agent<L: LlmClient> {
    llm_client: L,
}

impl<L: LlmClient> Agent<L> {
    pub fn new(llm_client: L) -> Self {
        Self { llm_client }
    }
}

// In tests
#[test]
fn test_with_mock() {
    let mock = MockLlmClient::new(vec!["response".to_string()]);
    let agent = Agent::new(mock);
    // Test agent
}
```

### Test Doubles

```rust
// Use test doubles for external dependencies
#[cfg(test)]
mod test_doubles {
    use super::*;

    pub struct TestConfig {
        pub test_value: String,
    }

    impl TestConfig {
        pub fn new() -> Self {
            Self {
                test_value: "test".to_string(),
            }
        }
    }
}
```

## Property-Based Testing

### Quickcheck

```rust
// Use quickcheck for property-based tests
use quickcheck::Arbitrary;
use quickcheck_macros::quickcheck;

#[derive(Clone, Debug)]
struct TestInput {
    value: String,
}

impl Arbitrary for TestInput {
    fn arbitrary(g: &mut Gen) -> Self {
        Self {
            value: String::arbitrary(g),
        }
    }
}

#[quickcheck]
fn test_property(input: TestInput) -> bool {
    // Test property that should hold for all inputs
    process(input.value).len() > 0
}
```

## Continuous Integration

### GitHub Actions

```yaml
# .github/workflows/test.yml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests
        run: cargo test --all-features
      - name: Run benchmarks
        run: cargo bench
      - name: Generate coverage
        run: cargo tarpaulin --out Html
```

## Test Documentation

### Test Documentation

```rust
/// Tests tool execution with valid input.
///
/// This test verifies that:
/// - Tool accepts valid input
/// - Tool executes successfully
/// - Tool returns expected output
#[test]
fn test_tool_execution_with_valid_input() {
    // Test implementation
}
```

### Test README

```markdown
# Testing

## Running Tests

```bash
cargo test
```

## Test Coverage

```bash
cargo tarpaulin --out Html
```

## Benchmarks

```bash
cargo bench
```
```

## Common Test Issues

### Async Test Timeouts

```rust
#[tokio::test]
#[timeout(5000)] // 5 second timeout
async fn test_long_running_operation() {
    // Test implementation
}
```

### Test Cleanup

```rust
#[test]
fn test_with_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();
    // Test implementation
    // temp_dir is automatically cleaned up
}
```

### Test Flakiness

```rust
// Use retries for flaky tests
#[test]
fn test_network_operation() {
    let mut attempts = 0;
    let max_attempts = 3;

    loop {
        attempts += 1;
        match try_network_operation() {
            Ok(result) => return,
            Err(e) if attempts < max_attempts => continue,
            Err(e) => panic!("Failed after {} attempts: {:?}", attempts, e),
        }
    }
}
```
