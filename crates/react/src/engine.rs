use crate::llm::{Llm, LlmError};
use crate::tool::{ToolRegistry, Tool};
use crate::memory::Memory;
use serde_json::Value;
use thiserror::Error;
use std::pin::Pin;
use futures::Future;

#[derive(Debug, Error)]
pub enum ReactError {
  #[error("LLM error: {0}")]
  Llm(#[from] LlmError),
  #[error("Tool error: {0}")]
  ToolError(String),
  #[error("Malformed response: {0}")]
  Malformed(String),
}

pub struct ReActEngine {
  llm: Box<dyn Llm>,
  tools: ToolRegistry,
  memory: Memory,
  max_steps: usize,
}

impl ReActEngine {
  pub fn new(llm: Box<dyn Llm>, max_steps: usize) -> Self {
    Self { llm, tools: ToolRegistry::new(), memory: Memory::new(), max_steps }
  }
  pub fn register_tool(&mut self, t: Box<dyn Tool>) {
     self.tools.register(t);
  }
  pub async fn run(&mut self, user_input: &str) -> Result<String, ReactError> {
     // Minimal ReAct loop with Action/Observation pattern
     let mut thought = String::new();
     for _ in 0..self.max_steps {
        // 1) Prompt LLM with user_input to generate Thought + Action
        let prompt = format!("User input: {}\nThought:", user_input);
        thought = self.llm.predict(&prompt).await.map_err(ReactError::Llm)?;
        // 2) Parse Action and Input from Thought
        let mut tool_name: Option<String> = None;
        let mut input: Value = Value::Null;
        // Robust parsing: support Action/Tool and Input/Parameters (case-insensitive)
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
        let tool_name = match tool_name { Some(n) => n, None => return Err(ReactError::Malformed("Missing Action in llm output".to_string())) };
        let res = self.tools.call(&tool_name, &input).map_err(|e| ReactError::ToolError(format!("{:?}", e)))?;
        self.memory.push(thought.clone(), tool_name.clone(), res.clone());
        // 3) Next LLM prompt with observation
        let prompt2 = format!("Observation: {}\nThought:", res);
        thought = self.llm.predict(&prompt2).await.map_err(ReactError::Llm)?;
        // 4) Check for final
        if thought.to_lowercase().contains("final answer") {
           if let Some(pos) = thought.find("Final Answer:") {
              let ans = thought[(pos + "Final Answer:".len())..].trim().to_string();
              return Ok(ans);
           } else {
              return Ok(thought.trim().to_string());
           }
        }
     }
     Err(ReactError::Malformed("Max steps reached without final answer".to_string()))
  }
}
