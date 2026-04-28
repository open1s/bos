use crate::llm::{
    LlmClient, LlmError, LlmMessage, LlmRequest, LlmResponse, StreamToken,
};
use crate::llm::types::{ReactContext, ReactSession};
use crate::resilience::{ReActResilience, ResilienceError};
use crate::runtime::ReActApp;
use crate::telemetry::{Telemetry, TelemetryEvent, TokenBudgetReport, TokenCounter, TokenUsage};
use crate::tool::{Tool, ToolRegistry};
use futures::{Stream, StreamExt};
use async_stream::stream;
use serde_json::Value;
use std::pin::Pin;
use thiserror::Error;
use tokio::time::{timeout, Duration};
use uuid::Uuid;

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

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("LLM is required")]
    MissingLlm,
}

pub struct ReActEngine<A: ReActApp + Default = crate::runtime::NoopApp> {
    llm: Box<dyn LlmClient<SessionType = A::Session, ContextType = A::Context> + Send + Sync>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
    llm_timeout_secs: u64,
    model: String,
    token_counter: TokenCounter,
    react_app: A,
    resilience: Option<ReActResilience>,
}

pub struct ReActEngineBuilder<A: ReActApp + Default = crate::runtime::NoopApp> {
    llm: Option<Box<dyn LlmClient<SessionType = A::Session, ContextType = A::Context>>>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<ReActResilience>,
    llm_timeout_secs: u64,
    model: String,
    token_counter: TokenCounter,
    _phantom: std::marker::PhantomData<A>,
}

impl<A: ReActApp + Default> ReActEngineBuilder<A> {
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
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn llm<S: Send + Sync + Clone + 'static, C: Send + Sync + Clone + 'static>(
        mut self,
        llm: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
    ) -> Self
    where
        A: ReActApp<Session = S, Context = C>,
    {
        self.llm = Some(llm);
        self
    }

    pub fn with_tool(mut self, t: Box<dyn Tool>) -> Self {
        self.tools.insert(t);
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

    pub fn build(self) -> Result<ReActEngine<A>, BuilderError>
    where
        A::Session: Default,
        A::Context: Default,
    {
        let llm = self.llm.ok_or(BuilderError::MissingLlm)?;
        Ok(ReActEngine {
            llm,
            tools: self.tools,
            max_steps: self.max_steps,
            telemetry: self.telemetry,
            llm_timeout_secs: self.llm_timeout_secs,
            model: self.model,
            token_counter: self.token_counter,
            react_app: A::default(),
            resilience: self.resilience,
        })
    }
}

impl<A: ReActApp + Default> Default for ReActEngineBuilder<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: ReActApp + Default> ReActEngine<A> {
    pub fn new<
        S: Send + Sync + Clone + Default + 'static,
        C: Send + Sync + Clone + Default + 'static,
    >(
        llm: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
        max_steps: usize,
    ) -> Self
    where
        A: ReActApp<Session = S, Context = C>,
    {
        Self {
            llm,
            tools: ToolRegistry::new(),
            max_steps,
            telemetry: Telemetry::new(),
            llm_timeout_secs: 120,
            model: String::new(),
            token_counter: TokenCounter::with_default(),
            react_app: A::default(),
            resilience: None,
        }
    }

    pub fn builder() -> ReActEngineBuilder<A> {
        ReActEngineBuilder::new()
    }

    pub fn register_tool(&mut self, t: Box<dyn Tool>) {
        self.tools.register(t);
    }

    /// Call LLM with optional resilience wrapper.
    pub async fn call_llm(
        &mut self,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<LlmResponse, ReactError>
    where
        A::Session: ReactSession,
    {
        if let Some(resilience) = &self.resilience {
            resilience.acquire().await.map_err(ReactError::from)?;
            resilience.check_circuit().map_err(ReactError::from)?;
            self.llm.complete(request, session, context).await.map_err(ReactError::from)
        } else {
            self.llm
                .complete(request, session, context)
                .await
                .map_err(ReactError::from)
        }
    }

    /// Call LLM for streaming with optional resilience wrapper.
    /// Returns an owned stream that doesn't borrow from self, allowing tool calls
    /// to be executed immediately within the stream loop.
    pub async fn call_llm_stream(
        &self,
        request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>>, ReactError>
    where
        A::Session: ReactSession,
    {
        if let Some(resilience) = &self.resilience {
            resilience.acquire().await.map_err(ReactError::from)?;
            resilience.check_circuit().map_err(ReactError::from)?;

            self.llm
            .stream_complete(request, session, context)
            .await
            .map_err(ReactError::from)
        } else {
            self.llm
            .stream_complete(request, session, context)
            .await
            .map_err(ReactError::from)
        }
    }

    /// Call tool - no resilience wrapper (only LLM calls need rate limiting)
    pub async fn call_tool(&self, name: &str, input: &Value) -> Result<Value, ReactError> {
        self.tools
            .call(name, input)
            .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
    }

    /// Core ReAct step loop. Runs up to max_steps iterations of:
    /// LLM call → match response (ToolCall / Text+ParsedIntent / Done) → tool execution → continue
    /// Returns the final thought text.
    async fn react_loop(
        &mut self,
        mut request: LlmRequest,
        session: &mut A::Session,
        context: &mut A::Context,
    ) -> Result<String, ReactError>
    where
        A::Session: ReactSession,
    {
        let mut thought = String::new();
        let mut loaded_skills: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        //build request
        session.push(LlmMessage::user(request.input.clone()));

        for _ in 0..self.max_steps {
            // ReActApp hook: before_llm_call
            self.react_app
                .before_llm_call(&mut request, session, context)
                .await;

            let llm_response = match timeout(
                Duration::from_secs(self.llm_timeout_secs),
                self.call_llm(request.clone(), session, context),
            )
            .await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ReactError::Timeout("LLM prediction timed out".to_string())),
            };

            self.react_app
                .after_llm_response(&llm_response, session, context)
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
                                let args: serde_json::Value = serde_json::from_str(&args_str)
                                    .unwrap_or(serde_json::json!({}));

                                if name == "load_skill" {
                                    let skill_name =
                                        args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                    if loaded_skills.contains_key(skill_name) {
                                        let cached = loaded_skills.get(skill_name).unwrap();
                                        session.push(LlmMessage::AssistantToolCall {
                                            tool_call_id: call_id.clone(),
                                            name: name.clone(),
                                            args: args.clone(),
                                        });
                                        session.push(LlmMessage::ToolResult {
                                            tool_call_id: call_id,
                                            content: format!(
                                                "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.\n\n{}",
                                                skill_name, cached
                                            ),
                                        });
                                        continue;
                                    }
                                }

                                self.react_app
                                    .before_tool_call(&name, &args, session, context)
                                    .await;

                                let result = self.call_tool(&name, &args).await;
                                self.react_app
                                    .after_tool_result(&name, &result, session, context)
                                    .await;

                                if let Ok(ret) = &result {
                                    if name == "load_skill" {
                                        let skill_name =
                                            args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                        let instructions = ret
                                            .get("instructions")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !instructions.is_empty() {
                                            loaded_skills.insert(
                                                skill_name.to_string(),
                                                instructions.to_string(),
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

        let result = self.react_loop(request, session, context).await?;

        Ok(result)
    }

    
     pub fn react_stream<'a>(
        &'a self,
        request: LlmRequest,
        session: &'a mut A::Session,
        context: &'a mut A::Context,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, ReactError>> + 'a>>
    where
        A::Session: ReactSession, A::Context: ReactContext,
    {
        let stream = stream! {
            let mut step_count = 0;
            let max_steps = self.max_steps;
            let mut loaded_skills: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            let mut request = request;

            while step_count < max_steps {
                step_count += 1;

                self.react_app
                    .before_llm_call(&mut request, session, context)
                    .await;

                let llm_stream = match self.call_llm_stream(request.clone(), session, context).await {
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
                        Ok(StreamToken::Done) => {
                            break;
                        }
                        Ok(StreamToken::ToolCall { name, args, id }) => {
                            saw_tool_call = true;
                            yield Ok(StreamToken::ToolCall { name: name.clone(), args: args.clone(), id: id.clone() });

                            self.react_app
                                .before_tool_call(&name, &args, session, context)
                                .await;

                            let call_id = id.unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                            let result = if name == "load_skill" {
                                let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                if let Some(cached) = loaded_skills.get(skill_name) {
                                    Ok(serde_json::json!({
                                        "name": skill_name,
                                        "instructions": cached,
                                        "cached": true
                                    }))
                                } else {
                                    self.call_tool(&name, &args).await
                                }
                            } else {
                                self.call_tool(&name, &args).await
                            };

                            self.react_app
                                .after_tool_result(&name, &result, session, context)
                                .await;

                            let result_text = match result {
                                Ok(ret) => {
                                    if name == "load_skill" {
                                        let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                        let instructions = ret.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
                                        if !instructions.is_empty() && !ret.get("cached").and_then(|v| v.as_bool()).unwrap_or(false) {
                                            loaded_skills.insert(skill_name.to_string(), instructions.to_string());
                                        }
                                    }
                                    ret.to_string()
                                }
                                Err(e) => format!("Error: {}", e),
                            };

                            session.push(LlmMessage::assistant_tool_call(call_id.clone(), name.clone(), args.clone()));
                            session.push(LlmMessage::tool_result(call_id.clone(), result_text));
                        }
                        Err(e) => {
                            yield Err(ReactError::Llm(e));
                            break;
                        }
                    }
                }

                if !full_response.is_empty() {
                    self.react_app.on_thought(&full_response, session, context).await;
                    session.push(LlmMessage::assistant(full_response.clone()));
                }

                if !saw_tool_call {
                    self.react_app.on_final_answer(&full_response, session, context).await;
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
}
