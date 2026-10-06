use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;

#[napi]
/// Publishes messages to a topic.
pub struct Publisher {
  pub(crate) inner: bus::Publisher,
}

#[napi]
impl Publisher {
  #[napi(factory)]
  /// Create a publisher for `topic`.
  pub async fn new(topic: String) -> Result<Self> {
    Ok(Publisher {
      inner: bus::Publisher::new(topic),
    })
  }

  #[napi(factory)]
  /// Create a publisher bound to `session`.
  pub async fn with_session(topic: String, session: &External<bus::Session>) -> Result<Self> {
    Ok(Publisher {
      inner: bus::Publisher::new(topic)
        .with_session(Arc::new((**session).clone()))
        .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?,
    })
  }

  #[napi(getter)]
  /// The published topic.
  pub fn topic(&self) -> String {
    self.inner.topic().to_string()
  }

  #[napi]
  /// Publish a text payload.
  pub async fn publish_text(&self, payload: String) -> Result<()> {
    self
      .inner
      .publish(&payload)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }

  #[napi]
  /// Publish a JSON payload.
  pub async fn publish_json(&self, data: serde_json::Value) -> Result<()> {
    let json_str = data.to_string();
    self
      .inner
      .publish(&json_str)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    Ok(())
  }
}
