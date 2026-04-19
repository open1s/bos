---
title: Refactor Architecture Concerns from Codebase Review
type: refactor
status: active
date: 2026-04-19
---

# Refactor Architecture Concerns from Codebase Review

## Overview

Address 6 high-value, low-risk refactoring targets identified during the BOS architecture review. Each target eliminates duplication, fixes bugs, or corrects semantic mismatches without breaking public API contracts.

## Problem Frame

The architecture review revealed several code quality concerns that reduce maintainability and correctness: duplicated ReAct loop logic (~400 lines), a typo in production variable names, a router that incorrectly reports tool support, a plugin streaming API that misuses the LLM response callback for token processing, and an unused SkillInjector that could add value if wired into the runtime.

## Requirements Trace

- R1. Eliminate code duplication in engine.rs (`react` vs `react_with_request`) — reduces maintenance burden and divergence risk
- R2. Eliminate code duplication in agentic.rs (`react` vs `run_simple` skill/tool registration) — same concern
- R3. Fix `histroy` typo → `history` in engine.rs — corrects 9 occurrences of a misspelled variable
- R4. Fix `LlmRouter.supports_tools()` to return `true` — current `false` causes callers to skip tool-aware paths
- R5. Fix plugin streaming semantic mismatch — `process_stream_token` should call a dedicated `on_stream_token` method, not recycle `on_llm_response`
- R6. Wire `SkillInjector` into `Agent.react()` system prompt — currently dead code; injection provides richer LLM context than tool-only skill access

## Scope Boundaries

- **In scope**: The 6 items listed above
- **Out of scope**:
  - Making `react::Tool` async (would require rewriting the entire adapter layer — too invasive)
  - Adding vendor fallback/health-check to `LlmRouter` (new feature, not a refactor)
  - Replacing `Arc<Mutex<Vec>>` message log with a lock-free structure (rearchitecting, not a refactor)
  - Replacing JSON-in-rkyv bus serialization with native rkyv types (breaking wire format change)
  - Adding structured error types to `ToolError` (API expansion, not a refactor)
  - Adding data mutation capability to hooks (new feature)

## Context & Research

### Relevant Code and Patterns

- `crates/react/src/engine.rs` — `ReActEngine::react()` (line 822) and `react_with_request()` (line 1086) are near-identical ~260-line methods
- `crates/agent/src/agent/agentic.rs` — `Agent::react()` and `Agent::run_simple()` both contain ~100 lines of duplicated tool/skill adapter construction
- `crates/react/src/llm/vendor/router.rs` — `supports_tools()` returns `false` on line 82
- `crates/agent/src/agent/plugin.rs` — `process_stream_token()` (line 328) converts tokens to `LlmResponseWrapper` and calls `on_llm_response`
- `crates/agent/src/skills/injector.rs` — `SkillInjector` with `inject_available()`, `inject_by_category()` etc. is never called from `Agent::react()`

### Key Technical Decisions

- **Dedup strategy for engine.rs**: Extract the shared ReAct step logic into a private method that both `react()` and `react_with_request()` call, parameterized by whether the caller provides an `LlmRequest` or builds one from scratch
- **Dedup strategy for agentic.rs**: Extract tool/skill adapter construction into a private `build_react_engine()` method on `Agent` that both `react()` and `run_simple()` call
- **Plugin streaming fix**: Add `on_stream_token` method to `AgentPlugin` trait with a default no-op implementation (backward compatible), then call it from `process_stream_token` instead of routing through `on_llm_response`
- **SkillInjector wiring**: Call `SkillInjector::inject_available()` to augment the system prompt in `Agent::react()`, listing available skills as context before the ReAct loop starts

## Implementation Units

- [ ] **Unit 1: Deduplicate engine.rs ReAct loop**
  **Goal:** Extract shared step logic from `react()` and `react_with_request()` into a single private method, eliminating ~260 lines of duplication.
  **Requirements:** R1
  **Dependencies:** None
  **Files:**
  - Modify: `crates/react/src/engine.rs`
  - Test: `crates/react/src/engine.rs` (existing tests in same file or crate test module)
  **Approach:**
  - Extract a private `react_loop(&mut self, context: &mut LlmContext, loaded_skills: &mut HashMap<String, String>) -> Result<String, ReactError>` method containing the core `for _ in 0..self.max_steps` loop, including the LLM call, response matching (ToolCall, Text/Partial, Done), skill caching, and telemetry emission
  - `react()` builds the `LlmContext` from scratch (system prompt + input messages + user message), then calls `react_loop()`, then strips the system message from `context.conversations` before storing
  - `react_with_request()` merges tools/skills into the provided `LlmRequest`, optionally prepends system message if conversations are empty, then calls `react_loop()` on `request.context`, then stores the full conversation
  - The return types differ: `react()` returns `(String, LlmContext)`, `react_with_request()` returns `String`. The loop returns `String`; `react()` wraps it with the context
  **Patterns to follow:** Existing method signatures and error types in engine.rs
  **Test scenarios:**
  - Happy path: `react()` still produces correct tool-call-then-text sequences
  - Happy path: `react_with_request()` with a pre-built request still works
  - Edge case: `react_with_request()` with empty conversations gets system message prepended
  - Edge case: `react_with_request()` with existing conversations does not get duplicate system message
  - Integration: Skill caching works correctly through both entry points
  **Verification:** `cargo test -p react` passes; `cargo clippy -p react` clean

- [ ] **Unit 2: Fix `histroy` typo → `history`**
  **Goal:** Rename all 9 occurrences of the misspelled `histroy` variable to `history` in engine.rs.
  **Requirements:** R3
  **Dependencies:** Unit 1 (apply after dedup to avoid rebase churn — if Unit 1 moves the lines, fix in the deduplicated version)
  **Files:**
  - Modify: `crates/react/src/engine.rs`
  **Approach:** Simple find-replace of `histroy` → `history` in the 3 blocks (lines ~1053-1055, ~1067-1069, ~1080-1082). After Unit 1, these will likely be in a single location within `react_loop()`.
  **Test scenarios:**
  - Test expectation: none — pure rename, no behavioral change
  **Verification:** `rg "histroy" crates/react/src/` returns no results; `cargo build -p react` succeeds

- [ ] **Unit 3: Deduplicate agentic.rs tool/skill adapter construction**
  **Goal:** Extract the duplicated tool adapter + skill tool + load_skill tool registration from `Agent::react()` and `Agent::run_simple()` into a shared private method.
  **Requirements:** R2
  **Dependencies:** None (independent of Unit 1)
  **Files:**
  - Modify: `crates/agent/src/agent/agentic.rs`
  - Test: existing tests in `crates/agent/`
  **Approach:**
  - Extract a private `build_react_engine(&self) -> Result<ReActEngine, AgentError>` method that: (1) constructs the LLM adapter (with or without plugins), (2) iterates the tool registry and wraps each tool in the appropriate adapter (PluginToolAdapter if plugins, else HookedToolAdapter), (3) registers skill tools and the load_skill tool, (4) configures builder with resilience, timeout, max_steps, model, system_prompt, (5) calls `builder.build()`
  - `react()` calls `build_react_engine()`, appends "Final Answer: Final Answer: your answer" to system prompt (existing behavior), then runs the engine
  - `run_simple()` calls `build_react_engine()`, then uses the engine for a single `call_llm()` + optional tool execution
  - The difference in adapter choice for skills (react uses HookedToolAdapter, run_simple uses ReactToolAdapter) should be parameterized or noted; if both can use the same adapter, unify
  **Patterns to follow:** Existing builder pattern in agentic.rs
  **Test scenarios:**
  - Happy path: `react()` still dispatches tool calls through hooks and plugins
  - Happy path: `run_simple()` still makes single LLM call with tool support
  - Integration: Plugin-wrapped tools receive plugin middleware in both paths
  - Integration: Hook-wrapped tools receive BeforeToolCall/AfterToolCall events in both paths
  **Verification:** `cargo test -p agent` passes; `cargo clippy -p agent` clean

- [ ] **Unit 4: Fix `LlmRouter.supports_tools()` to return true**
  **Goal:** Change `supports_tools()` from `false` to `true` so callers don't skip tool-aware code paths when using the router.
  **Requirements:** R4
  **Dependencies:** None
  **Files:**
  - Modify: `crates/react/src/llm/vendor/router.rs`
  - Test: `crates/react/src/llm/vendor/router.rs` or existing router tests
  **Approach:** Change `fn supports_tools(&self) -> bool { false }` to `fn supports_tools(&self) -> bool { true }`. The router delegates to vendors that all support tools. If a vendor is registered that doesn't support tools, that vendor's own `supports_tools()` returns false independently.
  **Test scenarios:**
  - Happy path: `LlmRouter.supports_tools()` returns `true`
  - Integration: Agent code paths that check `supports_tools()` now include tools in requests routed through the router
  **Verification:** `cargo test -p react` passes; grep for `supports_tools` consumers confirms no breakage

- [ ] **Unit 5: Fix plugin streaming semantic mismatch**
  **Goal:** Add `on_stream_token` method to `AgentPlugin` trait and call it from `process_stream_token` instead of routing through `on_llm_response`.
  **Requirements:** R5
  **Dependencies:** None
  **Files:**
  - Modify: `crates/agent/src/agent/plugin.rs`
  - Test: `crates/agent/src/agent/plugin.rs` (existing test module)
  **Approach:**
  - Add `async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> { let _ = token; None }` to `AgentPlugin` trait with default no-op impl (backward compatible — existing plugins don't need changes)
  - Rewrite `process_stream_token()` to call `on_stream_token()` instead of converting to `LlmResponseWrapper` and calling `on_llm_response()`
  - Keep `process_stream_token_blocking()` as-is, just delegates to the async version
  - Update `PluginLlmAdapter::stream_complete()` — it already calls `process_stream_token_blocking`, which will now use the correct method
  - Add a test: a plugin that modifies stream tokens via `on_stream_token` and verifies the modification propagates
  **Patterns to follow:** Existing `on_llm_request`/`on_llm_response`/`on_tool_call`/`on_tool_result` pattern in the trait
  **Test scenarios:**
  - Happy path: Plugin implementing `on_stream_token` can modify text tokens in a stream
  - Happy path: Plugin implementing `on_stream_token` can modify tool call tokens in a stream
  - Edge case: Plugin returning `None` from `on_stream_token` passes token through unchanged
  - Error path: Plugin panicking in `on_stream_token` is caught and skipped (same safety as other methods)
  - Backward compat: Existing plugins that don't implement `on_stream_token` still work (default no-op)
  **Verification:** `cargo test -p agent` passes; existing plugin tests still pass

- [ ] **Unit 6: Wire SkillInjector into Agent.react() system prompt**
  **Goal:** Use `SkillInjector::inject_available()` to augment the system prompt with a list of available skills, giving the LLM upfront context about what skills exist before it needs to call `load_skill`.
  **Requirements:** R6
  **Dependencies:** Unit 3 (modifies the same `react()` flow)
  **Files:**
  - Modify: `crates/agent/src/agent/agentic.rs`
  - Test: existing integration tests in `crates/agent/`
  **Approach:**
  - In `Agent::react()` (after Unit 3: in `build_react_engine` or just before engine execution), if `self.skills` is non-empty, use `SkillInjector` to append a skill availability block to the system prompt
  - Use `InjectionOptions::compact()` format to keep the augmentation minimal (skill names + descriptions only, no full instructions)
  - The injection goes after the existing system prompt text, before the "Final Answer" suffix
  - This supplements (does not replace) the existing skill-as-tool mechanism — skills remain callable as tools, and `load_skill` still works for on-demand full instructions
  **Patterns to follow:** `SkillInjector` API in `crates/agent/src/skills/injector.rs`
  **Test scenarios:**
  - Happy path: Agent with skills has skill names/descriptions injected into system prompt
  - Happy path: Agent without skills has no injection (no change to system prompt)
  - Integration: LLM can still call `load_skill` tool to get full instructions
  - Integration: Skill names appear in the system prompt context available to the ReAct engine
  **Verification:** `cargo test -p agent` passes; manual check that system prompt includes skill block when skills are loaded

## System-Wide Impact

- **Interaction graph:** Units 1-2 touch `react` crate internals. Units 3, 5-6 touch `agent` crate internals. Unit 4 touches the LLM router. No cross-crate API changes.
- **Error propagation:** No changes to error types or propagation paths.
- **State lifecycle risks:** Unit 1 refactors the ReAct loop — must preserve the `set_input_messages` / conversation update semantics exactly.
- **API surface parity:** `AgentPlugin` trait gets a new method with default impl — backward compatible. All other public APIs unchanged.
- **Integration coverage:** Existing test suites in `react` and `agent` crates cover the main execution paths. Unit 5 and 6 need new test cases.
- **Unchanged invariants:** Bus RPC, config, logging, pybos, jsbos crates are not touched. Python/JS bindings API unchanged.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Dedup extraction changes ReAct loop behavior subtly | Run full test suite before/after; compare conversation output for a sample react() call |
| `AgentPlugin::on_stream_token` default impl breaks a plugin that somehow relied on `on_llm_response` being called for stream tokens | Unlikely — the conversion was lossy (Partial → Text). Default `on_stream_token` returns None, preserving pass-through behavior |
| SkillInjector output makes system prompt too long for context windows | Use `InjectionOptions::compact()` (names + descriptions only, no instructions) |
| Unit 1 and Unit 3 touch large methods — merge conflicts if done in parallel | Unit 1 (react crate) and Unit 3 (agent crate) are in different crates, so no file-level conflicts. Execute in parallel |

## Execution Order

Units 1, 3, 4, 5 are independent and can run in parallel.
Unit 2 depends on Unit 1 (apply typo fix after dedup).
Unit 6 depends on Unit 3 (touches the same method).

Recommended parallel waves:
- **Wave 1**: Units 1, 3, 4, 5 (all independent)
- **Wave 2**: Units 2, 6 (depend on wave 1)
