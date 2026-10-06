#![allow(clippy::unnecessary_cast)]

use agent::agent::hooks::{AgentHook, HookContext, HookRegistry as InnerHookRegistry};
use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::Unknown;
use napi_derive::napi;
use std::collections::HashMap;
use std::sync::Arc;

#[napi]
/// Agent lifecycle events hooks can observe.
pub enum HookEvent {
  /// Before a tool call.
  BeforeToolCall,
  /// After a tool call.
  AfterToolCall,
  /// Before an LLM call.
  BeforeLlmCall,
  /// After an LLM call.
  AfterLlmCall,
  /// When a message is added.
  OnMessage,
  /// When a run completes.
  OnComplete,
  /// When an error occurs.
  OnError,
}

#[napi]
/// A hook verdict.
pub enum HookDecision {
  /// Continue the run.
  Continue,
  /// Abort the run.
  Abort,
  /// Fail the run.
  Error,
}

impl From<HookDecision> for agent::agent::hooks::HookDecision {
  fn from(src: HookDecision) -> Self {
    match src {
      HookDecision::Continue => agent::agent::hooks::HookDecision::Continue,
      HookDecision::Abort => agent::agent::hooks::HookDecision::Abort,
      HookDecision::Error => agent::agent::hooks::HookDecision::Error(String::new()),
    }
  }
}

#[napi(object)]
/// Context passed to a hook.
pub struct HookContextData {
  /// Agent the hook is running for.
  pub agent_id: String,
  /// Arbitrary string data.
  pub data: HashMap<String, String>,
}

pub struct JSHook {
  // Weak so a registered hook does not pin the Node event loop after the run.
  pub(super) callback: Arc<
    ThreadsafeFunction<
      HookContextData,
      napi::Unknown<'static>,
      HookContextData,
      napi::Status,
      true,
      true,
    >,
  >,
}

#[async_trait]
impl AgentHook for JSHook {
  async fn on_event(
    &self,
    _event: agent::agent::hooks::HookEvent,
    context: &HookContext,
  ) -> agent::agent::hooks::HookDecision {
    let ctx_data = HookContextData {
      agent_id: context.agent_id.clone(),
      data: context.data.clone(),
    };
    let callback = self.callback.clone();

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();

    // call_with_return_value already runs on a NAPI worker thread
    callback.call_with_return_value(
      Ok(ctx_data),
      ThreadsafeFunctionCallMode::NonBlocking,
      move |result: std::result::Result<Unknown<'_>, napi::Error>, env| -> napi::Result<()> {
        match result {
          Ok(val) => {
            let is_promise = val.is_promise().unwrap_or(false);
            if is_promise {
              let raw_env = env.raw();
              let raw_val = val.value().value;
              let promise_raw = PromiseRaw::<Unknown<'_>>::new(raw_env, raw_val);
              let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
              let _ = promise_raw.then(move |ctx: CallbackContext<Unknown<'_>>| {
                let decision = ctx
                  .value
                  .coerce_to_string()
                  .and_then(|s| s.into_utf8())
                  .and_then(|u| u.as_str().map(|s| s.to_string()))
                  .unwrap_or_else(|e| format!("error:{}", e));
                if let Some(tx) = tx.lock().unwrap().take() {
                  let _ = tx.send(decision);
                }
                Ok(())
              });
            } else {
              let decision = val
                .coerce_to_string()
                .and_then(|s| s.into_utf8())
                .and_then(|u| u.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|e| format!("error:{}", e));
              let _ = tx.send(decision);
            }
          }
          Err(e) => {
            let _ = tx.send(format!("error:{}", e));
          }
        }
        Ok(())
      },
    );

    let decision_str = rx.await.unwrap_or_default();
    if decision_str.starts_with("error") {
      agent::agent::hooks::HookDecision::Error(decision_str)
    } else if decision_str == "abort" {
      agent::agent::hooks::HookDecision::Abort
    } else {
      agent::agent::hooks::HookDecision::Continue
    }
  }
}

#[napi]
/// Registry of agent hooks.
pub struct HookRegistry {
  inner: InnerHookRegistry,
}

impl Default for HookRegistry {
  fn default() -> Self {
    Self::new()
  }
}

#[napi]
impl HookRegistry {
  #[napi(constructor)]
  /// Create an empty registry.
  pub fn new() -> Self {
    Self {
      inner: InnerHookRegistry::new(),
    }
  }

  /// Clone the underlying registry.
  pub fn clone_inner(&self) -> InnerHookRegistry {
    self.inner.clone()
  }

  /// Mutable access to the underlying registry.
  pub fn inner_mut(&mut self) -> &mut InnerHookRegistry {
    &mut self.inner
  }

  #[napi]
  /// Register `callback` for `event`.
  pub async fn register(
    &self,
    event: HookEvent,
    callback: ThreadsafeFunction<
      HookContextData,
      napi::Unknown<'static>,
      HookContextData,
      napi::Status,
      true,
      true,
    >,
  ) -> Result<()> {
    let event = match event {
      HookEvent::BeforeToolCall => agent::agent::hooks::HookEvent::BeforeToolCall,
      HookEvent::AfterToolCall => agent::agent::hooks::HookEvent::AfterToolCall,
      HookEvent::BeforeLlmCall => agent::agent::hooks::HookEvent::BeforeLlmCall,
      HookEvent::AfterLlmCall => agent::agent::hooks::HookEvent::AfterLlmCall,
      HookEvent::OnMessage => agent::agent::hooks::HookEvent::OnMessage,
      HookEvent::OnComplete => agent::agent::hooks::HookEvent::OnComplete,
      HookEvent::OnError => agent::agent::hooks::HookEvent::OnError,
    };

    let hook = JSHook {
      callback: callback.into(),
    };
    self.inner.register(event, Arc::new(hook));
    Ok(())
  }
}
