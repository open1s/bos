//! The ReAct engine: an LLM loop with tools, skills, streaming, and telemetry.

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

mod builder;
mod error;
mod llm_calls;
mod skill_cache;
mod tool_calls;
mod tool_run;

pub use error::{BuilderError, ReactError};
pub use skill_cache::{CachedSkill, SkillCache};
pub use tool_run::{ToolCallEvent, ToolRunManager};

/// The ReAct loop: LLM calls, tool execution, skills, telemetry, and resilience.
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
    tool_time_nanos: AtomicU64,
    stop_flag: Arc<AtomicBool>,
    run_manager: Arc<ToolRunManager>,
}

/// Builder for [`ReActEngine`].
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
impl<A: ReActApp> ReActEngine<A> {
    /// Start configuring an engine.
    pub fn builder() -> ReActEngineBuilder<A> {
        ReActEngineBuilder::new()
    }

    /// The manager tracking in-flight tool calls.
    pub fn tool_run_manager(&self) -> &ToolRunManager {
        &self.run_manager
    }

    /// Register a synchronous tool at runtime.
    pub fn register_tool(&self, t: Box<dyn Tool>) {
        self.tools.register_sync(t);
    }

    /// Register an asynchronous tool at runtime.
    pub fn register_async_tool(&self, t: Box<dyn AsyncTool>) {
        self.tools.register_async(t);
    }

    /// Inject `__call_id__` into the input JSON so the tool can observe its
    /// own call_id. The call_id is the engine's id for this invocation; tools
    /// use it together with the abort mechanism exposed by the binding layer
    /// (AbortSignal for JS, a Python-side signal object for nbos).
    fn inject_call_id(&self, input: &mut Value, call_id: &str) {
        if let Value::Object(map) = input {
            map.insert(
                "__call_id__".to_string(),
                Value::String(call_id.to_string()),
            );
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

    /// Strip the engine-injected `__call_id__` from args before they are
    /// recorded in the session. The session is sent back to the LLM as
    /// context; if the injected call_id leaked into it, the LLM would start
    /// echoing `__call_id__` in its own tool-call arguments.
    fn strip_call_id(args: &Value) -> Value {
        let mut clean = args.clone();
        if let Some(obj) = clean.as_object_mut() {
            obj.remove("__call_id__");
        }
        clean
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
                self.call_llm(persona.clone(), request.clone(), session, context),
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
                                            args: Self::strip_call_id(&args),
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

                                let tool_started = Instant::now();
                                let mut result = self.call_tool(&name, &mut args, &call_id).await;
                                self.tool_call_count.fetch_add(1, Ordering::Relaxed);
                                self.tool_time_nanos.fetch_add(
                                    tool_started.elapsed().as_nanos() as u64,
                                    Ordering::Relaxed,
                                );

                                match self
                                    .react_app
                                    .after_tool_result(
                                        &name,
                                        &mut result,
                                        &call_id,
                                        session,
                                        context,
                                    )
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
                                        args: Self::strip_call_id(&args),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: ret.to_string(),
                                    });
                                } else {
                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: Self::strip_call_id(&args),
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
                                            args: Self::strip_call_id(&args),
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

                                let tool_started = Instant::now();
                                let mut result = self.call_tool(&name, &mut args, &call_id).await;
                                self.tool_call_count.fetch_add(1, Ordering::Relaxed);
                                self.tool_time_nanos.fetch_add(
                                    tool_started.elapsed().as_nanos() as u64,
                                    Ordering::Relaxed,
                                );

                                match self
                                    .react_app
                                    .after_tool_result(
                                        &name,
                                        &mut result,
                                        &call_id,
                                        session,
                                        context,
                                    )
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
                                        args: Self::strip_call_id(&args),
                                    });
                                    session.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: ret.to_string(),
                                    });
                                } else {
                                    session.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: Self::strip_call_id(&args),
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
                                thought =
                                    thought[(pos + "Final Answer:".len())..].trim().to_string();
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

    /// Run the ReAct loop to completion and return the final answer.
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

        let result = self.react_loop(persona, request, session, context).await?;

        Ok(result)
    }

    /// Run the ReAct loop, streaming tokens as they arrive.
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
                        yield Err(e);
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
                        Ok(StreamToken::ToolResult { name, output, ms }) => {
                            // Providers never emit this; pass through for
                            // callers that replay recorded token streams.
                            yield Ok(StreamToken::ToolResult { name, output, ms });
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

                            let tool_started = Instant::now();
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
                            self.tool_time_nanos
                                .fetch_add(tool_started.elapsed().as_nanos() as u64, Ordering::Relaxed);

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

                            // Surface the finished call to stream observers
                            // (GUI timeline) with its wall-clock duration.
                            yield Ok(StreamToken::ToolResult {
                                name: name.clone(),
                                output: result_text.clone(),
                                ms: tool_started.elapsed().as_millis() as u64,
                            });

                            session.push(LlmMessage::assistant_tool_call(call_id.clone(), name.clone(), Self::strip_call_id(&args)));
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

    /// Usage reported by the most recent LLM call (the current request).
    ///
    /// For lifetime sums across every call, see
    /// [`ReActEngine::cumulative_token_usage`].
    pub fn token_usage(&self) -> TokenUsage {
        self.token_counter.usage()
    }

    /// Lifetime prompt and completion totals across every LLM call this
    /// engine has made.
    ///
    /// Unlike [`ReActEngine::token_usage`], these accumulate across runs
    /// that share a cached engine; take a difference between snapshots for
    /// a per-run value.
    pub fn cumulative_token_usage(&self) -> (u64, u64) {
        self.token_counter.lifetime_totals()
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

    /// Total wall time spent executing tools during this engine's lifetime.
    ///
    /// Like [`ReActEngine::tool_call_count`], this accumulates across runs
    /// that share a cached engine; take a difference between snapshots for
    /// a per-run value.
    pub fn tool_time(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.tool_time_nanos.load(Ordering::Relaxed))
    }

    /// Reset the tool call counter.
    pub fn reset_tool_call_count(&self) {
        self.tool_call_count.store(0, Ordering::Relaxed);
    }

    /// The shared stop flag.
    pub fn get_stop_flag(&self) -> Arc<AtomicBool> {
        self.stop_flag.clone()
    }

    /// Set the shared stop flag.
    pub fn set_stop_flag(&mut self, flag: bool) {
        self.stop_flag.store(flag, Ordering::SeqCst);
    }

    /// Cancel running tools and signal the loop to stop.
    pub fn stop(&mut self) {
        let running = self.run_manager.cancel_all_running();
        for (call_id, name) in running {
            if let Some(tool) = self.tools.get(&name) {
                tool.cancel(&call_id);
            }
        }
        self.set_stop_flag(true);
    }

    /// Signal the loop to stop.
    pub fn close(&mut self) {
        self.set_stop_flag(true);
    }
}
