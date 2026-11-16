use agent::agent::agentic::{Agent, AgentConfig, LlmProvider};
use agent::tools::FunctionTool;
use config::ConfigLoader;
use react::llm::vendor::{NvidiaVendor, OpenRouterVendor};
use std::sync::Arc;

struct VendorConfig {
    model: String,
    base_url: String,
    api_key: String,
}

impl VendorConfig {
    fn from_nvidia(config: &serde_json::Value) -> Option<Self> {
        let nvidia = config.get("llm")?.get("nvidia")?;
        Some(Self {
            model: nvidia.get("model")?.as_str()?.to_string(),
            base_url: nvidia.get("base_url")?.as_str()?.to_string(),
            api_key: nvidia.get("api_key")?.as_str()?.to_string(),
        })
    }

    fn from_openrouter(config: &serde_json::Value) -> Option<Self> {
        let or = config.get("llm")?.get("openrouter")?;
        Some(Self {
            model: or.get("model")?.as_str()?.to_string(),
            base_url: or.get("base_url")?.as_str()?.to_string(),
            api_key: or.get("api_key")?.as_str()?.to_string(),
        })
    }
}

fn build_llm_provider(config: &serde_json::Value) -> LlmProvider {
    let mut provider = LlmProvider::new();

    if let Some(cfg) = VendorConfig::from_nvidia(config) {
        let model = cfg.model.strip_prefix("nvidia/").unwrap_or(&cfg.model);
        println!("Registering nvidia: {} @ {}", model, cfg.base_url);
        let v = NvidiaVendor::new(cfg.base_url.clone(), model.to_string(), cfg.api_key.clone());
        provider.register_vendor("nvidia".into(), Box::new(v));
    }

    if let Some(cfg) = VendorConfig::from_openrouter(config) {
        let model = cfg.model.strip_prefix("openrouter/").unwrap_or(&cfg.model);
        println!("Registering openrouter: {} @ {}", model, cfg.base_url);
        let v = OpenRouterVendor::new(cfg.base_url, model.to_string(), cfg.api_key);
        provider.register_vendor("openrouter".into(), Box::new(v));
    }

    provider
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Basic Agent Usage ===\n");

    let mut loader = ConfigLoader::new().discover();
    if loader.sources().is_empty() {
        return Err("No config. Create ~/.bos/conf/config.toml with [llm.nvidia]".into());
    }
    let config = loader.load_sync().map_err(|e| e.to_string())?;

    let provider = build_llm_provider(&config);

    let nvidia_cfg = VendorConfig::from_nvidia(&config).ok_or("no llm.nvidia config")?;

    let config = AgentConfig {
        model: nvidia_cfg.model.clone(),
        ..Default::default()
    };

    let llm = Arc::new(provider);
    let agent = Agent::new(config, llm.clone());

    println!("Agent created with model: {}\n", agent.config().model);

    println!("--- Example 1: run_simple() ---");
    match agent.run_simple("What is 2 + 2?").await {
        Ok(response) => println!("Response: {}\n", response),
        Err(e) => println!("Error: {}\n", e),
    }

    println!("--- Example 2: react() ---");
    match agent.react("Calculate 10 * 5 + 3").await {
        Ok(response) => println!("Response: {}\n", response),
        Err(e) => println!("Error: {}\n", e),
    }

    println!("--- Example 3: stream() ---");
    use futures::StreamExt;
    let mut stream = agent.stream("Hello, how are you?");
    while let Some(result) = stream.next().await {
        match result {
            Ok(token) => match token {
                agent::StreamToken::Text(text) => print!("{}", text),
                agent::StreamToken::ReasoningContent(text) => print!("[ Reasoning: {} ]", text),
                agent::StreamToken::ToolCall { name, args, .. } => {
                    print!("[ Tool: {} args: {} ]", name, args)
                }
                agent::StreamToken::ToolResult { name, ms, .. } => {
                    print!("[ ToolResult: {} {}ms ]", name, ms)
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

    println!("\n--- Example 4: Responses API + reasoning_effort + tools ---");
    // The Responses API (`/v1/responses`) sends `reasoning: {effort}` nested
    // in the request body. Registering a tool lets the model delegate work to
    // local functions during `react()`.
    let responses_config = AgentConfig {
        model: nvidia_cfg.model.clone(),
        api_mode: "responses".to_string(),
        reasoning_effort: Some("high".to_string()),
        ..Default::default()
    };

    let mut responses_agent = Agent::new(responses_config, llm);
    responses_agent.add_tool(Arc::new(FunctionTool::new(
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
    println!("api_mode=responses reasoning_effort=high");
    match responses_agent.react("What is 3 + 4?").await {
        Ok(response) => println!("Response: {}\n", response),
        Err(e) => println!("Error: {}\n", e),
    }

    println!("\n=== Done ===");
    Ok(())
}
