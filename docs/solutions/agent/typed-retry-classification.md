---
module: agent
tags: [retry, resilience, llm, error-handling]
problem_type: best-practice
---

# Classify retryable errors by type, not by message text

## Problem

`ReActEngine::is_transient_error` decided whether to retry an LLM call by
formatting the error and grepping the debug text:

```rust
let err_str = format!("{:?}", err);
err_str.contains("429") || err_str.contains("timeout") || err_str.contains("502")
```

That is wrong in both directions. It retries a `400` whose body happens to
mention "timeout", and it misses transient conditions the list never
anticipated. It also couples retry policy to the `Debug` formatting of an
unrelated type.

## Solution

Move the decision onto the error type as `LlmError::is_retryable`, classify by
variant first, and parse the HTTP status when the message carries one:

```rust
pub fn is_retryable(&self) -> bool {
    match self {
        LlmError::RateLimited | LlmError::Timeout => true,
        LlmError::Http(message) | LlmError::Other(message) => retryable_message(message),
        LlmError::Parse(_) | LlmError::ApiKeyMissing => false,
    }
}
```

`retryable_message` extracts the leading status from the provider's
`"HTTP <code>: <body>"` shape and retries only `408`, `425`, `429`, and `5xx`.
Only when no status is present does it fall back to a small fragment list for
connection failures. The status code therefore wins over the body text.

## Why it works

- A 4xx client error is never retried, even when its body is confusing.
- New vendors keep working because they all emit the same `HTTP <code>:`
  prefix.
- The policy is testable directly, without constructing an HTTP client.