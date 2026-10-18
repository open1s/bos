//! Vendor clients and wire types for the supported LLM providers.

pub mod anthropic;
pub mod deepseek;
pub mod nvidia;
pub mod openai;
pub mod openaicompatible;
pub mod openrouter;
pub mod responses;
pub mod router;
pub use anthropic::{AnthropicVendor, AnthropicVendorBuilder};
pub use deepseek::{DeepSeekVendor, DeepSeekVendorBuilder};
pub use nvidia::NvidiaVendor;
pub use openai::{OpenAiClient, OpenAiVendor, OpenAiVendorBuilder};
pub use openaicompatible::{
    sse_has_done_signal, ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice,
    ChunkChoice, Delta, FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, OpenAIExtractor,
    PendingToolCall, StreamToolCallAccumulator, ToolCall, ToolCallDelta, Usage,
};
pub use openrouter::OpenRouterVendor;
pub use responses::{
    ResponsesContentPart, ResponsesExtractor, ResponsesFunctionCallAccumulator, ResponsesInputItem,
    ResponsesItem, ResponsesReasoningSummary, ResponsesRequest, ResponsesResponse,
    ResponsesStreamEvent, ResponsesTransport, ResponsesUsage,
};
pub use router::LlmRouter;

use std::time::Duration;

/// How long a built-in vendor waits for the next read (connect, response
/// headers, or the next streamed chunk) before failing the call.
///
/// Without it a provider that accepts the connection and then stalls leaves
/// the request pending forever; the engine's outer timeout only covers calls
/// made through the engine, not direct vendor use.
pub(crate) const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a built-in vendor waits for the TCP/TLS connection to open.
pub(crate) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Build the timeout-bounded HTTP client shared by the built-in vendors.
pub(crate) fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .read_timeout(DEFAULT_READ_TIMEOUT)
        .build()
        .expect("failed to build the shared HTTP client")
}

/// Merge `extra` into `leading_system`, returning `None` when `extra` is empty.
pub fn merge_system_prompt(extra: String, leading_system: Option<&str>) -> Option<String> {
    if extra.is_empty() {
        return None;
    }
    Some(match leading_system {
        Some(existing) if !existing.is_empty() => format!("{}\n{}", existing, extra),
        _ => extra,
    })
}
