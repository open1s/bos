use super::*;

impl<A: ReActApp> ReActEngineBuilder<A> {
    pub fn new() -> Self {
        Self {
            llm: None,
            tools: ToolRegistry::new(),
            max_steps: 10,
            telemetry: Telemetry::new(),
            resilience: None,
            llm_timeout_secs: 120,
            model: String::new(),
            token_counter: TokenCounter::with_default(),
            skill_cache: SkillCache::new(Duration::from_secs(300)), // 5 min TTL
            react_app: None,
            bus: None,
            agent_name: String::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn llm<S: Send + Sync + Clone + 'static, C: Send + Sync + Clone + 'static>(
        mut self,
        llm: Box<dyn LlmClient<S, C>>,
    ) -> Self
    where
        A: ReActApp<Session = S, Context = C>,
    {
        self.llm = Some(llm);
        self
    }

    pub fn with_tool(self, t: ToolVariant) -> Self {
        self.tools.register(t);
        self
    }

    pub fn with_sync_tool(self, t: Box<dyn Tool>) -> Self {
        self.tools.register_sync(t);
        self
    }

    pub fn with_async_tool(self, t: Box<dyn AsyncTool>) -> Self {
        self.tools.register_async(t);
        self
    }

    pub fn max_steps(mut self, steps: usize) -> Self {
        self.max_steps = steps;
        self
    }

    pub fn telemetry(mut self, telemetry: Telemetry) -> Self {
        self.telemetry = telemetry;
        self
    }

    pub fn resilience(mut self, resilience: ReActResilience) -> Self {
        log::debug!(
            "[ReActEngine] Resilience enabled: circuit_state={:?}, rate_limit_remaining={:?}",
            resilience.circuit_state(),
            resilience.rate_limit_remaining()
        );
        self.resilience = Some(resilience);
        self
    }

    pub fn llm_timeout(mut self, secs: u64) -> Self {
        self.llm_timeout_secs = secs;
        self
    }

    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    pub fn app(mut self, app: A) -> Self {
        self.react_app = Some(app);
        self
    }

    pub fn bus(mut self, bus: Bus) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn agent_name(mut self, name: String) -> Self {
        self.agent_name = name;
        self
    }
}

impl<A: ReActApp + Default> ReActEngineBuilder<A> {
    pub fn build(self) -> Result<ReActEngine<A>, BuilderError> {
        let llm = self.llm.ok_or(BuilderError::MissingLlm)?;
        let tools = Arc::new(self.tools);
        let bus = self.bus.clone();
        let agent_name = self.agent_name.clone();
        let has_bus = bus.is_some() && !agent_name.is_empty();
        let run_manager = Arc::new(if has_bus {
            ToolRunManager::new().with_bus(bus.unwrap(), agent_name)
        } else {
            ToolRunManager::new()
        });
        let mgr = run_manager.clone();
        let cancel_tools = tools.clone();
        if has_bus {
            run_manager.start_listener(tools.clone());
        }
        tools.register(ToolVariant::Sync(Box::new(FnTool {
            name: "cancel_tool".to_string(),
            description: "Cancel a running tool by its call_id. Pass the exact call_id from the tool call you want to cancel.".to_string(),
            f: Box::new(move |input: &Value| {
                let call_id = input.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(name) = mgr.cancel(call_id) {
                    if let Some(tool) = cancel_tools.get(&name) {
                        tool.cancel(call_id);
                    }
                    serde_json::json!({"status": "cancelled", "call_id": call_id})
                } else {
                    serde_json::json!({"status": "not_found", "call_id": call_id, "message": "No running tool found with this call_id"})
                }
            }),
        })));
        Ok(ReActEngine {
            llm,
            tools,
            max_steps: self.max_steps,
            telemetry: self.telemetry,
            llm_timeout_secs: self.llm_timeout_secs,
            model: self.model,
            token_counter: self.token_counter,
            react_app: self.react_app.unwrap_or_default(),
            resilience: self.resilience,
            skill_cache: self.skill_cache,
            tool_call_count: AtomicU64::new(0),
            stop_flag: Arc::new(AtomicBool::new(false)),
            run_manager,
        })
    }
}

impl<A: ReActApp + Default> Default for ReActEngineBuilder<A> {
    fn default() -> Self {
        Self::new()
    }
}
