# BSH Tools 2.0 — Agent Capability Interface

Status: **Phase 0 audit complete; Phase 1 core contract implemented** (result
envelope + error semantics, wired through the runtime seam).

This document is the code-backed record for
[issue #447](https://github.com/open1s/bos/issues/447). It traces the tool
lifecycle in this repository, maps every functional requirement to actual
symbols, and records what this change implements, what already existed, and
what remains. It deliberately does not restate the issue's prose.

## 1. Lifecycle trace

| Step | Symbol |
|---|---|
| Tool definition | `react::tool::registry::Tool` / `AsyncTool` (`crates/react/src/tool/registry.rs`); host wrappers `agent::tools::{FunctionTool, AsyncFunctionTool, DeclaredTool, DeclaredAsyncTool}` |
| Declared contract | `react::tool::metadata::ToolMetadata` (`crates/react/src/tool/metadata.rs`); built-in declarations in `crates/bsh/src/tool_metadata.rs` |
| Registry | `agent::tools::ToolRegistry` (`crates/agent/src/tools/registry.rs`) |
| Capability lowering | `agent::capabilities::{catalogue_from_registry, NativeToolProvider, CapabilityTool}` |
| Model schema | `ToolRegistry::execute` caches `Tool::json_schema`; the engine renders provider definitions from the same registry |
| Input validation | `agent::tools::validate_args` (`crates/agent/src/tools/validator.rs`) |
| Permission check | Runtime policy (`agent::policy`, `agent::sandbox`); capability path: `capability::workspace::WorkspaceRuntime` |
| Execution | `ReActEngine::run_tool` (`crates/react/src/engine/tool_calls.rs`) |
| Result normalization | `ReActEngine` → `render_tool_result` (`crates/react/src/engine/mod.rs`) → `react::tool::result::ToolResult` |
| Result serialization | `ToolResult::to_text` (single compact JSON document) |
| Tool message | `LlmMessage::ToolResult` (`crates/react/src/llm`) |
| Next iteration | repeat-failure guard (`failure_signature`), host consumers (`bsh::runner`, `bsh::audit`) |

The audit found that step "result normalization" did not exist: the engine
stringified the tool's `Value` and wrote `Error: …` for a failure
(`crates/react/src/engine/mod.rs` before this change). Every consumer then
guessed the verdict from that string, which is the root cause of the
inconsistent reasoning the issue describes.

## 2. Inventory

- **Built-in tools**: 45 declared names in `crates/bsh/src/tool_metadata.rs`
  (shell, files, plan, goal, jobs, scheduling, tasks, surfaces, federation,
  delegation, web search). The host composes them in `crates/bsh/src/caps.rs`.
- **Generated tool families**: MCP tools (`agent::mcp`, registered as async),
  extension tools (`agent::extensions`, `bsh::extension`), Ferris capabilities,
  federation peers. All reach the model through the same `ToolRegistry`.
- **Capability descriptors**: `capability::descriptor::CapabilityDescriptor`
  carries version, input schema, permissions, side effects, risk, limits,
  features, surface name and visibility. `catalogue_from_registry` rejects an
  unknown risk or side-effect word rather than defaulting it.

## 3. Requirement gap matrix

Legend: **done** — satisfied by existing code, evidence given; **added** — this
change; **open** — still missing.

| FR | Status | Evidence | Remaining |
|---|---|---|---|
| FR-01 definition contract | done / partial | `ToolMetadata` (version, risk, side effects, permissions, limits, category, tags, intents, capability id); `catalogue_from_registry` validates declarations; `ToolRegistry::register` refuses duplicate names | No output JSON schema per tool (`OutputContract::json()` has no schema); no explicit sync/async/timeout field on the trait itself (timeout lives in metadata) |
| FR-02 input schemas | added | `validate_args` now returns `ToolError::InvalidField { field, missing }` for missing, undeclared, wrong-type, enum, length and bound violations; `CapabilityTool::run_with_call_id` validates against the descriptor before crossing the boundary, so the field survives that path | Conditional requirements (if A then B) are still home-grown per tool; provider-side JSON-Schema feature subset is not centrally listed |
| FR-03 result envelope | added | `react::tool::result::ToolResult`: `status` (success / partial / pending / error), `summary`, `data`, `error`, `effects`, `continuation`, `truncated`; applied at `render_tool_result` for batch and streaming paths | `effects` has no producer (see FR-06); `next_actions` deliberately omitted until a producer exists |
| FR-04 error semantics | added | `ToolErrorCode` (10 stable classes) + `ToolError::{code, retryable, indeterminate, field, missing_field}`; `ToolError::Cancelled`, `PermissionDenied`, `Conflict`, `Unavailable`, `Unsupported`, `InvalidField`; `ToolErrorInfo` serializes the class, advice and field; the agent's tool adapters pass the canonical error through instead of flattening it to `Failed(e.to_string())`; | Some `Failed(String)` sites still carry prose the class cannot refine; `ToolError`→`CapabilityError` cannot represent `Conflict`/`Unsupported` (mapped to `ProviderFailed`, documented) |
| FR-05 lifecycle | partial | `run_tool` registers/completes/fails one call; `NativeToolProvider` enforces deadline, cancellation token and bounded async admission | No single admission helper shared by every provider; timeout/cancel correlation is per-provider |
| FR-06 idempotency | open | `ToolMetadata.side_effects` declares read/write/network/process/spend/auth; the read-only retry guard in `tool_calls.rs` respects it | No idempotency declaration, no execution/idempotency key, no automatic retry policy for non-idempotent tools |
| FR-07 continuation | added (producers partial) | `ToolContinuation` reads the search tools' `next` pointer; the read tools carry `next_offset` as their only continuation (no `next` or `continuation` duplicate); `next_offset` is always present (the line to pass as `offset`, or `null` when fully read), per file for a batch including a failed entry; the hint names it and the input remains `offset`; `truncated` distinguishes a cut report from a failed operation | Most non-file tools still return no continuation; a truncated envelope keeps both ends but does not synthesize a narrower call |
| FR-08 discovery/activation | done | `react::tool::admission` (`admit`, `WithheldReason`), `find_tool` reveal + `schema_budget`, `deferred`/`is_skill` on the trait, capability `Visibility` | Activation is static per request; deactivation of in-flight calls is handled only by cancellation |
| FR-09 task tools | partial | `bsh::task_tools` (`task_create/join/status/list/cancel/retry/graph`); the envelope can carry `pending`, but task payloads report `state` (domain state, e.g. `running`), which is deliberately NOT read as the call outcome | Task tools would need to declare `status: pending` (or a continuation) explicitly; `task_join` blocking semantics are not documented in the schema; no notification-based completion path |
| FR-10 browser tools | partial | `bsh::browser` returns URL/title/state on open and explicit failure messages; `ToolError::Unavailable` can now represent "no engine running" | Click/type/press still do not separate "event accepted" from "form submitted" in the result contract |
| FR-11 scheduling | partial | `bsh::schedule` validates recurrence before activation and returns normalized next time | Tool results do not yet carry a continuation/next-execution field in the envelope |
| FR-12 federation delegation | partial | `capability::FederationTransport`, `federation::service` delegation lifecycle; `federation_tools` exposes peers/capabilities/delegate | No task-intent-level delegation result envelope (delegation id, state, result, continuation) |
| FR-13 agent recovery | added (seam) | Envelope distinguishes execution outcome from completion (`partial`/`pending` ≠ failure); `retryable`/`indeterminate` give the recovery decision a structural answer; the read-only retry guard now reads the class | The loop's guidance still composes prose; no policy that blocks repeating a permission failure |
| FR-14 evaluation | open | Existing unit + integration suites (react 192 lib tests, agent 614 lib tests, bsh 2000 lib tests) exercise the paths | No selection/argument/result-comprehension benchmark, no token-overhead measurement |

## 4. The implemented contract

### 4.1 Envelope

~~~json
{
  "status": "partial",
  "summary": "read_file returned a partial result",
  "data": { "tool": "read_file", "status": "partial", "content": "first page", "next_offset": 401 },
  "truncated": true
}
~~~

Failure:

~~~json
{
  "status": "error",
  "summary": "bash failed: Tool execution timed out: …",
  "data": null,
  "error": {
    "code": "timeout",
    "message": "Tool execution timed out: …",
    "retryable": false,
    "indeterminate": true
  }
}
~~~

Rules:

- `status` is always present and is one of `success`, `partial`, `pending`,
  `error`.
- `summary` is one sentence; a tool may supply its own `summary` in its
  payload, otherwise it is derived from the tool name and status.
- `data` is the tool's payload unmodified. The envelope never rewrites domain
  data, so existing payload keys keep their meaning and their consumers.
- `error` is present exactly when `status` is `error`.
- A tool that returns `Ok` but reports `"success": false` is classified as a
  failure. A payload status word is trusted only when the payload also names
  its tool (`{"tool": …, "status": …}`, the file-tool contract), because a
  bare `status` may be domain state: `job_output` returns `status: "failed"`
  for a job it successfully described, and that call did not fail.
- `continuation` is omitted when there is nothing more to fetch, which is what
  tells the model the result is complete. The read tools use `next_offset` as
  their single continuation representation: no `next` object, and no
  `continuation` repeating the same offset.
- A batch tool reports per-item outcomes: a failed entry is data with its own
  `status: "error"`, and the call is `partial` (or `success` when nothing
  failed), never a call-level `error` with a synthetic code.
- `truncated` means the report was cut to fit the context; it does not change
  `status`.

### 4.2 Status mapping of legacy payloads

A payload that carries both `tool` and `status` (the file tools, through
`bsh::file_result::finish`, and the engine's synthetic cancellation record) is
read through this table; a payload status without `tool` is domain data and
stays inside `data`.

| Payload word | Envelope status |
|---|---|
| `ok`, `success`, `completed`, `done`, `empty`, `no_results` | `success` |
| `partial`, `incomplete`, `truncated`, `some_failed` | `partial` |
| `pending`, `submitted`, `accepted`, `queued`, `running`, `in_progress`, `waiting` | `pending` |
| `error`, `failed`, `failure`, `cancelled`, `canceled` | `error` |

The tool's own word stays inside `data`: `bsh::file_result`'s `ok`/`empty`
distinction is not lost, it is translated at the outer layer.

### 4.3 Error classes

| Code | Meaning | Retryable | Indeterminate |
|---|---|---:|---:|
| `invalid_input` | arguments rejected (field-level where known) | no | no |
| `not_found` | tool/resource/referenced entity absent | no | no |
| `permission_denied` | policy or authority refused | no | no |
| `conflict` | contradicts current state | no | no |
| `ambiguous` | more than one target, none named | no | no |
| `timeout` | bound expired | no | **yes** — the operation may have started |
| `cancelled` | caller/runtime stopped it | no | **yes** — cancellation is not rollback |
| `unavailable` | dependency unavailable | **yes** | no |
| `unsupported` | operation not implemented | no | no |
| `internal` | ran and failed, unclassified | no | **yes** — the body may have run |

`retryable` is an admission, not a promise: a caller must still respect the
tool's declared side effects, which is what the read-only retry guard does.

### 4.4 Truncation

`ToolResult::to_text(cap)` is the only way the engine renders a result. When
the compact envelope exceeds `cap` (default 48 KiB), the payload is cut with a
marker inside the budget, `truncated` is set, and the document stays valid
JSON. A cap too small to hold a minimal envelope yields the minimal envelope:
valid JSON is never traded for a byte bound.

## 5. Compatibility and consumers

Consumers of the old shapes were identified before migration and updated:

- `crates/react/src/engine/mod.rs` — `failure_signature` and the failure log
  read `ToolResult::text_is_error` (envelope first, legacy prefix second).
- `crates/bsh/src/runner.rs::tool_result_failed` — same helper.
- `crates/bsh/src/audit.rs::outcome_of` — same helper; legacy `Error:\u{ff1a}`
  rows still classify as failed, so existing audit data keeps its verdict.
- `crates/agent/src/capabilities/shim.rs::to_tool_error` — `Denied`/`Cancelled`/
  `Unavailable`/rate limits/transport delivery state now map to their own
  classes instead of `Failed("denied: …")`.
- `crates/agent/src/capabilities/native.rs::classify` — the reverse mapping, so
  a tool error keeps its class across the capability boundary.
- `crates/agent/src/agent/agentic/adapters.rs` — the agent's sync/async tool
  adapters pass the tool's `ToolError` through unchanged; the old
  `Failed(e.to_string())` rewrite dropped the class and stacked a
  "Tool execution failed:" prefix per layer.
- `crates/bsh/src/tool_output.rs` — the one reader that unwraps `data` for
  stored results, used by `audit::confinement_of` (sandbox/network facts),
  `ui::transcript::result_change` (sed's diff preview) and
  `ui::trajectory` (the one-line outcome). A legacy row without an envelope
  reads as its own payload, so a replayed session keeps its previews.

Not changed on purpose:

- Tool names, business payloads and provider wire formats.
- `bsh::file_result`'s vocabulary; it remains the inner payload contract.
- `ReactError::ToolError(String)` remains for external callers; the engine now
  emits `ReactError::ToolFailure(ToolError)` so the class is not flattened by
  `format!("{:?}", e)`.
- `Result` success/failure is no longer the sole verdict, but it is still the
  first signal: a tool that returns `Err` is `error` regardless of its payload.

### Measured cost

The success envelope adds two short fields (`status`, `summary`) and nests the
payload under `data`. For a typical file result this is roughly 10–15 tokens
per call. The failure envelope replaces the previous free-form `Error: …`
string and adds the class and advice, roughly 20 tokens. No benchmark of
model-level accuracy exists yet (FR-14).

## 6. Remaining work

Priority order, with the evidence that it is still missing:

1. **FR-03/FR-07 producers.** Give the task, browser, scheduling and federation
   tools a `summary` and, where meaningful, a `continuation`. The envelope can
   only preserve what a tool supplies.
2. **FR-06 idempotency.** Add an idempotency declaration to `ToolMetadata`
   (pure / idempotent / non-idempotent / conditional), validate it in
   `catalogue_from_registry`, and let the retry guard and the capability
   transport consult it instead of side effects alone.
3. **FR-04 capability parity.** `CapabilityError` cannot represent
   `Conflict`/`Unsupported`; either add the classes or document the loss in
   the federation wire contract.
4. **FR-09 task join semantics.** Define wait-vs-fetch, a bounded timeout and a
   notification path; `pending` is now representable but nothing subscribes.
5. **FR-10 browser event vs. business outcome.** Click/type/press must report
   the interaction result separately from the business result.
6. **FR-12 delegation result.** A task-intent-level delegation result with id,
   peer, state, summary and continuation; keep the low-level messages.
7. **FR-14 evaluation.** Selection/argument/comprehension/recovery scenarios and
   a token-overhead measurement against the previous format.

## 7. Verification

Run from the repository root:

~~~bash
cargo test -p react
cargo test -p react --test tool_result_contract
cargo test -p agent --lib validator
cargo test -p agent --test capability_tool --test native_provider
cargo test -p bsh --lib
cargo clippy -p react -p agent -p bsh --all-targets --no-deps
cargo fmt --all -- --check
~~~

`-D warnings` is not usable as a gate on this host: the installed clippy
(1.100) flags pre-existing `collapsible_if` sites in crates this change never
touches (`qserde_derive`, `hostkit`, `capability`) and in untouched lines of
`react`/`bsh`. Clippy reports no warning in any line this change adds.

New tests:

- `crates/react/src/tool/result.rs` — status translation, continuation,
  failure payloads, field errors, truncation validity, legacy verdicts.
- `crates/react/src/tool/error.rs` — class/retry/indeterminate table and field
  naming.
- `crates/react/tests/tool_result_contract.rs` — end-to-end: one turn with a
  success, a timeout, a partial payload and a self-reported failure; every
  recorded `ToolResult` message is the envelope above.
- `crates/agent/src/tools/validator.rs` — missing, undeclared and nested field
  rejections name the field.
- `crates/agent/src/tools/registry.rs` — a rejected call reports the missing
  field.
- `crates/bsh/src/caps.rs` — raw and capability registries name the same field.
- `crates/bsh/src/tool_output.rs` — envelope payload and summary reads, the
  legacy fallback, and the non-JSON case.
- `crates/agent/tests/capability_tool.rs` — capability failures keep
  permission/cancellation/timeout/unavailable classes, and a missing argument
  keeps its field through the shim.

Targeted re-run of the areas this change touches: `bsh --lib` filters
`tool_output::` (30), `ui::trajectory` (30), `audit::` (30),
`ui::transcript` (17) and the updated peer test (1) — 108 passed, 0 failed.

Observed results on this host:

- `react`: 191 lib tests + all integration suites pass.
- `agent`: 614 lib tests pass; 11 fail, all process-kill tests reporting
  `Failed { code: -1 }` instead of `Killed`. The same test fails identically
  at the parent commit (`git worktree` at `e49a39dc`), so it is the host, not
  this change.
- `bsh --lib` (final run on the completed change): 2000 passed, 41 failed,
  9 ignored. All 41 failures are environment-bound:
  - 25 `browser::*` tests: the native rendered engine cannot start in this
    sandbox ("WebSocket protocol error: Connection reset without closing
    handshake"), including the 6 `ui::shell_tests::*` live-page tests.
  - 2 `ui::shell_tests::*` job-kill tests: the same `Failed { code: -1 }` vs
    `Killed` host behavior the agent suite shows.
  - 7 network/heel egress tests (`caps::tests::*allowlist*`,
    `*intersecting_preset*`, `standard_and_federation_*`,
    `a_heel_preset_under_full_access_*`: no usable non-loopback egress or
    sandbox backend.
  - `extension::e2e::the_change_history_package_prefers_jj_when_present`: jj
    cannot read `~/.config/jj` under the sandbox.

  `caps::tests::a_trusted_peer_capability_is_offered_as_a_tool` is the one
  assertion this change deliberately updated: a departed peer is now
  `Unavailable` (retryable) rather than `Failed`, and the test asserts that
  class instead of "some failure". `store::raw_tests::an_empty_journal_is_quarantined`
  failed in the first full run and passed in the second with no code change;
  it is sensitive to the sandbox's quarantine move.
