use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::Arc;

#[napi]
pub struct Query {
  pub(crate) inner: bus::Query,
}

#[napi]
impl Query {
  #[napi(factory)]
  pub async fn new(topic: String) -> Result<Self> {
    Ok(Query {
      inner: bus::Query::new(topic),
    })
  }

  #[napi(factory)]
  pub async fn with_session(topic: String, session: &External<bus::Session>) -> Result<Self> {
    let query = bus::Query::new(topic)
      .with_session(Arc::new((**session).clone()))
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(Query { inner: query })
  }

  #[napi(getter)]
  pub fn topic(&self) -> String {
    self.inner.topic().to_string()
  }

  #[napi]
  pub async fn query_text(&self, payload: String) -> Result<String> {
    let out = self
      .inner
      .query::<String, String>(&payload)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(out)
  }

  #[napi]
  pub async fn query_text_timeout_ms(&self, payload: String, timeout_ms: i64) -> Result<String> {
    let out = self
      .inner
      .query_with_timeout::<String, String>(
        &payload,
        std::time::Duration::from_millis(timeout_ms as u64),
      )
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(out)
  }
}

#[allow(dead_code)]
#[napi]
pub struct Queryable {
  pub(crate) inner: Arc<tokio::sync::Mutex<bus::QueryableWrapper<String, String>>>,
  pub(crate) handler:
    Arc<std::sync::Mutex<Option<Arc<ThreadsafeFunction<String, UnknownReturnValue>>>>>,
  pub(crate) stream_handler:
    Arc<std::sync::Mutex<Option<Arc<ThreadsafeFunction<String, UnknownReturnValue>>>>>,
}

#[napi]
impl Queryable {
  #[napi(factory)]
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
  pub fn set_handler(&self, handler: ThreadsafeFunction<String, UnknownReturnValue>) -> Result<()> {
    let mut guard = self.handler.lock().unwrap();
    *guard = Some(Arc::new(handler));
    Ok(())
  }

  #[napi]
  pub async fn start(&self) -> Result<()> {
    let mut guard = self.inner.lock().await;

    if let Some(tsfn) = self.handler.lock().unwrap().take() {
      guard
        .set_handler(move |input: String| {
          let tsfn_clone = Arc::clone(&tsfn);
          async move {
            tsfn_clone.call(Ok(input.clone()), ThreadsafeFunctionCallMode::Blocking);
            Ok(input)
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
  pub async fn run(&self, handler: ThreadsafeFunction<String, UnknownReturnValue>) -> Result<()> {
    let mut guard = self.inner.lock().await;

    let tsfn = Arc::new(handler);
    guard
      .set_handler(move |input: String| {
        let tsfn_clone = Arc::clone(&tsfn);
        async move {
          tsfn_clone.call(Ok(input.clone()), ThreadsafeFunctionCallMode::Blocking);
          Ok(input)
        }
      })
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;

    guard
      .run()
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }

  #[napi]
  pub async fn run_json(
    &self,
    handler: ThreadsafeFunction<String, UnknownReturnValue>,
  ) -> Result<()> {
    self.run(handler).await
  }

  #[napi]
  pub async fn run_stream(
    &self,
    handler: ThreadsafeFunction<String, UnknownReturnValue>,
  ) -> Result<()> {
    let mut guard = self.inner.lock().await;

    let tsfn = Arc::new(handler);
    guard
      .set_stream_handler(move |input: String, tx| {
        let tsfn_clone = Arc::clone(&tsfn);
        async move {
          tsfn_clone.call(Ok(input.clone()), ThreadsafeFunctionCallMode::Blocking);
          let _ = tx.send(Ok(input));
        }
      })
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;

    guard
      .run()
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }
}
