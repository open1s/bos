use serde_json::Value;
use std::collections::HashMap;

// Public API used by tests
#[derive(Debug, Clone)]
pub struct Observation {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Memory {
    pub history: Vec<Observation>,
}

pub struct ToolRegistry {
    pub tools: HashMap<String, Box<dyn Fn(&Value) -> Value + 'static>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    ToolCall { name: String, args: Value },
}

pub struct SimpleExecutor;

impl SimpleExecutor {
    pub fn new() -> Self {
        SimpleExecutor
    }

    pub fn execute(
        &self,
        a: &Action,
        memory: &mut Memory,
        registry: &mut ToolRegistry,
    ) -> ExecutionOutput {
        match a {
            Action::ToolCall { name, args } => {
                if let Some(tool) = registry.tools.get(name) {
                    let res = (tool)(args);
                    memory.history.push(Observation {
                        text: res.to_string(),
                    });
                    ExecutionOutput {
                        text: res.to_string(),
                    }
                } else {
                    ExecutionOutput {
                        text: String::from("unknown tool"),
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub text: String,
}

// Re-export commonly used names for tests that expect crate root exports
pub use Action as ReactAction;
pub use ExecutionOutput as ReactExecutionOutput;
pub use Memory as ReactMemory;
pub use Observation as ReactObservation;
pub use SimpleExecutor as ReactSimpleExecutor;
pub use ToolRegistry as ReactToolRegistry;
