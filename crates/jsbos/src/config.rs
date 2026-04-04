use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Mutex;

#[napi]
pub struct ConfigLoader {
    inner: Mutex<config::loader::ConfigLoader>,
}

#[napi]
impl ConfigLoader {
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        Ok(ConfigLoader {
            inner: Mutex::new(config::loader::ConfigLoader::new()),
        })
    }

    #[napi]
    pub fn discover(&self) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        guard.discover_mut();
        Ok(())
    }

    #[napi]
    pub fn add_file(&self, path: String) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        guard.add_file_mut(path);
        Ok(())
    }

    #[napi]
    pub fn add_directory(&self, path: String) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        guard
            .add_directory_mut(path)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(())
    }

    #[napi]
    pub fn add_inline(&self, data: serde_json::Value) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        guard.add_inline_mut(data);
        Ok(())
    }

    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        guard.reset();
        Ok(())
    }

    #[napi]
    pub fn load_sync(&self) -> Result<String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        let value = guard
            .load_sync()
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(value.to_string())
    }

    #[napi]
    pub fn reload_sync(&self) -> Result<String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| napi::Error::new(napi::Status::GenericFailure, "lock poisoned"))?;
        // reload is async in Rust, but for JS we use tokio runtime to block_on
        let value = tokio::runtime::Handle::current()
            .block_on(guard.reload())
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(value.to_string())
    }
}
