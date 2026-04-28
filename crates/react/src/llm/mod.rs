//! LLM types and client abstraction.
pub mod types;
pub mod response;
pub mod client;
pub mod vendor;

pub use types::{LlmMessage, LlmRequest, LlmContext, LlmSession, LlmTool, Skill, Rule, Instruction, Stringfy, LlmError, VendorBuilderError};
pub use response::{LlmResponse, StreamToken, TokenStream, StreamResponseAccumulator, LlmResponseResult};
pub use client::{LlmClient, LlmHooks, ModelFallback, LlmResponseResultFuture};
pub use vendor::*;
// Backwards compatibility alias
pub use vendor::ChatMessage as OpenAiMessage;