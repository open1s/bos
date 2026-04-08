use crate::llm::{
    LlmClient, LlmContext, LlmError, LlmMessage, LlmRequest, LlmResponse, LlmTool, Skill,
    StreamToken,
};
use crate::memory::Memory;
use crate::resilience::{ReActResilience, ResilienceError};
use crate::telemetry::{Telemetry, TelemetryEvent};
use crate::token_counter::{TokenBudgetReport, TokenCounter, TokenUsage};
use crate::tool::{Tool, ToolRegistry};
use futures::Stream;
use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
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
    Resilience(#[from] ResilienceError<LlmError>),
}

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("LLM is required")]
    MissingLlm,
}

/// Plan mode configuration - controls how plans are generated and displayed
#[derive(Debug, Clone)]
pub enum PlanMode {
    /// No plan mode - execute immediately (default)
    None,
    /// Generate plan before first action, show to user for approval
    ShowFirst,
    /// Always show plan before executing any tool
    AlwaysShow,
    /// Generate plan but execute without waiting for approval (informational only)
    Silent,
}

/// Represents a generated plan from the ReAct engine
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// The user's original request
    pub user_input: String,
    /// Steps the agent intends to take
    pub steps: Vec<PlanStep>,
    /// Whether this plan requires user approval before execution
    pub requires_approval: bool,
    /// Estimated number of tool calls
    pub estimated_steps: usize,
}

/// A single step in an execution plan
#[derive(Debug, Clone)]
pub struct PlanStep {
    /// Step number (1-indexed)
    pub step_number: usize,
    /// The tool to call (if any)
    pub tool_name: Option<String>,
    /// The reasoning for this step
    pub reasoning: String,
    /// Input parameters for the tool
    pub parameters: Value,
}

/// Parse result indicating whether the LLM output is a tool call or text
#[derive(Debug)]
enum ParsedIntent {
    /// LLM explicitly requested a tool call (structured, from LLM API)
    ToolCall {
        name: String,
        input: Value,
        call_id: Option<String>,
    },
    /// LLM is providing a final answer (text only)
    FinalAnswer { text: String },
}

/// Parse LLM output into a clear intent using a strict boundary mechanism.
///
/// Boundary rules:
/// 1. "Final Answer:" prefix → absolute boundary, everything after is text
/// 2. Tool call format → only valid if tool exists in registry
/// 3. Plain text → always final answer
fn parse_llm_intent(
    thought: &str,
    available_tools: &HashMap<String, Box<dyn Tool>>,
) -> ParsedIntent {
    let thought = thought.trim();
    if thought.is_empty() {
        return ParsedIntent::FinalAnswer {
            text: String::new(),
        };
    }

    // "Final Answer:" is an absolute boundary — never parse as tool call
    if let Some(pos) = thought.find("Final Answer:") {
        let answer = thought[(pos + "Final Answer:".len())..].trim().to_string();
        return ParsedIntent::FinalAnswer { text: answer };
    }

    // Tool call only if format matches AND tool exists in registry
    if let Some((name, input)) = parse_unified_tool_call(thought) {
        if available_tools.contains_key(&name) {
            return ParsedIntent::ToolCall {
                name,
                input,
                call_id: None,
            };
        }
    }

    // Everything else is text
    ParsedIntent::FinalAnswer {
        text: thought.to_string(),
    }
}

/// Unified tool call parser supporting 5 formats with correct priority:
/// Priority 1: JSON object: {"name": "tool", "parameters": {...}}
/// Priority 2: ReAct: Action: tool\nInput: {...}
/// Priority 3: Function call: tool_name({"arg": "value"})
/// Priority 4: XML tags: <tool>args</tool>
/// Priority 5: LaTeX boxed: $\boxed{...}
///
/// Each parser validates its output and returns None if type checking fails.
fn parse_unified_tool_call(thought: &str) -> Option<(String, Value)> {
    let thought = thought.trim();
    if thought.is_empty() {
        return None;
    }

    // Priority order: JSON > ReAct > Function > XML > LaTeX
    parse_json_object(thought)
        .or_else(|| parse_react_format(thought))
        .or_else(|| parse_function_call(thought))
        .or_else(|| parse_xml_tags(thought))
        .or_else(|| parse_latex_boxed(thought))
}

/// Parse function call format: tool_name({"key": "value"}) or tool_name("value")
/// Returns: Option<(tool_name: String, args: Value)> where args must be an Object
fn parse_function_call(thought: &str) -> Option<(String, Value)> {
    let cleaned = thought
        .strip_prefix("<|python_tag|>")
        .or_else(|| thought.strip_prefix("<|python_tag|> "))
        .unwrap_or(thought);

    let paren_pos = cleaned.find('(')?;
    let func_name = cleaned[..paren_pos].trim().to_string();

    // Valid tool name: alphanumeric, underscore, slash, hyphen, dot
    if func_name.is_empty()
        || !func_name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '/' || c == '-' || c == '.')
    {
        return None;
    }

    let rest = &cleaned[paren_pos..];
    let close_paren = rest.find(')')?;
    let args_str = rest[1..close_paren].trim();

    if args_str.is_empty() {
        return Some((func_name, Value::Object(serde_json::Map::new())));
    }

    // Try parsing as JSON object - type check: must be Object
    if let Ok(obj) = serde_json::from_str::<Value>(args_str) {
        if obj.is_object() {
            return Some((func_name, obj));
        }
        // Single value: wrap in object with "value" key
        return Some((func_name, serde_json::json!({ "value": obj })));
    }

    // Try quoted string - type check: convert to Object with "value" key
    let stripped = args_str
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            args_str
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        });

    if let Some(s) = stripped {
        return Some((func_name, serde_json::json!({ "value": s })));
    }

    // Try comma-separated args: arg1, arg2, arg3 (or single key=value)
    // Type check: always returns Object
    let args: Vec<&str> = args_str.split(',').collect();
    if !args.is_empty() {
        let mut obj = serde_json::Map::new();
        for (i, arg) in args.iter().enumerate() {
            let arg_trimmed = arg.trim();
            if let Some(eq_pos) = arg_trimmed.find('=') {
                let key = arg_trimmed[..eq_pos].trim();
                let val = arg_trimmed[eq_pos + 1..].trim();
                if let Ok(v) = serde_json::from_str(val) {
                    obj.insert(key.to_string(), v);
                } else {
                    obj.insert(key.to_string(), Value::String(val.to_string()));
                }
            } else {
                obj.insert(format!("arg{}", i), Value::String(arg_trimmed.to_string()));
            }
        }
        return Some((func_name, Value::Object(obj)));
    }

    None
}

/// Parse JSON object format: {"name": "tool", "parameters": {...}}
/// Type check: returns (name: String, args: Value) where name must be non-empty string
fn parse_json_object(thought: &str) -> Option<(String, Value)> {
    let start = thought.find('{')?;

    let mut depth = 0;
    let mut end = None;
    for (i, c) in thought.chars().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end?;
    if end <= start {
        return None;
    }

    let mut json_str = &thought[start..end];

    // Handle double-brace wrapping: {{...}} → {...}
    if json_str.starts_with("{{") && json_str.ends_with("}}") {
        json_str = &json_str[1..json_str.len() - 1];
    }

    let val: Value = serde_json::from_str(json_str).ok()?;

    if let (Some(name), Some(params)) = (
        val.get("name").and_then(|v| v.as_str()),
        val.get("parameters")
            .or_else(|| val.get("args"))
            .or_else(|| val.get("arguments")),
    ) {
        // Type check: name must be non-empty
        if name.is_empty() {
            return None;
        }
        return Some((name.to_string(), params.clone()));
    }

    if let Some(obj) = val.as_object() {
        if obj.len() == 1 {
            if let Some((name, params)) = obj.iter().next() {
                return Some((name.clone(), params.clone()));
            }
        }
    }

    None
}

/// Parse ReAct format: Action: tool\nAction Input: {...}
/// Type check: returns (name: String, args: Value) where name must be non-empty
fn parse_react_format(thought: &str) -> Option<(String, Value)> {
    let mut tool_name: Option<String> = None;
    let mut input = Value::Null;

    for line in thought.lines() {
        let l = line.trim().to_lowercase();

        if l.starts_with("action:") || l.starts_with("tool:") || l.starts_with("invoke:") {
            if let Some(pos) = line.find(':') {
                tool_name = Some(line[pos + 1..].trim().to_string());
            }
        } else if l.starts_with("input:")
            || l.starts_with("parameters:")
            || l.starts_with("action input:")
            || l.starts_with("args:")
        {
            if let Some(pos) = line.find(':') {
                let raw = line[pos + 1..].trim();
                if !raw.is_empty() {
                    input = serde_json::from_str(raw)
                        .unwrap_or_else(|_| Value::String(raw.to_string()));
                }
            }
        }
    }

    // Type check: name must be non-empty
    tool_name
        .filter(|name| !name.is_empty())
        .map(|name| (name, input))
}

/// Parse LaTeX boxed format: $\boxed{"name": "tool", "parameters": {...}}$
/// Type check: delegates to parse_json_object and parse_function_call which have type validation
fn parse_latex_boxed(thought: &str) -> Option<(String, Value)> {
    let cleaned = thought
        .replace("$", "")
        .replace("\\$", "")
        .replace("\\boxed{", "")
        .replace("boxed{", "");

    let cleaned = if cleaned.ends_with("}}") {
        &cleaned[..cleaned.len() - 1]
    } else {
        &cleaned
    };

    parse_json_object(cleaned).or_else(|| parse_function_call(cleaned))
}

/// Parse XML tag format: <tool_name>args</tool_name> or <tool_name/>
/// Type check: tag name must be non-empty and have valid characters
fn parse_xml_tags(thought: &str) -> Option<(String, Value)> {
    let start_tag = thought.find('<')?;
    let end_tag = thought.find('>')?;
    if end_tag <= start_tag + 1 {
        return None;
    }

    let tag_content = &thought[start_tag + 1..end_tag];
    if tag_content.starts_with('/') || tag_content.starts_with('!') {
        return None;
    }

    // Self-closing or with content
    let is_self_closing = tag_content.ends_with('/');
    let tag_name = if is_self_closing {
        tag_content[..tag_content.len() - 1].trim().to_string()
    } else {
        tag_content
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    };

    if tag_name.is_empty() {
        return None;
    }

    // Valid tool name characters
    if !tag_name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '/' || c == '-' || c == '.')
    {
        return None;
    }

    // Find closing tag if not self-closing
    let args = if is_self_closing {
        Value::Object(serde_json::Map::new())
    } else {
        let close_pattern = format!("</{}>", tag_name);
        if let Some(close_pos) = thought.find(&close_pattern) {
            let args_content = &thought[end_tag + 1..close_pos];
            if let Ok(v) = serde_json::from_str(args_content) {
                v
            } else {
                Value::String(args_content.to_string())
            }
        } else {
            Value::Object(serde_json::Map::new())
        }
    };

    Some((tag_name, args))
}

#[allow(dead_code)]
pub struct ReActEngine {
    llm: Box<dyn LlmClient>,
    tools: ToolRegistry,
    memory: Memory,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<Arc<ReActResilience>>,
    llm_timeout_secs: u64,
    model: String,
    system_prompt: String,
    plan_mode: PlanMode,
    auto_continue: bool,
    checkpoint_interval: usize,
    token_counter: TokenCounter,
}

pub struct ReActEngineBuilder {
    llm: Option<Box<dyn LlmClient>>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<ReActResilience>,
    llm_timeout_secs: u64,
    model: String,
    system_prompt: String,
    plan_mode: PlanMode,
    auto_continue: bool,
    checkpoint_interval: usize,
    token_counter: TokenCounter,
}

impl ReActEngineBuilder {
    pub fn new() -> Self {
        Self {
            llm: None,
            tools: ToolRegistry::new(),
            max_steps: 10,
            telemetry: Telemetry::new(true),
            resilience: None,
            llm_timeout_secs: 120,
            model: String::new(),
            system_prompt: String::new(),
            plan_mode: PlanMode::None,
            auto_continue: false,
            checkpoint_interval: 5,
            token_counter: TokenCounter::with_default(),
        }
    }
}

impl Default for ReActEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReActEngineBuilder {
    pub fn llm(mut self, llm: Box<dyn LlmClient>) -> Self {
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

    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = prompt;
        self
    }

    pub fn plan_mode(mut self, mode: PlanMode) -> Self {
        self.plan_mode = mode;
        self
    }

    pub fn auto_continue(mut self, enabled: bool) -> Self {
        self.auto_continue = enabled;
        self
    }

    pub fn checkpoint_interval(mut self, interval: usize) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    pub fn build(self) -> Result<ReActEngine, BuilderError> {
        let llm = self.llm.ok_or(BuilderError::MissingLlm)?;
        Ok(ReActEngine {
            llm,
            tools: self.tools,
            memory: Memory::new(),
            max_steps: self.max_steps,
            telemetry: self.telemetry,
            resilience: self.resilience.map(Arc::new),
            llm_timeout_secs: self.llm_timeout_secs,
            model: self.model,
            system_prompt: self.system_prompt,
            plan_mode: self.plan_mode,
            auto_continue: self.auto_continue,
            checkpoint_interval: self.checkpoint_interval,
            token_counter: self.token_counter,
        })
    }
}

impl ReActEngine {
    pub fn new(llm: Box<dyn LlmClient>, max_steps: usize) -> Self {
        Self {
            llm,
            tools: ToolRegistry::new(),
            memory: Memory::new(),
            max_steps,
            telemetry: Telemetry::new(true),
            resilience: None,
            llm_timeout_secs: 120,
            model: String::new(),
            system_prompt: String::new(),
            plan_mode: PlanMode::None,
            auto_continue: false,
            checkpoint_interval: 5,
            token_counter: TokenCounter::with_default(),
        }
    }

    pub fn builder() -> ReActEngineBuilder {
        ReActEngineBuilder::new()
    }

    pub fn register_tool(&mut self, t: Box<dyn Tool>) {
        self.tools.register(t);
    }

    pub fn set_plan_mode(&mut self, mode: PlanMode) {
        self.plan_mode = mode;
    }

    /// Generate an execution plan for the given user input without executing it.
    /// This is the core Plan Mode implementation - shows plan before action.
    pub async fn plan(&mut self, user_input: &str) -> Result<ExecutionPlan, ReactError> {
        let openai_tools: Vec<LlmTool> = self
            .tools
            .tools
            .iter()
            .filter(|(_, tool)| !tool.is_skill())
            .map(|(name, tool)| LlmTool {
                name: name.to_string(),
                description: tool.description(),
                parameters: tool.json_schema(),
            })
            .collect();

        let plan_prompt = format!(
            "You are a planning agent. Given the user's request below, create a detailed execution plan.\n\
            Available tools:\n{}\n\n\
            User request: {}\n\n\
            Your plan should:\n\
            1. Analyze the request to understand what needs to be done\n\
            2. Identify which tools to use and in what order\n\
            3. Estimate the number of steps required\n\
            4. Output your plan in the following format:\n\
            PLAN:\n\
            Step 1: [tool_name] - [reasoning] - params: [input]\n\
            Step 2: [tool_name] - [reasoning] - params: [input]\n\
            ...\n\
            END_PLAN",
            self.tools_descriptions(),
            user_input
        );

        let context = LlmContext {
            tools: openai_tools,
            skills: Vec::new(),
            conversations: vec![LlmMessage::system(plan_prompt)],
            rules: Vec::new(),
            instructions: Vec::new(),
        };

        let request = LlmRequest {
            model: self.model.clone(),
            context,
            ..Default::default()
        };

        let llm_response = self.call_llm(request).await?;

        let steps = match llm_response {
            LlmResponse::Text(text) | LlmResponse::Partial(text) => self.parse_plan_steps(&text),
            LlmResponse::Done => Vec::new(),
            LlmResponse::ToolCall { .. } => Vec::new(),
        };

        let requires_approval =
            matches!(self.plan_mode, PlanMode::ShowFirst | PlanMode::AlwaysShow);

        Ok(ExecutionPlan {
            user_input: user_input.to_string(),
            steps,
            requires_approval,
            estimated_steps: 0,
        })
    }

    fn parse_plan_steps(&self, text: &str) -> Vec<PlanStep> {
        let mut steps = Vec::new();
        let mut step_number = 1;

        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("Step ") || line.starts_with("step ") {
                if let Some(colon_pos) = line.find(':') {
                    let content = line[colon_pos + 1..].trim();
                    let (tool_name, reasoning) = if let Some(dash_pos) = content.find('-') {
                        (
                            Some(content[..dash_pos].trim().to_string()),
                            content[dash_pos + 1..].trim().to_string(),
                        )
                    } else {
                        (None, content.to_string())
                    };
                    steps.push(PlanStep {
                        step_number,
                        tool_name,
                        reasoning,
                        parameters: Value::Null,
                    });
                    step_number += 1;
                }
            }
        }

        steps
    }

    /// Execute with plan mode - shows plan first, then executes if approved
    pub async fn react_with_plan(&mut self, user_input: &str) -> Result<String, ReactError> {
        match self.plan_mode {
            PlanMode::None => self.react(user_input).await,
            PlanMode::Silent => {
                let _ = self.plan(user_input).await;
                self.react(user_input).await
            }
            PlanMode::ShowFirst | PlanMode::AlwaysShow => {
                let plan = self.plan(user_input).await?;
                // Log the plan for visibility
                if !plan.steps.is_empty() {
                    info!("[Plan Mode] Generated plan with {} steps", plan.steps.len());
                    for step in &plan.steps {
                        info!(
                            "[Plan] Step {}: {:?} - {}",
                            step.step_number, step.tool_name, step.reasoning
                        );
                    }
                    // TODO: Implement actual user approval mechanism
                    // For now, we log the plan and continue. In production, this would
                    // integrate with an interactive approval system (e.g., TUI, webhook, etc.)
                    info!("[Plan Mode] Execute? (approval not yet implemented - use PlanMode::Silent for auto-execute)");
                }
                self.react(user_input).await
            }
        }
    }

    #[allow(unused)]
    fn tools_descriptions(&self) -> String {
        self.tools
            .tools
            .iter()
            .filter(|(_, tool)| !tool.is_skill())
            .map(|(name, tool)| format!("- {}: {}", name, tool.description()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Call LLM with optional resilience wrapper.
    async fn call_llm(&self, request: LlmRequest) -> Result<LlmResponse, ReactError> {
        let result = if let Some(ref resilience) = self.resilience {
            let llm = &self.llm;
            let request = request.clone();
            resilience
                .execute(move || llm.complete(request.clone()))
                .await
                .map_err(ReactError::from)?
        } else {
            self.llm.complete(request).await.map_err(ReactError::from)?
        };

        match &result {
            LlmResponse::Text(s) => info!("[ReAct] RECV: {}", s),
            LlmResponse::Partial(s) => info!("[ReAct] RECV: {}", s),
            LlmResponse::Done => info!("[ReAct] RECV: (done)"),
            LlmResponse::ToolCall { name, args, id } => {
                info!("[ReAct] RECV ToolCall: {} {:?} id={:?}", name, args, id);
            }
        }

        Ok(result)
    }

    /// Call LLM for streaming with optional resilience wrapper.
    /// Applies resilience to the future that creates the stream, then returns the stream.
    #[allow(dead_code)]
    async fn call_llm_stream(
        &self,
        request: LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send + '_>>, ReactError>
    {
        if let Some(ref resilience) = self.resilience {
            let llm = &self.llm;
            let request = request.clone();
            // Apply resilience to the future that produces the stream
            let stream_result = resilience
                .execute(move || llm.stream_complete(request.clone()))
                .await?;
            Ok(stream_result)
        } else {
            // Without resilience, just await and return the stream
            let stream = self.llm.stream_complete(request).await?;
            Ok(stream)
        }
    }

    /// Call tool - no resilience wrapper (only LLM calls need rate limiting)
    async fn call_tool(&self, name: &str, input: &Value) -> Result<Value, ReactError> {
        self.tools
            .call(name, input)
            .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
    }

    pub async fn react(&mut self, user_input: &str) -> Result<String, ReactError> {
        let openai_tools: Vec<LlmTool> = self
            .tools
            .tools
            .iter()
            .filter(|(_, tool)| !tool.is_skill())
            .map(|(name, tool)| LlmTool {
                name: name.to_string(),
                description: tool.description(),
                parameters: tool.json_schema(),
            })
            .collect();
        let openai_skills: Vec<Skill> = self
            .tools
            .tools
            .iter()
            .filter(|(_, tool)| tool.is_skill())
            .map(|(name, tool)| Skill {
                category: tool.category(),
                name: name.to_string(),
                description: tool.description(),
            })
            .collect();

        let mut context = LlmContext {
            tools: openai_tools,
            skills: openai_skills,
            conversations: Vec::new(),
            rules: Vec::new(),
            instructions: Vec::new(),
        };

        context
            .conversations
            .push(LlmMessage::system(self.system_prompt.clone()));
        context
            .conversations
            .push(LlmMessage::user(user_input.to_string()));

        let mut thought = String::new();
        let mut loaded_skills: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut step_count = 0;

        for _ in 0..self.max_steps {
            step_count += 1;

            if self.checkpoint_interval > 0 && step_count % self.checkpoint_interval == 0 {
                let checkpoint = serde_json::json!({
                    "step": step_count,
                    "thought": thought,
                    "context_size": context.conversations.len(),
                });
                self.telemetry.emit(&TelemetryEvent::Checkpoint(checkpoint));
            }
            let request = LlmRequest {
                model: self.model.clone(),
                context: context.clone(),
                ..Default::default()
            };

            info!("[ReACT] SEND: {:?}", request);
            let llm_response = match timeout(
                Duration::from_secs(self.llm_timeout_secs),
                self.call_llm(request),
            )
            .await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ReactError::Timeout("LLM prediction timed out".to_string())),
            };

            match llm_response {
                LlmResponse::ToolCall { name, args, id } => {
                    let call_id = id.unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                    if name == "load_skill" {
                        let skill_name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        if loaded_skills.contains_key(skill_name) {
                            let cached = loaded_skills.get(skill_name).unwrap();
                            context.conversations.push(LlmMessage::AssistantToolCall {
                                tool_call_id: call_id.clone(),
                                name: name.clone(),
                                args: args.clone(),
                            });
                            context.conversations.push(LlmMessage::ToolResult {
                                tool_call_id: call_id,
                                content: format!(
                                    "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.\n\n{}",
                                    skill_name, cached
                                ),
                            });
                            continue;
                        }
                    }

                    let result = self.call_tool(&name, &args).await;

                    if let Ok(ret) = &result {
                        if name == "load_skill" {
                            let skill_name =
                                args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let instructions = ret
                                .get("instructions")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !instructions.is_empty() {
                                loaded_skills
                                    .insert(skill_name.to_string(), instructions.to_string());
                            }
                        }

                        self.telemetry.emit(&TelemetryEvent::ToolInvocation {
                            tool: name.clone(),
                            input: args.clone(),
                            output: ret.clone(),
                        });

                        context.conversations.push(LlmMessage::AssistantToolCall {
                            tool_call_id: call_id.clone(),
                            name: name.clone(),
                            args: args.clone(),
                        });
                        context.conversations.push(LlmMessage::ToolResult {
                            tool_call_id: call_id,
                            content: ret.to_string(),
                        });
                    } else {
                        context.conversations.push(LlmMessage::AssistantToolCall {
                            tool_call_id: call_id.clone(),
                            name: name.clone(),
                            args: args.clone(),
                        });
                        context.conversations.push(LlmMessage::ToolResult {
                            tool_call_id: call_id,
                            content: format!("Error: {:?}", result),
                        });
                    }
                }
                LlmResponse::Text(text) | LlmResponse::Partial(text) => {
                    thought = text.clone();
                    info!("[ReAct] RECV: {}", thought);

                    self.telemetry.emit(&TelemetryEvent::ThoughtGenerated {
                        thought: thought.clone(),
                    });

                    match parse_llm_intent(&thought, &self.tools.tools) {
                        ParsedIntent::ToolCall {
                            name,
                            input,
                            call_id,
                        } => {
                            let call_id = call_id
                                .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));

                            if name == "load_skill" {
                                let skill_name =
                                    input.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                if loaded_skills.contains_key(skill_name) {
                                    let cached = loaded_skills.get(skill_name).unwrap();
                                    context.conversations.push(LlmMessage::AssistantToolCall {
                                        tool_call_id: call_id.clone(),
                                        name: name.clone(),
                                        args: input.clone(),
                                    });
                                    context.conversations.push(LlmMessage::ToolResult {
                                        tool_call_id: call_id,
                                        content: format!(
                                            "Skill '{}' is already loaded. DO NOT call load_skill again. Use the skill instructions below to answer the user's question directly.\n\n{}",
                                            skill_name, cached
                                        ),
                                    });
                                    continue;
                                }
                            }

                            let result = self.call_tool(&name, &input).await;

                            if let Ok(ret) = &result {
                                if name == "load_skill" {
                                    let skill_name =
                                        input.get("name").and_then(|v| v.as_str()).unwrap_or("");
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
                                    input: input.clone(),
                                    output: ret.clone(),
                                });

                                context.conversations.push(LlmMessage::AssistantToolCall {
                                    tool_call_id: call_id.clone(),
                                    name: name.clone(),
                                    args: input.clone(),
                                });
                                context.conversations.push(LlmMessage::ToolResult {
                                    tool_call_id: call_id,
                                    content: ret.to_string(),
                                });
                            } else {
                                context.conversations.push(LlmMessage::AssistantToolCall {
                                    tool_call_id: call_id.clone(),
                                    name: name.clone(),
                                    args: input.clone(),
                                });
                                context.conversations.push(LlmMessage::ToolResult {
                                    tool_call_id: call_id,
                                    content: format!("Error: {:?}", result),
                                });
                            }
                        }
                        ParsedIntent::FinalAnswer { text } => {
                            self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                                answer: text.clone(),
                            });
                            return Ok(text);
                        }
                    }
                }
                LlmResponse::Done => {
                    self.telemetry.emit(&TelemetryEvent::FinalAnswer {
                        answer: thought.clone(),
                    });
                    return Ok(thought);
                }
            }
        }
        self.telemetry.emit(&TelemetryEvent::FinalAnswer {
            answer: thought.clone(),
        });
        Ok(thought)
    }

    // Expose a simple memory checkpoint API for testing/observability
    pub fn save_memory_checkpoint(&self, path: &str) -> Result<(), std::io::Error> {
        self.memory.save_to_file(path)
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
