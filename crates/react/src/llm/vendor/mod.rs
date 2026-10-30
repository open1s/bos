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

/// Whether full wire bodies are opted into the log.
///
/// The full payload of every request and response used to ride the log for
/// every step — megabytes a day, with base64 images making single lines
/// worse. Full bodies are opt-in now: set `BOS_LOG_REQUEST_BODIES=1` when a
/// wire exchange needs debugging. The INFO one-line summary always stays,
/// and a TRACE line without the switch carries a bounded head.
pub(crate) fn wire_bodies_enabled() -> bool {
    std::env::var("BOS_LOG_REQUEST_BODIES")
        .ok()
        .is_some_and(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("on")
        })
}

/// Drops provider special-token spans from assistant text.
///
/// A gateway can leave the model's wire markup in `content` even when it
/// parsed the structured tool calls: a measured run streamed `<｜DSML｜ calls>`
/// into the visible answer, so the reader saw wire markup where the model's
/// words should be. A span starts at `<` followed by a halfwidth or fullwidth
/// bar and is closed by the next `>`; everything outside spans passes through.
/// The streaming form holds a marker split across transport chunks until the
/// next chunk closes (or disproves) it.
#[derive(Default)]
pub(crate) struct SpecialTokenFilter {
    /// Text held back because it may still become a marker.
    held: String,
}

impl SpecialTokenFilter {
    /// Filter one streamed chunk, returning the text that survives it.
    pub(crate) fn push(&mut self, text: &str) -> String {
        self.held.push_str(text);
        let mut out = String::new();
        let mut rest = self.held.as_str();
        loop {
            let Some(start) = rest.find('<') else {
                out.push_str(rest);
                self.held.clear();
                return out;
            };
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let mut after_open = tail.chars();
            after_open.next(); // the '<' itself
            match after_open.next() {
                None => {
                    // A lone '<' may open a marker in the next chunk.
                    self.held = tail.to_string();
                    return out;
                }
                Some(next) if next == '|' || next == '｜' => match tail.find('>') {
                    Some(end) => rest = &tail[end + 1..],
                    None => {
                        if tail.len() > 512 {
                            // Too long to be a marker: flush it as text.
                            out.push_str(tail);
                            self.held.clear();
                            return out;
                        }
                        // Hold the partial span; the next chunk may close it.
                        self.held = tail.to_string();
                        return out;
                    }
                },
                Some(_) => {
                    out.push('<');
                    rest = &tail[1..];
                }
            }
        }
    }

    /// Flush whatever is still held when the stream ends.
    pub(crate) fn finish(&mut self) -> String {
        std::mem::take(&mut self.held)
    }
}

/// Strip special-token spans from one complete string.
pub(crate) fn strip_special_tokens(text: &str) -> String {
    let mut filter = SpecialTokenFilter::default();
    let mut clean = filter.push(text);
    clean.push_str(&filter.finish());
    clean
}

/// A bounded head of a serialized wire body for a TRACE line, ending with the
/// size and the switch that unlocks the rest.
pub(crate) fn wire_body_head(json: &str) -> String {
    let mut head: String = json.chars().take(1_000).collect();
    if head.chars().count() < json.chars().count() {
        head.push_str(&format!(
            "… ({} chars; set BOS_LOG_REQUEST_BODIES=1 for the full body)",
            json.chars().count()
        ));
    }
    head
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

    /// A marker inside one transport chunk is dropped, and the model's words
    /// on both sides survive.
    #[test]
    fn a_special_token_inside_one_chunk_is_dropped() {
        assert_eq!(
            super::strip_special_tokens("before <｜DSML｜ calls> after"),
            "before  after"
        );
    }

    /// A marker split across chunks is held until the closing chunk, then
    /// dropped whole: the reader never sees half a marker.
    #[test]
    fn a_marker_split_across_chunks_is_dropped_once_closed() {
        let mut filter = super::SpecialTokenFilter::default();
        let mut out = filter.push("before <｜DS");
        out.push_str(&filter.push("ML｜ calls> after"));
        out.push_str(&filter.finish());
        assert_eq!(out, "before  after");
    }

    /// An ordinary '<' is text, not the start of a marker.
    #[test]
    fn a_lone_angle_bracket_passes_through() {
        let mut filter = super::SpecialTokenFilter::default();
        let mut out = filter.push("a < b");
        out.push_str(&filter.finish());
        assert_eq!(out, "a < b");
        assert_eq!(super::strip_special_tokens("two < three"), "two < three");
    }

    /// A span that never closes is the model's last words, not a marker: the
    /// end-of-stream flush keeps it rather than eating the answer.
    #[test]
    fn a_trailing_partial_span_is_flushed_not_lost() {
        let mut filter = super::SpecialTokenFilter::default();
        let mut out = filter.push("answer <｜unfinished");
        out.push_str(&filter.finish());
        assert_eq!(out, "answer <｜unfinished");
    }

    /// A '<|' run too long to be a marker is flushed as text instead of being
    /// held forever.
    #[test]
    fn an_overlong_open_span_is_not_a_marker() {
        let long = format!("<｜{}", "x".repeat(600));
        assert_eq!(super::strip_special_tokens(&long).len(), long.len());
    }
}
