# Agent Examples

Examples demonstrating how to use the `agent` crate.

## Prerequisites

```bash
# Set your API key
export OPENAI_API_KEY=your-api-key
# Or use NVIDIA
export NVAPI_KEY=your-nvidia-key
```

## Examples

### basic_usage

Basic agent usage without resilience features.

```bash
cargo run --example basic_usage
```

Demonstrates:
- Creating an agent with default configuration
- Using `run_simple()` for simple tasks
- Using `react()` for full ReAct loop with tools
- Using `stream()` for streaming responses

### with_resilience

Agent with circuit breaker and rate limiter for production use.

```bash
cargo run --example with_resilience
```

Demonstrates:
- Adding circuit breaker to prevent cascading failures
- Adding rate limiter to control API usage
- Configuration of retry behavior
- How circuit breaker opens after failures

## Key Concepts

### Without Resilience (basic_usage)
- Simple setup with default config
- No circuit breaker or rate limiting
- Direct API calls without retry logic

### With Resilience (with_resilience)
```rust
let config = AgentConfig {
    circuit_breaker: Some(CircuitBreakerConfig {
        max_failures: 3,
        cooldown: Duration::from_secs(30),
    }),
    rate_limit: Some(RateLimiterConfig {
        capacity: 10,
        window: Duration::from_secs(60),
        max_retries: 3,
        retry_backoff: Duration::from_secs(2),
        auto_wait: true,
    }),
    ..Default::default()
};
```

## Running Tests

```bash
# Run integration tests
cargo test -p agent --test agent_integration_test

# Run all agent tests
cargo test -p agent
```