use crate::llm::{Llm, LlmError};
use tokio::time::{timeout, Duration};
use log::info;
use crate::telemetry::{Telemetry, TelemetryEvent};
use crate::tool::{ToolRegistry, Tool};
use crate::memory::Memory;
use serde_json::Value;
use thiserror::Error;

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
                tool_name = Some(l[(pos+1)..].trim().to_string());
            }
        } else if lower.starts_with("input:") || lower.starts_with("parameters:") {
            if let Some(pos) = l.find(':') {
                let raw = l[(pos+1)..].trim();
                if !raw.is_empty() {
                    if let Ok(v) = serde_json::from_str(raw) { input = v; }
                }
            }
        }
    }
    (tool_name, input)
}

pub struct ReActEngine {
    llm: Box<dyn Llm>,
    tools: ToolRegistry,
    memory: Memory,
    max_steps: usize,
    telemetry: Telemetry,
}

pub struct ReActEngineBuilder {
    llm: Option<Box<dyn Llm>>,
    tools: ToolRegistry,
    max_steps: usize,
    telemetry: Telemetry,
}

impl ReActEngineBuilder {
    pub fn new() -> Self {
        Self {
            llm: None,
            tools: ToolRegistry::new(),
            max_steps: 3,
            telemetry: Telemetry::new(true),
        }
    }

    pub fn llm(mut self, llm: Box<dyn Llm>) -> Self {
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

    pub fn build(self) -> Result<ReActEngine, BuilderError> {
        let llm = self.llm.ok_or_else(|| BuilderError::MissingLlm)?;
        Ok(ReActEngine {
            llm,
            tools: self.tools,
            memory: Memory::new(),
            max_steps: self.max_steps,
            telemetry: self.telemetry,
        })
    }
}

impl ReActEngine {
    pub fn new(llm: Box<dyn Llm>, max_steps: usize) -> Self {
        Self {
            llm,
            tools: ToolRegistry::new(),
            memory: Memory::new(),
            max_steps,
            telemetry: Telemetry::new(true),
        }
    }

    pub fn register_tool(&mut self, t: Box<dyn Tool>) {
        self.tools.register(t);
    }

    pub async fn run(&mut self, user_input: &str) -> Result<String, ReactError> {
        // Minimal ReAct loop with Action/Observation pattern
        let mut thought = String::new();
        for _ in 0..self.max_steps {
            // 1) Prompt LLM with user_input to generate Thought + Action (with timeout)
            let prompt = format!("User input: {}\nThought:", user_input);
            info!("[ReActEngine] sending prompt to LLM: {}", prompt);
            let llm_out = match timeout(Duration::from_millis(1000), self.llm.predict(&prompt)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(ReactError::Llm(e)),
                Err(_) => return Err(ReactError::Timeout("LLM prediction timed out".to_string())),
            };
            thought = llm_out;
            self.telemetry.emit(&TelemetryEvent::ThoughtGenerated { thought: thought.clone() });
            info!("[ReActEngine] ThoughtGenerated: {}", thought);
            // 2) Parse Action and Input from Thought (via helper for robustness)
            let (tool_name, input) = parse_action_input(thought.as_str());
            let tool_name = match tool_name { Some(n) => n, None => return Err(ReactError::Malformed("Missing Action in llm output".to_string())) };
            let res = self.tools.call(&tool_name, &input).map_err(|e| ReactError::ToolError(format!("{:?}", e)))?;
            self.memory.push(thought.clone(), tool_name.clone(), res.clone());
            self.telemetry.emit(&TelemetryEvent::ToolInvocation { tool: tool_name.clone(), input: input.clone(), output: res.clone() });
            info!("[ReActEngine] tool '{}' produced observation: {}", tool_name, res);
            // 3) Next LLM prompt with observation
            let prompt2 = format!("Observation: {}\nThought:", res);
            thought = self.llm.predict(&prompt2).await.map_err(ReactError::Llm)?;
            self.telemetry.emit(&TelemetryEvent::ThoughtGenerated { thought: thought.clone() });
            // 4) Check for final
            if thought.to_lowercase().contains("final answer") {
                if let Some(pos) = thought.find("Final Answer:") {
                    let ans = thought[(pos + "Final Answer:".len())..].trim().to_string();
                    self.telemetry.emit(&TelemetryEvent::FinalAnswer { answer: ans.clone() });
                    return Ok(ans);
                } else {
                    let ans = thought.trim().to_string();
                    self.telemetry.emit(&TelemetryEvent::FinalAnswer { answer: ans.clone() });
                    return Ok(ans);
                }
            }
        }
        Err(ReactError::Malformed("Max steps reached without final answer".to_string()))
    }

    // Expose a simple memory checkpoint API for testing/observability
    pub fn save_memory_checkpoint(&self, path: &str) -> Result<(), std::io::Error> {
        self.memory.save_to_file(path)
    }
}