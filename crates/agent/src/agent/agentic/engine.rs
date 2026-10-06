use super::*;

impl Agent {
    /// Prepare context with tools and skills populated from the agent's state.
    /// Shared by react() and stream() to avoid code duplication.
    fn prepare_context(&self) -> AgentReactContext {
        let mut context = AgentReactContext::new(self.config.name.clone());

        if let Some(ref registry) = self.registry {
            let mut tools: Vec<react::llm::LlmTool> = registry
                .iter()
                .map(|(name, tool)| react::llm::LlmTool {
                    name: name.clone(),
                    description: tool.description(),
                    parameters: tool.json_schema(),
                    kind: react::llm::LlmToolKind::Function,
                    config: None,
                })
                .collect();

            for name in registry.async_tool_names() {
                if let Some(async_tool) = registry.get_async(&name) {
                    tools.push(react::llm::LlmTool {
                        name: async_tool.name().to_string(),
                        description: async_tool.description(),
                        parameters: async_tool.json_schema(),
                        kind: react::llm::LlmToolKind::Function,
                        config: None,
                    });
                }
            }

            context.tools = tools;
        }

        if !self.skills.is_empty() {
            context.skills = self
                .skills
                .iter()
                .map(|s| react::llm::Skill {
                    category: s.metadata.category.as_str().to_string(),
                    name: s.metadata.name.clone(),
                    description: s.metadata.description.clone(),
                })
                .collect();
        }

        context
    }

    /// Build a ReActEngine with the standard adapter stack (LLM, tools, skills).
    /// Shared by react(), run_simple(), and stream() to avoid duplicating adapter construction.
    fn build_react_engine(&self) -> Result<ReActEngine<AgentReActApp>, AgentError> {
        let react_llm = self.llm.clone().as_dyn();

        let app = AgentReActApp::new(
            Arc::new(self.hooks.clone()),
            Arc::new(self.plugins.clone()),
            self.config.name.clone(),
        );

        let mut builder = ReActEngineBuilder::<AgentReActApp>::new()
            .llm(react_llm)
            .resilience(self.resilience.clone())
            .llm_timeout(self.config.timeout_secs)
            .max_steps(self.config.max_steps)
            .model(self.config.model.clone())
            .app(app);

        if let Some(ref bus) = self.bus {
            builder = builder
                .bus(bus.clone())
                .agent_name(self.config.name.clone());
        }

        if let Some(ref registry) = self.registry {
            for (_name, tool) in registry.iter() {
                let tool_adapter = Box::new(ExtensibleToolAdapter::new(
                    tool.clone(),
                    self.plugins.clone(),
                    self.hooks.clone(),
                    self.config.name.clone(),
                ));
                builder = builder.with_tool(ToolVariant::Sync(tool_adapter));
            }

            // Handle async tools (like MCP tools) that implement AsyncTool
            for name in registry.async_tool_names() {
                if let Some(async_tool) = registry.get_async(&name) {
                    let tool_adapter = Box::new(AsyncExtensibleToolAdapter::new(async_tool));
                    builder = builder.with_tool(ToolVariant::Async(tool_adapter));
                }
            }
        }

        let has_skills = !self.skills.is_empty();
        if has_skills {
            let skill_names: Vec<String> = self
                .skills
                .iter()
                .map(|s| s.metadata.name.clone())
                .collect();

            for skill in &self.skills {
                let skill_name = skill.metadata.name.clone();
                let skill_desc = format!("Get instructions for the {} skill", skill_name);
                let skill_instructions = skill.instructions.clone();
                let skill_name_for_closure = skill_name.clone();
                let skill_tool = Arc::new(FunctionTool::skill(
                    &skill_name,
                    &skill_desc,
                    serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                    move |_args: &serde_json::Value| {
                        Ok(serde_json::json!({
                            "skill": skill_name_for_closure,
                            "instructions": skill_instructions
                        }))
                    },
                ));
                builder =
                    builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                        skill_tool,
                        self.plugins.clone(),
                        self.hooks.clone(),
                        self.config.name.clone(),
                    ))));
            }

            let load_skill_tool = Arc::new(FunctionTool::new(
                "load_skill",
                "Load a skill by name to get its instructions",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the skill to load"
                        }
                    },
                    "required": ["name"]
                }),
                {
                    let skills = self.skills.clone();
                    move |args: &serde_json::Value| {
                        let name = args
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let found = skills.iter().find(|s| s.metadata.name == name);
                        if let Some(skill) = found {
                            Ok(serde_json::json!({
                                "name": skill.metadata.name,
                                "description": skill.metadata.description,
                                "instructions": skill.instructions,
                                "skill_dir": skill.skill_dir.to_string_lossy()
                            }))
                        } else {
                            Ok(serde_json::json!({
                                "error": format!("Skill '{}' not found. Available: {}", name, skill_names.join(", "))
                            }))
                        }
                    }
                },
            ));
            builder = builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                load_skill_tool,
                self.plugins.clone(),
                self.hooks.clone(),
                self.config.name.clone(),
            ))));
        }

        builder
            .build()
            .map_err(|e| AgentError::Session(format!("ReAct build error: {}", e)))
    }

    /// Compose the system prompt for a run, appending recalled memories.
    ///
    /// Non-text input (images/audio only) has no query to match against, so
    /// it uses the configured prompt unchanged.
    async fn system_prompt_for(&self, task: &Content) -> String {
        let base = self.config.system_prompt.clone();
        let Some(query) = task.as_text() else {
            return base;
        };
        let memory = self.recalled_context(query).await;
        react::template::with_memory(&base, memory.as_deref())
    }

    /// Run the agent using ReAct engine. Uses the agent's existing session
    /// and writes results back after completion. Session is not locked during
    /// engine execution to avoid deadlocks with hooks.
    ///
    /// Supports multimodal input via `Content`:
    /// - `Content::text("string")` for simple text
    /// - `Content::parts([ContentPart::text(...), ContentPart::image(...)])` for multimodal
    pub async fn react(&self, task: impl Into<Content>) -> Result<String, AgentError> {
        let task_content = task.into();
        let wall_start = std::time::Instant::now();

        let (mut engine, mut context) = {
            let mut engine_cache = self.engine_cache.lock().unwrap();
            let mut context_cache = self.context_cache.lock().unwrap();
            if engine_cache.is_none() || context_cache.is_none() {
                drop(engine_cache);
                drop(context_cache);
                let eng = self.build_react_engine()?;
                let ctx = self.prepare_context();
                let mut ec = self.engine_cache.lock().unwrap();
                let mut cc = self.context_cache.lock().unwrap();
                *ec = Some(eng);
                *cc = Some(ctx);
                (Some(ec.take().unwrap()), Some(cc.take().unwrap()))
            } else {
                (
                    Some(engine_cache.take().unwrap()),
                    Some(context_cache.take().unwrap()),
                )
            }
        };

        let messages = {
            let mut session = self.session.lock().unwrap();
            session.take_messages()
        };
        let mut agent_session = AgentSession::new();
        agent_session.restore_messages(messages);
        let system_prompt = self.system_prompt_for(&task_content).await;

        let request = LlmRequest {
            model: self.config.model.clone(),
            input: task_content,
            temperature: Some(self.config.temperature),
            max_tokens: self.config.max_tokens,
            reasoning_effort: self
                .config
                .reasoning_effort
                .as_deref()
                .map(react::llm::ReasoningEffort::from_name),
            api_mode: react::llm::ApiMode::from_name(&self.config.api_mode),
            ..Default::default()
        };

        let engine_start = std::time::Instant::now();
        let result = engine
            .as_mut()
            .unwrap()
            .react(
                Some(system_prompt),
                request,
                &mut agent_session,
                &mut *context.as_mut().unwrap(),
            )
            .await;
        let engine_time = engine_start.elapsed();

        {
            let mut session = self.session.lock().unwrap();
            session.restore_messages(agent_session.take_messages());
        }

        match result {
            Ok(answer) => {
                let tokens = engine.as_ref().map(|e| e.token_usage());
                let tool_calls = engine.as_ref().map(|e| e.tool_call_count()).unwrap_or(0);
                {
                    let mut ec = self.engine_cache.lock().unwrap();
                    *ec = engine.take();
                    let mut cc = self.context_cache.lock().unwrap();
                    *cc = context.take();
                }
                let wall_time = wall_start.elapsed();
                if let Some(usage) = tokens {
                    self.metrics.record_call(
                        wall_time,
                        engine_time,
                        std::time::Duration::ZERO,
                        usage.prompt_tokens as u64,
                        usage.completion_tokens as u64,
                    );
                } else {
                    self.metrics.record_call(
                        wall_time,
                        engine_time,
                        std::time::Duration::ZERO,
                        0,
                        0,
                    );
                }
                if tool_calls > 0 {
                    self.metrics
                        .record_tool_calls(tool_calls, std::time::Duration::ZERO);
                }
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", "0");
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                Ok(answer)
            }
            Err(e) => {
                drop(engine);
                drop(context);
                if matches!(e, react::engine::ReactError::HookAbort(ref msg) if msg == "Execution stopped by user")
                {
                    let partial = {
                        let session = self.session.lock().unwrap();
                        session
                            .messages()
                            .iter()
                            .rev()
                            .filter_map(|m| match m {
                                LlmMessage::Assistant { content } => Some(content.clone()),
                                LlmMessage::AssistantToolCall { args, .. } => {
                                    Some(serde_json::to_string(args).unwrap_or_default())
                                }
                                _ => None,
                            })
                            .next()
                            .unwrap_or_default()
                    };
                    return Ok(partial);
                }
                self.metrics.record_llm_error();
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("error", e.to_string());
                let decision = self.hooks.trigger(HookEvent::OnError, ctx).await;
                match decision {
                    HookDecision::Error(msg) => log::warn!("OnError hook error: {}", msg),
                    HookDecision::Abort => return Ok("LLM call aborted by hook".to_string()),
                    HookDecision::Continue => {}
                }
                Err(AgentError::Session(format!("ReAct run error: {}", e)))
            }
        }
    }

    /// Run the agent (delegates to react with session + hooks via AgentReActApp).
    pub async fn run_simple(&self, task: impl Into<Content>) -> Result<String, AgentError> {
        self.react(task).await
    }

    /// Stream the agent response using ReAct-style loop.
    /// Supports tools and skills with multi-turn LLM calls - executes tools and continues
    /// until final response from LLM.
    pub fn stream(
        &self,
        task: impl Into<Content>,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send + '_>> {
        let task_content = task.into();

        let cached_engine = {
            let mut cache = self.engine_cache.lock().unwrap();
            cache.take()
        };
        let engine = match cached_engine {
            Some(e) => e,
            None => match self.build_react_engine() {
                Ok(e) => e,
                Err(e) => {
                    return Box::pin(async_stream::stream! {
                        yield Err(e);
                    });
                }
            },
        };

        let cached_context = {
            let mut cache = self.context_cache.lock().unwrap();
            cache.take()
        };
        let context = match cached_context {
            Some(c) => c,
            None => self.prepare_context(),
        };

        let messages = {
            let mut session = self.session.lock().unwrap();
            session.take_messages()
        };
        let mut agent_session = AgentSession::new();
        agent_session.restore_messages(messages);

        let stream = async_stream::stream! {
            let mut engine = engine;
            let mut context = context;

            let system_prompt = self.system_prompt_for(&task_content).await;
            let request = LlmRequest {
                model: self.config.model.clone(),
                input: task_content,
                temperature: Some(self.config.temperature),
                max_tokens: self.config.max_tokens,
                reasoning_effort: self
                    .config
                    .reasoning_effort
                    .as_deref()
                    .map(react::llm::ReasoningEffort::from_name),
                api_mode: react::llm::ApiMode::from_name(&self.config.api_mode),
                ..Default::default()
            };

            {
                let react_stream = engine.react_stream(Some(system_prompt), request, &mut agent_session, &mut context);
                futures::pin_mut!(react_stream);
                let plugins = self.plugins.clone();
                let hooks = self.hooks.clone();
                let agent_name = self.config.name.clone();

                while let Some(item) = react_stream.next().await {
                    match item {
                        Ok(token) => {
                            let final_token = if plugins.has_plugins() {
                                let wrapped = StreamTokenWrapper::new(&token);
                                match plugins.on_stream_token(wrapped).await {
                                    Some(modified) => modified.into_token(),
                                    None => continue,
                                }
                            } else {
                                token
                            };
                            yield Ok(final_token);
                        }
                        Err(e) => {
                            let mut ctx = HookContext::new(&agent_name);
                            ctx.set("error", e.to_string());
                            let decision = hooks.trigger(HookEvent::OnError, ctx).await;
                            match decision {
                                HookDecision::Error(msg) => log::warn!("OnError hook error: {}", msg),
                                HookDecision::Abort => {
                                    yield Ok(StreamToken::Done);
                                    return;
                                }
                                HookDecision::Continue => {}
                            }
                            yield Err(AgentError::Session(e.to_string()));
                        }
                    }
                }
            }

            {
                let mut session = self.session.lock().unwrap();
                session.restore_messages(agent_session.take_messages());
            }

            {
                let usage = engine.token_usage();
                let tokens = (usage.prompt_tokens as u64, usage.completion_tokens as u64);
                let tool_calls = engine.tool_call_count();
                let mut ts = self.last_stream_tokens.lock().unwrap();
                *ts = Some(tokens);
                let mut tc = self.last_stream_tool_calls.lock().unwrap();
                *tc = tool_calls;
            }

            {
                let usage = engine.token_usage();
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", (usage.prompt_tokens as u64).to_string());
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
            }

            {
                let mut cache = self.engine_cache.lock().unwrap();
                *cache = Some(engine);
            }
            {
                let mut cache = self.context_cache.lock().unwrap();
                *cache = Some(context);
            }
        };

        Box::pin(stream)
    }
}
