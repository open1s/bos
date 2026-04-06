# BOS Pitfall Improvements Plan

## Overview

Based on research into AI coding assistant pitfalls (PITFALLS.md), this plan addresses 10 critical improvements to make BOS a production-ready AI coding framework.

## Gap Analysis Summary

| Pitfall | BOS Status | Priority |
|---------|------------|----------|
| Session persistence | Basic support, missing workspace binding | HIGH |
| Context window exhaustion | TokenCounter exists, no auto-compaction | HIGH |
| Long-running command black box | Tool execution exists, no streaming | HIGH |
| Missing consent controls | Unknown - needs verification | MEDIUM |
| MCP lifecycle | Full protocol, needs restart controls | MEDIUM |
| No headless mode | Unknown - needs verification | MEDIUM |
| Missing plan mode | Unknown - needs verification | HIGH |
| API error handling | Unknown - needs verification | MEDIUM |
| Terminal rendering | BOS is library, not CLI | LOW |
| Security boundaries | Unknown - needs verification | HIGH |

## Implementation Phases

### Phase 1: Core Foundation (Week 1)

**Goal:** Address highest-impact, lowest-risk improvements

#### Plan 1.1: Session Persistence Enhancement
- Add workspace directory binding to sessions
- Add session naming/aliasing support
- Add session branching capability

**Files to modify:**
- `crates/agent/src/session/mod.rs`
- `crates/agent/src/session/manager.rs`
- `crates/agent/src/session/storage.rs`

**Verification:** `cargo test -p agent session`

#### Plan 1.2: Token Budget & Auto-Compaction
- Add token budget display API
- Implement auto-compaction before limits
- Add manual `/compact` command support

**Files to modify:**
- `crates/react/src/engine.rs` (TokenCounter)
- `crates/agent/src/session/manager.rs`

**Verification:** `cargo test -p react`

---

### Phase 2: Execution & Tooling (Week 2)

**Goal:** Improve command execution visibility

#### Plan 2.1: Command Streaming
- Add real-time stdout/stderr streaming
- Add exit code propagation
- Add progress indicators for long tasks

**Files to modify:**
- `crates/agent/src/tools/execute.rs`
- `crates/agent/src/tools/shell.rs`

**Verification:** Manual test with long-running command

#### Plan 2.2: Security Boundaries
- Add workspace validation for all file operations
- Block path traversal patterns
- Add destructive operation warnings

**Files to modify:**
- `crates/agent/src/tools/fs.rs`
- `crates/agent/src/tools/mod.rs`

**Verification:** Test path traversal attempts blocked

---

### Phase 3: Agent Control (Week 3)

**Goal:** Improve user control and visibility

#### Plan 3.1: Plan Mode
- Add plan-first workflow option
- Show reasoning before action
- Allow steering mid-flight

**Files to modify:**
- `crates/react/src/engine.rs`
- `crates/agent/src/agent.rs`

**Verification:** Test plan vs execute modes

#### Plan 3.2: Approval Controls
- Add granular approval profiles
- Distinguish read vs write vs destructive
- Add policy configuration

**Files to modify:**
- `crates/agent/src/agent.rs`
- `crates/agent/src/config/permissions.rs` (new)

**Verification:** Test approval prompts

---

### Phase 4: Reliability (Week 4)

**Goal:** Improve error handling and robustness

#### Plan 4.1: LLM Error Handling
- Add exponential backoff retry
- Add model fallback support
- Improve error messages

**Files to modify:**
- `crates/react/src/providers/`
- `crates/react/src/engine.rs`

**Verification:** Test API failures retry correctly

#### Plan 4.2: MCP Lifecycle Controls
- Add server restart API
- Add health status monitoring
- Add project-scoped MCP config

**Files to modify:**
- `crates/agent/src/mcp/client.rs`
- `crates/agent/src/mcp/server.rs`

**Verification:** Test MCP restart mid-session

---

### Phase 5: Integration (Week 5)

**Goal:** Improve developer experience

#### Plan 5.1: Headless Mode
- Add non-interactive flag
- Add JSON output mode
- Add SDK-friendly API

**Files to modify:**
- `crates/pybos/src/lib.rs`
- `crates/jsbos/src/lib.rs`

**Verification:** Run in CI without TTY

---

### Plan 5.2: Run Tests & Verify
- Run full test suite
- Fix any regressions
- Update documentation

**Verification:** All tests pass

## Dependencies

```
Plan 1.1 (Session) ─┬─► Plan 2.1 (Streaming)
                    │
Plan 1.2 (Tokens) ──┼─► Plan 3.1 (Plan Mode)
                    │
Plan 2.2 (Security) ┴─► Plan 3.2 (Approvals)

Plan 3.1 ──────────► Plan 4.1 (LLM Errors)
Plan 3.2 ──────────► Plan 4.2 (MCP Lifecycle)

Plan 4.1 ──────────► Plan 5.1 (Headless)
Plan 4.2 ──────────┘

Plan 5.1 ──────────► Plan 5.2 (Tests)
```

## Parallel Opportunities

- Plan 1.1 and 1.2 can run in parallel (different files)
- Plan 2.1 and 2.2 can run in parallel (different modules)
- Plan 3.1 and 3.2 can run in parallel (different features)
- Plan 4.1 and 4.2 can run in parallel (different layers)

## Success Criteria

- [ ] Session persistence includes workspace binding
- [ ] Token budget visible and auto-compaction works
- [ ] Long-running commands show real-time output
- [ ] File operations validate workspace boundaries
- [ ] Plan mode shows reasoning before action
- [ ] Approval controls distinguish operation types
- [ ] LLM errors retry with exponential backoff
- [ ] MCP servers can be restarted independently
- [ ] Headless mode works in CI environments
- [ ] All existing tests continue to pass

## Notes

- BOS is a library/framework, not a CLI itself
- The aicoder CLI (in examples/) is built on top of BOS
- Improvements to BOS will benefit aicoder and other consumers
- Some features (terminal rendering) are out of scope for BOS itself