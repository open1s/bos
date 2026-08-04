//! A tiny HTTP/1.1 mock server that emulates both the OpenAI Chat Completions
//! and Responses APIs for hermetic end-to-end tests.
//!
//! It records every request (path + JSON body) and lets the test supply a
//! handler that returns either a complete JSON reply or an SSE stream.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub path: String,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MockReply {
    /// A complete JSON response (Content-Type: application/json).
    Json(serde_json::Value),
    /// A streaming SSE response. Each value becomes a `data: {json}\n\n` frame.
    /// When `append_done` is true a final `data: [DONE]\n\n` is written, which
    /// the Chat Completions protocol uses as its terminal signal.
    Sse {
        events: Vec<serde_json::Value>,
        append_done: bool,
    },
}

pub struct MockLlmServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockLlmServer {
    pub async fn start<F>(handler: F) -> Self
    where
        F: Fn(&CapturedRequest) -> MockReply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("mock server local addr");
        let requests: Arc<Mutex<Vec<CapturedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let requests_task = requests.clone();
        let handler_task = handler.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        if let Ok((stream, _)) = accepted {
                            let requests = requests_task.clone();
                            let handler = handler_task.clone();
                            tokio::spawn(async move {
                                let _ = serve_connection(stream, requests, handler).await;
                            });
                        }
                    }
                }
            }
        });

        Self {
            addr,
            requests,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    /// Base URL such as `http://127.0.0.1:PORT`. Vendors append protocol
    /// paths like `/chat/completions` or `/responses`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for MockLlmServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handler: Arc<dyn Fn(&CapturedRequest) -> MockReply + Send + Sync>,
) -> std::io::Result<()> {
    let (path, body) = read_request(&mut stream).await?;

    let captured = CapturedRequest { path, body };
    requests
        .lock()
        .expect("requests lock")
        .push(captured.clone());

    match handler(&captured) {
        MockReply::Json(value) => {
            let body_bytes = serde_json::to_vec(&value).unwrap_or_default();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body_bytes.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(&body_bytes).await?;
        }
        MockReply::Sse {
            events,
            append_done,
        } => {
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes()).await?;
            for event in events {
                let frame = format!("data: {}\n\n", event);
                stream.write_all(frame.as_bytes()).await?;
                stream.flush().await?;
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            if append_done {
                stream.write_all(b"data: [DONE]\n\n").await?;
            }
        }
    }
    stream.flush().await
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, serde_json::Value)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut header_end = None;

    while header_end.is_none() {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            header_end = Some(pos);
        }
    }
    let header_end = header_end.unwrap_or(buf.len());
    let header_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default().to_string();

    let mut content_length = 0usize;
    for line in lines {
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    while buf.len() < header_end + 4 + content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = &buf[header_end + 4..(header_end + 4 + content_length).min(buf.len())];
    let parsed = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    Ok((path, parsed))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
