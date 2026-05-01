use std::future::Future;

use crate::llm::types::{ReactContext, ReactSession};
use crate::llm::{LlmRequest, LlmResponse};

/// Decision returned by ReActApp hooks to control execution flow.
/// All trait methods return this to allow short-circuiting.
#[derive(Debug, Clone, Default)]
pub enum HookDecision {
    /// Continue with normal execution
    #[default]
    Continue,
    /// Abort execution immediately
    Abort,
    /// Abort with an error message
    Error(String),
}

impl HookDecision {
    pub fn is_continue(&self) -> bool {
        matches!(self, HookDecision::Continue)
    }
}

pub trait ReActApp: Send + Sync {
    type Session: Send + Sync + ReactSession;
    type Context: Send + Sync + ReactContext;

    fn name(&self) -> &str {
        "react_app"
    }

    /// Called before each LLM call. Mutate req in-place to modify the request.
    /// Return HookDecision::Continue to proceed, or Abort/Error to short-circuit.
    fn before_llm_call(
        &self,
        _req: &mut LlmRequest,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = HookDecision> + Send {
        async { HookDecision::Continue }
    }

    /// Called after each LLM response (non-streaming). Mutate response in-place.
    fn after_llm_response(
        &self,
        _response: &mut LlmResponse,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Fired after each LLM call completes in streaming mode (at the Done token).
    /// Provides the accumulated response text and whether tool calls occurred this step.
    fn after_llm_response_step(
        &self,
        _response_text: &str,
        _had_tool_call: bool,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Called before each tool call. Mutate args in-place to modify tool parameters.
    /// Return HookDecision::Continue to proceed, or Abort/Error to short-circuit.
    fn before_tool_call(
        &self,
        _tool_name: &str,
        _args: &mut serde_json::Value,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = HookDecision> + Send {
        async { HookDecision::Continue }
    }

    /// Called after each tool result. Mutate result in-place to modify the result.
    fn after_tool_result(
        &self,
        _tool_name: &str,
        _result: &mut Result<serde_json::Value, crate::engine::ReactError>,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Called when the assistant emits a thought.
    fn on_thought(
        &self,
        _thought: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Called when a final answer is generated (no tool calls this step).
    fn on_final_answer(
        &self,
        _answer: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        async {}
    }
}

impl<T: ReActApp + ?Sized> ReActApp for Box<T> {
    type Session = T::Session;
    type Context = T::Context;
    fn name(&self) -> &str {
        (**self).name()
    }
    fn before_llm_call(
        &self,
        req: &mut LlmRequest,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = HookDecision> + Send {
        (**self).before_llm_call(req, session, context)
    }
    fn after_llm_response(
        &self,
        response: &mut LlmResponse,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        (**self).after_llm_response(response, session, context)
    }
    fn after_llm_response_step(
        &self,
        response_text: &str,
        had_tool_call: bool,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        (**self).after_llm_response_step(response_text, had_tool_call, session, context)
    }
    fn before_tool_call(
        &self,
        tool_name: &str,
        args: &mut serde_json::Value,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = HookDecision> + Send {
        (**self).before_tool_call(tool_name, args, session, context)
    }
    fn after_tool_result(
        &self,
        tool_name: &str,
        result: &mut Result<serde_json::Value, crate::engine::ReactError>,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        (**self).after_tool_result(tool_name, result, session, context)
    }
    fn on_thought(
        &self,
        thought: &str,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        (**self).on_thought(thought, session, context)
    }
    fn on_final_answer(
        &self,
        answer: &str,
        session: &mut Self::Session,
        context: &mut Self::Context,
    ) -> impl Future<Output = ()> + Send {
        (**self).on_final_answer(answer, session, context)
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
