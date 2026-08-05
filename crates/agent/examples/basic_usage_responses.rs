use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::tools::FunctionTool;
use logging::auto_init_tracing;
use react::llm::vendor::DeepSeekVendor;
use std::sync::Arc;

// Self-contained DeepSeek Responses-API test. Override the credentials via
// env when running outside of CI/local test so the secret is not committed:
//   BOS_BASE_URL, BOS_API_KEY, BOS_MODEL
fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    auto_init_tracing();
    println!("=== DeepSeek Responses API Test ===");

    let base_url = env_or("BOS_BASE_URL", "https://api.deepseek.com");
    let api_key = env_or("BOS_API_KEY", "sk-06d3155ad2104fab8802e698c719c750");
    let model = env_or("BOS_MODEL", "deepseek-v4-flash");

    let mut provider = LlmProvider::new();
    println!("Registering deepseek: {} @ {}", model, base_url);
    let vendor = DeepSeekVendor::new(base_url, model.clone(), api_key);
    provider.register_vendor("deepseek".into(), Box::new(vendor));
    let llm = Arc::new(provider);

    let mut config = AgentConfig::default();
    config.model = model.clone();
    config.api_mode = "responses".to_string();
    config.reasoning_effort = Some("high".to_string());

    let mut agent = Agent::new(config, llm.clone());
    agent.add_tool(Arc::new(FunctionTool::new(
        "add",
        "Add two integers",
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"},
                "b": {"type": "integer"}
            },
            "required": ["a", "b"]
        }),
        |args| -> Result<serde_json::Value, react::tool::ToolError> {
            let a = args["a"].as_i64().unwrap_or(0);
            let b = args["b"].as_i64().unwrap_or(0);
            Ok(serde_json::json!({"sum": a + b}))
        },
    )));

    println!(
        "Agent created: model={} api_mode=responses reasoning_effort=high\n",
        model
    );

    println!("--- 1. run_simple() (Responses, non-stream) ---");
    match agent.run_simple("Say hi in one word").await {
        Ok(r) => println!("Response: {}\n", r),
        Err(e) => println!("Error: {}\n", e),
    }

    println!("--- 2. react() with tool (function_call round trip) ---");
    match agent.react("What is 3 + 4? Use the add tool.").await {
        Ok(r) => println!("Response: {}\n", r),
        Err(e) => println!("Error: {}\n", e),
    }

    println!("--- 3. stream() (Responses SSE) ---");
    use futures::StreamExt;
    let mut stream = agent.stream("Count from 1 to 3, one per line");
    while let Some(result) = stream.next().await {
        match result {
            Ok(token) => match token {
                agent::StreamToken::Text(text) => print!("{}", text),
                agent::StreamToken::ReasoningContent(text) => print!("[ Reasoning: {} ]", text),
                agent::StreamToken::ToolCall { name, args, .. } => {
                    print!("[ Tool: {} args: {} ]", name, args)
                }
                agent::StreamToken::Done => println!("\n[ Done ]"),
                agent::StreamToken::Usage(u) => println!(
                    "\n[ Usage ] prompt={} completion={} total={}",
                    u.prompt_tokens, u.completion_tokens, u.total_tokens
                ),
                agent::StreamToken::Stopped => println!("\n[ Stopped ]"),
            },
            Err(e) => println!("Error: {}", e),
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
