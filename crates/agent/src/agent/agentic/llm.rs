use super::*;

pub struct LlmProvider {
    inner: LlmRouter<AgentSession, AgentReactContext>,
}

impl LlmProvider {
    pub fn new() -> Self {
        Self {
            inner: LlmRouter::new(),
        }
    }

    pub fn register_vendor(
        &mut self,
        name: String,
        vendor: Box<dyn LlmClient<AgentSession, AgentReactContext>>,
    ) {
        self.inner.register_vendor(name, vendor);
    }

    pub fn as_dyn(self: Arc<Self>) -> Box<dyn LlmClient<AgentSession, AgentReactContext>> {
        Box::new(ArcLlmClient(self))
    }

    pub fn with_nvidia(&mut self, model: &str, base_url: &str, api_key: &str) -> &mut Self {
        if !model.starts_with("nvidia/") {
            return self;
        }

        let model = model.strip_prefix("nvidia/").unwrap_or(model);
        self.register_vendor(
            "nvidia".into(),
            Box::new(NvidiaVendor::new(
                base_url.to_string(),
                model.to_string(),
                api_key.to_string(),
            )),
        );
        self
    }

    pub fn with_deepseek(&mut self, model: &str, base_url: &str, api_key: &str) -> &mut Self {
        if !model.starts_with("deepseek/") {
            return self;
        }

        let model = model.strip_prefix("deepseek/").unwrap_or(model);
        self.register_vendor(
            "deepseek".into(),
            Box::new(DeepSeekVendor::new(
                base_url.to_string(),
                model.to_string(),
                api_key.to_string(),
            )),
        );
        self
    }

    pub fn with_openrouter(&mut self, model: &str, base_url: &str, api_key: &str) -> &mut Self {
        if !model.starts_with("openrouter/") {
            return self;
        }

        let model = model.strip_prefix("openrouter/").unwrap_or(model);
        self.register_vendor(
            "openrouter".into(),
            Box::new(OpenRouterVendor::new(
                base_url.to_string(),
                model.to_string(),
                api_key.to_string(),
            )),
        );
        self
    }
}

/// Selects and constructs the LLM vendor for an agent config. Returns the
/// router key and the vendor boxed as the concrete agent session/context types.
///
/// `api_mode` is left untouched: every vendor (including DeepSeek, which
/// supports both `/responses` and `/chat/completions`) honors the configured
/// `api_mode` verbatim.
pub fn build_vendor(
    config: &AgentConfig,
) -> (String, Box<dyn LlmClient<AgentSession, AgentReactContext>>) {
    let (vendor_name, model_name) = if let Some(pos) = config.model.find('/') {
        (
            config.model[..pos].to_string(),
            config.model[pos + 1..].to_string(),
        )
    } else {
        ("openai".to_string(), config.model.clone())
    };

    let vendor: Box<dyn LlmClient<AgentSession, AgentReactContext>> = match vendor_name.as_str() {
        "deepseek" => Box::new(DeepSeekVendor::new(
            config.base_url.clone(),
            model_name,
            config.api_key.clone(),
        )),
        "nvidia" => Box::new(NvidiaVendor::new(
            config.base_url.clone(),
            model_name,
            config.api_key.clone(),
        )),
        "openrouter" => Box::new(OpenRouterVendor::new(
            config.base_url.clone(),
            model_name,
            config.api_key.clone(),
        )),
        _ => Box::new(OpenAiClient::new(
            config.base_url.clone(),
            model_name,
            config.api_key.clone(),
        )),
    };

    (vendor_name, vendor)
}

struct ArcLlmClient(Arc<LlmProvider>);

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for ArcLlmClient {
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactLlmResponse, ReactLlmError> {
        self.0.complete(persona, req, session, context).await
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<TokenStream, ReactLlmError> {
        self.0.stream_complete(persona, req, session, context).await
    }

    fn supports_tools(&self) -> bool {
        self.0.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.0.provider_name()
    }
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient<AgentSession, AgentReactContext> for LlmProvider {
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactLlmResponse, ReactLlmError> {
        self.inner.complete(persona, req, session, context).await
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut AgentSession,
        context: &mut AgentReactContext,
    ) -> Result<ReactTokenStream, ReactLlmError> {
        self.inner
            .stream_complete(persona, req, session, context)
            .await
    }

    fn supports_tools(&self) -> bool {
        self.inner.supports_tools()
    }

    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
}
