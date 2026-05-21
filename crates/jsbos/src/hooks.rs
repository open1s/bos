#![allow(clippy::unnecessary_cast)]

use agent::agent::hooks::{AgentHook, HookContext, HookRegistry as InnerHookRegistry};
use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use napi::Unknown;
use std::collections::HashMap;
use std::sync::Arc;

#[napi]
pub enum HookEvent {
  BeforeToolCall,
  AfterToolCall,
  BeforeLlmCall,
  AfterLlmCall,
  OnMessage,
  OnComplete,
  OnError,
}

#[napi]
pub enum HookDecision {
  Continue,
  Abort,
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
pub struct HookContextData {
  pub agent_id: String,
  pub data: HashMap<String, String>,
}

pub struct JSHook {
  pub(super) callback: Arc<ThreadsafeFunction<HookContextData>>,
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
      move |result: std::result::Result<Unknown<'_>, napi::Error>,
            env|
            -> napi::Result<()> {
        match result {
          Ok(val) => {
            let is_promise = val.is_promise().unwrap_or(false);
            if is_promise {
              let raw_env = env.raw();
              let raw_val = val.value().value;
              let promise_raw = PromiseRaw::<Unknown<'_>>::new(raw_env, raw_val);
              let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
              let _ = promise_raw.then(move |ctx: CallbackContext<Unknown<'_>>| {
                let decision = ctx.value.coerce_to_string()
                  .and_then(|s| s.into_utf8())
                  .and_then(|u| u.as_str().map(|s| s.to_string()))
                  .unwrap_or_else(|e| format!("error:{}", e));
                if let Some(tx) = tx.lock().unwrap().take() {
                  let _ = tx.send(decision);
                }
                Ok(())
              });
            } else {
              let decision = val.coerce_to_string()
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
pub struct HookRegistry {
  inner: InnerHookRegistry,
}

#[napi]
impl HookRegistry {
  #[napi(constructor)]
  pub fn new() -> Self {
    Self {
      inner: InnerHookRegistry::new(),
    }
  }

  pub fn clone_inner(&self) -> InnerHookRegistry {
    self.inner.clone()
  }

  pub fn inner_mut(&mut self) -> &mut InnerHookRegistry {
    &mut self.inner
  }

  #[napi]
  pub async fn register(
    &self,
    event: HookEvent,
    callback: ThreadsafeFunction<HookContextData>,
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
