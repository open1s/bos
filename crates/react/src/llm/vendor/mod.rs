//! Vendor clients and wire types for the supported LLM providers.

pub mod anthropic;
pub mod deepseek;
pub mod nvidia;
pub mod openai;
pub mod openaicompatible;
pub mod openrouter;
pub mod responses;
pub mod router;
pub use anthropic::{AnthropicVendor, AnthropicVendorBuilder};
pub use deepseek::{DeepSeekVendor, DeepSeekVendorBuilder};
pub use nvidia::NvidiaVendor;
pub use openai::{OpenAiClient, OpenAiVendor, OpenAiVendorBuilder};
pub use openaicompatible::{
    sse_has_done_signal, ChatCompletionChunk, ChatCompletionResponse, ChatMessage, Choice,
    ChunkChoice, Delta, FunctionCall, FunctionCallDelta, LogProbContent, LogProbs, OpenAIExtractor,
    PendingToolCall, StreamToolCallAccumulator, ToolCall, ToolCallDelta, Usage,
};
pub use openrouter::OpenRouterVendor;
pub use responses::{
    ResponsesContentPart, ResponsesExtractor, ResponsesFunctionCallAccumulator, ResponsesInputItem,
    ResponsesItem, ResponsesReasoningSummary, ResponsesRequest, ResponsesResponse,
    ResponsesStreamEvent, ResponsesTransport, ResponsesUsage,
};
pub use router::LlmRouter;

use std::time::Duration;

/// How long a built-in vendor waits for the next read (response headers or the
/// next streamed chunk) before failing the call.
///
/// Without it a provider that accepts the connection and then stalls leaves the
/// request pending forever. It has to be generous because the same clock covers
/// a STREAMED completion: a reasoning model can think for minutes and a
/// gateway can buffer until the answer is ready, and a 120s window cut both
/// short while reporting the cut as "error decoding response body". A
/// whole-response call is still bounded at 120s by the engine's own
/// `llm_timeout`; a stream is bounded by its Stop control.
pub(crate) const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a built-in vendor waits for the TCP/TLS connection to open.
pub(crate) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Build the timeout-bounded HTTP client shared by the built-in vendors.
pub(crate) fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .read_timeout(DEFAULT_READ_TIMEOUT)
        .build()
        .expect("failed to build the shared HTTP client")
}

/// The error a stream that stopped mid-body reports, with its cause.
///
/// `reqwest`'s own sentence for a body failure is only "error decoding response
/// body"; what actually happened is the cause underneath it ("operation timed
/// out", "unexpected end of file", "connection reset by peer"), which is the one
/// fact a reader needs. `LlmError` carries a message rather than a boxed source,
/// so the chain is joined here - and a timeout keeps its own kind, so retry
/// classification still sees it as transient.
pub(crate) fn stream_error(e: reqwest::Error) -> crate::llm::LlmError {
    use crate::llm::LlmError;
    if e.is_timeout() {
        return LlmError::Timeout;
    }
    let mut message = format!("Stream error: {e}");
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    LlmError::Other(message)
}

/// Merge `extra` into `leading_system`, returning `None` when `extra` is empty.
pub fn merge_system_prompt(extra: String, leading_system: Option<&str>) -> Option<String> {
    if extra.is_empty() {
        return None;
    }
    Some(match leading_system {
        Some(existing) if !existing.is_empty() => format!("{}\n{}", existing, extra),
        _ => extra,
    })
}

#[cfg(test)]
mod tests {
    use super::stream_error;
    use crate::llm::LlmError;
    use futures::StreamExt;

    /// A stalled body reads as a Timeout, and reqwest's own sentence for it is
    /// the misleading one.
    ///
    /// This is the whole bug in miniature: a stream whose next chunk never comes
    /// reports "error decoding response body" (reqwest wraps a body error in a
    /// decode error), so the app told the reader the response was malformed when
    /// it had actually timed out. The test pins both halves - the sentence
    /// reqwest gives and what this vendor makes of it.
    #[tokio::test]
    async fn a_stalled_body_is_a_timeout_not_a_decode_failure() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata\r\n",
                )
                .await
                .unwrap();
            // Hold the connection open past the read timeout without sending.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });

        let client = reqwest::Client::builder()
            .read_timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();
        let response = client.get(format!("http://{addr}/")).send().await.unwrap();
        let mut stream = response.bytes_stream();
        assert!(
            stream.next().await.expect("one chunk").is_ok(),
            "the first chunk arrives, so the stream is open"
        );
        let error = stream
            .next()
            .await
            .expect("the read fails")
            .expect_err("a stalled read is an error");
        assert!(error.is_timeout(), "the premise: {error}");
        assert_eq!(
            error.to_string(),
            "error decoding response body",
            "reqwest's sentence names the wrong failure"
        );
        assert!(
            matches!(stream_error(error), LlmError::Timeout),
            "and the vendor reports the stall it actually was"
        );
    }
}
