use agent::{Agent, AgentConfig, LlmResponse};
use brainos_common::MockLlmClient;
use std::sync::Arc;

#[tokio::test]
async fn demo_agent_new() {
    let config = AgentConfig {
        name: "test-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "Test agent.".to_string(),
        temperature: 0.7,
        max_tokens: Some(100),
        timeout_secs: 60,
    };

    let mock_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Response".to_string()),
        LlmResponse::Done,
    ]));

    let agent = Agent::new(config, mock_llm);
    assert_eq!(agent.config().name, "test-agent");
}

#[tokio::test]
async fn demo_agent_single_turn() {
    let config = AgentConfig {
        name: "single-turn-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "Test agent.".to_string(),
        temperature: 0.7,
        max_tokens: Some(100),
        timeout_secs: 60,
    };

    let mock_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Hello! This is a response.".to_string()),
        LlmResponse::Done,
    ]));

    let mut agent = Agent::new(config, mock_llm);
    let response = agent.run("Hello").await.unwrap();

    assert!(!response.is_empty());
    assert!(response.contains("Hello!"));
}

#[tokio::test]
async fn demo_agent_multi_turn() {
    let config = AgentConfig {
        name: "multi-turn-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "Test agent.".to_string(),
        temperature: 0.7,
        max_tokens: Some(100),
        timeout_secs: 60,
    };

    let mock_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Hi".to_string()),
        LlmResponse::Done,
        LlmResponse::Text("Answer 2".to_string()),
        LlmResponse::Done,
        LlmResponse::Text("Answer 3".to_string()),
        LlmResponse::Done,
    ]));

    let mut agent = Agent::new(config, mock_llm);

    let response1 = agent.run("Question 1?").await.unwrap();
    assert!(response1.contains("Hi"));

    let response2 = agent.run("Question 2?").await.unwrap();
    assert!(response2.contains("Answer 2"));

    let response3 = agent.run("Question 3?").await.unwrap();
    assert!(response3.contains("Answer 3"));
}

#[tokio::test]
async fn demo_agent_config() {
    let config = AgentConfig {
        name: "config-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "Test agent.".to_string(),
        temperature: 0.8,
        max_tokens: Some(500),
        timeout_secs: 120,
    };

    let mock_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Config validated".to_string()),
        LlmResponse::Done,
    ]));
    let agent = Agent::new(config.clone(), mock_llm);

    assert_eq!(agent.config().name, config.name);
    assert_eq!(agent.config().model, config.model);
    assert_eq!(agent.config().temperature, config.temperature);
    assert_eq!(agent.config().max_tokens, config.max_tokens);
    assert_eq!(agent.config().timeout_secs, config.timeout_secs);
}

#[tokio::test]
async fn demo_agent_mock() {
    let config = AgentConfig {
        name: "mock-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        system_prompt: "Test agent.".to_string(),
        temperature: 0.7,
        max_tokens: Some(100),
        timeout_secs: 60,
    };

    let expected_responses = vec![
        "First response",
        "Second response",
        "Third response",
    ];

    let mock_responses: Vec<LlmResponse> = expected_responses
        .iter()
        .flat_map(|text| {
            [
                LlmResponse::Text((*text).to_string()),
                LlmResponse::Done,
            ]
        })
        .collect();

    let mock_llm = Arc::new(MockLlmClient::new(mock_responses));
    let mut agent = Agent::new(config, mock_llm);

    let response1 = agent.run("Test 1").await.unwrap();
    assert_eq!(response1, expected_responses[0]);

    let response2 = agent.run("Test 2").await.unwrap();
    assert_eq!(response2, expected_responses[1]);

    let response3 = agent.run("Test 3").await.unwrap();
    assert_eq!(response3, expected_responses[2]);
}
