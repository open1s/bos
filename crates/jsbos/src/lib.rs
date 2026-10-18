//! Node.js bindings for the bos agent framework.
//!
//! Exposes the agent, bus, configuration, MCP, and plugin surfaces as napi
//! classes so JavaScript programs can build agents and message over the bus.
#![warn(missing_docs)]

use napi::threadsafe_function::ThreadsafeFunction;
use napi::Unknown;
use napi_derive::napi;
use std::sync::{Arc, Mutex, MutexGuard};

/// A lock-protected slot holding an optional JavaScript string handler.
pub(crate) type StringHandlerSlot =
  Arc<Mutex<Option<Arc<ThreadsafeFunction<String, Unknown<'static>>>>>>;

/// Lock `mutex`, recovering the guard if a panic poisoned it.
///
/// A panic must never unwind across the N-API boundary, and every mutex in this
/// crate guards an `Option<Arc<..>>` slot whose value stays usable after a
/// panic, so silently recovering is both safe and strictly better than
/// `unwrap()` aborting the host process.
pub(crate) fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
  mutex
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Call a JavaScript string handler and await its result.
///
/// A handler may be `async`: the returned promise is awaited instead of being
/// stringified into `"[object Promise]"` (which is what a plain
/// `coerce_to_string` does).
pub(crate) async fn call_js_string_handler(
  handler: &ThreadsafeFunction<String, Unknown<'static>>,
  input: String,
) -> std::result::Result<String, String> {
  use napi::bindgen_prelude::{CallbackContext, PromiseRaw};
  use napi::threadsafe_function::ThreadsafeFunctionCallMode;
  use napi::JsValue;

  let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<String, String>>();
  let tx = Arc::new(Mutex::new(Some(tx)));

  handler.call_with_return_value(
    Ok(input),
    ThreadsafeFunctionCallMode::NonBlocking,
    move |result: std::result::Result<Unknown<'_>, napi::Error>, env| -> napi::Result<()> {
      match result {
        Ok(val) => {
          let to_string = |val: &Unknown<'_>| {
            val
              .coerce_to_string()
              .and_then(|s| s.into_utf8())
              .and_then(|u| u.as_str().map(|s| s.to_string()))
              .map_err(|e| e.to_string())
          };
          if val.is_promise().unwrap_or(false) {
            let raw_env = env.raw();
            let raw_val = val.value().value;
            let promise_raw = PromiseRaw::<Unknown<'_>>::new(raw_env, raw_val);
            let tx = tx.clone();
            let _ = promise_raw.then(move |ctx: CallbackContext<Unknown<'_>>| {
              let outcome = to_string(&ctx.value);
              if let Some(tx) = lock_recover(&tx).take() {
                let _ = tx.send(outcome);
              }
              Ok(())
            });
          } else {
            let outcome = to_string(&val);
            if let Some(tx) = lock_recover(&tx).take() {
              let _ = tx.send(outcome);
            }
          }
        }
        Err(e) => {
          if let Some(tx) = lock_recover(&tx).take() {
            let _ = tx.send(Err(e.to_string()));
          }
        }
      }
      Ok(())
    },
  );

  rx.await
    .unwrap_or_else(|_| Err("handler channel closed".to_string()))
}

/// Convert a JavaScript millisecond count into a duration.
///
/// `i64 as u64` wraps a negative count into a duration of centuries, so a
/// caller passing `-1` would effectively wait forever instead of getting an
/// error for its mistake.
pub(crate) fn millis_from_js(name: &str, value: i64) -> napi::Result<std::time::Duration> {
  u64::try_from(value)
    .map(std::time::Duration::from_millis)
    .map_err(|_| {
      napi::Error::new(
        napi::Status::InvalidArg,
        format!("{name} must be a non-negative number of milliseconds"),
      )
    })
}

mod agent;
mod bus;
mod caller;
mod config;
mod hooks;
mod jsany;
mod llm_usage;
mod mcp;
mod perf;
mod plugin;
mod publisher;
mod query;
mod subscriber;

pub use agent::{Agent, AgentCallableServer, AgentConfig, AgentRpcClient};
pub use bus::{Bus, BusConfig, Session};
pub use caller::{Callable, Caller};
pub use config::ConfigLoader;
pub use hooks::{HookContextData, HookDecision, HookEvent, HookRegistry};
pub use llm_usage::{BudgetStatus, LlmUsage, PromptTokensDetails, TokenBudgetReport, TokenUsage};
pub use mcp::McpClient;
pub use plugin::{
  PluginLlmRequest, PluginLlmResponse, PluginRegistry, PluginStage, PluginToolCall,
  PluginToolResult,
};
pub use publisher::Publisher;
pub use query::{Query, Queryable};
pub use subscriber::Subscriber;

pub use perf::PerfSnapshot;

// Note: logging is a dependency but used in binaries

#[napi]
/// Return the binding version.
pub fn version() -> String {
  env!("CARGO_PKG_VERSION").to_string()
}

#[napi]
/// Initialize tracing for the process.
pub fn init_tracing() {
  logging::auto_init_tracing();
}

#[napi]
/// Emit a test log message.
pub fn log_test_message(message: String) {
  logging::log_test_message(&message);
}

#[cfg(test)]
mod tests {
  use super::{lock_recover, millis_from_js};
  use std::sync::Mutex;
  use std::time::Duration;

  #[test]
  fn millis_from_js_rejects_negative_counts() {
    assert!(millis_from_js("timeout_ms", -1).is_err());
    assert_eq!(
      millis_from_js("timeout_ms", 1500).unwrap(),
      Duration::from_millis(1500)
    );
  }

  #[test]
  fn lock_recover_returns_a_guard_for_a_poisoned_mutex() {
    let slot: Mutex<Option<u8>> = Mutex::new(None);
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      let _guard = slot.lock().unwrap();
      panic!("poison the mutex");
    }));
    assert!(poisoned.is_err());
    assert!(slot.is_poisoned(), "the mutex should be poisoned");

    *lock_recover(&slot) = Some(7);
    assert_eq!(*lock_recover(&slot), Some(7));
  }
}
