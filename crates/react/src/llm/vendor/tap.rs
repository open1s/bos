//! Process-global tap for the HTTP calls the built-in LLM vendors make.
//!
//! The vendors share one client builder, but not one request object, so the
//! tap wraps the send call itself: it records status, content length, TTFB and
//! total time without reading the body (a stream is consumed by the caller).
//! The host installs one callback; it must stay cheap and must not block.

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use reqwest::{Error, Response};
use serde::Serialize;

/// One observed LLM HTTP exchange.
#[derive(Debug, Clone, Serialize)]
pub struct HttpExchange {
    /// What made the call, for example `llm`.
    pub label: &'static str,
    /// HTTP method.
    pub method: String,
    /// Full URL (query stays; the host redacts what it publishes).
    pub url: String,
    /// Status code, when a response arrived.
    pub status: Option<u16>,
    /// Correlation id of the model request this send belongs to.
    pub request_id: Option<String>,
    /// Request body bytes, when known.
    pub request_bytes: u64,
    /// Response body bytes from content-length, or 0 for a stream.
    pub response_bytes: u64,
    /// Response content type.
    pub content_type: Option<String>,
    /// Time to response headers.
    pub ttfb_ms: u64,
    /// Time to the send call returning.
    pub total_ms: u64,
    /// Transport error, when the call failed.
    pub error: Option<String>,
}

/// A callback installed by the host.
pub type HttpTap = Arc<dyn Fn(HttpExchange) + Send + Sync>;

static TAP: OnceLock<Mutex<Option<HttpTap>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<HttpTap>> {
    TAP.get_or_init(|| Mutex::new(None))
}

/// Install or clear the process-global tap.
pub fn set_http_tap(tap: Option<HttpTap>) {
    if let Ok(mut slot) = slot().lock() {
        *slot = tap;
    }
}

/// Send one request and report the exchange to the tap.
pub(crate) async fn traced_send<F>(
    label: &'static str,
    method: &str,
    url: &str,
    request_id: Option<&str>,
    request_bytes: u64,
    send: F,
) -> Result<Response, Error>
where
    F: Future<Output = Result<Response, Error>>,
{
    let started = Instant::now();
    match send.await {
        Ok(response) => {
            let ttfb_ms = started.elapsed().as_millis() as u64;
            let status = response.status().as_u16();
            let response_bytes = response.content_length().unwrap_or(0);
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            report(HttpExchange {
                label,
                method: method.to_string(),
                url: url.to_string(),
                status: Some(status),
                request_id: request_id.map(str::to_string),
                request_bytes,
                response_bytes,
                content_type,
                ttfb_ms,
                total_ms: started.elapsed().as_millis() as u64,
                error: None,
            });
            Ok(response)
        }
        Err(error) => {
            report(HttpExchange {
                label,
                method: method.to_string(),
                url: url.to_string(),
                status: None,
                request_id: request_id.map(str::to_string),
                request_bytes,
                response_bytes: 0,
                content_type: None,
                ttfb_ms: 0,
                total_ms: started.elapsed().as_millis() as u64,
                error: Some(error.to_string()),
            });
            Err(error)
        }
    }
}

fn report(exchange: HttpExchange) {
    let tap = slot().lock().ok().and_then(|slot| slot.clone());
    if let Some(tap) = tap {
        tap(exchange);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn a_send_reports_status_and_size() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello")
                .await
                .unwrap();
        });

        let captured: Arc<Mutex<Vec<HttpExchange>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        set_http_tap(Some(Arc::new(move |exchange| {
            sink.lock().unwrap().push(exchange);
        })));

        let url = format!("http://{addr}/thing");
        let client = reqwest::Client::new();
        let response = traced_send(
            "test",
            "GET",
            &url,
            Some("req-test"),
            0,
            client.get(&url).send(),
        )
        .await
        .expect("response");
        assert_eq!(response.status().as_u16(), 200);

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1, "one exchange per send");
        let event = &events[0];
        assert_eq!(event.label, "test");
        assert_eq!(event.status, Some(200));
        assert_eq!(event.response_bytes, 5);
        assert_eq!(event.request_id.as_deref(), Some("req-test"));
        assert_eq!(event.content_type.as_deref(), Some("text/plain"));
        drop(events);
        set_http_tap(None);
    }
}
