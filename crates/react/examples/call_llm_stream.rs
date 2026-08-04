use config::ConfigLoader;
use futures::StreamExt;
use react::llm::vendor::{LlmRouter, NvidiaVendor, OpenRouterVendor};
use react::llm::{Content, LlmClient, LlmMessage, LlmRequest, ReactContext, ReactSession};

#[derive(Default)]
struct DummySession;

impl ReactSession for DummySession {
    fn push(&mut self, _msg: LlmMessage) {}
    fn history(&self) -> Option<&[LlmMessage]> {
        None
    }
}

#[derive(Default)]
struct DummyContext {
    tools: Vec<react::llm::LlmTool>,
}

impl ReactContext for DummyContext {
    fn session_id(&self) -> String {
        "dummy".to_string()
    }
    fn skills(&self) -> Option<&[react::llm::Skill]> {
        None
    }
    fn tools(&self) -> Option<&[react::llm::LlmTool]> {
        if self.tools.is_empty() {
            None
        } else {
            Some(&self.tools)
        }
    }
    fn rules(&self) -> Option<&[react::llm::Rule]> {
        None
    }
    fn instructions(&self) -> Option<&[react::llm::Instruction]> {
        None
    }
    fn add_tool(&mut self, tool: react::llm::LlmTool) {
        self.tools.push(tool);
    }

    fn notify_request(&self, _req: &LlmRequest) {}
    fn notify_response(&self, _resp: &react::llm::LlmResponse) {}
    fn notify_error(&self, _err: &react::llm::LlmError) {}
    fn on_chunk(&self, _chunk: &str) {}
    fn on_chunk_callback(&self) -> Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>> {
        None
    }
}

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

fn build_router(config: &serde_json::Value) -> LlmRouter<DummySession, DummyContext> {
    let mut router = LlmRouter::new();

    if let Some(cfg) = VendorConfig::from_nvidia(config) {
        let model = cfg.model.strip_prefix("nvidia/").unwrap_or(&cfg.model);
        println!("Registering nvidia: {} @ {}", model, cfg.base_url);
        let v = NvidiaVendor::new(cfg.base_url.clone(), model.to_string(), cfg.api_key.clone());
        router.register_vendor("nvidia".into(), Box::new(v));
    }

    if let Some(cfg) = VendorConfig::from_openrouter(config) {
        let model = cfg.model.strip_prefix("openrouter/").unwrap_or(&cfg.model);
        println!("Registering openrouter: {} @ {}", model, cfg.base_url);
        let v = OpenRouterVendor::new(cfg.base_url, model.to_string(), cfg.api_key);
        router.register_vendor("openrouter".into(), Box::new(v));
    }

    router
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut loader = ConfigLoader::new().discover();
    if loader.sources().is_empty() {
        return Err("No config. Create ~/.bos/conf/config.toml with [llm.nvidia]".into());
    }

    let config = loader.load().await?;
    let router = build_router(config);

    let nvidia_cfg = VendorConfig::from_nvidia(config).ok_or("no llm.nvidia config")?;
    let model = nvidia_cfg.model;

    let mut req = LlmRequest::new(model.clone())
        .temperature(0.7)
        .max_tokens(100)
        .reasoning_effort(react::llm::ReasoningEffort::Medium)
        .api_mode(react::llm::ApiMode::Responses);
    req.input = Content::text("Count from 1 to 5, one number per line");

    let mut session = DummySession::default();
    let mut ctx = DummyContext::default();
    // Function + hosted tools are sent on the Responses wire; streamed
    // function-call arguments are accumulated into a StreamToken::ToolCall.
    ctx.add_tool(react::llm::LlmTool::function(
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
    ));
    ctx.add_tool(react::llm::LlmTool::file_search(Some(serde_json::json!({
        "max_num_results": 5
    }))));

    let input_str = match &req.input {
        react::llm::Content::Text(s) => s.clone(),
        react::llm::Content::Parts(parts) => serde_json::to_string(parts).unwrap_or_default(),
    };
    println!("Streaming: {} with model {}", input_str, req.model);
    println!();

    let mut stream = router
        .stream_complete(None, req, &mut session, &mut ctx)
        .await?;

    while let Some(token) = stream.next().await {
        match token {
            Ok(t) => match t {
                react::llm::StreamToken::Text(text) => print!("{}", text),
                react::llm::StreamToken::ReasoningContent(text) => print!("[Think: {}] ", text),
                react::llm::StreamToken::ToolCall { name, args, .. } => {
                    print!("[Tool: {} args: {}] ", name, args)
                }
                react::llm::StreamToken::Done => println!("\n[Done]"),
                react::llm::StreamToken::Usage(u) => println!(
                    "\n[Usage] prompt={} completion={} total={}",
                    u.prompt_tokens, u.completion_tokens, u.total_tokens
                ),
                react::llm::StreamToken::Stopped => println!("\n[Stopped]"),
            },
            Err(e) => println!("Error: {:?}", e),
        }
    }

    Ok(())
}
