//! LLM types and client abstraction.
pub mod client;
pub mod response;
pub mod types;
pub mod vendor;

pub use client::{LlmClient, LlmResponseResultFuture};
pub use response::{
    LlmResponse, LlmResponseResult, StreamResponseAccumulator, StreamToken, TokenStream,
};
pub use types::{
    Binary, BinarySource, Content, ContentPart, Instruction,
    LlmContext, LlmError, LlmMessage, LlmRequest, LlmSession, LlmTool, ReactContext, ReactSession,
    Rule, Skill, Stringfy, VendorBuilderError,
};
pub use vendor::*;
pub use vendor::ChatMessage as OpenAiMessage;
