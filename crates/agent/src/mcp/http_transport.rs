use ureq;

pub struct HttpTransport {
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
            base_url: url,
            session_id: std::sync::Mutex::new(None),
        }
    }

    pub async fn send(&self, msg: &serde_json::Value) -> Result<String, HttpTransportError> {
        let base_url = self.base_url.clone();
        let body = serde_json::to_string(msg).map_err(|e| HttpTransportError::Http(e.to_string()))?;
        let session_id = self.session_id.lock().unwrap().clone();

        let response = tokio::task::spawn_blocking(move || {
            let mut req = ureq::post(&base_url)
                .set("Content-Type", "application/json")
                .set("Accept", "application/json, text/event-stream");

            if let Some(ref sid) = session_id {
                req = req.set("Mcp-Session-Id", sid);
            }

            req.send_string(&body)
        })
        .await
        .map_err(|e| HttpTransportError::Http(format!("Join error: {e}")))?
        .map_err(|e| HttpTransportError::Http(format!("Request error: {e}")))?;

        let status = response.status();
        let session_id_header = response.header("Mcp-Session-Id");
        if let Some(sid) = session_id_header {
            *self.session_id.lock().unwrap() = Some(sid.to_string());
        }

        let content_type = response
            .header("Content-Type")
            .unwrap_or("")
            .to_string();

        if content_type.contains("text/event-stream") {
            let text = response
                .into_string()
                .map_err(|e| HttpTransportError::Http(format!("Read error: {e}")))?;
            return self.parse_sse_response(&text);
        }

        if !(200..=299).contains(&status) && status != 202 {
            let body = response
                .into_string()
                .unwrap_or_else(|_| format!("HTTP {}", status));
            if body.is_empty() {
                return Err(HttpTransportError::Http(format!("HTTP {} (empty body)", status)));
            }
            return Err(HttpTransportError::Http(body));
        }

        let body = response
            .into_string()
            .map_err(|e| HttpTransportError::Http(format!("Read body error: {e}")))?;

        Ok(body)
    }

    pub fn set_session_id(&self, id: String) {
        *self.session_id.lock().unwrap() = Some(id);
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    pub async fn terminate_session(&self) -> Result<(), HttpTransportError> {
        let base_url = self.base_url.clone();
        let session_id = self.session_id.lock().unwrap().clone();
        *self.session_id.lock().unwrap() = None;

        if let Some(id) = session_id {
            tokio::task::spawn_blocking(move || {
                ureq::delete(&base_url)
                    .set("Mcp-Session-Id", &id)
                    .call()
                    .ok();
            })
            .await
            .ok();
        }
        Ok(())
    }

    fn parse_sse_response(&self, text: &str) -> Result<String, HttpTransportError> {
        let mut last_data = String::new();
        for line in text.lines() {
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