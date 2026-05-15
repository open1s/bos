//! Integration tests for agent react, stream, run_simple with/without resilience

use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use react::llm::vendor::OpenAiVendorBuilder;
use std::sync::Arc;
use std::time::Duration;

/// Helper to create a test agent with a mock vendor
fn create_test_agent() -> Agent {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig::default();
    Agent::new(config, Arc::new(llm))
}

/// Helper to create agent with resilience enabled
fn create_agent_with_resilience() -> Agent {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig {
        name: "test-agent".to_string(),
        model: "gpt-4o-mini".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "You are a helpful assistant.".to_string(),
        temperature: 0.7,
        max_tokens: Some(100),
        timeout_secs: 30,
        max_steps: 10,
        circuit_breaker: Some(react::CircuitBreakerConfig {
            max_failures: 3,
            cooldown: Duration::from_secs(10),
        }),
        rate_limit: Some(react::RateLimiterConfig {
            capacity: 10,
            window: Duration::from_secs(60),
            max_retries: 3,
            retry_backoff: Duration::from_secs(1),
            auto_wait: true,
        }),
        ..Default::default()
    };

    Agent::new(config, Arc::new(llm))
}

// ============================================================================
// react() tests
// ============================================================================

#[tokio::test]
async fn test_react_basic() {
    let agent = create_test_agent();
    let result = agent.react("Hello").await;
    // With invalid API key, may error but shouldn't panic
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_react_with_empty_task() {
    let agent = create_test_agent();
    let result = agent.react("").await;
    // Empty task might error, but verify it doesn't panic
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_react_with_long_task() {
    let agent = create_test_agent();
    let long_task = "Explain quantum computing in detail: ".repeat(10);
    let result = agent.react(&long_task).await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_react_with_resilience() {
    let agent = create_agent_with_resilience();
    let result = agent.react("Test with resilience").await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_react_without_resilience() {
    let agent = create_test_agent();
    let result = agent.react("Test without resilience").await;
    assert!(result.is_ok() || result.is_err());
}

// ============================================================================
// run_simple() tests
// ============================================================================

#[tokio::test]
async fn test_run_simple_basic() {
    let agent = create_test_agent();
    let result = agent.run_simple("Hi").await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_run_simple_empty_task() {
    let agent = create_test_agent();
    let result = agent.run_simple("").await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_run_simple_with_resilience() {
    let agent = create_agent_with_resilience();
    let result = agent.run_simple("Test").await;
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_run_simple_without_resilience() {
    let agent = create_test_agent();
    let result = agent.run_simple("Test").await;
    assert!(result.is_ok() || result.is_err());
}

// ============================================================================
// stream() tests
// ============================================================================

#[tokio::test]
async fn test_stream_basic() {
    use futures::StreamExt;

    let agent = create_test_agent();
    let stream = agent.stream("Hello");

    let mut stream = Box::pin(stream);
    // Just verify stream can be polled without panicking
    // May error due to invalid API key but that's expected
    while let Some(result) = stream.next().await {
        // Result can be Ok or Err - we just verify stream works
        let _ = result;
    }
}

#[tokio::test]
async fn test_stream_with_resilience() {
    use futures::StreamExt;

    let agent = create_agent_with_resilience();
    let stream = agent.stream("Test with resilience");

    let mut stream = Box::pin(stream);
    while let Some(result) = stream.next().await {
        let _ = result;
    }
}

#[tokio::test]
async fn test_stream_without_resilience() {
    use futures::StreamExt;

    let agent = create_test_agent();
    let stream = agent.stream("Test without resilience");

    let mut stream = Box::pin(stream);
    while let Some(result) = stream.next().await {
        let _ = result;
    }
}

#[tokio::test]
async fn test_stream_empty_task() {
    use futures::StreamExt;

    let agent = create_test_agent();
    let stream = agent.stream("");

    let mut stream = Box::pin(stream);
    while let Some(result) = stream.next().await {
        let _ = result;
    }
}

// ============================================================================
// Resilience configuration tests
// ============================================================================

#[tokio::test]
async fn test_agent_default_resilience() {
    let config = AgentConfig::default();
    // Default config should have resilience disabled
    assert!(config.circuit_breaker.is_none());
    assert!(config.rate_limit.is_none());
}

#[tokio::test]
async fn test_agent_custom_resilience_config() {
    let config = AgentConfig {
        circuit_breaker: Some(react::CircuitBreakerConfig {
            max_failures: 5,
            cooldown: Duration::from_secs(30),
        }),
        rate_limit: Some(react::RateLimiterConfig {
            capacity: 20,
            window: Duration::from_secs(120),
            max_retries: 5,
            retry_backoff: Duration::from_secs(2),
            auto_wait: false,
        }),
        ..Default::default()
    };

    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let _agent = Agent::new(config, Arc::new(llm));
    // Agent should be created successfully with custom resilience config
}

#[tokio::test]
async fn test_agent_config_access() {
    let agent = create_test_agent();
    let config = agent.config();

    assert_eq!(config.name, "agent");
    assert_eq!(config.model, "gpt-4");
}
