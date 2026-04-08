use bus::Bus;
use std::sync::Arc;

#[allow(dead_code)]
pub async fn session_from_bus(inner: Arc<tokio::sync::Mutex<Bus>>) -> Arc<bus::Session> {
  let guard = inner.lock().await;
  let bus_copy = guard.clone();
  bus_copy.session()
}
