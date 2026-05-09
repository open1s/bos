use config::ConfigLoader;
use futures::StreamExt;
use react::llm::vendor::{LlmRouter, NvidiaVendor, OpenRouterVendor};
use react::llm::{LlmClient, LlmMessage, LlmRequest, ReactContext, ReactSession};

#[derive(Default)]
struct DummySession;

impl ReactSession for DummySession {
    fn push(&mut self, _msg: LlmMessage) {}
    fn history(&self) -> Option<Vec<LlmMessage>> {
        None
    }
}

#[derive(Default)]
struct DummyContext;

impl ReactContext for DummyContext {
    fn session_id(&self) -> String {
        "dummy".to_string()
    }
    fn skills(&self) -> Option<Vec<react::llm::Skill>> {
        None
    }
    fn tools(&self) -> Option<Vec<react::llm::LlmTool>> {
        None
    }
    fn rules(&self) -> Option<Vec<react::llm::Rule>> {
        None
    }
    fn instructions(&self) -> Option<Vec<react::llm::Instruction>> {
        None
    }
    fn add_tool(&mut self, _tool: react::llm::LlmTool) {}

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

    let nvidia_cfg = VendorConfig::from_nvidia(config)
        .ok_or("no llm.nvidia config")?;
    let model = nvidia_cfg.model;

    let req = LlmRequest {
        model: model.clone(),
        input: "Count from 1 to 5, one number per line".into(),
        temperature: Some(0.7),
        max_tokens: Some(100),
        top_p: None,
        top_k: None,
    };

    let mut session = DummySession::default();
    let mut ctx = DummyContext::default();

    println!("Streaming: {} with model {}", req.input, req.model);
    println!();

    let mut stream = router.stream_complete(req, &mut session, &mut ctx).await?;

    while let Some(token) = stream.next().await {
        match token {
            Ok(t) => match t {
                react::llm::StreamToken::Text(text) => print!("{}", text),
                react::llm::StreamToken::ReasoningContent(text) => print!("[Think: {}] ", text),
                react::llm::StreamToken::ToolCall { name, args, .. } => {
                    print!("[Tool: {} args: {}] ", name, args)
                }
                react::llm::StreamToken::Done => println!("\n[Done]"),
            },
            Err(e) => println!("Error: {:?}", e),
        }
    }

    Ok(())
}
