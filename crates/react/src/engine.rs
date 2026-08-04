use crate::llm::types::{load_skill_tool, ReactContext, ReactSession};
use crate::llm::vendor::responses::{ResponsesContentPart, ResponsesItem};
use crate::llm::{LlmClient, LlmError, LlmMessage, LlmRequest, LlmResponse, StreamToken};
use crate::resilience::{ReActResilience, ResilienceError};
use crate::runtime::{HookDecision, ReActApp};
use crate::telemetry::{Telemetry, TelemetryEvent, TokenBudgetReport, TokenCounter, TokenUsage};
use crate::tool::registry::{AsyncTool, FnTool, ToolVariant};
use crate::tool::{Tool, ToolRegistry};
use async_stream::stream;
use bus::Bus;
use dashmap::DashMap;
use futures::{Stream, StreamExt};
use log::info;
use serde_json::Value;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

#[derive(serde::Serialize)]
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
/// Lifecycle event for a tool call published on the bus.
pub struct ToolCallEvent {
    pub call_id: String,
    pub tool: String,
    /// "started" | "completed" | "cancelled" | "failed"
    pub status: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Error)]
pub enum ReactError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    #[error("Tool error: {0}")]
    ToolError(String),
    #[error("Malformed response: {0}")]
    Malformed(String),
    #[error("Engine timeout: {0}")]
    Timeout(String),
    #[error("Resilience error: {0}")]
    Resilience(ResilienceError<LlmError>),
    #[error("Hook abort: {0}")]
    HookAbort(String),
}

// Explicit impl to route Inner(LlmError) -> Llm variant for better error handling
impl From<ResilienceError<LlmError>> for ReactError {
    fn from(e: ResilienceError<LlmError>) -> Self {
        match e {
            ResilienceError::Inner(llm_err) => ReactError::Llm(llm_err),
            _ => ReactError::Resilience(e),
        }
    }
}

impl From<ResilienceError<()>> for ReactError {
    fn from(e: ResilienceError<()>) -> Self {
        match e {
            ResilienceError::Inner(()) => ReactError::Malformed("Unexpected inner error".into()),
            ResilienceError::RateLimited => ReactError::Resilience(ResilienceError::RateLimited),
            ResilienceError::CircuitOpen => ReactError::Resilience(ResilienceError::CircuitOpen),
        }
    }
}

#[derive(Clone)]
pub struct ToolRunManager {
    running: Arc<DashMap<String, String>>,
    bus: Option<Bus>,
    agent_name: String,
}

impl ToolRunManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(DashMap::new()),
            bus: None,
            agent_name: String::new(),
        }
    }

    pub fn with_bus(mut self, bus: Bus, agent_name: String) -> Self {
        self.bus = Some(bus);
        self.agent_name = agent_name;
        self
    }

    fn events_topic(&self) -> String {
        format!("agent/{}/tool/events", self.agent_name)
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn publish_event(&self, call_id: &str, tool: &str, status: &str) {
        if let Some(bus) = &self.bus {
            let event = ToolCallEvent {
                call_id: call_id.to_string(),
                tool: tool.to_string(),
                status: status.to_string(),
                timestamp_ms: Self::now_ms(),
            };
            let topic = self.events_topic();
            let bus = bus.clone();
            tokio::spawn(async move {
                let payload = serde_json::to_string(&event).unwrap_or_default();
                let mut bus = bus;
                let _ = bus.publish(&topic, &payload).await;
            });
        }
    }

    pub fn register(&self, call_id: &str, name: &str) {
        self.running.insert(call_id.to_string(), name.to_string());
        self.publish_event(call_id, name, "started");
    }

    pub fn is_running(&self, call_id: &str) -> bool {
        self.running.contains_key(call_id)
    }

    pub fn cancel(&self, call_id: &str) -> Option<String> {
        let name = self.running.remove(call_id).map(|(_, n)| n);
        if let Some(ref n) = name {
            self.publish_event(call_id, n, "cancelled");
        }
        name
    }

    pub fn complete(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            self.publish_event(call_id, &name, "completed");
        }
    }

    pub fn fail(&self, call_id: &str) {
        if let Some((_, name)) = self.running.remove(call_id) {
            self.publish_event(call_id, &name, "failed");
        }
    }

    pub fn cancel_all_running(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for entry in self.running.iter() {
            result.push((entry.key().clone(), entry.value().clone()));
        }
        for (id, name) in &result {
            self.running.remove(id);
            self.publish_event(id, name, "cancelled");
        }
        result
    }

    pub fn start_listener(&self, tools: Arc<ToolRegistry>) {
        let bus = match self.bus.as_ref() {
            Some(b) => b.clone(),
            None => return,
        };
        let topic = format!("agent/{}/tool/cancel", self.agent_name);
        let run_mgr = Arc::new(self.clone());
        tokio::spawn(async move {
            let session = bus.session();
            let mut sub = bus::Subscriber::<String>::new(&topic)
                .with_session(session)
                .await
                .expect("failed to subscribe to cancellation topic");
            while let Some(msg) = sub.recv().await {
                let call_id = serde_json::from_str::<serde_json::Value>(&msg)
                    .ok()
                    .and_then(|v| v.get("call_id").and_then(|s| s.as_str()).map(String::from))
                    .unwrap_or(msg);
                if let Some(name) = run_mgr.cancel(&call_id) {
                    if let Some(tool) = tools.get(&name) {
                        tool.cancel(&call_id);
                    }
                }
            }
        });
    }
}

#[derive(Clone)]
pub struct CachedSkill {
    pub instructions: String,
    pub skill_dir: String,
    pub loaded_at: Instant,
}

pub struct SkillCache {
    cache: Arc<DashMap<String, CachedSkill>>,
    ttl: Duration,
}

impl SkillCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            ttl,
        }
    }

    pub fn get_or_insert(&self, skill_name: &str, instructions: String, skill_dir: String) -> Arc<CachedSkill> {
        if let Some(entry) = self.cache.get(skill_name) {
            if entry.loaded_at.elapsed() < self.ttl {
                return Arc::new(entry.clone());
            } else {
                self.cache.remove(skill_name);
            }
        }
        let skill = CachedSkill {
            instructions,
            skill_dir,
            loaded_at: Instant::now(),
        };
        self.cache.insert(skill_name.to_string(), skill.clone());
        Arc::new(skill)
    }

    pub fn get(&self, skill_name: &str) -> Option<Arc<CachedSkill>> {
        self.cache.get(skill_name).and_then(|entry| {
            if entry.loaded_at.elapsed() < self.ttl {
                Some(Arc::new(entry.clone()))
            } else {
                self.cache.remove(skill_name);
                None
            }
        })
    }
}

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("LLM is required")]
    MissingLlm,
}

pub struct ReActEngine<A: ReActApp> {
    llm: Box<dyn LlmClient<A::Session, A::Context> + Send + Sync>,
    tools: Arc<ToolRegistry>,
    max_steps: usize,
    telemetry: Telemetry,
    llm_timeout_secs: u64,
    model: String,
    token_counter: TokenCounter,
    react_app: A,
    resilience: Option<ReActResilience>,
    skill_cache: SkillCache,
    tool_call_count: AtomicU64,
    stop_flag: Arc<AtomicBool>,
    run_manager: Arc<ToolRunManager>,
}

pub struct ReActEngineBuilder<A: ReActApp> {
    llm: Option<Box<dyn LlmClient<A::Session, A::Context>>>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<ReActResilience>,
    llm_timeout_secs: u64,
    model: String,
    token_counter: TokenCounter,
    skill_cache: SkillCache,
    react_app: Option<A>,
    bus: Option<Bus>,
    agent_name: String,
    _phantom: std::marker::PhantomData<A>,
}

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

impl<A: ReActApp> ReActEngine<A> {
    pub fn builder() -> ReActEngineBuilder<A> {
        ReActEngineBuilder::new()
    }

    pub fn tool_run_manager(&self) -> &ToolRunManager {
        &self.run_manager
    }

    pub fn register_tool(&self, t: Box<dyn Tool>) {
        self.tools.register_sync(t);
    }

    pub fn register_async_tool(&self, t: Box<dyn AsyncTool>) {
        self.tools.register_async(t);
    }

    /// Check if an error is transient (retryable).
    fn is_transient_error(err: &LlmError) -> bool {
        let err_str = format!("{:?}", err);
        err_str.contains("429")
            || err_str.contains("Too Many Requests")
            || err_str.contains("rate limit")
            || err_str.contains("timeout")
            || err_str.contains("timed out")
            || err_str.contains("connection refused")
            || err_str.contains("service unavailable")
            || err_str.contains("502")
            || err_str.contains("503")
            || err_str.contains("504")
    }

    /// Call LLM with optional resilience wrapper and retry on transient errors.
    pub async fn call_llm(
        &mut self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<LlmResponse, ReactError>
    where
        A::Session: ReactSession,
    {
        let max_retries = self
            .resilience
            .as_ref()
            .map(|r| r.rate_limit_config().max_retries)
            .unwrap_or(3);

        let mut attempt = 0;
        let t0 = std::time::Instant::now();

        loop {
            let t_iter = std::time::Instant::now();
            let result = if let Some(resilience) = &self.resilience {
                resilience.acquire().await.map_err(ReactError::from)?;
                resilience.check_circuit().map_err(ReactError::from)?;
                self.llm.complete(persona.clone(), request.clone(), session, context).await
            } else {
                self.llm.complete(persona.clone(), request.clone(), session, context).await
            };
            info!(
                "[TIMING] call_llm attempt {}: {:?}",
                attempt,
                t_iter.elapsed()
            );

            // Record outcome in circuit breaker so it learns from actual LLM results
            if let Some(ref resilience) = self.resilience {
                match &result {
                    Ok(_) => resilience.record_success(),
                    Err(_) => resilience.record_failure(),
                }
            }

            if let Some(usage) = result.as_ref().ok().and_then(|r| r.usage()) {
                self.token_counter.update_from_response(usage);
            }

            // If successful, return
            if result.is_ok() {
                info!(
                    "[TIMING] call_llm total (attempt {}): {:?}",
                    attempt,
                    t0.elapsed()
                );
                return result.map_err(ReactError::from);
            }

            // Check if error is transient and we should retry
            let should_retry = if let Err(ref err) = result {
                Self::is_transient_error(err)
            } else {
                false
            };

            if !should_retry {
                info!(
                    "[TIMING] call_llm total (non-retry error): {:?}",
                    t0.elapsed()
                );
                return result.map_err(ReactError::from);
            }

            // Check if we should retry
            attempt += 1;
            if attempt >= max_retries {
                info!("[TIMING] call_llm total (max retries): {:?}", t0.elapsed());
                return result.map_err(ReactError::from);
            }

            // Exponential backoff: 500ms, 1s, 2s, 4s...
            let delay_ms = 500 * (1 << (attempt - 1));
            info!("[TIMING] call_llm retrying after {}ms delay", delay_ms);
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
    }

    /// Call LLM for streaming with optional resilience wrapper.
    /// Returns an owned stream that doesn't borrow from self, allowing tool calls
    /// to be executed immediately within the stream loop.
    pub async fn call_llm_stream(
        &self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, ReactError>
    where
        A::Session: ReactSession,
    {
        let result = if let Some(resilience) = &self.resilience {
            resilience.acquire().await.map_err(ReactError::from)?;
            resilience.check_circuit().map_err(ReactError::from)?;

            self.llm.stream_complete(persona.clone(), request, session, context).await
        } else {
            self.llm.stream_complete(persona.clone(), request, session, context).await
        };

        // Record outcome in circuit breaker so it learns from actual LLM results
        if let Some(ref resilience) = self.resilience {
            match &result {
                Ok(_) => resilience.record_success(),
                Err(_) => resilience.record_failure(),
            }
        }

        result.map_err(ReactError::from)
    }

    /// Inject `__call_id__` into the input JSON so the tool can observe its
    /// own call_id. The call_id is the engine's id for this invocation; tools
    /// use it together with the abort mechanism exposed by the binding layer
    /// (AbortSignal for JS, a Python-side signal object for nbos).
    fn inject_call_id(&self, input: &mut Value, call_id: &str) {
        if let Value::Object(map) = input {
            map.insert("__call_id__".to_string(), Value::String(call_id.to_string()));
        } else {
            // Non-object inputs (string/number) are still allowed by some
            // tools. Re-wrap as an object so we can attach the call_id.
            let original = std::mem::replace(input, Value::Null);
            *input = serde_json::json!({
                "__call_id__": call_id,
                "input": original,
            });
        }
    }

    /// Call tool - no resilience wrapper (only LLM calls need rate limiting)
    pub async fn call_tool(&self, name: &str, input: &mut Value, call_id: &str) -> Result<Value, ReactError> {
        let cancelable = self.tools.get(name).map(|t| t.is_cancelable()).unwrap_or(false);
        if cancelable {
            let cid = call_id.to_string();
            self.run_manager.register(&cid, name);
            // If cancel was requested between register and tool execution,
            // skip running the tool entirely.
            if !self.run_manager.is_running(&cid) {
                return Ok(serde_json::json!({
                    "status": "cancelled",
                    "call_id": cid,
                    "elapsed_ms": 0,
                }));
            }
            let result = self.tools.call(name, input).await;
            if result.is_ok() {
                self.run_manager.complete(&cid);
            } else {
                self.run_manager.fail(&cid);
            }
            result.map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        } else {
            self.tools
                .call(name, input)
                .await
                .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        }
    }

    /// Core ReAct step loop. Runs up to max_steps iterations of:
    /// LLM call → match response (ToolCall / Text+ParsedIntent / Done) → tool execution → continue
    /// Returns the final thought text.
    async fn react_loop(
        &mut self,
        persona: Option<String>,
        mut request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<String, ReactError>
    where
        A::Session: ReactSession,
    {
        self.set_stop_flag(false);
        let mut thought = String::new();

        //build request
        session.push(LlmMessage::user(request.input.clone()));

        for _ in 0..self.max_steps {
            if self.stop_flag.load(Ordering::SeqCst) {
                return Err(ReactError::HookAbort(
                    "Execution stopped by user".to_string(),
                ));
            }

            // ReActApp hook: before_llm_call
            match self
                .react_app
                .before_llm_call(&mut request, session, context)
                .await
            {
                HookDecision::Continue => {}
                HookDecision::Abort => {
                    return Err(ReactError::HookAbort("before_llm_call aborted".to_string()))
                }
                HookDecision::Error(msg) => return Err(ReactError::HookAbort(msg)),
            }

            let mut llm_response = match timeout(
                Duration::from_secs(self.llm_timeout_secs),
                self.call_llm(persona.clone(),request.clone(), session, context),
            )
            .await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ReactError::Timeout("LLM prediction timed out".to_string())),
            };

            self.react_app
                .after_llm_response(&mut llm_response, session, context)
                .await;

            match llm_response {
                LlmResponse::OpenAI(rsp) => {
                    let mut found_tool_call = false;

                    for choice in rsp.choices {
                        let message = &choice.message;

                        if let Some(tool_calls) = &message.tool_calls {
                            for tc in tool_calls {
                                found_tool_call = true;
                                let call_id = tc.id.clone();
                                let name = tc.function.name.clone().unwrap_or_default();
                                let args_str = tc.function.arguments.clone().unwrap_or_default();
                                let mut args: serde_json::Value = serde_json::from_str(&args_str)
                                    .unwrap_or(serde_json::json!({}));

                                if name == "load_skill" {
                                    let skill_name =
                                        args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    if let Some(cached_skill) = self.skill_cache.get(skill_name) {
                                        session.push(LlmMessage::AssistantToolCall {
                                            tool_call_id: call_id.clone(),
                                            name: name.clone(),
                                            args: args.clone(),
                                        });
                                        session.push(LlmMessage::ToolResult {
                                            tool_call_id: call_id,
                                            content: format!(
                                                "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.\n\nskill_dir: {}\n\n{}",
                                                skill_name, cached_skill.skill_dir, cached_skill.instructions
                                            ),
                                        });
                                        continue;
                                    }
                                }

                                self.inject_call_id(&mut args, &call_id);

                                match self
                                    .react_app
                                    .before_tool_call(&name, &mut args, &call_id, session, context)
                                    .await
                                {
                                    HookDecision::Continue => {}
                                    HookDecision::Abort => {
                                        return Err(ReactError::HookAbort(
                                            "before_tool_call aborted".to_string(),
                                        ));
                                    }
                                    HookDecision::Error(msg) => {
                                        return Err(ReactError::HookAbort(msg));
                                    }
                                }

                                let mut result = self.call_tool(&name, &mut args, &call_id).await;
                                self.tool_call_count.fetch_add(1, Ordering::Relaxed);

                                match self.react_app
                                    .after_tool_result(&name, &mut result, &call_id, session, context)
                                    .await 
                                {
                                    HookDecision::Continue => {}
                                    HookDecision::Abort => {
                                        return Err(ReactError::HookAbort(
                                            "after_tool_call aborted".to_string(),
                                        ));
                                    }
                                    HookDecision::Error(msg) => {
                                        return Err(ReactError::HookAbort(msg));
                                    }
                                }
                                

                                if let Ok(ret) = &result {
                                    if name == "load_skill" {
                                        let skill_name =
                                            args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                        let instructions = ret
                                            .get("instructions")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let skill_dir = ret
                                            .get("skill_dir")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !instructions.is_empty() {
                                            self.skill_cache.get_or_insert(
                                                skill_name,
                                                instructions.to_string(),
                                                skill_dir.to_string(),
                                            );
                                        }
                                    }

                                    self.telemetry.emit(&TelemetryEvent::ToolInvocation {
                                        tool: name.clone(),
                                        input: args.clone(),
                                        output: ret.clone(),
                                    });

                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: ret.to_string(),
                                    });
                                } else {
                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: format!("Error: {:?}", result),
                                    });
                                }
                            }
                        }

                        if !found_tool_call {
                            if let Some(content) = &message.content {
                                if !content.is_empty() {
                                    thought = content.clone();
                                    if let Some(pos) = thought.find("Final Answer:") {
                                        thought = thought[(pos + "Final Answer:".len())..]
                                            .trim()
                                            .to_string();
                                    }
                                    self.react_app.on_thought(&thought, session, context).await;
                                    session.push(LlmMessage::assistant(content.clone()));
                                }
                            }
                        }

                        let finish = choice.finish_reason.as_deref();
                        if finish.is_some() && finish != Some("tool_calls") {
                            session.push(LlmMessage::assistant(thought.clone()));
                            self.react_app
                                .on_final_answer(&thought, session, context)
                                .await;
                            self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                                answer: thought.clone(),
                            });
                            return Ok(thought);
                        }
                        if !found_tool_call {
                            session.push(LlmMessage::assistant(thought.clone()));
                            self.react_app
                                .on_final_answer(&thought, session, context)
                                .await;
                            self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                                answer: thought.clone(),
                            });
                            return Ok(thought);
                        }
                    }
                }
                LlmResponse::Responses(rsp) => {
                    let mut found_tool_call = false;
                    let mut assistant_text = String::new();

                    for item in &rsp.output {
                        match item {
                            ResponsesItem::FunctionCall {
                                call_id,
                                name,
                                arguments,
                                ..
                            } => {
                                found_tool_call = true;
                                let call_id = call_id.clone();
                                let name = name.clone();
                                let mut args: Value = serde_json::from_str(arguments)
                                    .unwrap_or(serde_json::json!({}));

                                if name == "load_skill" {
                                    let skill_name =
                                        args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    if let Some(cached_skill) = self.skill_cache.get(skill_name) {
                                        session.push(LlmMessage::AssistantToolCall {
                                            tool_call_id: call_id.clone(),
                                            name: name.clone(),
                                            args: args.clone(),
                                        });
                                        session.push(LlmMessage::ToolResult {
                                            tool_call_id: call_id,
                                            content: format!(
                                                "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.\n\nskill_dir: {}\n\n{}",
                                                skill_name, cached_skill.skill_dir, cached_skill.instructions
                                            ),
                                        });
                                        continue;
                                    }
                                }

                                self.inject_call_id(&mut args, &call_id);

                                match self
                                    .react_app
                                    .before_tool_call(&name, &mut args, &call_id, session, context)
                                    .await
                                {
                                    HookDecision::Continue => {}
                                    HookDecision::Abort => {
                                        return Err(ReactError::HookAbort(
                                            "before_tool_call aborted".to_string(),
                                        ));
                                    }
                                    HookDecision::Error(msg) => {
                                        return Err(ReactError::HookAbort(msg));
                                    }
                                }

                                let mut result = self.call_tool(&name, &mut args, &call_id).await;
                                self.tool_call_count.fetch_add(1, Ordering::Relaxed);

                                match self
                                    .react_app
                                    .after_tool_result(&name, &mut result, &call_id, session, context)
                                    .await
                                {
                                    HookDecision::Continue => {}
                                    HookDecision::Abort => {
                                        return Err(ReactError::HookAbort(
                                            "after_tool_call aborted".to_string(),
                                        ));
                                    }
                                    HookDecision::Error(msg) => {
                                        return Err(ReactError::HookAbort(msg));
                                    }
                                }

                                if let Ok(ret) = &result {
                                    if name == "load_skill" {
                                        let skill_name = args
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let instructions = ret
                                            .get("instructions")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let skill_dir = ret
                                            .get("skill_dir")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !instructions.is_empty() {
                                            self.skill_cache.get_or_insert(
                                                skill_name,
                                                instructions.to_string(),
                                                skill_dir.to_string(),
                                            );
                                        }
                                    }

                                    self.telemetry.emit(&TelemetryEvent::ToolInvocation {
                                        tool: name.clone(),
                                        input: args.clone(),
                                        output: ret.clone(),
                                    });

                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: ret.to_string(),
                                    });
                                } else {
                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: format!("Error: {:?}", result),
                                    });
                                }
                            }
                            ResponsesItem::Message { content, .. } => {
                                for part in content {
                                    if let ResponsesContentPart::OutputText { text, .. } = part {
                                        assistant_text.push_str(text);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    if !found_tool_call {
                        if !assistant_text.is_empty() {
                            thought = assistant_text.trim().to_string();
                            if let Some(pos) = thought.find("Final Answer:") {
                                thought = thought[(pos + "Final Answer:".len())..]
                                    .trim()
                                    .to_string();
                            }
                            self.react_app.on_thought(&thought, session, context).await;
                            session.push(LlmMessage::assistant(thought.clone()));
                        }
                        self.react_app
                            .on_final_answer(&thought, session, context)
                            .await;
                        self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                            answer: thought.clone(),
                        });
                        return Ok(thought);
                    }
                }
            }
        }

        session.push(LlmMessage::assistant(thought.clone()));
        self.react_app
            .on_final_answer(&thought, session, context)
            .await;
        self.telemetry.emit(&TelemetryEvent::FinalAnswer {
            answer: thought.clone(),
        });
        Ok(thought)
    }

    pub async fn react(
        &mut self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<String, ReactError>
    where
        A::Session: ReactSession,
    {
        if !request.model.is_empty() {
            self.model.clone_from(&request.model);
        }

        context.add_tool(load_skill_tool());

        let result = self.react_loop(persona,request, session, context).await?;

        Ok(result)
    }

    pub fn react_stream<'a>(
        &'a mut self,
        persona: Option<String>,
        request: LlmRequest,
        session: &'a mut A::Session,
        context: &'a mut A::Context,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, ReactError>> + Send + 'a>>
    where
        A::Session: ReactSession,
        A::Context: ReactContext,
    {
        self.set_stop_flag(false);

        session.push(LlmMessage::user(request.input.clone()));

        let stream = stream! {
            let mut loaded_skills: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
            let mut request = request;

            context.add_tool(load_skill_tool());

            loop {
                if self.stop_flag.load(Ordering::SeqCst) {
                    yield Err(ReactError::HookAbort("Execution stopped by user".to_string()));
                    break;
                }

                match self.react_app
                    .before_llm_call(&mut request, session, context)
                    .await
                {
                    HookDecision::Continue => {}
                    HookDecision::Abort => {
                        yield Err(ReactError::HookAbort("before_llm_call aborted".to_string()));
                        break;
                    }
                    HookDecision::Error(msg) => {
                        yield Err(ReactError::HookAbort(msg));
                        break;
                    }
                }

                let llm_stream = match self.call_llm_stream(persona.clone(),request.clone(), session, context).await {
                    Ok(s) => s,
                    Err(e) => {
                        yield Err(ReactError::from(e));
                        break;
                    }
                };

                futures::pin_mut!(llm_stream);
                let mut full_response = String::new();
                let mut saw_tool_call = false;

                while let Some(item) = llm_stream.next().await {
                    match item {
                        Ok(StreamToken::Text(text)) => {
                            full_response.push_str(&text);
                            yield Ok(StreamToken::Text(text));
                        }
                        Ok(StreamToken::ReasoningContent(text)) => {
                            yield Ok(StreamToken::ReasoningContent(text));
                        }
                        Ok(StreamToken::Usage(usage)) => {
                            let token_usage = TokenUsage::new(
                                usage.prompt_tokens,
                                usage.completion_tokens,
                            );
                            self.token_counter.update_from_response(token_usage);
                            yield Ok(StreamToken::Usage(usage));
                        }
                        Ok(StreamToken::Done) => {
                            break;// End of LLM response stream
                        }
                        Ok(StreamToken::ToolCall { name, mut args, id }) => {
                            saw_tool_call = true;
                            yield Ok(StreamToken::ToolCall { name: name.clone(), args: args.clone(), id: id.clone() });

                            let call_id = id.unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                            self.inject_call_id(&mut args, &call_id);

                            match self.react_app
                                .before_tool_call(&name, &mut args, &call_id, session, context)
                                .await
                            {
                                HookDecision::Continue => {}
                                HookDecision::Abort => {
                                    yield Err(ReactError::HookAbort(
                                        "before_tool_call aborted".to_string(),
                                    ));
                                    break;
                                }
                                HookDecision::Error(msg) => {
                                    yield Err(ReactError::HookAbort(msg));
                                    break;
                                }
                            }

                            let mut result = if name == "load_skill" {
                                let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                if let Some((instructions, skill_dir)) = loaded_skills.get(skill_name) {
                                    Ok(serde_json::json!({
                                        "name": skill_name,
                                        "instructions": instructions,
                                        "skill_dir": skill_dir,
                                        "cached": true
                                    }))
                                } else {
                                    self.call_tool(&name, &mut args, &call_id).await
                                }
                            } else {
                                self.call_tool(&name, &mut args, &call_id).await
                            };
                            self.tool_call_count.fetch_add(1, Ordering::Relaxed);

                            match self.react_app
                                .after_tool_result(&name, &mut result, &call_id, session, context)
                                .await 
                                {
                                HookDecision::Continue => {}
                                HookDecision::Abort => {
                                    yield Err(ReactError::HookAbort(
                                        "after_tool_call aborted".to_string(),
                                    ));
                                    break;
                                }
                                HookDecision::Error(msg) => {
                                    yield Err(ReactError::HookAbort(msg));
                                    break;
                                }
                            }

                            let result_text = match result {
                                Ok(ref ret) => {
                                    if name == "load_skill" {
                                        let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                        let instructions = ret.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
                                        let skill_dir = ret.get("skill_dir").and_then(|v| v.as_str()).unwrap_or("");
                                        if !instructions.is_empty() && !ret.get("cached").and_then(|v| v.as_bool()).unwrap_or(false) {
                                            loaded_skills.insert(skill_name.to_string(), (instructions.to_string(), skill_dir.to_string()));
                                        }
                                    }
                                    self.telemetry.emit(&TelemetryEvent::ToolInvocation {
                                        tool: name.clone(),
                                        input: args.clone(),
                                        output: ret.clone(),
                                    });
                                    ret.to_string()
                                }
                                Err(ref e) => format!("Error: {}", e),
                            };

                            session.push(LlmMessage::assistant_tool_call(call_id.clone(), name.clone(), args.clone()));
                            session.push(LlmMessage::tool_result(call_id.clone(), result_text));
                        }
                        Err(e) => {
                            yield Err(ReactError::Llm(e));
                            break;
                        }
                        Ok(StreamToken::Stopped) => {
                            yield Ok(StreamToken::Stopped);
                            break;
                        }
                    }
                }

                self.react_app
                    .after_llm_response_step(&full_response, saw_tool_call, session, context)
                    .await;

                if !full_response.is_empty() {
                    self.react_app.on_thought(&full_response, session, context).await;
                    if !saw_tool_call {
                        session.push(LlmMessage::assistant(full_response.clone()));
                    }
                }

                if !saw_tool_call {
                    self.react_app.on_final_answer(&full_response, session, context).await;
                    self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                        answer: full_response.clone(),
                    });
                    yield Ok(StreamToken::Done);
                    break;
                }
            }
        };

        Box::pin(stream)
    }

    /// Get current token usage for this session
    pub fn token_usage(&self) -> TokenUsage {
        self.token_counter.usage()
    }

    /// Get a budget report showing usage vs limits
    pub fn token_budget_report(&self) -> TokenBudgetReport {
        self.token_counter.report()
    }

    /// Reset the token counter for a new session
    pub fn reset_token_counter(&mut self) {
        self.token_counter = TokenCounter::with_default();
    }

    /// Get the number of tool calls made during this session.
    pub fn tool_call_count(&self) -> u64 {
        self.tool_call_count.load(Ordering::Relaxed)
    }

    /// Reset the tool call counter.
    pub fn reset_tool_call_count(&self) {
        self.tool_call_count.store(0, Ordering::Relaxed);
    }

    pub fn get_stop_flag(&self) -> Arc<AtomicBool>{
        return self.stop_flag.clone();
    }

    pub fn set_stop_flag(&mut self,flag: bool) {
        self.stop_flag.store(flag, Ordering::SeqCst);
    }

    pub fn stop(&mut self){
        let running = self.run_manager.cancel_all_running();
        for (call_id, name) in running {
            if let Some(tool) = self.tools.get(&name) {
                tool.cancel(&call_id);
            }
        }
        self.set_stop_flag(true);
    }

    pub fn close(&mut self){
        self.set_stop_flag(true);
    }
}
