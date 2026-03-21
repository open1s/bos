---
phase: 4
slug: advanced-features
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-21
---

# Phase 4 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Cargo (rustc + tokio::test) |
| **Config file** | Cargo.toml (unit tests), examples/*/Cargo.toml (integration) |
| **Quick run command** | `cargo test -p agent --lib` |
| **Full suite command** | `cargo test --all --all-targets` |
| **Estimated runtime** | ~30 seconds (without ignored integration tests) |

---

## Sampling Rate

- **After every task commit:** Run `cargo check -p agent && cargo test -p agent --lib`
- **After every plan wave:** Run `cargo test --all --all-targets`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 04-01-01 | 01 | 1 | STRM-01 | unit | `cargo test -p agent sse` | ✅ W0 | ⬜ pending |
| 04-01-02 | 01 | 1 | STRM-02 | integration | `cargo test -p demo-streaming token_publish` | ✅ W0 | ⬜ pending |
| 04-01-03 | 01 | 1 | STRM-03 | integration | `cargo test -p demo-streaming backpressure` | ✅ W0 | ⬜ pending |
| 04-02-01 | 02 | 2 | SCHD-01 | integration | `cargo test -p demo-scheduler sequential` | ✅ W0 | ⬜ pending |
| 04-02-02 | 02 | 2 | SCHD-02 | integration | `cargo test -p demo-scheduler parallel` | ✅ W0 | ⬜ pending |
| 04-02-03 | 02 | 2 | SCHD-03 | integration | `cargo test -p demo-scheduler conditional` | ✅ W0 | ⬜ pending |
| 04-02-04 | 02 | 2 | SCHD-04 | integration | `cargo test -p demo-scheduler retry` | ✅ W0 | ⬜ pending |
| 04-02-05 | 02 | 2 | SCHD-04 (timeout) | integration | `cargo test -p demo-scheduler timeout` | ✅ W0 | ⬜ pending |
| 04-03-01 | 03 | 3 | SKIL-01 | unit | `cargo test -p agent skills` | ❌ exists | ⬜ pending |
| 04-03-02 | 03 | 3 | SKIL-02 | integration | `cargo test -p demo-skills load` | ✅ W0 | ⬜ pending |
| 04-03-03 | 03 | 3 | SKIL-03 | integration | `cargo test -p demo-skills inject` | ✅ W0 | ⬜ pending |
| 04-03-04 | 03 | 3 | MCP-01 | integration | `cargo test -p demo-mcp client` | ✅ W0 | ⬜ pending |
| 04-03-05 | 03 | 3 | MCP-02 | integration | `cargo test -p demo-mcp adapter` | ✅ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

**Legend:**
- `✅ W0` = Test file created in Wave 0
- `❌ exists` = Test file already exists (re-use existing)
- Wave 0: Task with `tdd="true"` creates test stub first

---

## Wave 0 Requirements

### Streaming Tests (Plan 01-01)
- [ ] `examples/demo-streaming/tests/streaming_test.rs` — SSE decode, token publish, rate limit, backpressure
- [ ] `examples/demo-streaming/src/main.rs` — Demo binary
- [ ] `examples/demo-streaming/src/subscriber.rs` — New subscriber component

### Scheduler Tests (Plan 02-01)
- [ ] `examples/demo-scheduler/tests/scheduler_test.rs` — Sequential, parallel, conditional, retry, timeout
- [ ] `examples/demo-scheduler/src/main.rs` — Demo binary with A2A client
- [ ] `crates/agent/src/scheduler/executor.rs` — Replace stub with implementation

### Skills & MCP Tests (Plan 03-01)
- [ ] `examples/demo-skills-mcp/tests/skills_mcp_test.rs` — Load, compose, inject, MCP client, MCP adapter
- [ ] `examples/demo-skills-mcp/src/main.rs` — Demo binary
- [ ] `examples/demo-skills-mcp/skills/*/SKILL.md` — Example skill files (4 files)
- [ ] `examples/demo-skills-mcp/skills/*/references/checklist.md` — Reference file example
- [ ] `crates/agent/src/mcp/client.rs` — Add resources/prompts methods (optional)

### Existing Infrastructure
- [ ] `crates/agent/src/streaming/mod.rs` — Unit tests exist (reuse)
- [ ] `crates/agent/src/skills/loader.rs` — Unit tests exist (reuse)
- [ ] `crates/agent/src/skills/injector.rs` — Unit tests exist (reuse)
- [ ] `crates/agent/src/mcp/tests.rs` — Protocol tests exist (reuse)

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Real-time token arrival | STRM-01 | Visual verification of streaming | Run `cargo run -p demo-streaming`, observe tokens arrive incrementally in subscriber output |
| Backpressure adaptation | STRM-03 | Requires Zenoh load simulation | Run demo with high token rate, verify rate decreases per log output |
| Workflow step order | SCHD-01 | Log inspection | Run sequential workflow, verify log order matches step definitions |
| Parallel execution | SCHD-02 | Timing verification | Run parallel workflow, verify duration ≈ max(single step) not sum(steps) |
| Condition branching | SCHD-03 | Output inspection | Run conditional workflow with different inputs, verify correct branch taken |
| MCP server connection | MCP-01 | External dependency | Run demo-mcp, verify mcp-everything server connects and lists tools |
| Skill injection in prompt | SKIL-02 | System prompt inspection | Run agent with skills, print system prompt, verify `<available_skills>` present |

*Note: Manual verifications are for demonstration purposes. Automated tests cover the logic.*

---

## Test Execution Guidelines

### Unit Tests
```bash
# Run streaming unit tests
cargo test -p agent streaming

# Run scheduler unit tests
cargo test -p agent scheduler

# Run skills unit tests
cargo test -p agent skills

# Run MCP unit tests
cargo test -p agent mcp
```

### Integration Tests
```bash
# Run streaming demo tests
cargo test -p demo-streaming

# Run scheduler demo tests
cargo test -p demo-scheduler

# Run skills/mcp demo tests
cargo test -p demo-skills-mcp
```

### Ignored Tests (Require Zenoh Router)
Integration tests in `streaming/integration_tests.rs` are marked `#[ignore]`. To run:
```bash
# Start Zenoh router first
zenohd

# Then run with --ignored
cargo test -p agent streaming --ignored
```

### Demo Binaries
```bash
# Run streaming demo
cd examples/demo-streaming
cargo run -- --topic-prefix demo/streaming --openai-key $OPENAI_API_KEY

# Run scheduler demo
cd examples/demo-scheduler
cargo run --workflow sequential

# Run skills demo
cd examples/demo-skills-mcp
cargo run --list-skills

# Run MCP demo
cd examples/demo-skills-mcp
cargo run --mcp mcp-everything
```

---

## Success Metrics

### Quantitative Targets
| Metric | Target | How to Measure |
|--------|--------|----------------|
| Streaming throughput | >100 tokens/sec | Log timestamps, count tokens in 1s window |
| Zenoh latency | <50ms | Measure publish → receive delta |
| Scheduler sequential order | 100% correct | Log order matches defined steps |
| Parallel completion time | ≈ max(single) | Duration ≈ slowest step, not sum |
| Retry attempts | Match max_retries | Count attempt field in StepResult |
| Skill load time | <100ms for 10 skills | Time `SkillLoader::discover()` |
| MCP connect time | <1s | Time `McpClient::initialize()` |
| MCP tool count | >0 for mcp-everything | `list_tools()` result length |

### Qualitative Checks
- Streaming demo shows live token arrival (not batched output at end)
- Scheduler demo logs show step order with timing info
- Skills demo shows `<available_skills>` XML in agent prompt
- MCP demo shows tool names and descriptions from server

---

## Known Test Flakes

| Test | Flake Type | Workaround |
|------|-----------|-----------|
| `test_sse_to_bus_tokens` | Requires Zenoh router | Mark `#[ignore]`, requires zenohd in CI |
| `test_backpressure_adaptive` | Timing-sensitive | Increase timeout to 500ms |
| `test_mcp_client_connect` | Server not installed | Skip with detection if "mcp-everything" not in PATH |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (W0 in map)
- [ ] No watch-mode flags in cargo test
- [ ] Feedback latency < 30s (cargo test -p agent --lib)
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending

---

*Validation strategy created: 2026-03-21*
*Reference: Phase 4 RESEARCH.md for context*
