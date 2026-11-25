//! The DeepSeek vendor, an OpenAI-compatible provider.

use async_trait::async_trait;

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
    VendorBuilderError,
};

use super::OpenAiVendor;
use super::openai::ReasoningReplay;

/// DeepSeek vendor. DeepSeek's API is OpenAI-compatible on both protocols:
/// `/chat/completions` for `ApiMode::Chat` and `/responses` for
/// `ApiMode::Responses`. The HTTP work is delegated to [`OpenAiVendor`] (which
/// already routes `Responses` mode through [`crate::llm::vendor::ResponsesTransport`]); this type
/// exists to give DeepSeek its own identity and correct defaults.
pub struct DeepSeekVendor {
    inner: OpenAiVendor,
}

impl Clone for DeepSeekVendor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl DeepSeekVendor {
    /// Create a DeepSeek vendor with an explicit endpoint, model, and key.
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            // DeepSeek's thinking mode requires every earlier turn's reasoning
            // back once tools are in play. The engine must therefore keep the
            // blocks across turn boundaries, and a strict Responses endpoint
            // that returns no encrypted state still replays their plain text as
            // `reasoning_text` items.
            inner: OpenAiVendor::new(endpoint, model, api_key)
                .with_reasoning_replay(ReasoningReplay::Content)
                .with_replay_text_reasoning(true),
        }
    }

    /// Replace the delegate's idle-socket guard with `timeout`.
    ///
    /// The wire wait and the engine's per-request ceiling are one number: see
    /// [`OpenAiVendor::with_read_timeout`].
    #[must_use]
    pub fn with_read_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.inner = self.inner.with_read_timeout(timeout);
        self
    }

    /// Start building a DeepSeek vendor.
    pub fn builder() -> DeepSeekVendorBuilder {
        DeepSeekVendorBuilder::new()
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for DeepSeekVendor
{
    async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        LlmClient::<S, C>::list_models(&self.inner).await
    }
    async fn complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        self.inner.complete(persona, req, session, context).await
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        req: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        self.inner
            .stream_complete(persona, req, session, context)
            .await
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "deepseek"
    }
    fn replays_reasoning_across_turns(&self, _model: &str) -> bool {
        // A request that carries tools is rejected with 400 when any earlier
        // assistant turn's `reasoning_content` / `reasoning_text` is missing,
        // even for a turn that made no tool call.
        true
    }
}

/// Builder for [`DeepSeekVendor`].
pub struct DeepSeekVendorBuilder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl DeepSeekVendorBuilder {
    /// Create a builder with DeepSeek defaults.
    pub fn new() -> Self {
        Self {
            endpoint: "https://api.deepseek.com".to_string(),
            model: "deepseek-chat".to_string(),
            api_key: None,
        }
    }

    /// Set the API endpoint.
    pub fn endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Set the model id.
    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Set the API key.
    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    /// Build the vendor, or fail if no key was set.
    pub fn build(self) -> Result<DeepSeekVendor, VendorBuilderError> {
        let api_key = self.api_key.ok_or(VendorBuilderError::MissingApiKey)?;
        Ok(DeepSeekVendor::new(self.endpoint, self.model, api_key))
    }
}

impl Default for DeepSeekVendorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ApiMode, Content, LlmMessage, LlmSession, ProviderRequest};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A DeepSeek Responses request whose history carries only visible
    /// reasoning text (the shape BSH reseeds from its transcript) must put a
    /// `reasoning_text` item back on the wire; otherwise the service answers
    /// 400 and the second turn is lost.
    #[tokio::test]
    async fn a_responses_request_replays_transcript_reasoning_as_text() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("the address");
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut buf = vec![0u8; 64 * 1024];
            let read = socket.read(&mut buf).await.expect("the request");
            let text = String::from_utf8_lossy(&buf[..read]).to_string();
            let body = text
                .split_once("\r\n\r\n")
                .map(|(_, body)| body.to_string())
                .unwrap_or_default();
            let _ = tx.send(body);
            let payload = r#"{"id":"resp-1","object":"response","created_at":1,"status":"completed","model":"deepseek-flash","output":[{"type":"message","id":"msg-1","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                payload.len(),
                payload
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("the response");
        });

        let vendor = DeepSeekVendor::new(
            format!("http://{addr}"),
            "deepseek-flash".to_string(),
            "k".to_string(),
        );
        let input = Content::text("next");
        let mut request = LlmRequest::new("deepseek-flash");
        request.api_mode = ApiMode::Responses;
        request.input = input.clone();
        request.provider_request = Some(ProviderRequest {
            model: "deepseek-flash".to_string(),
            system_prompt: Some("policy".to_string()),
            messages: vec![
                LlmMessage::user("first"),
                LlmMessage::AssistantReasoning {
                    content: "earlier chain of thought".to_string(),
                    provider_state: None,
                },
                LlmMessage::assistant("earlier answer"),
            ],
            input,
            api_mode: ApiMode::Responses,
            ..Default::default()
        });
        vendor
            .complete(
                None,
                request,
                &mut LlmSession::new(),
                &mut crate::llm::LlmContext::default(),
            )
            .await
            .expect("the mock answers");

        let body: serde_json::Value = serde_json::from_str(&rx.await.expect("the captured body"))
            .expect("the request is JSON");
        let reasoning = body["input"]
            .as_array()
            .expect("input items")
            .iter()
            .find(|item| item["type"] == "reasoning")
            .expect("a reasoning item");
        assert_eq!(
            reasoning["content"],
            serde_json::json!([{ "type": "reasoning_text", "text": "earlier chain of thought" }]),
            "the strict endpoint gets the text back: {body}"
        );
        assert!(
            reasoning.get("id").is_none(),
            "a synthesized item carries no id: {body}"
        );
    }

    /// On the chat wire DeepSeek refuses an assistant reply that follows a tool
    /// result without `reasoning_content`, so the adapter gives the reply the
    /// space placeholder when the turn kept no reasoning of its own.
    #[test]
    fn a_chat_reply_after_a_tool_result_gets_placeholder_reasoning() {
        let vendor = DeepSeekVendor::new(
            "https://api.deepseek.com".to_string(),
            "deepseek-chat".to_string(),
            "k".to_string(),
        );
        let input = Content::text("continue");
        let mut request = LlmRequest::new("deepseek-chat");
        request.input = input.clone();
        request.provider_request = Some(ProviderRequest {
            model: "deepseek-chat".to_string(),
            messages: vec![
                LlmMessage::user("run it"),
                LlmMessage::assistant_tool_call("call-1", "probe", serde_json::json!({})),
                LlmMessage::ToolResult {
                    tool_call_id: "call-1".to_string(),
                    content: "result".to_string(),
                },
                LlmMessage::assistant("done"),
            ],
            input,
            ..Default::default()
        });
        let wire = vendor.inner.request_preview(
            &request,
            &LlmSession::new(),
            &crate::llm::LlmContext::default(),
        );
        let messages = wire["messages"].as_array().expect("messages");
        let tool_index = messages
            .iter()
            .position(|message| message["role"] == "tool")
            .expect("the tool result");
        let after_tool = &messages[tool_index + 1];
        assert_eq!(
            after_tool["reasoning_content"], " ",
            "the reply leads with the accepted placeholder: {wire}"
        );
        assert_eq!(after_tool["content"], "done");
    }
}
