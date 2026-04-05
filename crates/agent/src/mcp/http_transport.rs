use reqwest::{Client, Response};

pub struct HttpTransport {
    client: Client,
    base_url: String,
    session_id: std::sync::Mutex<Option<String>>,
}

#[derive(Debug, Clone)]
pub enum HttpTransportError {
    Http(String),
    Connect(String),
    Session(String),
}

impl std::fmt::Display for HttpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(s) => write!(f, "HTTP error: {s}"),
            Self::Connect(s) => write!(f, "Connection error: {s}"),
            Self::Session(s) => write!(f, "Session error: {s}"),
        }
    }
}

impl std::error::Error for HttpTransportError {}

impl HttpTransport {
    pub fn new(base_url: impl Into<String>) -> Self {
        let url = base_url.into().trim_end_matches('/').to_string();
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap(),
            base_url: url,
            session_id: std::sync::Mutex::new(None),
        }
    }

    pub async fn send(&self, msg: &serde_json::Value) -> Result<String, HttpTransportError> {
        let mut req = self
            .client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(msg);

        if let Some(session) = self.session_id.lock().unwrap().clone() {
            req = req.header("Mcp-Session-Id", session);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| HttpTransportError::Http(e.to_string()))?;

        self.capture_session_id(&resp);

        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("text/event-stream") {
            return self.read_sse_response(resp).await;
        }

        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| format!("HTTP {status}"));
            return Err(HttpTransportError::Http(body));
        }

        resp.text()
            .await
            .map_err(|e| HttpTransportError::Http(e.to_string()))
    }

    pub fn set_session_id(&self, id: String) {
        *self.session_id.lock().unwrap() = Some(id);
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    pub async fn terminate_session(&self) -> Result<(), HttpTransportError> {
        let session = self.session_id.lock().unwrap().clone();
        if let Some(id) = session {
            let req = self
                .client
                .delete(&self.base_url)
                .header("Mcp-Session-Id", &id);

            let _ = req.send().await;
            *self.session_id.lock().unwrap() = None;
        }
        Ok(())
    }

    fn capture_session_id(&self, resp: &Response) {
        if let Some(sid) = resp
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().unwrap() = Some(sid.to_string());
        }
    }

    async fn read_sse_response(&self, resp: Response) -> Result<String, HttpTransportError> {
        use tokio::io::AsyncBufReadExt;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HttpTransportError::Http(e.to_string()))?;

        let text = String::from_utf8(bytes.to_vec())
            .map_err(|e| HttpTransportError::Http(e.to_string()))?;

        let mut lines = tokio::io::BufReader::new(std::io::Cursor::new(text));
        let mut buf = String::new();
        let mut last_data = String::new();

        while lines.read_line(&mut buf).await.unwrap_or(0) > 0 {
            let line = buf.clone();
            buf.clear();
            let trimmed = line.trim_end();

            if let Some(stripped) = trimmed.strip_prefix("data: ") {
                last_data = stripped.to_string();
            } else if trimmed.is_empty() && !last_data.is_empty() {
                return Ok(last_data.clone());
            }
        }

        if !last_data.is_empty() {
            Ok(last_data)
        } else {
            Err(HttpTransportError::Http("No data in SSE stream".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transport_new_strips_trailing_slash() {
        let transport = HttpTransport::new("http://localhost:8080/mcp/");
        assert_eq!(transport.base_url, "http://localhost:8080/mcp");
    }

    #[test]
    fn test_http_transport_error_display() {
        let err = HttpTransportError::Http("test".to_string());
        assert_eq!(err.to_string(), "HTTP error: test");

        let err = HttpTransportError::Connect("refused".to_string());
        assert_eq!(err.to_string(), "Connection error: refused");

        let err = HttpTransportError::Session("expired".to_string());
        assert_eq!(err.to_string(), "Session error: expired");
    }
}
