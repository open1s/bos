use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyContext {
    pub user_input: Option<String>,
    pub tool_name: String,
    pub tool_args: serde_json::Value,
    pub call_count: usize,
}

impl PolicyContext {
    pub fn new() -> Self {
        PolicyContext {
            user_input: None,
            tool_name: String::new(),
            tool_args: serde_json::Value::Null,
            call_count: 0,
        }
    }

    pub fn with_user_input(mut self, input: String) -> Self {
        self.user_input = Some(input);
        self
    }

    pub fn with_tool(mut self, name: String, args: serde_json::Value) -> Self {
        self.tool_name = name;
        self.tool_args = args;
        self
    }

    pub fn with_call_count(mut self, count: usize) -> Self {
        self.call_count = count;
        self
    }
}

impl Default for PolicyContext {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConsentLevel {
    Allow,
    Deny,
    Prompt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentPolicy {
    pub read_allowed: ConsentLevel,
    pub write_allowed: ConsentLevel,
    pub destructive_requires_prompt: bool,
}

impl Default for ConsentPolicy {
    fn default() -> Self {
        Self {
            read_allowed: ConsentLevel::Allow,
            write_allowed: ConsentLevel::Deny,
            destructive_requires_prompt: true,
        }
    }
}

impl ConsentPolicy {
    pub fn permissive() -> Self {
        Self {
            read_allowed: ConsentLevel::Allow,
            write_allowed: ConsentLevel::Allow,
            destructive_requires_prompt: false,
        }
    }

    pub fn strict() -> Self {
        Self {
            read_allowed: ConsentLevel::Allow,
            write_allowed: ConsentLevel::Prompt,
            destructive_requires_prompt: true,
        }
    }

    pub fn requires_approval(&self, tool_name: &str, _ctx: &PolicyContext) -> bool {
        let write_tools = ["bash", "write", "edit", "delete", "mkdir", "rm"];
        if write_tools.iter().any(|t| tool_name.contains(t)) {
            match self.write_allowed {
                ConsentLevel::Prompt => true,
                ConsentLevel::Deny => false,
                ConsentLevel::Allow => false,
            }
        } else {
            match self.read_allowed {
                ConsentLevel::Prompt => true,
                _ => false,
            }
        }
    }

    pub fn is_allowed(&self, tool_name: &str, ctx: &PolicyContext) -> bool {
        if self.requires_approval(tool_name, ctx) {
            return false;
        }
        true
    }
}

pub trait ToolPolicy: Send + Sync {
    fn is_allowed(&self, tool_name: &str, _ctx: &PolicyContext) -> bool;
}

pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn is_allowed(&self, _tool_name: &str, _ctx: &PolicyContext) -> bool {
        true
    }
}

pub type BoxedPolicy = Arc<dyn ToolPolicy>;
