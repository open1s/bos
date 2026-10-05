use super::*;

#[derive(Debug, Error)]
pub enum ReactError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    #[error("Tool error: {0}")]
    ToolError(String),
    #[error("Malformed response: {0}")]
    Malformed(String),
    #[error("Engine timeout: {0}")]
    Timeout(String),
    #[error("Resilience error: {0}")]
    Resilience(ResilienceError<LlmError>),
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
pub enum BuilderError {
    #[error("LLM is required")]
    MissingLlm,
}
