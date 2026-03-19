use futures::Stream;
use std::pin::Pin;

use crate::llm::StreamToken;

pub struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn decode_chunk(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let text = String::from_utf8_lossy(chunk);
        let mut events = Vec::new();

        for line in text.lines() {
            if line.is_empty() {
                if !self.buffer.is_empty() {
                    let data = std::mem::take(&mut self.buffer);
                    if data.trim() == "[DONE]" {
                        events.push(SseEvent::Done);
                    } else {
                        events.push(SseEvent::Data(data));
                    }
                }
            } else if line.starts_with("data: ") {
                self.buffer.push_str(&line[6..]);
            }
        }
        events
    }
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

pub enum SseEvent {
    Data(String),
    Done,
    Error(String),
}

pub type TokenStream =
    Pin<Box<dyn Stream<Item = Result<StreamToken, crate::error::LlmError>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_decoder_single_event() {
        let mut decoder = SseDecoder::new();
        let chunk = b"data: {\"x\":1}\n\ndata: [DONE]\n\n";
        let events = decoder.decode_chunk(chunk);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SseEvent::Data(ref s) if s.contains("x")));
        assert!(matches!(events[1], SseEvent::Done));
    }

    #[test]
    fn test_sse_decoder_delta_chunk() {
        let mut decoder = SseDecoder::new();
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        let events = decoder.decode_chunk(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SseEvent::Data(ref s) if s.contains("hello")));
    }

    #[test]
    fn test_sse_decoder_empty_input() {
        let mut decoder = SseDecoder::new();
        let events = decoder.decode_chunk(b"");
        assert!(events.is_empty());
    }

#[test]
fn test_sse_decoder_ignore_non_data_lines() {
    let mut decoder = SseDecoder::new();
    let events = decoder.decode_chunk(b"event: message\nid: 123\ndata: {\"content\":\"hi\"}\n\n");
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], SseEvent::Data(ref s) if s.contains("hi")));
}

#[cfg(test)]
mod integration_tests;

    #[test]
    fn test_sse_decoder_ignore_non_data_lines() {
        let mut decoder = SseDecoder::new();
        let events =
            decoder.decode_chunk(b"event: message\nid: 123\ndata: {\"content\":\"hi\"}\n\n");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SseEvent::Data(ref s) if s.contains("hi")));
    }
}
