---
phase: 02
slug: agent-protocols
status: draft
nyquist_compliant: true
wave_0_complete: false
created: 2026-03-20
---

# Phase 02 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust cargo test (built-in) |
| **Config file** | none — uses Cargo.toml target configuration |
| **Quick run command** | `cargo test -p agent --lib -- --test-threads=1` |
| **Full suite command** | `cargo test --workspace -- --test-threads=1` |
| **Estimated runtime** | ~45 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p agent --lib`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 60 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 02-01-01 | 01 | 1 | A2A-01 | unit | `cargo test -p agent --lib a2a` | ✅ | ⬜ pending |
| 02-01-02 | 01 | 1 | A2A-02 | unit | `cargo test -p agent --lib a2a` | ✅ | ⬜ pending |
| 02-01-03 | 01 | 1 | A2A-03 | unit | `cargo test -p agent --lib a2a` | ✅ | ⬜ pending |
| 02-01-04 | 01 | 1 | A2A-04 | unit | `cargo test -p agent --lib a2a` | ✅ | ⬜ pending |
| 02-01-05 | 01 | 1 | A2A-01..04 | integration | `cargo test -p agent --lib a2a_integration` | ✅ | ⬜ pending |
| 02-02-01 | 02 | 2 | MCP-01 | unit | `cargo test -p agent --lib mcp` | ✅ | ⬜ pending |
| 02-02-02 | 02 | 2 | MCP-02 | unit | `cargo test -p agent --lib mcp` | ✅ | ⬜ pending |
| 02-02-03 | 02 | 2 | MCP-03 | integration | `cargo test -p agent --lib mcp_integration` | ✅ | ⬜ pending |
| 02-03-01 | 03 | 3 | SKIL-01 | unit | `cargo test -p agent --lib skills` | ✅ | ⬜ pending |
| 02-03-02 | 03 | 3 | SKIL-02 | unit | `cargo test -p agent --lib skills` | ✅ | ⬜ pending |
| 02-03-03 | 03 | 3 | SKIL-03 | unit | `cargo test -p agent --lib skills` | ✅ | ⬜ pending |
| 02-03-04 | 03 | 3 | SKIL-01..04 | integration | `cargo test -p agent --lib skills_integration` | ✅ | ⬜ pending |
| 02-04-01 | 04 | 4 | STRM-02 | unit | `cargo test -p agent --lib streaming` | ✅ | ⬜ pending |
| 02-04-02 | 04 | 4 | STRM-03 | unit | `cargo test -p agent --lib streaming` | ✅ | ⬜ pending |
| 02-04-03 | 04 | 4 | STRM-02..03 | integration | `cargo test -p agent --lib streaming_integration` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/agent/src/a2a/mod.rs` — A2A message types and client - ✅ exists
- [ ] `crates/agent/src/a2a/tests.rs` — A2A unit tests - ⬜ pending
- [ ] `crates/agent/src/mcp/mod.rs` — MCP STDIO client - ✅ exists
- [ ] `crates/agent/src/mcp/tests.rs` — MCP unit tests - ⬜ pending
- [ ] `crates/agent/src/skills/mod.rs` — Skills loader and composer - ✅ exists
- [ ] `crates/agent/src/skills/tests.rs` — Skills unit tests - ⬜ pending
- [ ] `crates/agent/src/streaming/mod.rs` — Token streaming over bus - ✅ exists
- [ ] `crates/agent/src/streaming/tests.rs` — Streaming unit tests - ⬜ pending

*Existing infrastructure: 4 modules with basic tests (30 passing). Need expansion for all requirements.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Zenoh bus message routing | A2A-03, A2A-04 | Requires live bus instance | 1. Start Zenoh broker 2. Run agent A discovery 3. Verify agent B receives AgentCard 4. Send task from B to A via topic 5. Verify A processes task |
| MCP STDIO protocol | MCP-01 | Requires external MCP server | 1. Run mock MCP server with STDIO transport 2. Connect via McpTransport 3. Verify tool registration 4. Execute tool via bus proxy 5. Verify response propagation |
| Skills loading from filesystem | SKIL-02 | Requires skill directory structure | 1. Create test skill with metadata.md 2. Load via SkillLoader 3. Verify metadata parsing 4. Verify tool registration with namespace prefix |
| Backpressure under high token rate | STRM-03 | Requires sustained load | 1. Mock LLM source generating 1000 tokens/sec 2. Verify backpressure controller slows publisher 3. Verify buffer doesn't overflow |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 60s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
