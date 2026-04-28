use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use super::types::{LlmRequest, LlmError, ReactSession, ReactContext, Skill, LlmTool, Rule, Instruction};
use super::response::{TokenStream, LlmResponseResult, LlmResponse};

pub type LlmResponseResultFuture<'a> = Pin<Box<dyn Future<Output = LlmResponseResult> + Send + 'a>>;

/// Callback hooks invoked by vendors during request/response processing.
/// Allows the caller to intercept and observe LLM interactions.
pub struct LlmHooks {
    pub on_request: Option<Arc<dyn Fn(&LlmRequest) + Send + Sync>>,
    pub on_chunk: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    pub on_response: Option<Arc<dyn Fn(&LlmResponse) + Send + Sync>>,
    pub on_error: Option<Arc<dyn Fn(&LlmError) + Send + Sync>>,
}

impl Clone for LlmHooks {
    fn clone(&self) -> Self {
        Self {
            on_request: self.on_request.as_ref().map(Arc::clone),
            on_chunk: self.on_chunk.as_ref().map(Arc::clone),
            on_response: self.on_response.as_ref().map(Arc::clone),
            on_error: self.on_error.as_ref().map(Arc::clone),
        }
    }
}

impl Default for LlmHooks {
    fn default() -> Self {
        Self { on_request: None, on_chunk: None, on_response: None, on_error: None }
    }
}

impl LlmHooks {
    pub fn new() -> Self { Self::default() }
    pub fn with_on_request<F>(mut self, f: F) -> Self where F: Fn(&LlmRequest) + Send + Sync + 'static {
        self.on_request = Some(Arc::new(f)); self
    }
    pub fn with_on_chunk<F>(mut self, f: F) -> Self where F: Fn(&str) + Send + Sync + 'static {
        self.on_chunk = Some(Arc::new(f)); self
    }
    pub fn with_on_response<F>(mut self, f: F) -> Self where F: Fn(&LlmResponse) + Send + Sync + 'static {
        self.on_response = Some(Arc::new(f)); self
    }
    pub fn with_on_error<F>(mut self, f: F) -> Self where F: Fn(&LlmError) + Send + Sync + 'static {
        self.on_error = Some(Arc::new(f)); self
    }
    pub(crate) fn notify_request(&self, req: &LlmRequest) {
        if let Some(ref cb) = self.on_request { cb(req); }
    }
    #[allow(dead_code)]
    pub(crate) fn notify_chunk(&self, chunk: &str) {
        if let Some(ref cb) = self.on_chunk { cb(chunk); }
    }
    pub(crate) fn notify_response(&self, resp: &LlmResponse) {
        if let Some(ref cb) = self.on_response { cb(resp); }
    }
    pub(crate) fn notify_error(&self, err: &LlmError) {
        if let Some(ref cb) = self.on_error { cb(err); }
    }
}

impl std::fmt::Debug for LlmHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmHooks")
            .field("has_on_request", &self.on_request.is_some())
            .field("has_on_chunk", &self.on_chunk.is_some())
            .field("has_on_response", &self.on_response.is_some())
            .field("has_on_error", &self.on_error.is_some())
            .finish()
    }
}

impl ReactContext for LlmHooks {
    fn session_id(&self) -> String {
        "llm_hooks_session".to_string()
    }

    fn skills(&self) -> Option<Vec<Skill>> {
        None
    }

    fn tools(&self) -> Option<Vec<LlmTool>> {
        None
    }

    fn rules(&self) -> Option<Vec<Rule>> {
        None
    }

    fn instructions(&self) -> Option<Vec<Instruction>> {
        None
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    type SessionType: Send + Sync + ReactSession;
    type ContextType: Send + Sync + ReactContext;

    async fn complete(
        &self,
        req: LlmRequest,
        session: &mut Self::SessionType,
        context: &mut Self::ContextType,
    ) -> LlmResponseResult;

    async fn stream_complete(
        &self,
        req: LlmRequest,
        session: &mut Self::SessionType,
        context: &mut Self::ContextType,
    ) -> Result<TokenStream, LlmError>;

    fn supports_tools(&self) -> bool { false }
    fn provider_name(&self) -> &'static str { "unknown" }
}

pub struct ModelFallback<S, C> {
    primary: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
    fallback: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
    fallback_on_error: bool,
}

impl<S, C> ModelFallback<S, C> {
    pub fn new(
        primary: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
        fallback: Box<dyn LlmClient<SessionType = S, ContextType = C>>,
    ) -> Self {
        Self { primary, fallback, fallback_on_error: true }
    }
    pub fn with_fallback_enabled(mut self, enabled: bool) -> Self {
        self.fallback_on_error = enabled;
        self
    }
}

#[async_trait]
impl<S: Send + Sync + Clone + ReactSession, C: Send + Sync + Clone + ReactContext> LlmClient for ModelFallback<S, C> {
    type SessionType = S;
    type ContextType = C;

    async fn complete(
        &self,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        let result = self.primary.complete(req.clone(), session, context).await;
        if result.is_err() && self.fallback_on_error {
            self.fallback.complete(req, session, context).await
        } else {
            result
        }
    }

    async fn stream_complete(
        &self,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        let result = self.primary.stream_complete(req.clone(), session, context).await;
        if result.is_err() && self.fallback_on_error {
            self.fallback.stream_complete(req, session, context).await
        } else {
            result
        }
    }

    fn supports_tools(&self) -> bool { self.primary.supports_tools() }
    fn provider_name(&self) -> &'static str { "model_fallback" }
}