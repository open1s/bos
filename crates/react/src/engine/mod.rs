//! The ReAct engine: an LLM loop with tools, skills, streaming, and telemetry.

use crate::llm::types::{load_skill_tool, ReactContext, ReactSession};
use crate::llm::vendor::responses::{ResponsesContentPart, ResponsesItem};
use crate::llm::{LlmClient, LlmError, LlmMessage, LlmRequest, LlmResponse, StreamToken};
use crate::resilience::{ReActResilience, ResilienceError};
use crate::runtime::{HookDecision, ReActApp};
use crate::telemetry::{Telemetry, TelemetryEvent, TokenBudgetReport, TokenCounter, TokenUsage};
use crate::tool::registry::{FnTool, ToolVariant};
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

/// What the model is told when a turn reaches its step guard.
///
/// The guard exists so a runaway tool loop cannot spend the machine's time
/// forever. It must never end a turn mid-thought: the turn that reaches it gets
/// one closing request carrying this note, so the reader always receives what
/// the run established and what is left, instead of a `Done` with no answer.
pub const STEP_GUARD_NOTE: &str = "You have reached this turn's step budget, so no further tool calls will run. Reply in plain text now: what you established, what is still incomplete, and the next concrete step. Do not call any tools.";

/// What the model is told when a step came back empty.
///
/// A reasoning model can spend a whole step thinking and emit neither text nor
/// a tool call; without this the turn would close on a blank answer and the
/// reader would receive nothing. The retry is bounded to one per turn, and it
/// asks for the two things a harness can act on: a tool call, or an answer.
pub const EMPTY_ANSWER_NOTE: &str = "Your last reply contained no text and no tool call. Do not think further: call a tool now, or answer the reader in plain text.";

/// What the model is told when the same tool failure repeats three times.
///
/// A harness cannot fix an environment that keeps refusing the same call, and
/// letting the model retry it burns a full LLM round per attempt. The third
/// identical failure closes the tool loop and asks for a report instead.
pub const REPEAT_FAILURE_NOTE: &str = "The same tool failure has repeated three times. Stop retrying it. Report the exact error, what you tried, and what must change in the environment; do not call that tool again this turn.";

/// The number of identical failures that closes the tool loop.
const REPEAT_FAILURE_LIMIT: usize = 3;

/// Counts identical failed tool results so a retry loop cannot run forever.
#[derive(Default)]
struct RepeatedFailure {
    seen: std::collections::HashMap<String, usize>,
}

impl RepeatedFailure {
    /// Record one result; true exactly when it crosses the limit.
    fn note(&mut self, tool: &str, result: &str) -> bool {
        let Some(signature) = failure_signature(tool, result) else {
            return false;
        };
        let count = self.seen.entry(signature).or_insert(0);
        *count += 1;
        *count == REPEAT_FAILURE_LIMIT
    }
}

/// The most text one tool result may add to the model context.
///
/// A tool that prints a megabyte (a build log, a base64 dump) must not become
/// the next request. The head and tail are kept because the failure and the
/// summary usually sit at opposite ends; the middle is the part a model can
/// ask for again with a narrower call.
const TOOL_RESULT_CAP: usize = 48 * 1024;

/// Cap one tool result, preserving both ends and naming what was dropped.
fn cap_tool_result(text: &str) -> String {
    if text.len() <= TOOL_RESULT_CAP {
        return text.to_string();
    }
    // Reserve room for the omission marker so the cap never grows a result.
    let half = TOOL_RESULT_CAP.saturating_sub(256) / 2;
    let mut head_end = half;
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - half;
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n\n[... {} bytes omitted; ask for a narrower read or grep ...]\n\n{}",
        &text[..head_end],
        tail_start.saturating_sub(head_end),
        &text[tail_start..]
    )
}

/// A normalized signature for a failed tool result, or none for a success.
///
/// Digits are dropped (they carry PIDs, timings and line numbers, which differ
/// between two attempts at the same broken command) and the rest is lowercased
/// and collapsed to one line. The prefix is bounded so two megabyte failures
/// still compare cheaply.
fn failure_signature(tool: &str, result: &str) -> Option<String> {
    let failed = result.starts_with("Error:") || result.contains("\"success\":false");
    if !failed {
        return None;
    }
    let mut signature = String::with_capacity(96);
    signature.push_str(tool);
    signature.push(':');
    let prefix = tool.len() + 1;
    let limit = prefix + 160;
    for ch in result.chars() {
        if ch.is_ascii_digit() {
            continue;
        }
        if ch.is_alphanumeric() {
            signature.push(ch.to_ascii_lowercase());
        } else if signature.len() > prefix && !signature.ends_with(' ') {
            signature.push(' ');
        }
        if signature.len() >= limit {
            break;
        }
    }
    Some(signature)
}

/// The prompt overhead a mid-turn budget must leave room for.
///
/// The session history is only one bucket of a request: the persona and the
/// tool schemas ride outside it. Six thousand tokens covers the shipped BSH
/// system prompt and its core tool array, so a history at this ceiling still
/// fits the provider window.
const MID_TURN_FIXED_RESERVE: usize = 6_000;
/// The newest messages a mid-turn compaction keeps verbatim.
const MID_TURN_KEEP_RECENT: usize = 8;
/// The longest mechanical summary a mid-turn compaction writes.
const MID_TURN_SUMMARY_CHARS: usize = 4_000;

/// The history ceiling for mid-turn compaction, or none when the budget is off
/// or too small to leave a usable history window.
fn mid_turn_ceiling(budget: usize) -> Option<usize> {
    (budget > MID_TURN_FIXED_RESERVE).then(|| budget - MID_TURN_FIXED_RESERVE)
}

/// The ReAct loop: LLM calls, tool execution, skills, telemetry, and resilience.
pub struct ReActEngine<A: ReActApp> {
    llm: Box<dyn LlmClient<A::Session, A::Context> + Send + Sync>,
    tools: Arc<ToolRegistry>,
    /// Send-side ceiling for one request; 0 disables mid-turn compaction.
    context_budget: usize,
    /// The step guard, when the caller asks for one. `None` is the harness
    /// contract: a turn ends when the MODEL stops asking for tools, and a
    /// healthy run must not be cut off by a number. The bound exists for
    /// embedders that need a runaway guard, not as the exit rule.
    max_steps: Option<usize>,
    parallel_tool_calls: bool,
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
    max_steps: Option<usize>,
    context_budget: usize,
    parallel_tool_calls: bool,
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

    /// Register a synchronous tool at runtime.
    pub fn register_tool(&self, t: Box<dyn Tool>) {
        self.tools.register_sync(t);
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

        // A turn ends when the MODEL stops asking for tools. The step bound is
        // a guard, not the exit rule: when it trips, the model gets one closing
        // turn, so a guarded turn still ends with an answer.
        let mut steps = 0usize;
        let mut closing = false;
        // One directed retry when a step comes back empty (see
        // EMPTY_ANSWER_NOTE); a second blank reply ends the turn.
        let mut nudged = false;
        let mut repeated = RepeatedFailure::default();
        'turn: loop {
            if self.max_steps.is_some_and(|bound| steps >= bound) {
                if closing {
                    break;
                }
                closing = true;
                session.push(LlmMessage::user_text(STEP_GUARD_NOTE));
            }
            steps += 1;

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

            self.ensure_context_fits(session, persona.as_deref());

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

                        // The model's own words belong in the transcript even
                        // when the same message also carries tool calls: a
                        // dropped preamble is context the next step has to
                        // rebuild, and it is what the reader was told. Recorded
                        // BEFORE the calls so the session keeps the model's own
                        // order.
                        let mut recorded = false;
                        if let Some(content) = &message.content {
                            if !content.trim().is_empty() {
                                session.push(LlmMessage::assistant(content.clone()));
                                recorded = true;
                            }
                        }

                        // On the closing turn the model is asked for prose: a
                        // call it still emits is not run, so the guard costs an
                        // answer rather than one more truncated step.
                        if let Some(tool_calls) = message.tool_calls.as_ref().filter(|_| !closing) {
                            // Phase A: every call of this response is parsed and
                            // passes its before-hook first. The hooks take the
                            // session, so they stay in the model own order.
                            let mut batch: Vec<(String, String, serde_json::Value)> = Vec::new();
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
                                                "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.

skill_dir: {}

{}",
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

                                batch.push((name, call_id, args));
                            }

                            // The calls of one response do not depend on each
                            // other, so a batch of more than one runs concurrently
                            // when the caller asked for it (see AgentConfig). Each
                            // call keeps its own mutable args, so a tool that
                            // rewrites its input is read back the way it left it.
                            let results: Vec<Result<serde_json::Value, ReactError>> =
                                if self.parallel_tool_calls && batch.len() > 1 {
                                    let batch_started = Instant::now();
                                    let this: &Self = &*self;
                                    let joined = futures::future::join_all(batch.iter_mut().map(
                                        |entry: &mut (String, String, serde_json::Value)| {
                                            let (name, call_id, args) =
                                                (&entry.0, &entry.1, &mut entry.2);
                                            async move { this.call_tool(name, args, call_id).await }
                                        },
                                    ))
                                    .await;
                                    self.tool_call_count
                                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                                    self.tool_time_nanos.fetch_add(
                                        batch_started.elapsed().as_nanos() as u64,
                                        Ordering::Relaxed,
                                    );
                                    joined
                                } else {
                                    let mut out = Vec::with_capacity(batch.len());
                                    for (name, call_id, args) in batch.iter_mut() {
                                        let tool_started = Instant::now();
                                        let result = self.call_tool(name, args, call_id).await;
                                        self.tool_call_count.fetch_add(1, Ordering::Relaxed);
                                        self.tool_time_nanos.fetch_add(
                                            tool_started.elapsed().as_nanos() as u64,
                                            Ordering::Relaxed,
                                        );
                                        out.push(result);
                                    }
                                    out
                                };

                            // Hooks and the session keep the model's order, so what
                            // the transcript records does not depend on which call
                            // finished first.
                            for ((name, call_id, args), mut result) in
                                batch.into_iter().zip(results)
                            {
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
                                let failure_text = match &result {
                                    Ok(ret) => ret.to_string(),
                                    Err(error) => format!("Error: {error:?}"),
                                };
                                if repeated.note(&name, &failure_text) {
                                    closing = true;
                                    session.push(LlmMessage::user_text(REPEAT_FAILURE_NOTE));
                                }
                            }
                        }

                        // A response with no tool call is the answer: the text
                        // recorded above IS the final thought.
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
                                }
                            }
                        }

                        // A blank step ends the turn with nothing: give the model
                        // one directed retry instead of closing on silence.
                        if !found_tool_call && thought.trim().is_empty() && !nudged {
                            nudged = true;
                            session.push(LlmMessage::user_text(EMPTY_ANSWER_NOTE));
                            continue 'turn;
                        }

                        let finish = choice.finish_reason.as_deref();
                        if finish.is_some() && finish != Some("tool_calls") {
                            if !recorded {
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
                        if !found_tool_call {
                            if !recorded {
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
                                // Closing turn: the call is not run (see the
                                // OpenAI arm); its text becomes the answer.
                                if closing {
                                    continue;
                                }
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
                                let failure_text = match &result {
                                    Ok(ret) => ret.to_string(),
                                    Err(error) => format!("Error: {error:?}"),
                                };
                                if repeated.note(&name, &failure_text) {
                                    closing = true;
                                    session.push(LlmMessage::user_text(REPEAT_FAILURE_NOTE));
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
            let mut steps = 0usize;
            // See `react_loop`: the bound guards against a runaway loop, and a
            // guarded turn closes with an answer instead of a bare `Done`.
            let mut closing = false;
            // One directed retry when a step streams nothing at all.
            let mut nudged = false;
            // The same failed call three times closes the loop and asks for a
            // report instead of paying one more round.
            let mut repeated = RepeatedFailure::default();

            context.add_tool(load_skill_tool());

            loop {
                if self.stop_flag.load(Ordering::SeqCst) {
                    yield Err(ReactError::HookAbort("Execution stopped by user".to_string()));
                    break;
                }

                // The streaming loop drives the same ReAct steps as
                // `react_loop`, so it honours the same guard. The bound stops a
                // runaway loop; it does not end a turn, which is why reaching it
                // pushes one closing request instead of yielding `Done` with no
                // answer.
                if self.max_steps.is_some_and(|bound| steps >= bound) {
                    if closing {
                        yield Ok(StreamToken::Done);
                        break;
                    }
                    closing = true;
                    session.push(LlmMessage::user_text(STEP_GUARD_NOTE));
                }
                steps += 1;

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

                self.ensure_context_fits(session, persona.as_deref());

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
                // Whether the words collected so far already reached the session.
                let mut recorded = false;

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
                            // The provider's cache breakdown, when it sends one: the
                            // only source for a cache-hit figure, and a provider that
                            // omits it has to read as unknown rather than as 0%.
                            let cached = usage
                                .prompt_tokens_details
                                .as_ref()
                                .and_then(|d| d.cached_tokens)
                                .unwrap_or(0);
                            let token_usage = TokenUsage::new(
                                usage.prompt_tokens,
                                usage.completion_tokens,
                            )
                            .with_cached(cached);
                            self.token_counter.update_from_response(token_usage);
                            yield Ok(StreamToken::Usage(usage));
                        }
                        Ok(StreamToken::Done) => {
                            break;// End of LLM response stream
                        }
                        Ok(StreamToken::ToolCall { name, mut args, id }) => {
                            // The closing turn asks for prose, not work: a call
                            // here is not run, so the turn ends with an answer
                            // instead of one more step.
                            if closing {
                                continue;
                            }
                            saw_tool_call = true;
                            // Keep the words that introduced the call: the
                            // transcript is the model's own context, so a dropped
                            // preamble is context it has to rebuild on the next
                            // step. Pushed before the call, in the model's order.
                            if !recorded && !full_response.trim().is_empty() {
                                session.push(LlmMessage::assistant(full_response.clone()));
                                recorded = true;
                            }
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
                            let result_text = cap_tool_result(&result_text);

                            // A failed call is the one line the log was
                            // missing: policy refusals, broken environments and
                            // timeouts used to be visible only to the model.
                            if result.is_err() || result_text.contains("\"success\":false") {
                                let head: String = result_text.chars().take(240).collect();
                                log::warn!("tool {name} failed: {head}");
                            }

                            // Surface the finished call to stream observers
                            // (GUI timeline) with its wall-clock duration.
                            yield Ok(StreamToken::ToolResult {
                                name: name.clone(),
                                output: result_text.clone(),
                                ms: tool_started.elapsed().as_millis() as u64,
                            });

                            session.push(LlmMessage::assistant_tool_call(call_id.clone(), name.clone(), Self::strip_call_id(&args)));
                            session.push(LlmMessage::tool_result(call_id.clone(), result_text.clone()));
                            // A failing command that is retried unchanged is
                            // the environment refusing, not the model learning:
                            // three strikes and the turn closes with a report.
                            if repeated.note(&name, &result_text) {
                                closing = true;
                                session.push(LlmMessage::user_text(REPEAT_FAILURE_NOTE));
                            }
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
                    if !recorded {
                        session.push(LlmMessage::assistant(full_response.clone()));
                    }
                }

                if !saw_tool_call {
                    // A step that produced neither words nor a call would end
                    // the turn on silence. Ask once for the two things a harness
                    // can act on - a tool call or an answer - then close.
                    if full_response.trim().is_empty() && !nudged {
                        nudged = true;
                        session.push(LlmMessage::user_text(EMPTY_ANSWER_NOTE));
                        continue;
                    }
                    // A turn ends only when nothing waits for its next step.
                    // This is the reference's next-step inbox rule (the agent
                    // loop checks the inbox length before closing a turn): a
                    // steer that arrived while this response streamed keeps the
                    // turn open, and the next iteration's before_llm_call
                    // appends it as a user turn. Without this, a steer could
                    // only be taken when a tool call happened to force another
                    // step.
                    if self.react_app.has_pending_step_input() {
                        continue;
                    }
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

    /// Lifetime count of prompt tokens the provider served from its cache.
    ///
    /// Zero for a provider that never reports the cache breakdown. Take a
    /// difference between snapshots for a per-run value, the way
    /// [`ReActEngine::cumulative_token_usage`] is used.
    pub fn cumulative_cached_tokens(&self) -> u64 {
        self.token_counter.lifetime_cached_tokens()
    }

    /// Lifetime prompt and completion totals across every LLM call this engine
    /// has made.
    ///
    /// Unlike [ReActEngine::token_usage], these accumulate across runs that
    /// share a cached engine; take a difference between snapshots for a
    /// per-run value.
    pub fn cumulative_token_usage(&self) -> (u64, u64) {
        self.token_counter.lifetime_totals()
    }

    /// Get a budget report showing usage vs limits
    pub fn token_budget_report(&self) -> TokenBudgetReport {
        self.token_counter.report()
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

    /// Mechanically condense the history when the next request would exceed
    /// the send-side budget.
    ///
    /// This is the mid-turn counterpart of the host's pre-turn compaction: a
    /// long tool loop can add tens of kilobytes after the turn opened, and
    /// without this check the next request pays for a window nobody priced.
    /// The mechanical summary costs no second model call, which is what makes
    /// it safe to run at a step boundary.
    fn ensure_context_fits(&self, session: &mut A::Session, persona: Option<&str>) {
        let Some(ceiling) = mid_turn_ceiling(self.context_budget) else {
            return;
        };
        let persona_tokens = persona
            .map(|text| text.chars().count().div_ceil(4))
            .unwrap_or(0);
        for _ in 0..4 {
            let Some(tokens) = session.estimate_tokens() else {
                return;
            };
            if tokens + persona_tokens < ceiling {
                return;
            }
            if !session.compact_mechanical(MID_TURN_KEEP_RECENT, MID_TURN_SUMMARY_CHARS) {
                return;
            }
        }
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

#[cfg(test)]
mod repeat_guard_tests {
    use super::{
        cap_tool_result, failure_signature, mid_turn_ceiling, RepeatedFailure,
        MID_TURN_FIXED_RESERVE, REPEAT_FAILURE_LIMIT, TOOL_RESULT_CAP,
    };

    #[test]
    fn a_successful_result_has_no_failure_signature() {
        assert!(failure_signature("bash", "{\"success\":true,\"exit_code\":0}").is_none());
        assert!(failure_signature("read_file", "{\"content\":\"ok\"}").is_none());
    }

    #[test]
    fn digits_and_spacing_do_not_split_one_failure() {
        let first = "{\"stdout\":\"\",\"stderr\":\"timed out after 30s\",\"exit_code\":1,\"success\":false}";
        let second = "{\"stdout\":\"\",\"stderr\":\"timed out after 60s\",\"exit_code\":1,\"success\":false}";
        assert_eq!(
            failure_signature("bash", first),
            failure_signature("bash", second)
        );
    }

    #[test]
    fn three_identical_failures_trip_exactly_once() {
        let result = "{\"stdout\":\"Timed out waiting\",\"success\":false}";
        let mut guard = RepeatedFailure::default();
        for _ in 0..REPEAT_FAILURE_LIMIT - 1 {
            assert!(!guard.note("bash", result));
        }
        assert!(guard.note("bash", result), "the third failure trips");
        assert!(!guard.note("bash", result), "the note is pushed once");
    }

    #[test]
    fn a_budget_without_room_for_history_stays_off() {
        assert_eq!(mid_turn_ceiling(0), None);
        assert_eq!(mid_turn_ceiling(MID_TURN_FIXED_RESERVE), None);
        assert_eq!(
            mid_turn_ceiling(MID_TURN_FIXED_RESERVE + 1),
            Some(1),
            "one token of history room is a real window"
        );
    }

    #[test]
    fn a_huge_tool_result_keeps_both_ends_and_names_the_drop() {
        let text = format!("HEAD{}TAIL", "x".repeat(TOOL_RESULT_CAP));
        let capped = cap_tool_result(&text);
        assert!(capped.starts_with("HEAD"));
        assert!(capped.ends_with("TAIL"));
        assert!(capped.contains("bytes omitted"));
        assert!(
            capped.len() < text.len(),
            "the cap actually drops bytes: {} vs {}",
            capped.len(),
            text.len()
        );
    }

    #[test]
    fn a_different_tool_does_not_share_the_counter() {
        let result = "{\"success\":false,\"stderr\":\"refused\"}";
        let mut guard = RepeatedFailure::default();
        for _ in 0..REPEAT_FAILURE_LIMIT - 1 {
            guard.note("bash", result);
        }
        assert!(!guard.note("read_file", result));
    }
}
