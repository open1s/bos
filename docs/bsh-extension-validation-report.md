# BSH Extension Framework — Validation and Security Review

**Reviewed:** 2026-11-01  
**Repository:** `open1s/bos`, default branch `main`  
**Scope:** `docs/BOS-BSH-Textbook.md`, Extension Standard, conformance matrix, peer-review report, extension manifest/runtime/policy/store code, example manifests and GitHub Actions workflow.

## Executive summary

The textbook is broadly aligned with a substantial implementation, but it must not imply that every documented security boundary is active on every platform. The most important newly reproduced-by-static-inspection gap is that the Linux process sandbox is not enabled by the production default. This is tracked as [Issue #31](https://github.com/open1s/bos/issues/31).

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

### Execution commands to run in a real checkout

Run from the repository root after building BSH:

```bash
cargo build -p bsh
export BSH=./target/debug/bsh

for example in hello allinone process-echo wasm-echo extism-echo extism-hook mcp-echo hook-demo language-python computer-use; do
  echo "===== $example: validate ====="
  "$BSH" ext validate "examples/extensions/$example" || exit 1
  echo "===== $example: test ====="
  "$BSH" ext test "examples/extensions/$example" || exit 1
done
```

Then run `ext mock` for every declared tool with representative inputs. Runtime-specific end-to-end tests must separately exercise process, MCP, WASM and Extism; a successful `validate` or `mock` is not proof of sandbox enforcement.

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

1. `docs/bsh-extension-standard.md` §15 currently says Linux runs the bubblewrap plan.
2. `docs/bsh-extension-conformance.md` §12/§20 explicitly lists Linux filesystem sandboxing as a remaining gap.
3. The process runtime source initializes `sandbox` with `cfg!(target_os = "macos")`, while the Linux spawn branch requires `self.sandbox && cfg!(target_os = "linux")`. The Linux branch therefore is not selected by the production default.

The textbook has been amended in this review branch to state the effective behaviour and link Issue #31.

## 3. Security review

### Finding SEC-01 — Linux process sandbox disabled by default

**Severity:** High  
**Status:** Open — [Issue #31](https://github.com/open1s/bos/issues/31)

Evidence:

- `ProcessRuntime::new()` sets `sandbox: cfg!(target_os = "macos")`.
- The Linux process-spawn path invokes `bwrap` only when `self.sandbox && cfg!(target_os = "linux")`.
- The extension runtime's `with_sandbox(true)` call found in this review is in a macOS-only test, not the production construction path.

Impact: Linux process extensions are spawned without the documented bubblewrap namespace and mount restrictions. They run with the BSH user's filesystem permissions, subject to other process limits and environment filtering.

Required remediation:

- Enable sandboxing by default on supported Linux systems.
- Fail closed for untrusted extensions if bubblewrap or required kernel features are unavailable; do not silently launch unsandboxed.
- Add a Linux integration test proving writes outside the allowed package/workspace/data/temp mounts fail.
- Make any development opt-out explicit and conspicuous.

### Finding SEC-02 — Process child networking is not mediated by network.http

**Severity:** Design/security boundary clarification  
**Status:** Documented in this review; no separate issue opened.

The macOS profile is a filesystem-write profile and starts with network access allowed. The Linux bubblewrap plan includes `--share-net`, which shares the host network namespace. A process child with `process.execute` can therefore open sockets directly; the host's `network.http` domain/method grants do not mediate arbitrary child sockets.

This may be an intentional consequence of granting arbitrary process execution, but the capability model must say so clearly. Do not describe `network.http` as controlling network access by process children. Stronger enforcement would require a network namespace/firewall/proxy design, not just a filesystem sandbox.

### Finding SEC-03 — Windows MCP cancellation does not compile

**Severity:** High for Windows CI / platform coverage  
**Status:** Open — [Issue #33](https://github.com/open1s/bos/issues/33)

The Windows extension test job in Actions run #149 failed to compile `crates/bsh/src/extension/runtime/mcp.rs:683`: `McpCore::kill_child()` unconditionally calls `libc::kill`, while `libc` is declared only under `cfg(unix)` in `crates/bsh/Cargo.toml`. This is a concrete CI failure (E0433), not a speculative static concern. Use a platform abstraction or cfg-specific child termination and retain a Windows regression test.

### Additional security checks

- **Capability dispatch:** source shows a shared authorization seam used by agent tools, host-initiated language/integration calls, workflow steps and panel actions. Keep regression tests on each dispatch surface.
- **Path scopes:** source normalizes lexical paths before scope comparison. Continue to treat symlink and TOCTOU handling as separate filesystem-boundary concerns.
- **Installed integrity:** current scan code compares the computed package hash to the registry record and re-verifies signatures; test quarantine/refusal and publisher-key removal.
- **WASM host calls:** subscriptions are checked against declared topics and `events.subscribe`; host filesystem operations authorize before writes. Preserve tests that assert denied operations have no side effects.
- **Resource limits:** keep per-runtime tests for output bytes, memory, fuel and wall-clock deadlines. Pure-compute WASM cancellation remains bounded by epoch/fuel rather than immediate instruction interruption, as the peer-review report states.
- **Cross-platform:** Linux sandbox execution and Windows process-limit behaviour require actual platform CI, not just platform-independent argument-plan unit tests. The current Windows extension job is failing at compile time on the Unix-only `libc` call tracked by Issue #33.

## 4. Acceptance criteria

- [ ] All ten runnable examples pass `ext validate`.
- [ ] All ten pass `ext test`.
- [ ] Every tool in every example has at least one representative `ext mock` test where the runtime supports mock.
- [ ] Process, MCP, WASM and Extism have end-to-end tests against the actual example package.
- [ ] Linux process extensions are sandboxed by default or refused; no silent unsandboxed fallback.
- [ ] Linux integration test verifies filesystem boundaries and process teardown.
- [ ] Capability denial tests cover agent, panel, language, integration and workflow dispatch.
- [ ] Network capability documentation explicitly distinguishes host-mediated HTTP from child-process sockets.
- [ ] Textbook, normative standard and conformance matrix agree on each platform's effective guarantees.

## 5. Review limitations

- No local checkout or shell execution was available in this review session.
- GitHub Actions run #149 was inspected: the Windows extension-test job failed with E0433 at `mcp.rs:683`; Linux/MSRV and other jobs were still running at the time of inspection. This run is associated with PR #30's commit, not a successful validation of the documentation changes in this PR.
- This is not a formal penetration test, dynamic fuzzing campaign, or independent cryptographic audit.
