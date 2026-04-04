use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::Arc;

#[napi]
pub struct Caller {
    pub(crate) inner: bus::Caller,
}

#[napi]
impl Caller {
    #[napi(factory)]
    pub async fn new(name: String) -> Result<Self> {
        Ok(Caller {
            inner: bus::Caller::new(name, None),
        })
    }

    #[napi(factory)]
    pub async fn with_session(name: String, session: External<Arc<bus::Session>>) -> Result<Self> {
        Ok(Caller {
            inner: bus::Caller::new(name, Some(Arc::clone(&session))),
        })
    }

    #[napi]
    pub async fn call_text(&self, payload: String) -> Result<String> {
        let out = self
            .inner
            .call::<String, String>(&payload)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(out)
    }
}

#[napi]
pub struct Callable {
    inner: Arc<tokio::sync::Mutex<Option<bus::Callable<String, String>>>>,
    pub(crate) handler: Arc<std::sync::Mutex<Option<ThreadsafeFunction<String>>>>,
    is_started: Arc<std::sync::atomic::AtomicBool>,
}

#[napi]
impl Callable {
    pub(crate) fn new(callable: bus::Callable<String, String>) -> Self {
        Callable {
            inner: Arc::new(tokio::sync::Mutex::new(Some(callable))),
            handler: Arc::new(std::sync::Mutex::new(None)),
            is_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[napi]
    pub fn set_handler(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        let mut guard = self.handler.lock().unwrap();
        *guard = Some(handler);
        Ok(())
    }

    #[napi]
    pub fn is_started(&self) -> bool {
        self.is_started.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[napi]
    pub async fn start(&self) -> Result<()> {
        if self.is_started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(napi::Error::new(napi::Status::GenericFailure, "Callable already started"));
        }

        let mut guard = self.inner.lock().await;
        let callable = guard.as_mut().ok_or_else(|| {
            napi::Error::new(napi::Status::GenericFailure, "Callable not available")
        })?;

        if let Some(tsfn) = self.handler.lock().unwrap().take() {
            callable
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

        callable
            .start()
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        
        self.is_started.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    #[napi]
    pub async fn run(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        {
            let mut guard = self.handler.lock().unwrap();
            *guard = Some(handler);
        }
        self.start().await
    }

    #[napi]
    pub async fn run_json(&self, handler: ThreadsafeFunction<String>) -> Result<()> {
        self.run(handler).await
    }
}
