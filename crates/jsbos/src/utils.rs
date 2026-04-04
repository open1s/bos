use std::sync::Arc;
use bus::Bus;

pub async fn session_from_bus(inner: Arc<tokio::sync::Mutex<Bus>>) -> Arc<bus::Session> {
    let guard = inner.lock().await;
    let bus_copy = guard.clone();
    bus_copy.session()
}
