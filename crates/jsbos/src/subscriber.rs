use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::jsany::JSAny;

#[napi]
/// Subscribes to a topic.
pub struct Subscriber {
  pub(crate) inner: Arc<tokio::sync::Mutex<bus::Subscriber<String>>>,
  pub(crate) running: Arc<AtomicBool>,
  pub(crate) topic: String,
}

#[napi]
impl Subscriber {
  #[napi(factory)]
  /// Create a subscriber for `topic`.
  pub async fn new(topic: String) -> Result<Self> {
    let stored = topic.clone();
    Ok(Subscriber {
      inner: Arc::new(tokio::sync::Mutex::new(bus::Subscriber::new(topic))),
      running: Arc::new(AtomicBool::new(false)),
      topic: stored,
    })
  }

  #[napi(factory)]
  /// Create a subscriber bound to `session`.
  pub async fn with_session(topic: String, session: &External<bus::Session>) -> Result<Self> {
    let stored = topic.clone();
    let sub = bus::Subscriber::<String>::new(topic)
      .with_session(Arc::new((**session).clone()))
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(Subscriber {
      inner: Arc::new(tokio::sync::Mutex::new(sub)),
      running: Arc::new(AtomicBool::new(false)),
      topic: stored,
    })
  }

  #[napi(getter)]
  /// The subscribed topic.
  pub fn topic(&self) -> String {
    self.topic.clone()
  }

  #[napi]
  /// Receive the next message.
  pub async fn recv(&self) -> Result<Option<String>> {
    let mut guard = self.inner.lock().await;
    Ok(guard.recv().await)
  }

  #[napi]
  /// Receive the next message, waiting up to `timeout_ms`.
  pub async fn recv_with_timeout_ms(&self, timeout_ms: i64) -> Result<Option<String>> {
    let mut guard = self.inner.lock().await;
    Ok(
      guard
        .recv_with_timeout(std::time::Duration::from_millis(timeout_ms as u64))
        .await,
    )
  }

  #[napi]
  /// Receive the next message parsed as JSON.
  pub async fn recv_json_with_timeout_ms(
    &self,
    timeout_ms: i64,
  ) -> Result<Option<serde_json::Value>> {
    let mut guard = self.inner.lock().await;
    let msg = guard
      .recv_with_timeout(std::time::Duration::from_millis(timeout_ms as u64))
      .await;
    match msg {
      Some(s) => {
        let value: serde_json::Value = serde_json::from_str(&s)
          .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(Some(value))
      }
      None => Ok(None),
    }
  }

  #[napi]
  /// Deliver messages to `handler` until stopped.
  pub async fn run(&self, handler: ThreadsafeFunction<JSAny>) -> Result<()> {
    let inner = self.inner.clone();
    let tsfn = Arc::new(handler);
    let running = self.running.clone();

    if running.swap(true, Ordering::SeqCst) {
      return Err(napi::Error::new(
        napi::Status::GenericFailure,
        "already running",
      ));
    }

    tokio::spawn(async move {
      loop {
        if !running.load(Ordering::SeqCst) {
          break;
        }

        let message = {
          let mut guard = inner.lock().await;
          guard
            .recv_with_timeout(std::time::Duration::from_millis(500))
            .await
        };

        if let Some(msg) = message {
          let tsfn_clone = Arc::clone(&tsfn);
          tsfn_clone.call_with_return_value(
            Ok(JSAny(serde_json::Value::String(msg))),
            ThreadsafeFunctionCallMode::NonBlocking,
            |_result, _env| Ok(()),
          );
        }
      }
      running.store(false, Ordering::SeqCst);
    });

    Ok(())
  }

  #[napi]
  /// Deliver messages parsed as JSON to `handler`.
  pub async fn run_json(&self, handler: ThreadsafeFunction<JSAny>) -> Result<()> {
    let inner = self.inner.clone();
    let tsfn = Arc::new(handler);
    let running = self.running.clone();

    if running.swap(true, Ordering::SeqCst) {
      return Err(napi::Error::new(
        napi::Status::GenericFailure,
        "already running",
      ));
    }

    tokio::spawn(async move {
      loop {
        if !running.load(Ordering::SeqCst) {
          break;
        }

        let message = {
          let mut guard = inner.lock().await;
          guard
            .recv_with_timeout(std::time::Duration::from_millis(500))
            .await
        };

        if let Some(msg) = message {
          let value: serde_json::Value =
            serde_json::from_str(&msg).unwrap_or(serde_json::Value::String(msg));
          let tsfn_clone = Arc::clone(&tsfn);
          tsfn_clone.call_with_return_value(
            Ok(JSAny(value)),
            ThreadsafeFunctionCallMode::NonBlocking,
            |_result, _env| Ok(()),
          );
        }
      }
      running.store(false, Ordering::SeqCst);
    });

    Ok(())
  }

  #[napi]
  /// Stop the delivery loop.
  pub async fn stop(&self) -> Result<()> {
    self.running.store(false, Ordering::SeqCst);
    let mut guard = self.inner.lock().await;
    guard.stop();
    Ok(())
  }
}

impl Drop for Subscriber {
  fn drop(&mut self) {
    self.running.store(false, Ordering::SeqCst);
    if let Ok(mut guard) = self.inner.try_lock() {
      guard.stop();
    }
  }
}
