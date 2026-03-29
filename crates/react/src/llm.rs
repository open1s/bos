use futures::future::BoxFuture;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM error: {0}")]
    Other(String),
}

pub trait Llm: Send + Sync {
    fn predict(
        &self,
        prompt: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send>>;
}
