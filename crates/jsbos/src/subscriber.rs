use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::jsany::JSAny;

#[napi]
pub struct Subscriber {
  pub(crate) inner: Arc<tokio::sync::Mutex<bus::Subscriber<String>>>,
  pub(crate) running: Arc<AtomicBool>,
}

#[napi]
impl Subscriber {
  #[napi(factory)]
  pub async fn new(topic: String) -> Result<Self> {
    Ok(Subscriber {
      inner: Arc::new(tokio::sync::Mutex::new(bus::Subscriber::new(topic))),
      running: Arc::new(AtomicBool::new(false)),
    })
  }

  #[napi(factory)]
  pub async fn with_session(topic: String, session: &External<bus::Session>) -> Result<Self> {
    let sub = bus::Subscriber::<String>::new(topic)
      .with_session(Arc::new((**session).clone()))
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(Subscriber {
      inner: Arc::new(tokio::sync::Mutex::new(sub)),
      running: Arc::new(AtomicBool::new(false)),
    })
  }

  #[napi(getter)]
  pub fn topic(&self) -> String {
    self.inner.blocking_lock().topic().to_string()
  }

  #[napi]
  pub async fn recv(&self) -> Result<Option<String>> {
    let mut guard = self.inner.lock().await;
    Ok(guard.recv().await)
  }

  #[napi]
  pub async fn recv_with_timeout_ms(&self, timeout_ms: i64) -> Result<Option<String>> {
    let mut guard = self.inner.lock().await;
    Ok(
      guard
        .recv_with_timeout(std::time::Duration::from_millis(timeout_ms as u64))
        .await,
    )
  }

  #[napi]
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
          guard.recv().await
        };

        match message {
          Some(msg) => {
            let tsfn_clone = Arc::clone(&tsfn);
            tsfn_clone.call_with_return_value(
              Ok(JSAny(serde_json::Value::String(msg))),
              ThreadsafeFunctionCallMode::NonBlocking,
              |_result, _env| Ok(()),
            );
          }
          None => break,
        }
      }
      running.store(false, Ordering::SeqCst);
    });

    Ok(())
  }

  #[napi]
  pub async fn run_json(&self, handler: ThreadsafeFunction<JSAny>) -> Result<()> {
    self.run(handler).await
  }

  #[napi]
  pub async fn stop(&self) -> Result<()> {
    self.running.store(false, Ordering::SeqCst);
    Ok(())
  }
}
