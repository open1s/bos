use std::sync::Arc;

// Contextual information passed to policy evaluators. This can be extended in Patch D.
pub struct PolicyContext {
    // Placeholder for future fields (e.g., memory/history snapshot, tool call count)
}

impl PolicyContext {
    pub fn new() -> Self {
        PolicyContext {}
    }
}

// Core policy trait: per-tool decision point.
pub trait ToolPolicy: Send + Sync {
    fn is_allowed(&self, tool_name: &str, _ctx: &PolicyContext) -> bool;
}

// Very small example policy that always allows. This is a placeholder until Patch D provides real policies.
pub struct AllowAllPolicy;

impl ToolPolicy for AllowAllPolicy {
    fn is_allowed(&self, _tool_name: &str, _ctx: &PolicyContext) -> bool {
        true
    }
}

pub type BoxedPolicy = Arc<dyn ToolPolicy>;
