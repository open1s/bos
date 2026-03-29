pub mod llm;
pub mod tool;
pub mod memory;
pub mod prompts;
pub mod calculator_tool;
pub mod search_tool;
pub mod engine;
use serde_json::Value;
use std::collections::HashMap;
#[derive(Debug, Clone)]
pub enum Action {
  ToolCall { name: String, args: Value },
}
#[derive(Debug, Clone)]
pub struct Observation {
  pub value: Value,
}
#[derive(Debug)]
pub struct SimpleExecutor {
  pub registry: ToolRegistry,
}
impl SimpleExecutor {
  pub fn new() -> Self { Self { registry: ToolRegistry { tools: HashMap::new() } } }
  pub fn new_with_registry(registry: ToolRegistry) -> Self { Self { registry } }
  pub fn execute(&self, action: &Action, memory: &mut Memory, registry: &mut ToolRegistry) -> ExecutionOutput {
    // Minimal executor: supports ToolCall only
    match action {
      Action::ToolCall { name, args } => {
        let res = registry.call(name, args).unwrap_or_else(|e| Value::String(format!("{{\"error\":\"{:?}\"}}", e)));
        memory.push(String::new(), name.clone(), res.clone());
        ExecutionOutput { text: res.to_string(), memory: memory.clone() }
      }
    }
  }
}
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
  pub text: String,
  pub memory: Memory,
}

pub use llm::{Llm, LlmError};
pub use tool::{Tool, ToolRegistry};
pub use memory::Memory;
pub use engine::ReActEngine;
pub use prompts::PromptTemplate;
 
