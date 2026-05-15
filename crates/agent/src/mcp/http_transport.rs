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
        let body =
            serde_json::to_string(msg).map_err(|e| HttpTransportError::Http(e.to_string()))?;
        let session_id = self.session_id.lock().unwrap().clone();

        let response = tokio::task::spawn_blocking(move || {
            let mut req = ureq::post(&base_url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");

            if let Some(ref sid) = session_id {
                req = req.header("Mcp-Session-Id", sid);
            }

            req.send(body.as_bytes())
                .map_err(|e| format!("Request error: {e}"))
        })
        .await
        .map_err(|e| HttpTransportError::Http(format!("Join error: {e}")))?;

        let response = response.map_err(|e| HttpTransportError::Http(e))?;

        let status = response.status().as_u16();
        let session_id_header = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok());
        if let Some(sid) = session_id_header {
            *self.session_id.lock().unwrap() = Some(sid.to_string());
        }

        let content_type = response
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if content_type.contains("text/event-stream") {
            let text = response
                .into_body()
                .read_to_string()
                .map_err(|e| HttpTransportError::Http(format!("Read error: {e}")))?;
            return self.parse_sse_response(&text);
        }

        if !(200..=299).contains(&status) && status != 202 {
            let body_text = response
                .into_body()
                .read_to_string()
                .unwrap_or_else(|_| String::new());
            if body_text.is_empty() {
                return Err(HttpTransportError::Http(format!(
                    "HTTP {} (empty body)",
                    status
                )));
            }
            return Err(HttpTransportError::Http(body_text));
        }

        let body_str = response
            .into_body()
            .read_to_string()
            .map_err(|e| HttpTransportError::Http(format!("Read body error: {e}")))?;

        Ok(body_str)
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
                    .header("Mcp-Session-Id", &id)
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
