use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use super::response::{LlmResponseResult, TokenStream};
use super::types::{LlmError, LlmRequest, ReactContext, ReactSession};

pub type LlmResponseResultFuture<'a> = Pin<Box<dyn Future<Output = LlmResponseResult> + Send + 'a>>;

#[async_trait]
pub trait LlmClient<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext>:
    Send + Sync
{
    async fn complete(
        &self,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult;

    async fn stream_complete(
        &self,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError>;

    fn supports_tools(&self) -> bool {
        false
    }
    fn provider_name(&self) -> &'static str {
        "unknown"
    }
}
