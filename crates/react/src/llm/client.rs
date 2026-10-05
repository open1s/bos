//! The provider-agnostic LLM client trait.

//! The provider-agnostic LLM client trait.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use super::response::{LlmResponseResult, TokenStream};
use super::types::{LlmError, LlmRequest, ReactContext, ReactSession};

/// A boxed future resolving to an LLM response.
pub type LlmResponseResultFuture<'a> = Pin<Box<dyn Future<Output = LlmResponseResult> + Send + 'a>>;

#[async_trait]
/// Provider-agnostic client for chat completions.
pub trait LlmClient<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext>:
    Send + Sync
{
    /// Run a non-streaming completion.
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult;

    /// Run a streaming completion, returning a token stream.
    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError>;

    /// Whether this client supports tool calls.
    fn supports_tools(&self) -> bool {
        false
    }
    /// Short provider identifier.
    fn provider_name(&self) -> &'static str {
        "unknown"
    }
}
