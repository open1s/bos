use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
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
    pub async fn with_session(topic: String, session: External<Arc<bus::Session>>) -> Result<Self> {
        let query = bus::Query::new(topic)
            .with_session(Arc::clone(&session))
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

#[napi]
pub struct Queryable {
    pub(crate) inner: Arc<tokio::sync::Mutex<bus::QueryableWrapper<String, String>>>,
    pub(crate) handler: Arc<std::sync::Mutex<Option<ThreadsafeFunction<String>>>>,
}

#[napi]
impl Queryable {
    #[napi(factory)]
    pub async fn new(topic: String) -> Result<Self> {
        let mut wrapper = bus::QueryableWrapper::<String, String>::new(topic);
        wrapper.set_handler(|input| async move { Ok(input) }).map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, e.to_string())
        })?;
        Ok(Queryable {
            inner: Arc::new(tokio::sync::Mutex::new(wrapper)),
            handler: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    #[napi]
    pub fn set_handler(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        let mut guard = self.handler.lock().unwrap();
        *guard = Some(handler);
        Ok(())
    }

    #[napi]
    pub async fn start(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;

        if let Some(tsfn) = self.handler.lock().unwrap().take() {
            guard
                .set_handler(move |input: String| {
                    let tsfn_clone = tsfn.clone();
                    async move {
                        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
                        let tx_clone = tx.clone();
                        tsfn_clone.call_with_return_value::<String, _>(
                            Ok(input.clone()),
                            ThreadsafeFunctionCallMode::Blocking,
                            move |result: String| -> napi::Result<()> {
                                let _ = tx_clone.send(Ok(result));
                                Ok(())
                            },
                        );
                        match rx.recv() {
                            Ok(Ok(result)) => Ok(result),
                            Ok(Err(e)) => Err(bus::ZenohError::Query(e.to_string())),
                            Err(_) => Err(bus::ZenohError::Query(
                                "handler channel closed".to_string(),
                            )),
                        }
                    }
                })
                .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        }

        guard.run().map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, e.to_string())
        })?;
        Ok(())
    }

    #[napi]
    pub async fn run(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        let mut guard = self.inner.lock().await;
        
        let tsfn = handler;
        guard
            .set_handler(move |input: String| {
                let tsfn_clone = tsfn.clone();
                async move {
                    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
                    let tx_clone = tx.clone();
                    tsfn_clone.call_with_return_value::<String, _>(
                        Ok(input.clone()),
                        ThreadsafeFunctionCallMode::Blocking,
                        move |result: String| -> napi::Result<()> {
                            let _ = tx_clone.send(Ok(result));
                            Ok(())
                        },
                    );
                    match rx.recv() {
                        Ok(Ok(result)) => Ok(result),
                        Ok(Err(e)) => Err(bus::ZenohError::Query(e.to_string())),
                        Err(_) => Err(bus::ZenohError::Query(
                            "handler channel closed".to_string(),
                        )),
                    }
                }
            })
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;

        guard.run().map_err(|e| {
            napi::Error::new(napi::Status::GenericFailure, e.to_string())
        })?;
        Ok(())
    }

    #[napi]
    pub async fn run_json(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        self.run(handler).await
    }

    #[napi]
    pub async fn run_stream(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        self.run(handler).await
    }
}
