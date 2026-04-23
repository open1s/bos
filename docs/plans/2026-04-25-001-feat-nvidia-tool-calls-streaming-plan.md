---
title: "feat: Complete NVIDIA tool_calls streaming + integration test"
type: feat
status: active
date: 2026-04-25
origin: crates/react/src/llm/vendor/nvidia.rs#L380-399
---

# Complete NVIDIA Tool Calls Streaming + Integration Test

## Overview

Complete the commented-out `tool_calls` streaming handler in `nvidia.rs` and add an integration test that uses the real config loader to validate the feature end-to-end with an actual LLM provider.

## Problem Frame

The NVIDIA vendor's `stream_complete` method has a fully-implemented but commented-out `delta.tool_calls` handler (lines 380–399). When the NVIDIA API sends tool calls in the OpenAI-compatible `delta.tool_calls` format, these events are silently dropped because the handler is commented out. Only the older `delta.function_call` format is currently processed.

The commented code is structurally correct but references the wrong field names for the internal `ToolCall` parsing structs. The fix requires uncommenting the block and adapting it to use the correct field accessors from the already-defined `NvidiaToolCall` / `ToolCall` deserialization structs in the file.

## Requirements Trace

- R1. `stream_complete` must emit `StreamToken::ToolCall` for both `delta.tool_calls` and `delta.function_call` formats
- R2. Integration test must load real LLM credentials from `~/.bos/conf/config.toml` via `ConfigLoader`
- R3. Integration test must send a request that triggers tool calls and verify the tokens are correctly emitted

## Scope Boundaries

- Does **not** modify the non-streaming `complete` path (already handles tool calls correctly via the full response object)
- Does **not** change `NvidiaVendor::new` / builder — continues to accept endpoint, model, api_key directly
- Does **not** modify OpenAI, OpenRouter, or other vendors

## Context & Research

### Relevant Code and Patterns

- **`crates/react/src/llm.rs:355-364`** — `StreamToken` enum definition with `ToolCall { name, args, id }` fields
- **`crates/react/src/llm/vendor/nvidia.rs:65-94`** — `NvidiaResponse`, `Choice`, `MessageContent`, `ToolCall`, `FunctionCall` deserialization structs (used for streaming delta parsing)
- **`crates/react/src/llm/vendor/nvidia.rs:401-416`** — Active `delta.function_call` handler (works correctly; serves as reference for the `delta.tool_calls` fix)
- **`crates/react/src/llm/vendor/nvidia.rs:380-399`** — **Commented-out `delta.tool_calls` handler — the target fix**
- **`crates/react/src/llm/vendor/openai.rs`** — OpenAI vendor handles both `delta.tool_calls` and `delta.function_call` in streaming; use as pattern reference
- **`crates/react/tests/llm_integration_test.rs`** — Existing integration test that loads real config and tests NVIDIA streaming; demonstrates config loading + `LlmConfig` helper pattern
- **`crates/config/src/loader.rs`** — `ConfigLoader::discover()` + `load()` for TOML config loading from standard paths

### Institutional Learnings

- Config discovery paths: `/etc/bos/conf`, `~/.bos/conf`, `~/.config/bos/conf`, `./bos/conf`
- Config is loaded as `serde_json::Value`; `LlmConfig` struct is a local helper that extracts fields from the loaded JSON
- Integration tests with real LLMs are gated on valid credentials in `config.toml`; tests skip gracefully when credentials are absent

### External References

- NVIDIA API `delta.tool_calls` format mirrors OpenAI streaming tool calls: array of `{ id, type: "function", function: { name, arguments } }`
- OpenAI streaming delta spec: <https://platform.openai.com/docs/api-reference/chat/streaming>

## Key Technical Decisions

- **Uncomment + adapt rather than rewrite**: The commented block at lines 380–399 is structurally correct. The fix is to adapt field accesses to match the actual `ToolCall` struct definitions (which use `function` as a nested struct, not `function_call`). See decision below.
- **Use `call.id` directly**: Unlike the commented code which tried to use `call.id` on the raw extracted data, the proper access is `call.id.clone()` from the deserialized `ToolCall` struct.
- **Generate UUID only when `id` is absent**: Match the existing `function_call` path behavior — generate a UUID when `call.id` is `None`.
- **Reuse existing `NvidiaToolCall` / `ToolCall` types**: The file already defines `ToolCall` (line 85) with fields `id: Option<String>` and `function: FunctionCall`. The uncommented handler should use these types, not inline field access.

## Open Questions

### Resolved During Planning

- **Which field accessor style to use?** The commented code uses `call.id` and `call.function` on a partially-parsed structure. The actual parsed type is `ToolCall` (line 85) which has `id: Option<String>` and `function: FunctionCall`. The fix is to iterate over `choice.delta.tool_calls` (which deserializes as `Vec<ToolCall>`) and access `call.id.clone()` and `call.function.name` / `call.function.arguments`.

### Deferred to Implementation

- Whether the integration test should target `nvidia/nemotron` or another model that reliably returns tool calls — depends on what models are configured in the user's `config.toml`

## Implementation Units

- [ ] **Unit 1: Uncomment and fix `delta.tool_calls` handler in `nvidia.rs`**

  **Goal:** Enable `StreamToken::ToolCall` emission when NVIDIA API sends tool calls via `delta.tool_calls`

  **Requirements:** R1

  **Dependencies:** None

  **Files:**
  - Modify: `crates/react/src/llm/vendor/nvidia.rs`

  **Approach:**
  - Locate the commented block at lines 380–399
  - Uncomment it
  - The block currently references `call.id` and `call.function` on a type that doesn't match the actual parsed `ToolCall` struct
  - The correct access pattern is: iterate `choice.delta.tool_calls` (each element is a `ToolCall` with `id: Option<String>` and `function: FunctionCall`)
  - Replace the commented block's field accesses with proper struct field access: `call.id.clone()` and `call.function.name` / `call.function.arguments`
  - Follow the same emit pattern as `function_call` handler (lines 401–416): parse `arguments` as JSON, send `StreamToken::ToolCall { name, args, id }`

  **Patterns to follow:**
  - `crates/react/src/llm/vendor/nvidia.rs:401-416` — the active `function_call` handler emits `StreamToken::ToolCall` with generated UUID fallback
  - `crates/react/src/llm/vendor/openai.rs` — cross-vendor reference for `delta.tool_calls` handling

  **Test scenarios:**
  - Scenario: NVIDIA API sends SSE chunk with `delta.tool_calls: [{ id: "call_abc", type: "function", function: { name: "get_weather", arguments: "{\"location\":\"SF\"}" } }]` → expects `StreamToken::ToolCall { name: "get_weather", args: {"location":"SF"}, id: Some("call_abc") }`
  - Scenario: Same as above but `id` is null → expects `StreamToken::ToolCall` with `id: Some(generated_uuid)`
  - Scenario: Stream contains both `delta.content` and `delta.tool_calls` in different chunks → both tokens emitted in order
  - Scenario: Empty `tool_calls` array → no token emitted (already handled by `if let Some(calls)` guard)

  **Verification:**
  - `cargo test -p react nvidia::tests` passes (existing unit test at line 497 also exercises the extractor with tool call chunks)
  - New integration test (Unit 2) validates end-to-end with real config

- [ ] **Unit 2: Add integration test using real LLM from config loader**

  **Goal:** Add a test that loads real NVIDIA credentials from config, sends a request that triggers tool calls, and verifies correct `StreamToken::ToolCall` emission

  **Requirements:** R2, R3

  **Dependencies:** Unit 1 (the handler must be uncommented first, otherwise tool calls are silently dropped)

  **Files:**
  - Modify: `crates/react/tests/llm_integration_test.rs` (append new test)

  **Approach:**
  - Use the existing config loading pattern from the file: `ConfigLoader::new().discover()` + `load().await`
  - Define `LlmConfig` helper struct (same pattern as existing `from_global_model` / `from_openrouter` helpers)
  - Create `NvidiaVendor` from loaded config: `NvidiaVendor::new(base_url, model, api_key)`
  - Construct an `LlmRequest` with a system prompt that encourages tool use (e.g., "You have access to a get_weather tool. Always use it when asked about weather.")
  - Call `stream_complete` and collect tokens
  - Assert that at least one `StreamToken::ToolCall` was emitted with non-empty `name` and `args`
  - If no `config.toml` is found or credentials are missing, the test should skip gracefully (use `Option` or `Result` with `#[ignore]` attribute or `should_panic` guard)

  **Patterns to follow:**
  - `crates/react/tests/llm_integration_test.rs:test_nvidia_vendor_stream_with_config` — existing streaming test as reference for config loading and token collection
  - `crates/react/tests/llm_integration_test.rs` — config loading pattern, `LlmConfig` struct definition, vendor instantiation

  **Test scenarios:**
  - Happy path: With valid NVIDIA credentials and a tool-capable model (e.g., `nvidia/nemotron` or similar), the test sends a weather query and verifies `StreamToken::ToolCall` with `get_weather` name is emitted
  - Graceful skip: When `~/.bos/conf/config.toml` is absent or lacks NVIDIA credentials, the test exits early without failure (allows CI to pass without credentials)
  - Token ordering: `StreamToken::ToolCall` must appear before `StreamToken::Done`

  **Verification:**
  - `cargo test -p react -- --nocapture` shows integration test result
  - If credentials are available and model supports tool calls: test passes with `ToolCall` token captured
  - If credentials unavailable: test exits early with clear "skipping" message (not a failure)

## System-Wide Impact

- **Error propagation**: If `delta.tool_calls` parsing fails (malformed JSON in arguments), the error is logged and the tool call is silently skipped (consistent with `function_call` path behavior at lines 405–408 which returns `Value::Null` on parse error)
- **Unchanged invariants**: Non-tool-call streams behave identically; the fix only adds handling for the previously-ignored `tool_calls` format

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Config file absent in test environment | Test uses early return / skip pattern already established in the file |
| NVIDIA model doesn't support tool calls or returns them via `function_call` instead | `delta.function_call` path is already active; dual-path handling means either format works |
| Regression in streaming token ordering | Existing unit tests (line 497) and integration tests validate token emission |
| Changes to `delta.tool_calls` parsing affect other vendors | No cross-vendor changes; only `nvidia.rs` is modified |

## Documentation / Operational Notes

- No user-facing documentation changes required (this is an internal Rust crate fix)
- Integration test serves as living documentation for how to wire config loading to vendor instantiation

## Sources & References

- Related code: `crates/react/src/llm/vendor/nvidia.rs`
- StreamToken definition: `crates/react/src/llm.rs:355-364`
- Config loading pattern: `crates/react/tests/llm_integration_test.rs`
- OpenAI reference implementation: `crates/react/src/llm/vendor/openai.rs`
- ConfigLoader: `crates/config/src/loader.rs`