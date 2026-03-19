---
phase: 01
slug: core-agent
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-03-19
---

# Phase 01 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust (`cargo test`) |
| **Config file** | `crates/agent/Cargo.toml` (Wave 0 creates) |
| **Quick run command** | `cargo test -p agent` |
| **Full suite command** | `cargo test -p agent --lib --doc --integration` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p agent`
- **After every plan wave:** Run `cargo test -p agent --lib --doc`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 01-01-01 | 01 | 1 | CORE-01 | unit | `cargo test -p agent client --lib` | ✅ W0 | ⬜ pending |
| 01-01-02 | 01 | 1 | CORE-02 | unit | `cargo test -p agent agent --lib` | ✅ W0 | ⬜ pending |
| 01-01-03 | 01 | 1 | CORE-03 | unit | `cargo test -p agent loop --lib` | ✅ W0 | ⬜ pending |
| 01-02-01 | 02 | 1 | TOOL-01 | unit | `cargo test -p agent tool --lib` | ✅ W0 | ⬜ pending |
| 01-02-02 | 02 | 1 | TOOL-02 | unit | `cargo test -p agent registry --lib` | ✅ W0 | ⬜ pending |
| 01-02-03 | 02 | 1 | TOOL-03 | unit | `cargo test -p agent translator --lib` | ✅ W0 | ⬜ pending |
| 01-02-04 | 02 | 1 | TOOL-04 | unit | `cargo test -p agent validation --lib` | ✅ W0 | ⬜ pending |
| 01-02-05 | 02 | 1 | TOOL-05 | unit | `cargo test -p agent bus_client --lib` | ✅ W0 | ⬜ pending |
| 01-03-01 | 03 | 1 | STRM-01 | unit | `cargo test -p agent sse --lib` | ✅ W0 | ⬜ pending |
| 01-03-02 | 03 | 1 | CORE-04 | unit | `cargo test -p agent config --lib` | ✅ W0 | ⬜ pending |
| 01-03-03 | 03 | 1 | STRM-01+CORE-01 | integration | `cargo test -p agent --test '*integration*'` | ✅ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/agent/src/llm/client.rs` — `cargo test -p agent client` (mock LLM responses)
- [ ] `crates/agent/src/agent/agent.rs` — `cargo test -p agent agent` (agent construction, message history)
- [ ] `crates/agent/src/tools/tool.rs` — `cargo test -p agent tool` (tool trait implementations)
- [ ] `crates/agent/src/tools/registry.rs` — `cargo test -p agent registry` (registration, lookup)
- [ ] `crates/agent/src/tools/translator.rs` — `cargo test -p agent translator` (schema translation)
- [ ] `crates/agent/src/tools/validator.rs` — `cargo test -p agent validation` (schema validation)
- [ ] `crates/agent/src/tools/bus_client.rs` — `cargo test -p agent bus_client` (bus tool execution)
- [ ] `crates/agent/src/streaming/sse.rs` — `cargo test -p agent sse` (SSE parsing from fake stream)
- [ ] `crates/agent/src/agent/config.rs` — `cargo test -p agent config` (config deserialization)
- [ ] `crates/agent/tests/integration/` — `cargo test -p agent --test '*integration*'` (full agent + tool + stream)

*If none: "Existing infrastructure covers all phase requirements."*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Real LLM API call end-to-end | CORE-01+CORE-03 | Requires live API key + network | 1. Set `OPENAI_API_KEY` env var 2. Create TOML config with valid key 3. Run `cargo run --example live_agent` 4. Verify tokens stream in real-time |
| Real Zenoh bus tool call | TOOL-05 | Requires running Zenoh broker + remote service | 1. Start Zenoh broker 2. Start a remote tool service 3. Configure agent with bus tool 4. Verify tool result comes back |

*If none: "All phase behaviors have automated verification."*

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
