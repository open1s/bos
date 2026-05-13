# PyBrainOS Test Suite

Comprehensive Python tests for the BOS Python bindings (nbos).

## Setup

### Prerequisites

- Python 3.9+
- Rust 1.70+ (for building maturin extension)
- pytest and pytest-asyncio

### Installation

1. **Build the nbos extension**:
   ```bash
   maturin develop -m crates/nbos/Cargo.toml
   ```

2. **Install test dependencies**:
   ```bash
   pip install pytest pytest-asyncio
   ```

## Running Tests

### Run all tests:
```bash
pytest crates/nbos/tests/ -v
```

### Run specific test file:
```bash
pytest crates/nbos/tests/test_config_loader.py -v
pytest crates/nbos/tests/test_bus.py -v
pytest crates/nbos/tests/test_publisher_subscriber.py -v
pytest crates/nbos/tests/test_query_queryable.py -v
pytest crates/nbos/tests/test_caller_callable.py -v
pytest crates/nbos/tests/test_agent.py -v
pytest crates/nbos/tests/test_integration.py -v
```

### Run specific test:
```bash
pytest crates/nbos/tests/test_config_loader.py::TestConfigLoader::test_config_loader_creation -v
```

### Run with output:
```bash
pytest crates/nbos/tests/ -v -s
```

### Run with coverage:
```bash
pytest crates/nbos/tests/ --cov=nbos --cov-report=html
```

## Test Organization

### test_config_loader.py
Tests for `ConfigLoader` binding:
- Configuration loading from files and inline data
- Merge strategies (override, deep_merge, first, accumulate)
- Type conversion (strings, numbers, booleans, lists, dicts)
- Error handling

### test_bus.py
Tests for `Bus` and `BusConfig` binding:
- Bus creation and configuration
- Publishing text and JSON messages
- Multiple topics and concurrent publishes
- Large message handling

### test_publisher_subscriber.py
Tests for `Publisher` and `Subscriber` binding:
- Publisher-subscriber communication
- Multiple messages and topics
- JSON serialization
- Timeout handling
- Broadcasting to multiple subscribers
- Concurrent operations

### test_query_queryable.py
Tests for `Query` and `Queryable` binding:
- Request-response pattern via Query/Queryable
- Text and JSON payloads
- Handler invocation and input/output
- Multiple concurrent queries
- Service registry pattern

### test_caller_callable.py
Tests for `Caller` and `Callable` binding:
- RPC-style callable services
- Caller invocation
- Multiple concurrent calls
- Special characters and large payloads
- Comparison with Query pattern

### test_agent.py
Tests for `Agent` and `AgentConfig` binding:
- Agent creation and configuration
- Multiple agents on same/different buses
- Agent lifecycle
- Configuration parameters

### test_integration.py
Integration tests combining multiple components:
- Bus with Publisher/Subscriber
- Bus with Query/Queryable
- Bus with Caller/Callable
- Multiple services on same bus
- ConfigLoader with Bus
- Multiple agents with services
- JSON communication across stack
- Stress tests and workflow scenarios

## Test Patterns

### Async Tests
Uses `@pytest.mark.asyncio` decorator:
```python
@pytest.mark.asyncio
async def test_example():
    bus = await Bus.create(BusConfig())
    # test code
```

### Fixtures (when needed)
```python
@pytest.fixture
async def bus():
    return await Bus.create(BusConfig())
```

### Error Handling
```python
with pytest.raises(Exception):
    # code that should raise
```

## Performance Notes

- Tests use async/await for non-blocking operations
- Concurrent tests verify thread-safety
- Timeouts are generous (1000ms) to account for CI environment variability
- Integration tests combine multiple components to verify compatibility

## Troubleshooting

### "nbos module not found"
Make sure to build the extension first:
```bash
maturin develop -m crates/nbos/Cargo.toml
```

### Tests timeout
Some CI environments may require longer timeouts. Modify in test files if needed:
```python
await component.recv_with_timeout_ms(2000)  # Increase from 1000
```

### Async test errors
Ensure pytest.ini has `asyncio_mode = auto`:
```ini
[pytest]
asyncio_mode = auto
```

## Continuous Integration

The test suite is designed to run in CI/CD environments. All tests:
- Are isolated (no shared state between tests)
- Clean up resources properly
- Use reasonable timeouts
- Don't require network access
- Don't require external services

## Coverage Goals

- ConfigLoader: 100% coverage
- Bus: Core functionality coverage
- Publisher/Subscriber: Basic and advanced scenarios
- Query/Queryable: Request-response patterns
- Caller/Callable: RPC patterns
- Agent: Lifecycle and configuration
- Integration: Multi-component workflows

## Adding New Tests

1. Create test functions with `test_` prefix
2. Mark async tests with `@pytest.mark.asyncio`
3. Use descriptive names for test discovery
4. Include docstrings explaining what's tested
5. Clean up resources (close connections, etc.)
6. Add to appropriate test file or create new one

Example:
```python
@pytest.mark.asyncio
async def test_new_feature():
    """Test description"""
    bus = await Bus.create(BusConfig())
    # test implementation
    assert expected == actual
```

## See Also

- [README.md](../README.md) - API documentation
- [crates/nbos/Cargo.toml](../Cargo.toml) - Dependencies
- [pytest documentation](https://docs.pytest.org/)
- [pytest-asyncio](https://github.com/pytest-dev/pytest-asyncio)
