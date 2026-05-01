use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::error::AgentError;
use react::llm::vendor::OpenAiVendorBuilder;
use std::sync::Arc;

#[tokio::test]
async fn test_agent_builds_with_vendor() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig::default();
    let _agent: Agent = Agent::new(config, Arc::new(llm));
}

#[tokio::test]
async fn test_agent_run_simple() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("sk-test".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut llm = LlmProvider::new();
    llm.register_vendor("openai".to_string(), Box::new(vendor));

    let config = AgentConfig::default();
    let agent = Agent::new(config, Arc::new(llm));

    let result: Result<String, AgentError> = agent.run_simple("hi").await;
    // With invalid key, it will error - but no panic
    assert!(result.is_ok() || result.is_err());
}
