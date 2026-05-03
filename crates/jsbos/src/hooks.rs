#![allow(clippy::unnecessary_cast)]

use agent::agent::hooks::{AgentHook, HookContext, HookRegistry as InnerHookRegistry};
use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

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

    let (tx, rx) = oneshot::channel::<String>();

    let handle = tokio::runtime::Handle::current();
    handle.spawn_blocking(move || {
      callback.call_with_return_value(
        Ok(ctx_data),
        ThreadsafeFunctionCallMode::Blocking,
        move |result: std::result::Result<napi::Unknown<'_>, napi::Error>,
              _env|
              -> napi::Result<()> {
          let decision = match result {
            Ok(_val) => "continue".to_string(),
            Err(e) => format!("error:{}", e),
          };
          let _ = tx.send(decision);
          Ok(())
        },
      );
    });

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
