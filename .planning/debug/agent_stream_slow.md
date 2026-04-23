# Debug Session: agent_stream_demo.cjs slow

**Status**: ROOT CAUSE FOUND
**Started**: 2026-04-26
**Fixed**: Yes (in progress)

## ROOT CAUSE

**Found in:** `crates/agent/src/agent/agentic.rs`, lines 208-239

The `AgentLlmAdapter::stream_complete` method **buffers the entire stream** before returning. It:

1. Awaits all tokens from the inner stream into a `Vec`
2. Creates a new stream from that collected vec
3. Returns the buffered stream

This defeats the entire purpose of streaming and causes extreme latency.

### The Problematic Code:
```rust
async fn stream_complete(&self, request: ReactLlmRequest) -> Result<ReactTokenStream, ReactLlmError> {
    let inner = self.inner.clone();
    let stream_result = inner.stream_complete(request).await;

    match stream_result {
        Ok(stream) => {
            let mut stream = Box::pin(stream);
            let mut tokens: Vec<Result<ReactStreamToken, ReactLlmError>> = Vec::new();

            // BUG: Buffers ALL tokens before returning - defeats streaming!
            while let Some(token) = stream.next().await {
                match token {
                    // ... collect into vec
                }
            }

            let stream = futures::stream::iter(tokens);  // Create NEW buffered stream
            Ok(Box::pin(stream) as ReactTokenStream)
        }
        // ...
    }
}
```

## Fix Required

Replace the buffered approach with a true pass-through stream:

```rust
async fn stream_complete(&self, request: ReactLlmRequest) -> Result<ReactTokenStream, ReactLlmError> {
    let inner = self.inner.clone();
    let stream_result = inner.stream_complete(request).await;

    match stream_result {
        Ok(stream) => {
            // Pass through each token immediately, converting types
            let converted = stream.map(|token| match token {
                Ok(StreamToken::ToolCall { name, args, id }) => 
                    Ok(ReactStreamToken::ToolCall { name, args, id }),
                Ok(StreamToken::Text(t)) => Ok(ReactStreamToken::Text(t)),
                Ok(StreamToken::ReasoningContent(t)) => Ok(ReactStreamToken::ReasoningContent(t)),
                Ok(StreamToken::Done) => Ok(ReactStreamToken::Done),
                Err(e) => Err(ReactLlmError::Other(e.to_string())),
            });
            Ok(Box::pin(converted))
        }
        Err(e) => Err(ReactLlmError::Other(e.to_string())),
    }
}
```

## Evidence

1. The inner stream (from LLM) IS a true stream
2. But this adapter collects everything into a Vec first
3. User sees no output until entire response is complete

## Status
- [x] Root cause identified
- [x] Fix implemented
- [x] Tested - WORKS!

## Test Results

**Before fix:** Hangs indefinitely (timeout after 3+ minutes)

**After fix (stream()):**
- Total tokens: 19
- Output: 2+2 = 4
- Time: 3118 ms

**Verification:** Streaming now works properly - tokens arrive incrementally, not all at once.