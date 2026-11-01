# BSH Extension Framework — Validation and Security Review

**Reviewed:** 2026-11-01  
**Repository:** `open1s/bos`, default branch `main`  
**Scope:** `docs/BOS-BSH-Textbook.md`, Extension Standard, conformance matrix, peer-review report, extension manifest/runtime/policy/store code, example manifests and GitHub Actions workflow.

## Executive summary

The review's Linux sandbox-default and MCP call-path fixes were merged in [PR #35](https://github.com/open1s/bos/pull/35). MCP Unix resource limits and startup cancellation fixes were subsequently merged in [PR #40](https://github.com/open1s/bos/pull/40) and [PR #42](https://github.com/open1s/bos/pull/42). A follow-up compile failure exposed the distinction between `std::process::Command` and `tokio::process::Command`; the shared RLIMIT hook and separate adapters are now on `main` in commit `d5a8314`. Two Windows security gaps remain explicit: extension Process/MCP children have no AppContainer isolation ([Issue #43](https://github.com/open1s/bos/issues/43)) and no Job Object resource limits ([Issue #39](https://github.com/open1s/bos/issues/39)). The latest main Rust workspace workflow is queued, so Linux integration and all-example acceptance are not yet verified.

This review is source-level and repository-level. It did **not** execute the BSH binary or build/run the example packages: the connected GitHub interface exposes repository files and Actions state, not a local checkout/terminal. No example is marked runtime-verified based only on reading its files.

## 1. Example verification

### Static inventory inspected

The textbook's Appendix F lists ten runnable examples. The repository's `examples/extensions/` directory also contains `builtin-template`, which is a template rather than an eleventh runnable package.

| Example | Static inspection | Runtime execution in this review |
|---|---|---|
| `hello` | Manifest and echo descriptor read | Not run |
| `allinone` | Manifest and documented quick-start commands read | Not run |
| `process-echo` | Process manifest read | Not run |
| `wasm-echo` | Component manifest and entry path read | Not run |
| `extism-echo` | Listed in textbook inventory | Not run |
| `extism-hook` | Manifest read; hook contribution and capability declarations inspected | Not run |
| `mcp-echo` | Referenced by textbook and runtime tests | Not run |
| `hook-demo` | Manifest read; contribution and event declarations inspected | Not run |
| `language-python` | Listed in textbook inventory | Not run |
| `computer-use` | Manifest and README commands referenced by code search | Not run |

### Automated verification added in PR #35

The PR adds `scripts/ci/verify-bsh-examples.sh` and wires it into the Linux Rust workflow. It:

- runs `ext validate` and `ext test` for all ten runnable examples;
- runs `ext mock` for all 25 declared tools across those examples;
- asserts the All-in-One `fail` tool returns its expected non-zero result;
- asserts all nine macOS-only Computer Use tools return the documented Linux platform refusal before any input/screen operation.

This automation is committed but its latest-commit CI result is still pending. Runtime mocks verify the host/runtime call path; they do not replace the dedicated Linux filesystem-boundary tests or a macOS Computer Use smoke test.

### CI observation

The Rust workflow includes Linux workspace tests, Windows extension tests, and an MSRV check. At review time, the workflow run associated with the textbook PR was still in progress. The workflow does not visibly contain a matrix that invokes `bsh ext validate` and `bsh ext test` over all ten textbook examples. This is a coverage improvement, not a claim that the examples are currently broken.

## 2. API conformance

The following source-level comparisons were made against the normative `docs/bsh-extension-standard.md` and the implementation:

- Extension and contribution identity rules, including segment limits and full dotted contribution paths.
- Manifest runtime kinds, compatibility versions, capability declarations and resource limits.
- Process protocol handshake, request correlation, deadlines and output ceilings.
- WASM/Extism host calls, event subscriptions, hooks and cancellation semantics.
- Capability evaluation, grant scopes, audit records and dispatch entry points.
- Installed-package hashing, signature verification, store and market installation.
- Example manifests for declarative, process, component-WASM, Extism and MCP paths.

The existing `docs/bsh-extension-peer-review.md` records the earlier C1/H1–H9 and M1–M26 findings as fixed in its reviewed working tree. Current source inspection confirms several of those fixes are present, including centralized `authorize_each` usage, installed-package hash comparison, WASM declared-topic authorization, storage write-count limits, and MCP spawn policy wiring. This report does not re-open those historical findings without contrary current-source evidence.

### Contract drift requiring attention

The original `main` branch had stale security statements and an inactive Linux ProcessRuntime sandbox default. PR #35 updates the textbook, normative standard and conformance matrix to match the remediation branch. A separate MCP call-path review also found and addressed the direct-spawn bypass tracked by [Issue #36](https://github.com/open1s/bos/issues/36).

## 3. Security review

### Finding SEC-01 — Linux ProcessRuntime sandbox disabled by default

**Severity:** High  
**Status:** Code merged in [PR #35](https://github.com/open1s/bos/pull/35); Linux runtime verification pending — [Issue #31](https://github.com/open1s/bos/issues/31)

Evidence on the reviewed `main` revision:

- `ProcessRuntime::new()` set `sandbox: cfg!(target_os = "macos")`.
- The Linux spawn branch invoked `bwrap` only when `self.sandbox && cfg!(target_os = "linux")`.

The PR enables the sandbox by default on Linux, fails closed when bubblewrap cannot run, and adds a Linux integration test that attempts a write to host HOME outside the package bind.

### Finding SEC-04 — Linux MCP runtime bypassed the process sandbox

**Severity:** High  
**Status:** Code merged in [PR #35](https://github.com/open1s/bos/pull/35); Linux runtime verification pending — [Issue #36](https://github.com/open1s/bos/issues/36)

Evidence on the reviewed `main` revision:

- `McpCore::start()` used `Command::new(&program)` directly and did not invoke bubblewrap.
- MCP still launches package-controlled code under `process.execute`, so the capability grant alone did not provide filesystem isolation.

The PR reuses the shared Linux bubblewrap launch plan for MCP and adds an integration test that verifies an MCP child cannot write outside its package root. Missing bubblewrap or namespace failure aborts the spawn; no unsandboxed fallback is provided.

### Finding SEC-02 — Process child networking is not mediated by network.http

**Severity:** Design/security boundary clarification  
**Status:** Documented in this review; no separate issue opened.

The macOS profile is a filesystem-write profile and starts with network access allowed. The Linux bubblewrap plan includes `--share-net`, which shares the host network namespace. A process child with `process.execute` can therefore open sockets directly; the host's `network.http` domain/method grants do not mediate arbitrary child sockets.

This may be an intentional consequence of granting arbitrary process execution, but the capability model must say so clearly. Do not describe `network.http` as controlling network access by process children. Stronger enforcement would require a network namespace/firewall/proxy design, not just a filesystem sandbox.

### Finding SEC-03 — Windows MCP cancellation did not compile

**Severity:** High for Windows CI / platform coverage  
**Status:** Code merged in [PR #35](https://github.com/open1s/bos/pull/35); latest Windows CI verification pending — [Issue #33](https://github.com/open1s/bos/issues/33)

Actions run #149 failed with E0433 because `McpCore::kill_child()` unconditionally called `libc::kill` while `libc` was declared only under `cfg(unix)`. The PR replaces pid signalling with a separately lockable `std::process::Child` handle and adds a cross-platform termination test.

### Additional security checks

- **Capability dispatch:** source shows a shared authorization seam used by agent tools, host-initiated language/integration calls, workflow steps and panel actions. Keep regression tests on each dispatch surface.
- **Path scopes:** source normalizes lexical paths before scope comparison. Continue to treat symlink and TOCTOU handling as separate filesystem-boundary concerns.
- **Installed integrity:** current scan code compares the computed package hash to the registry record and re-verifies signatures; test quarantine/refusal and publisher-key removal.
- **WASM host calls:** subscriptions are checked against declared topics and `events.subscribe`; host filesystem operations authorize before writes. Preserve tests that assert denied operations have no side effects.
- **Resource limits:** ProcessRuntime and McpRuntime now have separate adapters over a shared Unix RLIMIT hook for RLIMIT_AS, RLIMIT_CPU and RLIMIT_NOFILE. The Linux MCP integration test checks the effective limits and filesystem boundary, but the current main-branch run has not yet completed. Windows Job Object memory/CPU/process-tree limits remain unavailable ([Issue #39](https://github.com/open1s/bos/issues/39)). Pure-compute WASM cancellation remains bounded by epoch/fuel rather than immediate instruction interruption.
- **Cross-platform:** Windows extension tests passed on the PR #35 merge commit; the latest main-branch Windows and Linux jobs are still running/queued. Windows Process/MCP children currently execute without AppContainer isolation ([Issue #43](https://github.com/open1s/bos/issues/43)); Job Object limits are also absent ([Issue #39](https://github.com/open1s/bos/issues/39)). Linux sandbox/resource integration and the ten-example automation still require the main-branch Rust workspace job to complete successfully.

## 4. Acceptance criteria

- [ ] All ten runnable examples pass `ext validate`.
- [ ] All ten pass `ext test`.
- [ ] All 25 declared tools are covered by `ext mock` or an explicitly expected platform/negative result.
- [ ] Process, MCP, WASM and Extism have end-to-end tests against the actual example package.
- [ ] Linux ProcessRuntime and McpRuntime children are sandboxed by default or refused; no silent unsandboxed fallback.
- [ ] Linux integration tests verify filesystem boundaries for both runtimes and process teardown.
- [ ] Capability denial tests cover agent, panel, language, integration and workflow dispatch.
- [ ] Network capability documentation explicitly distinguishes host-mediated HTTP from child-process sockets.
- [ ] Textbook, normative standard and conformance matrix agree on each platform's effective guarantees.

## 5. Review limitations

- No local checkout or shell execution was available in this review session.
- PR #35 was merged at `77a55ec`; PR #40 and PR #42 are also merged. The post-merge workflow for `6d7053c` failed: Linux/MSRV and Windows builds found the `std::process::Command` versus `tokio::process::Command` mismatch in the shared RLIMIT helper, while the core test job also reported rustfmt differences. Commit `d5a8314` added separate standard-library and Tokio command adapters and applied rustfmt. The latest `main` run for `4f56cb4` has Binding Parity green, NBOS in progress, and Rust Workspace, JSBOS and the main summary queued. Keep Issues #31, #36 and #38 open until Linux integration, all-example validation and MCP resource tests pass. Issue #41 is closed after the cancellation fix. This is not a formal penetration test, dynamic fuzzing campaign, or independent cryptographic audit.
- This is not a formal penetration test, dynamic fuzzing campaign, or independent cryptographic audit.
