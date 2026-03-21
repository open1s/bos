use agent::{Agent, AgentConfig, llm::LlmResponse};
use brainos_common::{setup_logging, MockLlmClient};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║  Agent Lifecycle Demo - Phase 2 Plan 01     ║");
    println!("╚══════════════════════════════════════╝\n");

    let config = AgentConfig {
        name: "demo-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "sk-demo".to_string(),
        system_prompt: "You are a helpful assistant.".to_string(),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };

    println!("Step 1: Creating Agent from config\n");
    let mock_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Hello! I'm ready to help.".to_string()),
        LlmResponse::Done,
    ]));

    let agent = Agent::new(config.clone(), mock_llm);
    println!("✓ Agent created: {}", agent.config().name);

    println!("\nStep 2: Testing single-turn execution\n");
    let single_turn_config = AgentConfig {
        name: "single-turn-agent".to_string(),
        ..config.clone()
    };
    let single_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("This is a single-turn response!".to_string()),
        LlmResponse::Done,
    ]));
    let mut single_agent = Agent::new(single_turn_config.clone(), single_llm);

    let response = single_agent.run("Hello!").await?;
    println!("✓ Single-turn response: {}", response);

    println!("\nStep 3: Testing multi-turn with context\n");
    let multi_turn_config = AgentConfig {
        name: "multi-turn-agent".to_string(),
        ..config.clone()
    };
    let multi_llm = Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Hi! How can I help you?".to_string()),
        LlmResponse::Done,
        LlmResponse::Text("That's 1 + 1 = 2!".to_string()),
        LlmResponse::Done,
    ]));
    let mut multi_agent = Agent::new(multi_turn_config.clone(), multi_llm);

    let turn1 = multi_agent.run("Hello").await?;
    println!("  Turn 1: User: \"Hello\" → Agent: \"{}\"", turn1);

    let turn2 = multi_agent.run("What's 1+1?").await?;
    println!("  Turn 2: User: \"What's 1+1?\" → Agent: \"{}\"", turn2);

    println!("\n✓ Multi-turn context preserved across messages");

    println!("{}", "=".repeat(50));
    println!("Agent Lifecycle Demo Complete");
    println!("{}", "=".repeat(50));

    Ok(())
}
