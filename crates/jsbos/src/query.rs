use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use std::sync::Arc;

use crate::call_js_string_handler;

#[napi]
/// Runs queries against a queryable.
pub struct Query {
  pub(crate) inner: bus::Query,
}

#[napi]
impl Query {
  #[napi(factory)]
  /// Create a query for `topic`.
  pub async fn new(topic: String) -> Result<Self> {
    Ok(Query {
      inner: bus::Query::new(topic),
    })
  }

  #[napi(factory)]
  /// Create a query bound to `session`.
  pub async fn with_session(topic: String, session: &External<bus::Session>) -> Result<Self> {
    let query = bus::Query::new(topic)
      .with_session(Arc::new((**session).clone()))
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(Query { inner: query })
  }

  #[napi(getter)]
  /// The queried topic.
  pub fn topic(&self) -> String {
    self.inner.topic().to_string()
  }

  #[napi]
  /// Query the handler and return its text result.
  pub async fn query_text(&self, payload: String) -> Result<String> {
    let out = self
      .inner
      .query::<String, String>(&payload)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(out)
  }

  #[napi]
  /// Query the handler, waiting up to `timeout_ms`.
  pub async fn query_text_timeout_ms(&self, payload: String, timeout_ms: i64) -> Result<String> {
    let timeout = crate::millis_from_js("timeout_ms", timeout_ms)?;
    let out = self
      .inner
      .query_with_timeout::<String, String>(&payload, timeout)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(out)
  }
}

#[allow(dead_code)]
#[napi]
/// Serves queries over the bus.
pub struct Queryable {
  pub(crate) inner: Arc<tokio::sync::Mutex<bus::QueryableWrapper<String, String>>>,
  pub(crate) handler: crate::StringHandlerSlot,
  pub(crate) stream_handler: crate::StringHandlerSlot,
}

#[napi]
impl Queryable {
  #[napi(factory)]
  /// Create a queryable for `topic`.
  pub async fn new(topic: String) -> Result<Self> {
    let mut wrapper = bus::QueryableWrapper::<String, String>::new(topic);
    wrapper
      .set_handler(|input| async move { Ok(input) })
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(Queryable {
      inner: Arc::new(tokio::sync::Mutex::new(wrapper)),
      handler: Arc::new(std::sync::Mutex::new(None)),
      stream_handler: Arc::new(std::sync::Mutex::new(None)),
    })
  }

  #[napi]
  /// Set the handler invoked for each query.
  pub fn set_handler(
    &self,
    handler: ThreadsafeFunction<String, napi::Unknown<'static>>,
  ) -> Result<()> {
    let mut guard = crate::lock_recover(&self.handler);
    *guard = Some(Arc::new(handler));
    Ok(())
  }

  #[napi]
  /// Start serving with the configured handler.
  pub async fn start(&self) -> Result<()> {
    let mut guard = self.inner.lock().await;

    if let Some(tsfn) = crate::lock_recover(&self.handler).take() {
      guard
        .set_handler(move |input: String| {
          let tsfn_clone = Arc::clone(&tsfn);
          async move {
            call_js_string_handler(&tsfn_clone, input)
              .await
              .map_err(bus::ZenohError::Query)
          }
        })
        .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    }

    guard
      .run()
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }

  #[napi]
  /// Set `handler` and start serving.
  pub async fn run(
    &self,
    handler: ThreadsafeFunction<String, napi::Unknown<'static>>,
  ) -> Result<()> {
    let mut guard = self.inner.lock().await;

    let tsfn = Arc::new(handler);
    guard
      .set_handler(move |input: String| {
        let tsfn_clone = Arc::clone(&tsfn);
        async move {
          call_js_string_handler(&tsfn_clone, input)
            .await
            .map_err(bus::ZenohError::Query)
        }
      })
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;

    guard
      .run()
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }

  #[napi]
  /// Set `handler` and start serving JSON.
  pub async fn run_json(
    &self,
    handler: ThreadsafeFunction<String, napi::Unknown<'static>>,
  ) -> Result<()> {
    self.run(handler).await
  }

  /// Stop the queryable, aborting the handler task and dropping the subscription.
  #[napi]
  pub async fn stop(&self) -> Result<()> {
    let mut guard = self.inner.lock().await;
    guard.stop();
    Ok(())
  }

  #[napi]
  /// Set `handler` and start serving streaming results.
  pub async fn run_stream(
    &self,
    handler: ThreadsafeFunction<String, napi::Unknown<'static>>,
  ) -> Result<()> {
    let mut guard = self.inner.lock().await;

    let tsfn = Arc::new(handler);
    guard
      .set_stream_handler(move |input: String, tx| {
        let tsfn_clone = Arc::clone(&tsfn);
        async move {
          let _ = tx
            .send(
              call_js_string_handler(&tsfn_clone, input)
                .await
                .map_err(bus::ZenohError::Query),
            )
            .await;
        }
      })
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;

    guard
      .run()
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }
}
