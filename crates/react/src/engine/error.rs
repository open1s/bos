use super::*;

#[derive(Debug, Error)]
/// Errors raised while running the ReAct loop.
pub enum ReactError {
    /// The underlying LLM call failed.
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    /// A tool failed.
    #[error("Tool error: {0}")]
    ToolError(String),
    /// The model returned a malformed response.
    #[error("Malformed response: {0}")]
    Malformed(String),
    /// The engine timed out.
    #[error("Engine timeout: {0}")]
    Timeout(String),
    /// A resilience guard rejected the call.
    #[error("Resilience error: {0}")]
    Resilience(ResilienceError<LlmError>),
    /// A hook aborted the run.
    #[error("Hook abort: {0}")]
    HookAbort(String),
}

// Explicit impl to route Inner(LlmError) -> Llm variant for better error handling
impl From<ResilienceError<LlmError>> for ReactError {
    fn from(e: ResilienceError<LlmError>) -> Self {
        match e {
            ResilienceError::Inner(llm_err) => ReactError::Llm(llm_err),
            _ => ReactError::Resilience(e),
        }
    }
}

impl From<ResilienceError<()>> for ReactError {
    fn from(e: ResilienceError<()>) -> Self {
        match e {
            ResilienceError::Inner(()) => ReactError::Malformed("Unexpected inner error".into()),
            ResilienceError::RateLimited => ReactError::Resilience(ResilienceError::RateLimited),
            ResilienceError::CircuitOpen => ReactError::Resilience(ResilienceError::CircuitOpen),
        }
    }
}

#[derive(Debug, Error)]
/// Errors raised while building a [`ReActEngine`].
pub enum BuilderError {
    /// No LLM client was supplied.
    #[error("LLM is required")]
    MissingLlm,
}
