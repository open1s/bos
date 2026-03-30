use crate::llm::{LlmClient, LlmError, LlmMessage, LlmRequest, LlmResponse, StreamToken};
use crate::memory::Memory;
use crate::resilience::{ReActResilience, ResilienceError};
use crate::telemetry::{Telemetry, TelemetryEvent};
use crate::tool::{Tool, ToolRegistry};
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;
use tokio::time::{timeout, Duration};

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

fn parse_action_input(thought: &str) -> (Option<String>, Value) {
    let mut tool_name: Option<String> = None;
    let mut input: Value = Value::Null;
    for line in thought.lines() {
        let l = line.trim();
        let lower = l.to_ascii_lowercase();
        if lower.starts_with("action:") || lower.starts_with("tool:") {
            if let Some(pos) = l.find(':') {
                tool_name = Some(l[(pos + 1)..].trim().to_string());
            }
        } else if lower.starts_with("input:") || lower.starts_with("parameters:") || lower.starts_with("action input:") {
            if let Some(pos) = l.find(':') {
                let raw = l[(pos + 1)..].trim();
                if !raw.is_empty() {
                    if let Ok(v) = serde_json::from_str::<Value>(raw) {
                        input = v;
                    } else {
                        input = serde_json::json!({ "expression": raw });
                    }
                }
            }
        }
    }
    (tool_name, input)
}

pub struct ReActEngine {
    llm: Box<dyn LlmClient>,
    tools: ToolRegistry,
    memory: Memory,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<Arc<ReActResilience>>,
}

pub struct ReActEngineBuilder {
    llm: Option<Box<dyn LlmClient>>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
    resilience: Option<ReActResilience>,
}

impl ReActEngineBuilder {
    pub fn new() -> Self {
        Self {
            llm: None,
            tools: ToolRegistry::new(),
            max_steps: 3,
            telemetry: Telemetry::new(true),
            resilience: None,
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

    pub fn build(self) -> Result<ReActEngine, BuilderError> {
        let llm = self.llm.ok_or(BuilderError::MissingLlm)?;
        Ok(ReActEngine {
            llm,
            tools: self.tools,
            memory: Memory::new(),
            max_steps: self.max_steps,
            telemetry: self.telemetry,
            resilience: self.resilience.map(Arc::new),
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
        }
    }

    pub fn builder() -> ReActEngineBuilder {
        ReActEngineBuilder::new()
    }

    pub fn register_tool(&mut self, t: Box<dyn Tool>) {
        self.tools.register(t);
    }

    /// Call LLM with optional resilience wrapper.
    async fn call_llm(&self, request: LlmRequest) -> Result<String, ReactError> {
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

        match result {
            LlmResponse::Text(s) => Ok(s),
            LlmResponse::Partial(s) => Ok(s),
            LlmResponse::Done => Ok(String::new()),
            LlmResponse::ToolCall { name, args, id: _ } => {
                Err(ReactError::Malformed(format!("Unexpected tool call: {} {:?}", name, args)))
            }
        }
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
            let stream_result = resilience.execute(move || llm.stream_complete(request.clone())).await?;
            Ok(stream_result)
        } else {
            // Without resilience, just await and return the stream
            let stream = self.llm.stream_complete(request).await?;
            Ok(stream)
        }
    }

    /// Call tool with optional resilience wrapper.
    async fn call_tool(&self, name: &str, input: &Value) -> Result<Value, ReactError> {
        if let Some(ref resilience) = self.resilience {
            let name = name.to_string();
            let input = input.clone();
            let tools = &self.tools;
            let name = name.clone();
            resilience
                .execute(move || {
                    let tools = tools;
                    let input = input.clone();
                    let name = name.clone();
                    async move {
                        tools.call(&name, &input).map_err(|e| format!("{:?}", e))
                    }
                })
                .await
                .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        } else {
            self.tools
                .call(name, input)
                .map_err(|e| ReactError::ToolError(format!("{:?}", e)))
        }
    }

    pub async fn run(&mut self, user_input: &str) -> Result<String, ReactError> {
        let mut thought: String;
        for _ in 0..self.max_steps {
            let request = LlmRequest {
                model: self.llm.provider_name().to_string(),
                messages: vec![LlmMessage::User {
                    content: format!("User input: {}\nThought:", user_input),
                }],
                ..Default::default()
            };
            let llm_out = match timeout(Duration::from_millis(1000), self.call_llm(request)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(ReactError::Timeout("LLM prediction timed out".to_string())),
            };
            thought = llm_out;
            self.telemetry.emit(&TelemetryEvent::ThoughtGenerated {
                thought: thought.clone(),
            });

            if thought.to_lowercase().contains("final answer") {
                let ans = if let Some(pos) = thought.find("Final Answer:") {
                    thought[(pos + "Final Answer:".len())..].trim().to_string()
                } else {
                    thought.trim().to_string()
                };
                self.telemetry.emit(&TelemetryEvent::FinalAnswer { answer: ans.clone() });
                return Ok(ans);
            }

            let (tool_name, input) = parse_action_input(thought.as_str());
            let tool_name = match tool_name {
                Some(n) => n,
                None => {
                    return Err(ReactError::Malformed("Missing Action in llm output".to_string()))
                }
            };
            let res = self.call_tool(&tool_name, &input).await?;
            self.memory.push(thought.clone(), tool_name.clone(), res.clone());
            self.telemetry.emit(&TelemetryEvent::ToolInvocation {
                tool: tool_name.clone(),
                input: input.clone(),
                output: res.clone(),
            });

            let request = LlmRequest {
                model: self.llm.provider_name().to_string(),
                messages: vec![LlmMessage::User {
                    content: format!("Observation: {}\nThought:", res),
                }],
                ..Default::default()
            };
            thought = self.call_llm(request).await?;
            self.telemetry.emit(&TelemetryEvent::ThoughtGenerated {
                thought: thought.clone(),
            });

            if thought.to_lowercase().contains("final answer") {
                let ans = if let Some(pos) = thought.find("Final Answer:") {
                    thought[(pos + "Final Answer:".len())..].trim().to_string()
                } else {
                    thought.trim().to_string()
                };
                self.telemetry.emit(&TelemetryEvent::FinalAnswer { answer: ans.clone() });
                return Ok(ans);
            }
        }
        Err(ReactError::Malformed("Max steps reached without final answer".to_string()))
    }

    // Expose a simple memory checkpoint API for testing/observability
    pub fn save_memory_checkpoint(&self, path: &str) -> Result<(), std::io::Error> {
        self.memory.save_to_file(path)
    }
}
