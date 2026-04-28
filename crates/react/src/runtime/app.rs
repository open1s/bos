use crate::llm::{LlmRequest, LlmResponse};
use crate::llm::types::{ReactContext, ReactSession};

#[allow(async_fn_in_trait)]
pub trait ReActApp: Send + Sync {
    type Session:  Send + Sync + ReactSession;
    type Context:  Send + Sync + ReactContext;

    fn name(&self) -> &str { "react_app" }

    async fn before_llm_call(
        &self,
        _req: &mut LlmRequest,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}

    async fn after_llm_response(
        &self,
        _response: &LlmResponse,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}

    async fn before_tool_call(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}

    async fn after_tool_result(
        &self,
        _tool_name: &str,
        _result: &Result<serde_json::Value, crate::engine::ReactError>,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}

    async fn on_thought(
        &self,
        _thought: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}

    async fn on_final_answer(
        &self,
        _answer: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {}
}

impl<T: ReActApp + ?Sized> ReActApp for Box<T> {
    type Session = T::Session;
    type Context = T::Context;
    fn name(&self) -> &str { (**self).name() }
    async fn before_llm_call(&self, req: &mut LlmRequest, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).before_llm_call(req, session, context).await
    }
    async fn after_llm_response(&self, response: &LlmResponse, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).after_llm_response(response, session, context).await
    }
    async fn before_tool_call(&self, tool_name: &str, args: &serde_json::Value, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).before_tool_call(tool_name, args, session, context).await
    }
    async fn after_tool_result(&self, tool_name: &str, result: &Result<serde_json::Value, crate::engine::ReactError>, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).after_tool_result(tool_name, result, session, context).await
    }
    async fn on_thought(&self, thought: &str, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).on_thought(thought, session, context).await
    }
    async fn on_final_answer(&self, answer: &str, session: &mut Self::Session, context: &mut Self::Context) {
        (**self).on_final_answer(answer, session, context).await
    }
}

pub struct NoopApp;

impl Default for NoopApp {
    fn default() -> Self {
        NoopApp
    }
}

impl ReActApp for NoopApp {
    type Session = ();
    type Context = ();
}