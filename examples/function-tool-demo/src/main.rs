use agent::{Agent, AgentConfig};
use agent::tools::{FunctionTool, ToolError};
use std::sync::Arc;

/// Example demonstrating the FunctionTool wrapper and function registration API.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════╗");
    println!("║   Function Registration API Demo          ║");
    println!("╚══════════════════════════════════════╝\n");

    // Example 1: FunctionTool directly
    println!("--- Example 1: Creating FunctionTool manually ---\n");

    let add_tool = Arc::new(FunctionTool::numeric(
        "add",
        "Add two numbers",
        2,
        |args: &serde_json::Value| {
            let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
            let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
            Ok(serde_json::json!(a + b))
        },
    ));

    // Example 2: register_function()
    println!("--- Example 2: Using register_function() ----\n");

    let mock_llm = Arc::new(agent::llm::OpenAiClient::new(
        std::env::var("OPENAI_API_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "sk-test".to_string()),
    ));

    let config = AgentConfig {
        name: "demo-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: std::env::var("OPENAI_API_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key: std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "sk-test".to_string()),
        system_prompt: "You are a helpful assistant.".to_string(),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };

    let mut agent = Agent::new(config, mock_llm);

    agent.register_function(
        "multiply",
        "Multiply two numbers",
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number", "description": "First number"},
                "b": {"type": "number", "description": "Second number"}
            },
            "required": ["a", "b"]
        }),
        |args: &serde_json::Value| {
            let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
            let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
            Ok(serde_json::json!(a * b))
        },
    )?;

    println!("✓ Registered multiply function via register_function()\n");

    // Example 3: register_numeric_function()
    println!("--- Example 3: Using register_numeric_function() ---\n");

    agent.register_numeric_function(
        "subtract",
        "Subtract second from first",
        2,
        |args: &serde_json::Value| {
            let a = args["a"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("a required".to_string()))?;
            let b = args["b"].as_f64().ok_or_else(|| ToolError::ExecutionFailed("b required".to_string()))?;
            Ok(serde_json::json!(a - b))
        },
    )?;

    // Use with panic on error for simple scripts
    agent.add_tool(add_tool);

    println!("✓ Added add, multiply, subtract tools to agent\n");

    // Example 4: Direct tool execution
    println!("--- Example 4: Executing tools directly ---\n");

    let registry = agent.get_tool_registry().unwrap();
    let args = serde_json::json!({"a": 10.0, "b": 5.0});

    let result = registry.execute("add", &args).await?;
    println!("✓ add(10, 5) = {}", result.as_f64().unwrap());

    let result = registry.execute("multiply", &args).await?;
    println!("✓ multiply(10, 5) = {}", result.as_f64().unwrap());

    let result = registry.execute("subtract", &args).await?;
    println!("✓ subtract(10, 5) = {}", result.as_f64().unwrap());

    println!("\n--- Available Tools ---\n");
    for tool_name in registry.list() {
        println!("  - {}", tool_name);
    }


    println!("--- AgentBuilder integration ---\n");

    use agent::agent::config::AgentBuilder;

    println!("AgentBuilder now integrates with tool registry:");
    println!("  - Tools can be added via .with_tool()");
    println!("  - Tools defined in config are loaded automatically");
    println!("  - All tools are attached to Agent on build()");

    println!("\n✅ All examples completed successfully!");

    Ok(())
}
