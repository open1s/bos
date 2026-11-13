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

/// The smallest foldable span, in estimated tokens, worth a summarizer call.
///
/// A checkpoint's framing alone prices about 75 tokens, and a model-written
/// summary of a span this small routinely prices larger than the span (the
/// `NotSmaller` refusal), so below this floor a model-written compaction can
/// only cost time. The mechanical mid-turn compaction still holds the hard
/// send-side ceiling without any model call.
const COMPACT_MIN_SPAN_TOKENS: usize = 512;

/// Append one prompt section, separated from what is already there.
///
/// The system prompt is joined by blank lines everywhere else in the corpus, so
/// one helper owns the separator: a section that is empty adds nothing, and the
/// first section never opens with a leading newline.
fn push_paragraph(base: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if !base.is_empty() {
        base.push_str("\n\n");
    }
    base.push_str(text);
}

/// The runtime task key for one agent turn.
///
/// One agent serves one session. Every run clears this task's activation, so
/// the scope is a turn: a skill selected for one task does not leak into the
/// next task on the same session.
const SKILL_TASK: &str = "session";

/// Resolve and compose a model-selected Skill through the task-scoped runtime.
///
/// The runtime plans dependencies, activates only the selected set under trust
/// and budget checks, and returns the composed instruction block. A body the
/// index cannot read (an extension overlay with no readable manifest) is taken
/// from the in-memory registration, but a manifest whose digest changed is
/// still refused.
fn compose_skill_load(
    base: &[crate::skills::SkillContent],
    live: &[crate::skills::SkillContent],
    name: &str,
    runtime: &std::sync::Arc<std::sync::Mutex<crate::skills::SkillRuntime>>,
) -> Result<crate::skills::ComposedSkills, String> {
    use crate::skills::{SkillContent, SkillIndex, SkillLoadError, SkillPermission};

    let all: Vec<&SkillContent> = base.iter().chain(live.iter()).collect();
    let index = SkillIndex::from_metadata(all.iter().map(|skill| skill.metadata.clone()));
    let mut runtime = runtime
        .lock()
        .map_err(|_| "skill runtime lock is poisoned".to_string())?;
    runtime.replace_index(index);
    // The plan covers every Skill already active for this task plus the new
    // selection. Planning from the new name alone would leave an earlier
    // activation outside the plan while composition still included it.
    let mut roots = runtime.active(SKILL_TASK);
    roots.push(name.to_string());
    roots.sort();
    roots.dedup();
    let plan = runtime
        .plan(SKILL_TASK, &roots)
        .map_err(|error| error.to_string())?;

    for planned in &plan.order {
        let Some(registered) = all
            .iter()
            .find(|skill| &skill.metadata.name == planned)
            .copied()
        else {
            return Err(format!(
                "the activation plan names '{planned}', but no such skill is registered"
            ));
        };
        let instructions = if registered.metadata.digest.is_empty() {
            registered.instructions.clone()
        } else {
            match runtime.index().load(planned) {
                Ok(loaded) => loaded.instructions,
                Err(SkillLoadError::Stale {
                    expected, actual, ..
                }) => {
                    return Err(format!(
                        "Skill '{planned}' changed after it was indexed (expected digest {expected}, found {actual}); reload the catalogue and try again"
                    ));
                }
                Err(_) => registered.instructions.clone(),
            }
        };
        let body = SkillContent {
            metadata: registered.metadata.clone(),
            instructions,
            skill_dir: registered.skill_dir.clone(),
        };
        runtime
            .activate_with_body(SKILL_TASK, &body, SkillPermission::Model)
            .map_err(|error| error.to_string())?;
    }

    runtime
        .compose(SKILL_TASK)
        .map_err(|error| error.to_string())
}

impl Agent {
    /// Prepare context with tools and skills populated from the agent's state.
    /// Shared by react() and stream() to avoid code duplication.
    ///
    /// The request's tool array and skill catalogue are the only place the
    /// MODEL learns what it may call, so both are drawn from the agent's whole
    /// surface: the build registry plus the live overlay that late
    /// registrations - BSH extension packages among them - install into. The
    /// engine executes a live tool either way, which is exactly why an overlay
    /// entry missing here is silent: the tool exists, runs when called, and is
    /// never called, because nothing told the model it exists.
    pub(super) fn prepare_context(&self) -> AgentReactContext {
        let mut context = AgentReactContext::new(self.config.name.clone());

        // One derivation of the model-facing array, so the request and every
        // surface that prices or lists it cannot drift: see Agent::model_tools.
        context.tools = self.model_tools();

        // The hosted web-search tool is not in the registry, because nothing
        // here runs it: the provider does, and returns the results in its own
        // stream.
        if offers_web_search(self.config.web_search, &self.config.api_mode) {
            context.tools.push(react::llm::LlmTool::web_search(None));
        }

        // The array is part of the provider's cached prefix, so its order must
        // be a function of the set, not of what was appended last. The
        // discovery search is already part of `model_tools` when anything is
        // withheld; nothing is added here.
        context.tools.sort_by(|a, b| a.name.cmp(&b.name));

        // Skills follow the tool rule: the request carries no catalogue, and
        // the model asks for what it needs with `find_skill`. The catalogue
        // stays available for a host that wants the model to see every skill up
        // front (see `Agent::with_skill_catalogue`); a skill whose manifest says
        // `disable-model-invocation: true` is out of both, because the reader's
        // `/name` gesture still loads it.
        if self.skill_search() {
            context
                .tools
                .push(super::discovery::find_skill_definition());
            context.tools.sort_by(|a, b| a.name.cmp(&b.name));
        } else {
            context.skills = self.model_skills();
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
            .parallel_tool_calls(self.config.parallel_tool_calls)
            .context_budget(self.context_budget)
            // ONE per-turn ceiling for the definitions a request carries: the
            // array the host admitted and every later `find_tool` reveal are
            // priced against this same number.
            .schema_budget(self.tool_schema_budget)
            // The host's tool-output setting caps what one result may add to
            // the MODEL's session, not only what the UI displays; zero keeps
            // the engine default.
            .tool_result_cap(self.config.tool_output_cap)
            .model(self.config.model.clone())
            .app(app);
        // A zero budget is "no budget": the loop ends when the model stops
        // asking for tools. The shell runs this way so a long, healthy turn is
        // never cut off by a number; an embedder that wants a runaway guard
        // sets one.
        if self.config.max_steps > 0 {
            builder = builder.max_steps(self.config.max_steps);
        }

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

        if let Ok(live) = self.live_tools.read() {
            for (_name, tool) in live.iter() {
                let tool_adapter = Box::new(ExtensibleToolAdapter::new(
                    tool.clone(),
                    self.plugins.clone(),
                    self.hooks.clone(),
                    self.config.name.clone(),
                ));
                builder = builder.with_tool(ToolVariant::Sync(tool_adapter));
            }
            for name in live.async_tool_names() {
                if let Some(async_tool) = live.get_async(&name) {
                    let tool_adapter = Box::new(AsyncExtensibleToolAdapter::new(async_tool));
                    builder = builder.with_tool(ToolVariant::Async(tool_adapter));
                }
            }
        }

        // A withheld tool is still callable; these two are how the model finds
        // one and runs it. Registering them only when something is withheld
        // keeps the search honest: with nothing to find it is a name that can
        // only answer "nothing".
        if self.has_deferred_tools() {
            // The host's own withheld names travel with the search, so a tool
            // it listed can be found even though the tool itself carries no
            // mark. A tool's own mark is read per call, which keeps a package
            // registered after this build discoverable too.
            let withheld = super::discovery::Withheld::new(
                self.deferred_tools.clone(),
                self.deferred_prefixes.clone(),
            )
            // The budget's own answer travels the same way: a definition
            // withheld to fit the request must be offered by the search, or the
            // way back to it would be a name the model can never reach.
            .with_budget(Arc::clone(&self.schema_withheld));
            let find = super::discovery::find_tool(
                self.registry.clone(),
                Arc::clone(&self.live_tools),
                Arc::clone(&self.registry_view),
                withheld,
            );
            builder = builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                Arc::new(find),
                self.plugins.clone(),
                self.hooks.clone(),
                self.config.name.clone(),
            ))));
        }

        // Skills are searched the same way when the catalogue is off: one tool
        // that names the ones a request could use, with `load_skill` opening
        // the one it picked.
        if self.skill_search() {
            let search =
                super::discovery::find_skill(self.skills.clone(), Arc::clone(&self.live_skills));
            builder = builder.with_tool(ToolVariant::Sync(Box::new(ExtensibleToolAdapter::new(
                Arc::new(search),
                self.plugins.clone(),
                self.hooks.clone(),
                self.config.name.clone(),
            ))));
        }

        // The loader advertises the MODEL-INVOCABLE skills only: a skill whose
        // manifest reserves it for the reader is still registered (the `/name`
        // gesture loads it), but the tool must not open it. With none left, no
        // loader is registered at all rather than one that can only refuse.
        //
        // The list is the catalogue's own projection, overlay included: a
        // package's skills arrive after the build, and a base-only gate left a
        // build whose only skills came from a package with a catalogue that
        // named them and no tool that could open them.
        let model_skills = self.model_skills();
        if !model_skills.is_empty() {
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
                    // The live half is read on every call, not captured here:
                    // an extension can be activated or suspended while this
                    // engine runs, and a loader built from a snapshot would
                    // report a skill the agent has just gained as missing - or
                    // keep opening one whose package was switched off.
                    let base = self.skills.clone();
                    let live = Arc::clone(&self.live_skills);
                    let runtime = Arc::clone(&self.skill_runtime);
                    move |args: &serde_json::Value| {
                        let name = args
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let live = live.read().ok();
                        let live = live.as_deref().map(Vec::as_slice).unwrap_or(&[]);
                        match crate::skills::lookup_for_model_in(&base, live, name) {
                            crate::skills::ModelSkillLookup::Found(skill) => {
                                // The runtime plans dependencies, activates
                                // only the selected set under trust and budget
                                // checks, verifies any readable manifest's
                                // digest and composes one instruction block.
                                // The task scope is this turn, reset when the
                                // run starts.
                                let composed = match compose_skill_load(&base, live, name, &runtime)
                                {
                                    Ok(composed) => composed,
                                    Err(error) => {
                                        return Ok(serde_json::json!({ "error": error }));
                                    }
                                };
                                Ok(serde_json::json!({
                                    "name": skill.metadata.name,
                                    "description": skill.metadata.description,
                                    "instructions": composed.text,
                                    "skills": composed.skills,
                                    "instruction_bytes": composed.bytes,
                                    "deduplicated": composed.deduplicated,
                                    "skill_dir": skill.skill_dir.to_string_lossy(),
                                    "version": skill.metadata.version.display(),
                                    "category": skill.metadata.category.as_str(),
                                    "tags": skill.metadata.tags,
                                    "source": skill.metadata.source.as_str(),
                                    "publisher": skill.metadata.publisher,
                                    "digest": skill.metadata.digest,
                                }))
                            }
                            // The reference's refusal, verbatim in shape: the name
                            // exists, the model may not open it.
                            crate::skills::ModelSkillLookup::UserOnly(_) => Ok(serde_json::json!({
                                "error": format!("Skill '{name}' is not available for model invocation")
                            })),
                            crate::skills::ModelSkillLookup::Unknown => {
                                let mut available: Vec<String> = base
                                    .iter()
                                    .chain(live.iter())
                                    .map(|skill| skill.metadata.name.clone())
                                    .collect();
                                available.sort();
                                available.dedup();
                                Ok(serde_json::json!({
                                    "error": format!("Skill '{}' not found. Available: {}", name, available.join(", "))
                                }))
                            }
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
    /// Discovery is written FIRST, ahead of the standing contract, because the
    /// search is a step before acting rather than a fallback after a failure:
    /// a paragraph appended after the whole prompt arrived once the model had
    /// already picked its first action.
    ///
    /// The block is read from the REQUEST's own tool array, not from a second
    /// derivation of the same policy. That is the whole guarantee: if the
    /// request carries `find_tool` or `find_skill`, the prompt names it; if it
    /// does not, the prompt is silent about it. A build that changes what it
    /// advertises therefore cannot leave the prompt pointing at a search it
    /// dropped, or hiding one it kept.
    ///
    /// Non-text input (images/audio only) has no query to match against, so
    /// it uses the composed prompt unchanged.
    async fn system_prompt_for(&self, task: &Content, tools: &[react::llm::LlmTool]) -> String {
        let mut base = super::discovery::prompt_block(tools);
        push_paragraph(&mut base, &self.config.system_prompt);
        if let Ok(prompts) = self.live_prompts.read() {
            for prompt in prompts.iter() {
                push_paragraph(&mut base, prompt);
            }
        }
        let Some(query) = task.as_text() else {
            return base;
        };
        let memory = self.recalled_context(query).await;
        react::template::with_memory(&base, memory.as_deref())
    }

    /// The full system prompt a turn for `task` will carry: the discovery
    /// block, the configured persona, any live prompt sections and the recalled
    /// memory - the exact assembly [`Self::system_prompt_for`] sends to the
    /// provider. A host surfacing the injection shows the whole of it, so no key
    /// info the model receives is missing from the inject event.
    pub async fn injected_system_prompt(&self, task: &Content) -> String {
        let context = self.prepare_context();
        self.system_prompt_for(task, context.tools.as_slice()).await
    }

    /// Run the agent using ReAct engine. Uses the agent's existing session
    /// and writes results back after completion. Session is not locked during
    /// engine execution to avoid deadlocks with hooks.
    ///
    /// Supports multimodal input via `Content`:
    /// - `Content::text("string")` for simple text
    /// - `Content::parts([ContentPart::text(...), ContentPart::image(...)])` for multimodal
    pub async fn react(&self, task: impl Into<Content>) -> Result<String, AgentError> {
        self.reset_skill_runtime();
        let task_content = task.into();
        let wall_start = std::time::Instant::now();
        let turn_start = crate::agent::TurnStartWrapper::new(
            self.config.name.clone(),
            task_content.as_text().unwrap_or_default(),
        );
        if self.plugins.on_turn_start(turn_start).await.is_none() {
            return Ok("turn aborted by plugin".to_string());
        }

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
        let tools = context
            .as_ref()
            .expect("a prepared context for this run")
            .tools
            .as_slice();
        let system_prompt = self.system_prompt_for(&task_content, tools).await;
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
            // What the host classified this turn as: the engine applies the
            // action contract from it, and never sends it to the provider.
            intent: self.action_intent(),
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
                let _ = self
                    .plugins
                    .on_turn_end(crate::agent::TurnEndWrapper {
                        agent: self.config.name.clone(),
                        output: Some(answer.clone()),
                        error: None,
                    })
                    .await;
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
                let _ = self
                    .plugins
                    .on_turn_end(crate::agent::TurnEndWrapper {
                        agent: self.config.name.clone(),
                        output: None,
                        error: Some(e.to_string()),
                    })
                    .await;
                Err(react_failure(e))
            }
        }
    }

    /// Run the agent (delegates to react with session + hooks via AgentReActApp).
    pub async fn run_simple(&self, task: impl Into<Content>) -> Result<String, AgentError> {
        self.react(task).await
    }

    /// Drop the previous turn's Skill activations and composed context.
    fn reset_skill_runtime(&self) {
        if let Ok(mut runtime) = self.skill_runtime.lock() {
            runtime.finish(SKILL_TASK);
        }
        // A cached context can carry the previous task's skill block (an
        // aborted run never reached on_final_answer), so the reset clears it
        // as well as the runtime.
        if let Ok(mut cache) = self.context_cache.lock() {
            if let Some(context) = cache.as_mut() {
                context.clear_skill_instructions();
            }
        }
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
        // A span this small cannot pay for its own summarizer call. The
        // automatic trigger prices the whole request - system prompt, tool
        // schemas and messages - while only the message span is foldable, so a
        // shell with a large fixed overhead trips the trigger every turn while
        // the foldable part stays in the hundreds of tokens. Each of those
        // calls is a full completion on the turn's critical path, and a
        // checkpoint framed from a tiny span routinely prices LARGER than the
        // span it replaces (the NotSmaller refusal). Below this floor the
        // honest outcome is "nothing worth folding": skip the call, keep the
        // history, and let the mechanical mid-turn compaction hold the hard
        // send-side ceiling.
        if before_tokens < COMPACT_MIN_SPAN_TOKENS {
            log::debug!(
                "compaction skipped: foldable span {before_tokens} tokens is under the {COMPACT_MIN_SPAN_TOKENS} floor"
            );
            return Ok(None);
        }

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
        self.reset_skill_runtime();

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

            let system_prompt = self
                .system_prompt_for(&task_content, context.tools.as_slice())
                .await;
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
                intent: self.action_intent(),
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

    fn host_skill_policy() -> crate::skills::TrustPolicy {
        crate::skills::TrustPolicy {
            allowed_sources: [
                crate::skills::SkillSource::User,
                crate::skills::SkillSource::Builtin,
                crate::skills::SkillSource::ThirdParty,
                crate::skills::SkillSource::Federated,
            ]
            .into_iter()
            .collect(),
            require_publisher: true,
        }
    }

    fn write_runtime_skill(dir: &std::path::Path, name: &str, body: &str, extra: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).expect("dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n{extra}---\n{body}"),
        )
        .expect("write");
    }

    #[test]
    fn the_runtime_composes_dependency_first_and_refuses_stale_content() {
        use crate::skills::{SkillIndex, SkillLimits, SkillRuntime};
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().expect("temp dir");
        write_runtime_skill(dir.path(), "base", "BASE", "");
        write_runtime_skill(dir.path(), "app", "APP", "dependencies:\n  - name: base\n");
        let mut loader = crate::skills::SkillLoader::new(dir.path().to_path_buf());
        loader.discover().expect("discover");
        let app = loader.load("app").expect("app");
        let base = loader.load("base").expect("base");
        let runtime = Arc::new(Mutex::new(SkillRuntime::new(
            Arc::new(SkillIndex::from_metadata(Vec::new())),
            host_skill_policy(),
            SkillLimits::default(),
        )));

        let composed =
            super::compose_skill_load(&[app.clone(), base.clone()], &[], "app", &runtime)
                .expect("compose");
        assert_eq!(composed.skills, vec!["base", "app"]);
        let base_at = composed.text.find("BASE").expect("base body");
        let app_at = composed.text.find("APP").expect("app body");
        assert!(base_at < app_at, "{}", composed.text);

        // A manifest edited after discovery is refused, not served stale.
        std::fs::write(
            dir.path().join("app").join("SKILL.md"),
            "---\nname: app\ndescription: app\ndependencies:\n  - name: base\n---\nCHANGED",
        )
        .expect("rewrite");
        let error = super::compose_skill_load(&[app, base], &[], "app", &runtime)
            .expect_err("stale content is refused");
        assert!(error.contains("changed"), "{error}");
    }

    #[test]
    fn loading_a_second_skill_keeps_the_first_in_the_plan() {
        use crate::skills::{SkillIndex, SkillLimits, SkillRuntime};
        use std::sync::{Arc, Mutex};

        let dir = tempfile::tempdir().expect("temp dir");
        write_runtime_skill(dir.path(), "base", "BASE", "");
        write_runtime_skill(dir.path(), "app", "APP", "dependencies:\n  - name: base\n");
        let mut loader = crate::skills::SkillLoader::new(dir.path().to_path_buf());
        loader.discover().expect("discover");
        let app = loader.load("app").expect("app");
        let base = loader.load("base").expect("base");
        let runtime = Arc::new(Mutex::new(SkillRuntime::new(
            Arc::new(SkillIndex::from_metadata(Vec::new())),
            host_skill_policy(),
            SkillLimits::default(),
        )));

        let first = super::compose_skill_load(&[app.clone(), base.clone()], &[], "app", &runtime)
            .expect("first");
        assert_eq!(first.skills, vec!["base", "app"]);

        // Loading the dependency directly must not drop the earlier root or
        // duplicate its body: the plan is recomputed over the active set.
        let second =
            super::compose_skill_load(&[app, base], &[], "base", &runtime).expect("second");
        assert_eq!(second.skills, vec!["base", "app"]);
        assert_eq!(second.text.matches("APP").count(), 1);
        assert_eq!(second.text.matches("BASE").count(), 1);
    }

    #[test]
    fn the_runtime_refuses_an_unpublished_third_party_skill() {
        use crate::skills::{SkillIndex, SkillLimits, SkillRuntime};
        use std::sync::{Arc, Mutex};

        let mut metadata = crate::skills::SkillMetadata::new(
            "pkg".to_string(),
            "a package skill".to_string(),
            std::path::PathBuf::from("/nowhere/SKILL.md"),
        );
        metadata.source = crate::skills::SkillSource::ThirdParty;
        metadata.digest = String::new();
        let content = crate::skills::SkillContent {
            metadata,
            instructions: "PACKAGE".to_string(),
            skill_dir: std::path::PathBuf::from("/nowhere"),
        };
        let runtime = Arc::new(Mutex::new(SkillRuntime::new(
            Arc::new(SkillIndex::from_metadata(Vec::new())),
            host_skill_policy(),
            SkillLimits::default(),
        )));
        let error = super::compose_skill_load(&[content], &[], "pkg", &runtime)
            .expect_err("trust policy refuses it");
        assert!(error.contains("publisher"), "{error}");
    }
}
