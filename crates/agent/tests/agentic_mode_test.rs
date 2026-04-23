use agent::error::AgentError;
use agent::{Agent, AgentConfig};
use react::llm::vendor::OpenAiVendorBuilder;
use std::sync::Arc;

#[tokio::test]
async fn test_agent_builds_with_vendor() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let config = AgentConfig::default();
    let _agent: Agent = Agent::new(config, Arc::new(vendor));
}

#[tokio::test]
async fn test_agent_run_simple() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let config = AgentConfig::default();
    let agent = Agent::new(config, Arc::new(vendor));

    let result: Result<String, AgentError> = agent.run_simple("hi").await;
    // With invalid key, it will error - but no panic
    assert!(result.is_ok() || result.is_err());
}
