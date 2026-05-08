use config::ConfigLoader;
use react::llm::vendor::{LlmRouter, NvidiaVendor, OpenAiClient, OpenRouterVendor};
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

    fn from_openai(config: &serde_json::Value) -> Option<Self> {
        let openai = config.get("llm")?.get("openai")?;
        Some(Self {
            model: openai.get("model")?.as_str()?.to_string(),
            base_url: openai.get("base_url")?.as_str()?.to_string(),
            api_key: openai.get("api_key")?.as_str()?.to_string(),
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

    if let Some(cfg) = VendorConfig::from_openai(config) {
        if !cfg.model.contains('/') {
            println!("Registering openai: {} @ {}", cfg.model, cfg.base_url);
            let v = OpenAiClient::new(cfg.base_url.clone(), cfg.model.clone(), cfg.api_key.clone());
            router.register_vendor("openai".into(), Box::new(v));
        }
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
        return Err("No config. Create ~/.bos/conf/config.toml with [global_model]".into());
    }

    let config = loader.load().await?;
    let router = build_router(config);

    let nvidia_cfg = VendorConfig::from_nvidia(config)
        .ok_or("no llm.nvidia config")?;
    let model = nvidia_cfg.model;

    let req = LlmRequest {
        model: model.clone(),
        input: "Say hello in 3 words".into(),
        temperature: Some(0.7),
        max_tokens: Some(50),
        top_p: None,
        top_k: None,
    };

    let mut session = DummySession::default();
    let mut ctx = DummyContext::default();

    println!("Calling LLM with model: {}", req.model);
    let result = router.complete(req, &mut session, &mut ctx).await;
    match result {
        Ok(resp) => println!("Response: {:?}", resp),
        Err(e) => eprintln!("Error: {:?}", e),
    }

    println!("Done");
    Ok(())
}