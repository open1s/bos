use super::*;

use crate::agent::context::CompactionOutcome;

/// A React failure that keeps its LLM kind.
///
/// Flattening every failure into `AgentError::Session` put a second, wrong label
/// on an LLM error ("Session error: LLM error: LLM error: Stream error: ...").
/// The provider's own sentence is the whole message now, behind one
/// "LLM error:"; only a non-LLM failure becomes a session sentence.
fn react_failure(e: react::engine::ReactError) -> AgentError {
    match e {
        react::engine::ReactError::Llm(llm) => AgentError::Llm(llm),
        other => AgentError::Session(other.to_string()),
    }
}

/// Owns the session history for one run and puts it back when the run ends.
///
/// Both [`Agent::react`] and [`Agent::stream`] take the message log out of the
/// agent so the engine can drive it without holding the session lock. A
/// caller can drop the future or the stream before the generator reaches its
/// cleanup (a host that stops reading at `Done` does exactly that), so the
/// history is restored from `Drop` as well: cancellation must not silently
/// erase the transcript.
struct SessionRestore<'a> {
    agent: &'a Agent,
    session: Option<AgentSession>,
}

impl<'a> SessionRestore<'a> {
    fn new(agent: &'a Agent, messages: Vec<LlmMessage>) -> Self {
        let mut session = AgentSession::new();
        session.restore_messages(messages);
        Self {
            agent,
            session: Some(session),
        }
    }

    fn session_mut(&mut self) -> &mut AgentSession {
        self.session
            .as_mut()
            .expect("session is present until restore runs")
    }

    /// Move the current messages back into the agent's session.
    fn restore(&mut self) {
        if let Some(mut session) = self.session.take() {
            let messages = session.take_messages();
            let mut target = self.agent.session.lock().unwrap();
            target.restore_messages(messages);
        }
    }
}

impl Drop for SessionRestore<'_> {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Whether the hosted web-search tool is offered on this request.
///
/// Both halves are required. The switch says the reader wants it; the protocol
/// says the endpoint can carry it, because a hosted tool is an object only the
/// Responses API understands - an OpenAI-compatible chat endpoint rejects the
/// request outright, and the anthropic wire has no such field. Offering it on
/// those would break every turn rather than add a feature, so the protocol is
/// part of the rule and not a caller's problem to remember.
fn offers_web_search(enabled: bool, api_mode: &str) -> bool {
    enabled && api_mode == "responses"
}

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

        // The hosted web-search tool is not in the registry, because nothing
        // here runs it: the provider does, and returns the results in its own
        // stream.
        if offers_web_search(self.config.web_search, &self.config.api_mode) {
            context.tools.push(react::llm::LlmTool::web_search(None));
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
        )
        // The same mailbox the host steers into: the app drains it at the top
        // of every ReAct step, which is the only point a message can enter a
        // request without corrupting the one already in flight.
        .with_steers(self.steers.clone());

        let mut builder = ReActEngineBuilder::<AgentReActApp>::new()
            .llm(react_llm)
            .resilience(self.resilience.clone())
            .llm_timeout(self.config.timeout_secs)
            .max_steps(self.config.max_steps)
            .parallel_tool_calls(self.config.parallel_tool_calls)
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

            // One tool for every skill is not an option: it puts a no-op entry
            // in the request's tool array for each skill installed, which the
            // model then has to read and choose between on every step. The
            // catalog is already in the system prompt (the vendor appends the
            // names and descriptions), so a single named loader is the whole
            // surface: the model reads the list, calls load_skill for the one
            // it needs, and the instructions arrive as the tool result.
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

        // A cached engine's counters span its whole lifetime; snapshot them
        // before the run so metrics record only what this run added.
        let (pre_prompt_tokens, pre_completion_tokens) = engine
            .as_ref()
            .map(|e| e.cumulative_token_usage())
            .unwrap_or((0, 0));
        let pre_tool_calls = engine.as_ref().map(|e| e.tool_call_count()).unwrap_or(0);
        let pre_tool_time = engine
            .as_ref()
            .map(|e| e.tool_time())
            .unwrap_or(std::time::Duration::ZERO);

        let messages = {
            let mut session = self.session.lock().unwrap();
            session.take_messages()
        };
        let mut session_restore = SessionRestore::new(self, messages);
        let system_prompt = self.system_prompt_for(&task_content).await;
        let prompt_text = task_content.as_text().unwrap_or_default().to_string();

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
                session_restore.session_mut(),
                &mut *context.as_mut().unwrap(),
            )
            .await;
        let engine_time = engine_start.elapsed();

        session_restore.restore();

        match result {
            Ok(answer) => {
                let cumulative_tokens = engine
                    .as_ref()
                    .map(|e| e.cumulative_token_usage())
                    .unwrap_or((0, 0));
                let tool_calls = engine.as_ref().map(|e| e.tool_call_count()).unwrap_or(0);
                let tool_time = engine
                    .as_ref()
                    .map(|e| e.tool_time())
                    .unwrap_or(std::time::Duration::ZERO);
                {
                    let mut ec = self.engine_cache.lock().unwrap();
                    *ec = engine.take();
                    let mut cc = self.context_cache.lock().unwrap();
                    *cc = context.take();
                }
                let wall_time = wall_start.elapsed();
                // Counters on a cached engine accumulate; record this run's delta.
                let input_delta = cumulative_tokens.0.saturating_sub(pre_prompt_tokens);
                let output_delta = cumulative_tokens.1.saturating_sub(pre_completion_tokens);
                self.metrics.record_call(
                    wall_time,
                    engine_time,
                    std::time::Duration::ZERO,
                    input_delta,
                    output_delta,
                );
                let tool_calls_delta = tool_calls.saturating_sub(pre_tool_calls);
                let tool_time_delta = tool_time
                    .checked_sub(pre_tool_time)
                    .unwrap_or(std::time::Duration::ZERO);
                if tool_calls_delta > 0 || tool_time_delta > std::time::Duration::ZERO {
                    self.metrics
                        .record_tool_calls(tool_calls_delta, tool_time_delta);
                }
                self.hooks
                    .trigger_all(HookEvent::OnMessage, HookContext::new(&self.config.name))
                    .await;
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", (input_delta + output_delta).to_string());
                self.hooks.trigger_all(HookEvent::OnComplete, ctx).await;
                self.remember_exchange(&prompt_text, &answer).await;
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
                Err(react_failure(e))
            }
        }
    }

    /// Run the agent (delegates to react with session + hooks via AgentReActApp).
    pub async fn run_simple(&self, task: impl Into<Content>) -> Result<String, AgentError> {
        self.react(task).await
    }

    /// Condense the session's older history into a model-written checkpoint.
    ///
    /// This is the seam the `/compact` command drives: it is **not** a turn.
    /// The ReAct loop is never entered, no tool is offered or executed, and
    /// nothing is appended to the session by the conversation path. One
    /// auxiliary completion is made, and its text — and only its text — becomes
    /// the checkpoint that replaces the older span.
    ///
    /// `retain_tokens` is the recent tail kept verbatim, priced the way the
    /// compaction contract prices it. Zero — what the manual command asks for —
    /// means "condense everything safe", so the boundary walks back to the head
    /// and then forward to the nearest tool-pairing balanced cut. A system
    /// prompt at the head is never folded.
    ///
    /// Returns `None` when the history holds nothing safe to fold, which is the
    /// `No compactable history yet.` outcome rather than an error. An empty or
    /// whitespace-only summary is refused the same way, so a model that answers
    /// with nothing cannot silently erase history.
    ///
    /// `signal` is the caller's cancellation: dropping the future (a stopped
    /// command) aborts the request, and an aborted call leaves the session
    /// untouched because the checkpoint is installed only after the response
    /// resolves.
    pub async fn compact_now(
        &self,
        retain_tokens: usize,
        signal: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<CompactionOutcome>, AgentError> {
        use std::sync::atomic::Ordering;

        // Selection first, before any request: nothing safe to fold means no
        // model call at all.
        let (span, before_tokens) = {
            let session = self.session.lock().unwrap();
            let Some((removed, _)) = session.split_for_compaction_at(retain_tokens) else {
                return Ok(None);
            };
            let priced: usize = removed
                .iter()
                .map(crate::agent::context::estimate_message_tokens)
                .sum();
            (removed, priced)
        };

        // The summarizer reads the shadowed span as plain text and answers with
        // the checkpoint. Tools are withheld: the instruction forbids acting,
        // and a tool schema would only invite a call the backend must discard.
        //
        // The digest is BOUNDED. Sending the whole span was the defect behind
        // "could not produce a useful summary": a history larger than the
        // model window produced a provider 400 that the old string error then
        // reported as a useless summary. The budget covers the system prompt,
        // the instruction and the digest together, and the old/new slice rule
        // keeps the original intent plus the work in progress.
        let instruction_tokens =
            crate::agent::context::estimate_tokens(crate::agent::context::COMPACTION_INSTRUCTION);
        let system_tokens = crate::agent::context::estimate_tokens(&self.config.system_prompt);
        let request_budget = self.config.compaction_input_tokens;
        let overhead = instruction_tokens + system_tokens + 64;
        let digest_budget = if request_budget == 0 {
            0
        } else {
            request_budget.saturating_sub(overhead)
        };
        let digest = crate::agent::context::render_compaction_digest(&span, digest_budget);
        let prompt = format!(
            "{digest}\n\n---\n\n{}",
            crate::agent::context::COMPACTION_INSTRUCTION
        );

        if signal.load(Ordering::SeqCst) {
            return Err(AgentError::Stopped);
        }

        // A checkpoint is extraction, not writing: the summary should be as
        // faithful as the reader can make it, and its output is capped well
        // below the session's normal completion budget so a reasoning model
        // cannot reserve the whole remaining window for this auxiliary call.
        let summary_cap = self
            .config
            .max_tokens
            .filter(|cap| *cap > 0)
            .map_or(crate::agent::context::COMPACTION_MAX_OUTPUT_TOKENS, |cap| {
                cap.min(crate::agent::context::COMPACTION_MAX_OUTPUT_TOKENS)
            });
        let request = LlmRequest {
            model: self.config.model.clone(),
            input: Content::Text(prompt),
            temperature: Some(0.0),
            max_tokens: Some(summary_cap),
            reasoning_effort: self
                .config
                .reasoning_effort
                .as_deref()
                .map(react::llm::ReasoningEffort::from_name),
            api_mode: react::llm::ApiMode::from_name(&self.config.api_mode),
            ..Default::default()
        };

        let response = self
            .llm
            .complete(
                Some(self.config.system_prompt.clone()),
                request,
                &mut AgentSession::new(),
                &mut self.prepare_context(),
            )
            .await
            .map_err(|err| {
                // The row names the cause; the log keeps the full provider text
                // for the attempt the old generic sentence said was recorded.
                log::warn!("compaction summarizer request failed: {err}");
                AgentError::Compaction(crate::agent::context::CompactionFailure::from_llm_error(
                    &err,
                ))
            })?;

        if signal.load(Ordering::SeqCst) {
            return Err(AgentError::Stopped);
        }

        let summary = crate::agent::context::response_text(&response);
        // The shipped backend distinguishes "nothing safe to fold" (null) from
        // a summarizer that produced no usable text (a summary failure). Keep
        // that split: an empty answer is an error, not a silent no-op.
        if summary.trim().is_empty() {
            log::warn!("compaction summarizer returned an empty summary");
            return Err(AgentError::Compaction(
                crate::agent::context::CompactionFailure::EmptySummary,
            ));
        }
        // The checkpoint must buy context back. The shipped backend rejects a
        // framed summary whose priced replacement is not smaller than the span
        // it shadows; without this gate a verbose summary grows the context it
        // was supposed to shrink.
        let framed = crate::agent::context::render_checkpoint(&summary);
        let framed_tokens = crate::agent::context::estimate_message_tokens(&LlmMessage::User {
            content: Content::Text(framed),
        });
        if framed_tokens >= before_tokens {
            log::warn!(
                "compaction summary would not shrink the context: {framed_tokens} >= {before_tokens}"
            );
            return Err(AgentError::Compaction(
                crate::agent::context::CompactionFailure::NotSmaller {
                    framed_tokens,
                    span_tokens: before_tokens,
                },
            ));
        }

        // Install only now: a failed or cancelled request must leave the
        // session exactly as it was.
        let after_tokens;
        let replaced;
        {
            let mut session = self.session.lock().unwrap();
            replaced = session.install_priced_checkpoint(retain_tokens, &summary);
            after_tokens = session
                .messages()
                .iter()
                .map(crate::agent::context::estimate_message_tokens)
                .sum();
        }
        if replaced == 0 {
            return Ok(None);
        }
        Ok(Some(crate::agent::context::CompactionOutcome {
            replaced,
            before_tokens,
            after_tokens,
            summary,
        }))
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

        // Snapshot engine counters before the run so the values stored for
        // this stream describe this run, not the engine's whole lifetime.
        let (pre_prompt_tokens, pre_completion_tokens) = engine.cumulative_token_usage();
        let pre_cached_tokens = engine.cumulative_cached_tokens();
        let pre_tool_calls = engine.tool_call_count();
        let pre_tool_time = engine.tool_time();

        let messages = {
            let mut session = self.session.lock().unwrap();
            session.take_messages()
        };
        let session_restore = SessionRestore::new(self, messages);

        let stream = async_stream::stream! {
            let mut engine = engine;
            let mut context = context;
            let mut session_restore = session_restore;
            let mut reply = String::new();

            // Reset the per-stream readings first: a run that aborts early
            // must not replay the previous stream's values.
            {
                let mut ts = self.last_stream_tokens.lock().unwrap();
                *ts = Some((0, 0));
                let mut cs = self.last_stream_cache_tokens.lock().unwrap();
                *cs = Some(0);
                let mut tc = self.last_stream_tool_calls.lock().unwrap();
                *tc = 0;
                let mut tt = self.last_stream_tool_time.lock().unwrap();
                *tt = std::time::Duration::ZERO;
            }

            let system_prompt = self.system_prompt_for(&task_content).await;
            let prompt_text = task_content.as_text().unwrap_or_default().to_string();
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

            // The terminal token is held back until the run's bookkeeping has
            // finished: a host that stops reading at `Done` must still get the
            // remembered exchange, the metrics, the hooks, and a cached engine.
            let mut terminal_token: Option<StreamToken> = None;
            {
                let react_stream =
                    engine.react_stream(Some(system_prompt), request, session_restore.session_mut(), &mut context);
                futures::pin_mut!(react_stream);
                let plugin_list = self.plugins.plugins();
                let hooks = self.hooks.clone();
                let agent_name = self.config.name.clone();

                while let Some(item) = react_stream.next().await {
                    match item {
                        Ok(token) if matches!(token, StreamToken::Done | StreamToken::Stopped) => {
                            terminal_token = Some(token);
                            break;
                        }
                        Ok(token) => {
                            // Snapshot the plugins once per stream and move the
                            // token into the wrapper: this runs for every chunk.
                            let final_token = if plugin_list.is_empty() {
                                token
                            } else {
                                let wrapped = StreamTokenWrapper::from_token(token);
                                match PluginRegistry::on_stream_token_with(&plugin_list, wrapped)
                                    .await
                                {
                                    Some(modified) => modified.into_token(),
                                    None => continue,
                                }
                            };
                            if let StreamToken::Text(text) = &final_token {
                                reply.push_str(text);
                            }
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
                            yield Err(react_failure(e));
                        }
                    }
                }
            }

            // Remember the exchange only after the run completed cleanly: an
            // aborted stream returns from inside the loop above.
            self.remember_exchange(&prompt_text, &reply).await;

            session_restore.restore();

            {
                let cumulative = engine.cumulative_token_usage();
                let tokens = (
                    cumulative.0.saturating_sub(pre_prompt_tokens),
                    cumulative.1.saturating_sub(pre_completion_tokens),
                );
                let tool_calls = engine.tool_call_count().saturating_sub(pre_tool_calls);
                let tool_time = engine.tool_time().saturating_sub(pre_tool_time);
                let cached_tokens =
                    engine.cumulative_cached_tokens().saturating_sub(pre_cached_tokens);
                let mut ts = self.last_stream_tokens.lock().unwrap();
                *ts = Some(tokens);
                let mut cs = self.last_stream_cache_tokens.lock().unwrap();
                *cs = Some(cached_tokens);
                let mut tc = self.last_stream_tool_calls.lock().unwrap();
                *tc = tool_calls;
                let mut tt = self.last_stream_tool_time.lock().unwrap();
                *tt = tool_time;
            }

            {
                let cumulative = engine.cumulative_token_usage();
                let total = cumulative.0.saturating_sub(pre_prompt_tokens)
                    + cumulative.1.saturating_sub(pre_completion_tokens);
                let mut ctx = HookContext::new(&self.config.name);
                ctx.set("total_tokens", total.to_string());
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

            if let Some(token) = terminal_token {
                yield Ok(token);
            }
        };

        Box::pin(stream)
    }
}

#[cfg(test)]
mod web_tool_tests {
    use super::{offers_web_search, Agent, AgentConfig};

    /// The switch removes the hosted tool from the REQUEST's tool array, not
    /// only from a call-time decision: a preset with `web_search = false`
    /// builds an agent whose request names no `web_search`, so the provider is
    /// never offered it.
    #[test]
    fn web_search_false_withdraws_the_hosted_tool_from_the_request() {
        let with_search = Agent::from_config(AgentConfig {
            web_search: true,
            api_mode: "responses".to_string(),
            ..AgentConfig::default()
        });
        let names: Vec<String> = with_search
            .prepare_context()
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(
            names.iter().any(|name| name == "web_search"),
            "the premise: the switch on offers the hosted tool: {names:?}"
        );

        let without = Agent::from_config(AgentConfig {
            web_search: false,
            api_mode: "responses".to_string(),
            ..AgentConfig::default()
        });
        let names: Vec<String> = without
            .prepare_context()
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(
            !names.iter().any(|name| name == "web_search"),
            "web_search = false must remove the tool from the request: {names:?}"
        );
    }

    /// The hosted web tool needs the switch AND a protocol that can carry it.
    ///
    /// A hosted tool is an object only the Responses API understands; offering it to
    /// a chat-completions or anthropic endpoint would make every request fail rather
    /// than add a capability, so the protocol is half of the rule.
    #[test]
    fn the_hosted_web_tool_needs_the_switch_and_the_protocol() {
        assert!(
            offers_web_search(true, "responses"),
            "on, responses: offered"
        );
        assert!(
            !offers_web_search(false, "responses"),
            "off, responses: withdrawn"
        );
        assert!(
            !offers_web_search(true, "chat"),
            "on, chat: refused, the endpoint cannot carry a hosted tool"
        );
        assert!(
            !offers_web_search(true, "anthropic"),
            "on, anthropic: refused"
        );
        assert!(!offers_web_search(true, ""), "on, unset protocol: refused");
    }
}

#[cfg(test)]
mod failure_tests {
    use super::react_failure;
    use crate::error::{AgentError, LlmError};

    /// An LLM stream failure is reported once, as an LLM error.
    ///
    /// The turn used to surface it as `Session error: LLM error: LLM error:
    /// Stream error: error decoding response body` - two labels for one failure
    /// and the outer one wrong. The provider's sentence is the whole message
    /// now, behind the single label `AgentError::Llm` adds.
    #[test]
    fn an_llm_stream_failure_carries_one_label() {
        let error = react_failure(react::engine::ReactError::Llm(LlmError::Other(
            "Stream error: error decoding response body: operation timed out".to_string(),
        )));
        assert_eq!(
            error.to_string(),
            "LLM error: Stream error: error decoding response body: operation timed out"
        );
        assert!(
            matches!(error, AgentError::Llm(_)),
            "and the LLM kind is kept, not flattened into a session sentence"
        );
    }

    /// A failure that is not the LLM's stays a session sentence.
    #[test]
    fn a_hook_abort_stays_a_session_failure() {
        let error = react_failure(react::engine::ReactError::HookAbort("stopped".to_string()));
        assert_eq!(error.to_string(), "Session error: Hook abort: stopped");
    }
}
