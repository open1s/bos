pub mod nvidia;
pub mod openai;
pub mod openaicompatible;
pub mod openrouter;
pub mod responses;
pub mod router;
pub use nvidia::NvidiaVendor;
pub use openai::{OpenAiClient, OpenAiVendor, OpenAiVendorBuilder};
pub use openaicompatible::{
    ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice, ChunkChoice, Delta,
    FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, OpenAIExtractor,
    PendingToolCall, StreamToolCallAccumulator, ToolCall, ToolCallDelta, Usage,
    sse_has_done_signal,
};
pub use openrouter::OpenRouterVendor;
pub use responses::{
    ResponsesContentPart, ResponsesExtractor, ResponsesFunctionCallAccumulator, ResponsesInputItem,
    ResponsesItem, ResponsesReasoningSummary, ResponsesRequest, ResponsesResponse,
    ResponsesStreamEvent, ResponsesTransport, ResponsesUsage,
};
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
