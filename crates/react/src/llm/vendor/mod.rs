pub mod nvidia;
pub mod openai;
pub mod openaicompatible;
pub mod openrouter;
pub mod router;
pub use nvidia::NvidiaVendor;
pub use openai::{OpenAiClient, OpenAiVendor, OpenAiVendorBuilder};
pub use openaicompatible::{
    ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice, ChunkChoice, Delta,
    FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, OpenAIExtractor, ToolCall,
    ToolCallDelta, Usage,
};
pub use openrouter::OpenRouterVendor;
pub use router::LlmRouter;

pub fn merge_system_prompt(extra: String, leading_system: Option<&str>) -> Option<String> {
    if extra.is_empty() {
        return None;
    }
    Some(match leading_system {
        Some(existing) if !existing.is_empty() => format!("{}\n{}", existing, extra),
        _ => extra,
    })
}
