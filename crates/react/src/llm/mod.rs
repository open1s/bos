//! LLM types and client abstraction.
pub mod client;
pub mod response;
pub mod types;
pub mod vendor;

/// The adapter implementation version used in context identities (issue #432).
pub const ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use client::{LlmClient, LlmResponseResultFuture};
pub use response::{
    LlmResponse, LlmResponseResult, StreamResponseAccumulator, StreamToken, TokenStream,
};
pub use types::{
    ActionIntent, ApiMode, Binary, BinarySource, ChunkCallback, Content, ContentPart, ContextBlock,
    ContextRole, ContextTrust, HostGuidance, Instruction, LlmContext, LlmError, LlmMessage,
    LlmRequest, LlmSession, LlmTool, LlmToolKind, Precedence, ProviderRequest,
    ProviderRequestSource, ReactContext, ReactSession, ReasoningEffort, ReasoningState, Rule,
    Skill, Stringfy, VendorBuilderError, render_skill_catalog,
};
pub use vendor::ChatMessage as OpenAiMessage;
pub use vendor::*;
