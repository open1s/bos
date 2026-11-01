---
module: bsh
tags: [extensions, review, peer-review, security, lifecycle, runtimes]
problem_type: review
---

# BSH Extension Framework — Peer Review

**Reviewed at:** working copy `7ca9d8d9` (empty change on `main` `9a393002`), 2026-11-01.
**Scope:** `crates/bsh/src/extension/**` (25 files, 20,849 lines), `crates/bsh/src/ui/extension_panel.rs`,
the host seams in `caps.rs`, `shell.rs`, `state.rs`, and the contract documents
(`docs/bsh-extension-standard.md`, `docs/bsh-extension-conformance.md`,
`docs/bsh-extension-review.md`).
**Method:** full read of the host, policy, storage, UI and agent wiring; three
independent module audits (capability/signing, runtimes, manifest/CLI/lifecycle)
whose findings were re-checked against the source before being accepted here;
test suite run against a rebuild of the current tree.

**Verdict.** The framework is real, not a facade: the manifest parser is strict,
the package store is staged and hashed, the process protocol implements the
documented handshake and limits, scoped grants and an audit log exist, and the
happy path (validate → install → grant → activate → call → disable → shut down)
is covered. The weakness is **uniformity of enforcement**. Per-call capability
authorization is wired only on the agent-tool path; integrity is verified only at
install; cancellation works for process/MCP but not for WASM/Extism; and several
documented surfaces (images, panel icons, extension configuration, MCP events,
`extension.unloaded`) never reach the reader or never fire. The prior
`docs/bsh-extension-review.md` and `docs/bsh-extension-conformance.md` mark all of
this "done"; section 6 lists where that is not accurate.

## 0. Evidence and limits

- `cargo test -p bsh --lib extension::` on the rebuilt tree: **116 passed, 9 failed**.
  All 9 failures are process-spawning tests (`extension::runtime::process::*`,
  the hook-demo end-to-end cases), and all fail at the same point —
  `process closed stdout before answering`. They are **environmental**: this
  review runs inside a file sandbox where `/usr/bin/sandbox-exec` refuses to
  apply a nested profile (`sandbox_apply: Operation not permitted`), and the
  process runtime always wraps children in `sandbox-exec` on macOS. They should
  pass outside a sandbox; nothing here should be read as evidence that they fail
  on a normal machine.
- Not exercised: Windows and Linux process limits (documented as compiled but
  unexercised), signature verification against a real publisher, GUI rendering
  of extension panels.
- Coverage is thin exactly where the trust decisions live: `signing.rs` has 1
  test, `wasm.rs` 1, `mcp.rs` 1, `market.rs` 2, `sources.rs` 2.

## 1. Critical

### C1. The MCP runtime spawns its server with no `process.execute` authorization

**Evidence.** `crates/bsh/src/extension/runtime/mcp.rs:171-182` — `McpRuntime`
has no policy field at all. `mcp.rs:341` reaches `command.spawn()` with no
`authorize_call` anywhere in the file. The only gate is activation, which checks
the capabilities the package itself declared
(`host.rs:633-638`). Compare the process runtime, which authorizes on every
spawn with program and args (`runtime/process.rs:707-718`).

**Impact.** A package with `runtime.kind = "mcp"` and no capabilities declared is
activated and executes an arbitrary command (`entry`, `args`) with the reader's
privileges, bypassing both the explicit `process.execute` grant and the
per-spawn `program:...` scope. This defeats the one critical control the
standard reserves for exactly this operation (`docs/bsh-extension-standard.md`
§6, §14).

**Fix.** Thread `PolicyEngine` + `TrustLevel` into `McpRuntime` and authorize
`ProcessExecute` with `program` and `args` on every spawn; also reject an
`mcp` manifest that does not list `process.execute` in `required`.

## 2. High

### H1. Per-call capability authorization exists only on the agent-tool path

**Evidence.** `agent.rs:271-287` authorizes every descriptor capability before a
tool call. The other dispatch paths call `Runtime::call_tool` directly:

- panel actions: `ui/extension_panel.rs:236`
- language checks: `host.rs:1061-1071`
- integrations: `host.rs:1125`
- workflow steps: `runtime/workflow.rs:123, 220`

None of them evaluates the tool's `capabilities` or writes an audit line.

**Impact.** A panel button, `bsh ext language-check` or `bsh ext integrate` can run
a tool that declares (and is denied) `workspace.write`, `secrets.read` or
`process.execute`. The declaration is silently unenforced on those surfaces, and
the audit log has no record that the operation happened. The standard calls this
seam "the least-privilege seam" (§5) and requires an audit record for every
decision (§6.2, §14).

**Fix.** Put authorization (and audit) in one place — either inside
`Runtime::call_tool` for every runtime, or in a host-level dispatcher that all
callers use — instead of in the agent wrapper only.

### H2. Installed packages are never re-hashed or re-verified on load

**Evidence.** `package.rs:72-76` computes `package_hash` during load and never
compares it with anything. `package.rs:900-912` takes `trust` and
`verification` from the cached registry record. The only comparison in the
tree is `host.rs:1601-1609`, inside `doctor()`.

**Impact.** Editing a file under `installed/<id>/<version>/` leaves the package
loadable and still reported as `verified`, so a signed package's verified trust
keeps auto-allowing medium-risk capabilities (`capability.rs:823-825`).
`bsh ext untrust` rewrites `trust.toml` but never reaches an installed package.
This directly contradicts the standard's checklist item "hash verification on
load and a doctor check for post-install tampering" (§14) — only the doctor half
exists, and the doctor is a command the reader must remember to run.

**Fix.** In `scan_installed`/rebuild, compare the freshly computed hash with
`record.hash` (and re-run signature verification) and refuse or quarantine a
mismatch.

### H3. `path:` grants are matched lexically, so `..` escapes the granted prefix

**Evidence.** `capability.rs:536-540` — `Self::Path(prefix) => call.path…starts_with(prefix)`.
The call path is built as `workspace.join(path)` with no normalization
(`agent.rs:147-156`).

**Impact.** `Path::starts_with` is component-wise and keeps `ParentDir`, so
`/ws/sub/../../etc/passwd` starts with `/ws/sub`. A grant scoped
`path:/ws/sub` therefore covers a read outside it, and the community default for
`workspace.read` (`Path(workspace)`, `capability.rs:849-851`) covers
`{"path":"../../etc/passwd"}`. Contradicts §6.1 ("A grant scoped to one path
does not cover another path"). The same lexical construction is used by the WASM
and Extism `path` scopes.

**Fix.** Reject `.`/`..` components and canonicalize the extracted path before
storing it in `CallScope` or comparing prefixes.

### H4. Cancelling a WASM call cannot interrupt the guest and permanently disables the runtime

**Evidence.** `wasm.rs:808-816` — `cancel()` sets `cancelled` and tries to take the
`instance` mutex to set the epoch deadline; `call_tool` holds that same mutex
across the synchronous guest call (`wasm.rs:836-855`), so `cancel` blocks until
the call finishes. The flag is never cleared, and `call_tool` refuses every later
call while it is set (`wasm.rs:829-833`). Extism's `cancel` (`extism.rs:582-585`)
sets a flag that `call_tool` never reads (`extism.rs:591-607`).

**Impact.** Pressing Stop on a WASM tool neither stops the running guest nor
leaves a usable runtime: the first later call fails with "runtime was cancelled"
until the package is rebuilt. §15.6 promises "Cancellation traps a WASM guest".
Extism cancellation is a no-op.

**Fix.** Make cancellation lock-free (atomic checked by host calls; keep an epoch
handle that does not need the instance lock), clear the flag on
activate/prepare, and give Extism a real abort path.

### H5. A package whose only contribution is skills never applies them

**Evidence.** `host.rs:953-960` computes `has_agent_contribution` from tools,
commands, panels, prompts and contexts, then `continue`s when all are empty.
`contributes.skills` is not in the list, although the skill registration lives
behind that guard (`host.rs:972-973` → `agent.rs:480-489`).

**Impact.** `contributes = { skills = ["helper"] }` is legal per §3/§4.5, installs,
shows `applied`, lists the skill in its status row, and registers nothing.

**Fix.** Include skills in `has_agent_contribution` (and verify each declared
`skills/<id>/SKILL.md` exists — see M12).

### H6. WASM and Extism never enforce `max_output_bytes`

**Evidence.** `wasm.rs:856-868` parses `result.content` with no size check;
`extism.rs:603` returns the host call's value directly. Process
(`process.rs:631-643`), MCP (`mcp.rs:570-584`) and declarative
(`runtime/mod.rs:400-408`) all check.

**Impact.** A component declaring `max_output_bytes = 65536` can return up to its
memory ceiling (256 MiB by default) into the agent transcript — the exact
resource-exhaustion path the ceiling exists for (§4.6, §5).

**Fix.** Measure the serialized result and return `ExtensionError::Limit` above
`resources.output_bytes()`, as the other runtimes do.

### H7. A `program:exe:args` grant can never satisfy activation

**Evidence.** `capability.rs:726-735` skips the load-time `process.execute` check
only when `scope.program.is_none()`, on the premise that "the load-time scope has
neither". `host.rs:626-631` sets `scope.program = manifest entry`, so the skip
never fires for a process package; `GrantScope::ProgramArgs::covers` requires
`call.args` (`capability.rs:558-563`), which is `None` at load. The package is
denied with "grant is scoped to program:python3:--version, which does not cover
this call".

**Impact.** The documented scope form (§6.1, and the textbook's
`bsh ext grant … --scope program:python3:--version`) makes the package
permanently unusable — the *narrower* grant is worse than the broad one.

**Fix.** Carry `runtime.args` into the load-time scope, or let the
`ProcessExecute` skip fire when the grant is `ProgramArgs`.

### H8. Installing an upgrade silently re-enables a disabled extension

**Evidence.** `package.rs:1053-1057` — the install's state update sets
`state.enabled = true` unconditionally.

**Impact.** §7 says the enabled flag "survives a package upgrade". A reader who
disabled a package gets it re-enabled by the next version install, and with
default-enabled installs it may start code the reader had switched off.

**Fix.** Preserve the existing `enabled` value; default to `true` only for a new
record.

### H9. `bsh ext validate` accepts Extism and WASM packages with missing contribution files

**Evidence.** `cli.rs:344-359` — `RuntimeKind::Extism => Ok(())` and
`_ => Ok(())`, directly under a comment that says validate must refuse a package
"before it is copied". Extism loads descriptors and contributions at
`host.rs:858-860`; a WASM `entry` is only checked by `WasmRuntime::new`.
`validate` therefore exits 0 for a package that will fail on the next host build.

**Impact.** Contradicts §13 ("validate returns 1 on … a missing/unparseable
contribution file") and breaks the author loop the CLI exists for: `install`
accepts the package and the failure surfaces on activation instead.

**Fix.** Run the same load `build_runtime` does for every runtime kind (and check
the wasm entry exists).

## 3. Medium

| # | Finding | Evidence | Effect / fix |
|---|---|---|---|
| M1 | Panel action stale-revision check is dead code: the request's revision is stamped from the live session at dispatch time, so it can never mismatch | `ui/extension_panel.rs:205-217`; `ui.rs:751-768` | A control rendered from an older document always acts on the newest one. Capture the revision at render time and compare it (or delete the check and the §10 claim) |
| M2 | The global host is keyed on a config only some callers populate: `install()` passes the preset allowlist, `statuses()`/`themes()`/`panels()`/`with_host()` pass `None` | `host.rs:1755-1783` vs `1908`, `1939`, `1978`; `caps.rs:147` | With a preset that sets `extensions` (e.g. `read-only` = `Some([])`), every status/theme read reopens the host, tears down runtimes and re-runs discovery; the pane also reports `applied` under a different policy than the turn uses |
| M3 | MCP tool ids are sanitized and the *sanitized* id is sent back to the server | `mcp.rs:434-437`, `613-626`, `564-566` | `getUser` becomes `getuser` and the server rejects it; `"a b"`/`"a_b"` collapse silently. Keep the wire name and map ids separately |
| M4 | The Grant button writes `GrantScope::Any` for `secrets.read`, `network.http`, `computer.use` and `process.execute`-absent cases, in one click | `host.rs:1187-1197` | Grants every secret / every host, wider than the standard's `secret:NAME` and `domain:` forms. Ask per capability with the narrowest scope, or at least show the scopes before writing |
| M5 | Storage's 10,000-file cap is enforced only by `list()` | `storage.rs:135-147` vs `204-208` | A guest can write without bound; the namespace then permanently errors on `storage_list`. Count in `write` |
| M6 | Market index `hash` is parsed and never checked; id/version are not compared either; `entry.path` splits only on `/` | `market.rs:52-54`, `129-132`, `239-259` | A compromised index can advertise one id/hash and install another; on Windows `..\\..\\x` passes the traversal check. Validate with `validate_relative_path` and compare hash/id/version |
| M7 | WASM host `subscribe` checks neither `events.subscribe` nor `contributes.events` | `wasm.rs:271-279` | A component can subscribe to any topic (e.g. `model.requested`) that its manifest never declared; Extism does check. Authorize before subscribing |
| M8 | WASM host path helper creates directories before the containment check, and `write_file` calls it before authorizing | `wasm.rs:193-205`, `321-329` | A `workspace.read`-only package can create directories, and a denied write already mutated the filesystem. Authorize and canonicalize first |
| M9 | WASM ignores the manifest `max_execution_ms` on host-initiated calls | `wasm.rs:630-631`, `841`; callers pass 30/60 s at `host.rs:1055-1060`, `1119-1124` | A component declaring 2 s can run 60 s through a language/integration contribution. Clamp with `resources.execution_ms()` |
| M10 | A WASM package's declared commands/panels/prompts/contexts are silently dropped | `host.rs:877-893` returns `DeclarativeContributions::default()`; `host.rs:953-960` then skips a commands-only package | §3/§14 "no silent fallback". Load the files like the process/extism arms, or reject those declarations |
| M11 | Process `commands/execute` skips the session reset and the output ceiling that `tools/call` applies | `process.rs:940-954` | After a command timeout the stale response stays on the pipe; command results bypass `max_output_bytes`. Reuse the same code path |
| M12 | Declared skills are never matched to files; a missing `skills/<id>/SKILL.md` validates, installs, and appears in the status row | `manifest.rs:305-324` (no Skill kind), `host.rs:1545-1552`, `agent.rs:480-489` | Status lies about what was contributed; §3 requires the file. Validate each declared skill and report what actually registered |
| M13 | A well-formed JSON-RPC error tears down the process/MCP session | `process.rs:646-652` treats every `Err` as "pipe unknown", but `process.rs:147-152` returns `Err` for a decoded error object; same in `mcp.rs:726-729`/`127-132` | Every application-level failure kills the child and loses its in-process state; distinguish transport errors from JSON-RPC errors |
| M14 | MCP calls block the executor and ignore the caller deadline | `mcp.rs:556-569` (std mutex + `recv_timeout` in an async fn), `665-683` (`_context`) | The agent's timeout/cancel `select` cannot fire, and cancel needs the same mutex. Use a blocking task + channels and `min(context.timeout_ms, max_execution_ms)` |
| M15 | `agent.skills.provide` is denied by default for community/local, though §6's table allows it | `capability.rs:167-170` (Medium), `826-832` (only Low allowed); standard line 329 | A community package that honestly declares the capability it uses can never activate; the other three medium rows match the table, so this is an outlier |
| M16 | Cross-kind contribution uniqueness excludes skills, presets and both provider kinds | `manifest.rs:305-324`, `540`; duplicates within those lists unchecked except skills | §2.2 says unique "across every contribution kind". Add them to `ContributionKind`/`all_contributions` |
| M17 | Declarative UI images and panel icons are never rendered: `Image` prints its `src`/`alt` text, and `PanelSurface.icon` is never read | `ui/extension_panel.rs:613-627`; no reader of `.icon` in `ui/` | §10 lists `image` and the panel example carries `"icon": "assets/git.svg"`; `asset/…` is validated but has no resolver, and no https image is fetched. Draw them or drop them from the contract |
| M18 | The composer's extension commands are captured once in `Shell::new` | `ui/shell.rs:1767`, `853`; consumed at `ui/command_menu.rs:529` | Install/enable from Settings does not change the palette until restart, although panels/themes/statuses are re-read |
| M19 | Documented host events are missing: `extension.unloaded` is never published, `config.changed` only from the CLI, and a panel action publishes nothing | `host.rs:672-692` (loaded/failed only); `cli.rs:1373`; no `publish` in `ui/extension_panel.rs` | §9 lists them as host seams ("UI: panel actions"). A package subscribing to `extension.unloaded` never fires |
| M20 | MCP offers no event subscriptions or pump, though the review matrix marks MCP events "yes" | no `pump`/`subscribe` in `mcp.rs`; `docs/bsh-extension-review.md:53` | `contributes.events` on an MCP package is accepted and silently ignored |
| M21 | The lifecycle machine refuses the documented retry edge and bypasses the table on activation | `lifecycle.rs:226-250` has no `(Failed, Loading)`; `lifecycle.rs:179-193` writes `Active` with no check; host ignores transition errors (`host.rs:644-648`) | §7 lists "Failed → Loading (retry)" and says an undefined transition is an error. Make `activated` go through `transition` and add the retry edge |
| M22 | `reproduce` accepts any already-installed id/version without comparing the locked hash | `cli.rs:1579-1588` | §12/§13 define reproduce as "exactly the locked versions"; different bytes are reported `present` |
| M23 | Contribution files are only existence-checked for themes/providers/presets; a malformed one validates and then vanishes | `runtime/declarative.rs:502-514`; `host.rs:1688-1689`, `1381-1384` (`.ok()`) | §13 requires validate to fail on an unparseable file; §14 forbids silent fallback. A theme with bad JSON is silently not applied |
| M24 | A disk manifest claiming `runtime.kind = "builtin"` passes `validate` and `install` | `manifest.rs:410-417`; `cli.rs:358` (`_ => Ok(())`); refusal only at `host.rs:895-898` | §4.3/§15.8 say a disk manifest may not claim it. Refuse in `Package::load` for non-builtin sources |
| M25 | `--json`/`--root` are parsed only before the subcommand, so the documented trailing forms do nothing | `cli.rs:86-103`; `command_list`/`command_info`/`command_doctor` take the pre-parsed flag | `bsh ext list --json` prints text; `bsh ext uninstall <id> --root /tmp/sandbox` silently operates on the real store (the most dangerous of the two) |
| M26 | `qualify` truncates blindly, so a long namespace can erase the local name | `types.rs:79-85` | A legal 63-byte last id segment + any tool → every tool in the package becomes the bare namespace, all but one dropped with a log line. Keep a stable suffix (short hash) or reject at validation |

## 4. Low

| # | Finding | Evidence |
|---|---|---|
| L1 | Signature is verified over the source hash, but the installed copy's hash is recorded without comparing the two (TOCTOU) | `package.rs:960-1025` |
| L2 | Duplicate assignment (`package.source_path` set twice) — evidence of a botched edit | `package.rs:1029-1030` |
| L3 | Storage creates parent directories before the containment check | `storage.rs:60-81` |
| L4 | `GrantScope` serializes program args with a comma join and re-parses by splitting on commas, so `--exclude=a,b` changes meaning across restart | `capability.rs:271-273`, `318-325` |
| L5 | App exit blocks the UI thread on each runtime's 1-second process shutdown, sequentially | `lib.rs:78`; `host.rs:1132-1143`; `process.rs:855-862` |
| L6 | `install()` holds the process-global host mutex while `apply()` reads skills dirs and globs contexts, blocking `publish()`/`record_tool_call()` from other threads | `host.rs:1748-1783`; `agent.rs:341` |
| L7 | Workflow wrapper drops progress and does not forward cancel or memory/fuel metrics | `runtime/workflow.rs:189-249`; trait defaults `runtime/mod.rs:285-287`, `316-325` |
| L8 | A corrupt `state.toml` re-enables every disabled extension (`state_of` falls back to `ExtensionState::default()`, which is `enabled = true`) | `package.rs:713-718`, `384-392` |
| L9 | Skill ids are not contribution ids for validation purposes (see M16), and a skill may share an id with a tool without a diagnostic | `manifest.rs:539-560` |

## 5. Missing features (surface never implemented)

> Status after the fix pass: items 2-5 and 7 are implemented (asset images and
> icons, live refresh, MCP events, unloaded/config/ui.action events, workflow
> forwarding). Item 6 is covered by load-time re-verification against
> `trust.toml`. Item 1 (a configuration editor in Settings) remains open: it is
> a new settings surface whose shape (schema-driven form vs raw JSON) is a
> product decision, not a defect. Item 8 is environment evidence, not code.

1. **Extension configuration UI.** The manifest carries `configuration.schema` and
   `defaults`, `config.rs` validates and migrates them, and `bsh ext config` can set
   them — but Settings → Extensions has no editor, so the reader cannot change a
   package's configuration in the GUI at all.
2. **Images and panel icons.** The `image` UI component and `PanelSurface.icon`
   are validated and then drawn as text/nothing (M17). `asset/…` has no resolver,
   and remote images are never fetched.
3. **Live surface refresh.** Install/enable/disable refresh status rows only; the
   composer command list (M18) and the mounted panel set are snapshots taken at
   `Shell::new`, so a newly installed package needs a restart to appear in those
   two places.
4. **MCP event subscriptions.** No subscription, pump or delivery (M20). If MCP
   is to be a first-class runtime, either implement this or reject
   `contributes.events` on an MCP manifest with a diagnostic.
5. **Host lifecycle events.** `extension.unloaded`, shell-side `config.changed`,
   and the panel-action event promised by §9's table (M19).
6. **A real verification story.** There is no publisher verification path; the
   standard says so, but the UI still shows a "verified" label sourced from a
   registry record that load never re-checks (H2). Until that exists, "verified"
   means "was verified once, at install".
7. **Per-run progress for wrappers.** A workflow tool (a first-class contribution)
   cannot stream progress, be cancelled cleanly, or report memory/fuel (L7).
8. **Platform evidence.** Windows/Linux process limits and the Linux bwrap path are
   compiled but unexercised in this tree's environment (stated in §15.7, but it
   also means the process-runtime claims are macOS-only evidence).

## 6. Where the documents overstate the implementation

- `docs/bsh-extension-conformance.md:41` marks declarative UI as including
  "stale-revision refusal". It is unreachable: the panel dispatch stamps the
  *current* revision (M1).
- `docs/bsh-extension-review.md:53` gives MCP "Events: yes". There is no
  subscription or pump (M20).
- `docs/bsh-extension-review.md:52` and the conformance matrix mark WASM
  commands "yes"; `host.rs:877-893` loads none of those files, and
  `bsh-extension-standard.md` §1 itself lists commands as
  "declarative, process, built-in" — the code and the standard disagree and the
  review doc sides with neither.
- `docs/bsh-extension-standard.md:31` says workflows are "Recorded; no executor in
  this build", while `:107` says "(executed)" and `runtime/workflow.rs`
  implements an executor. `host.rs` comments ("workflows, recorded only") repeat
  the stale claim.
- `docs/bsh-extension-review.md:40` says audio capture has no host producer;
  `shell.rs:2224` publishes `audio.captured`.
- `docs/bsh-extension-review.md:74-76` claims shutdown drains every runtime and
  that no subscriptions leak. The e2e assertion does check the count after an
  explicit `shutdown()`, but a *reopen* (M2) drops the old host without
  `shutdown()`; runtimes kept alive by an already-built agent's
  `ExtensionTool` keep their old bus subscription, and the old pump thread's
  generation never advances, so it keeps delivering on the old bus.
- The dev guide's own example `--scope program:python3:--version` cannot work
  (H7).

## 7. Test gaps and suggested order of work

**Order (security first, then correctness, then surface):**

1. C1 (MCP spawn), H1 (one authorization seam), H2 (verify on load), H3
   (normalize scoped paths), H4 (WASM cancel). These are the findings that let a
   package do something the reader did not authorize.
2. H5–H9, M1–M3, M5–M6, M11–M14: user-visible wrong behaviour with narrow fixes.
3. M15–M26 and L1–L9: contract drift, serialization, UI surface.
4. Section 5 features, in the order a reader meets them (configuration editor,
   images/icons, live refresh).

**Tests that would have caught these** (none exist today):

- a panel action that calls a tool declaring a denied capability (H1);
- a test that modifies a file under `installed/` and asserts the host refuses to
  apply it (H2);
- a `path:/ws` grant + `{"path":"../outside"}` (H3);
- cancel a running WASM call, then call again (H4);
- a skills-only package applies its skill (H5);
- a WASM/Extism tool returning more than `max_output_bytes` (H6);
- a `program:python3:--version` grant activating the package (H7);
- install v2 over a disabled v1 and assert it stays disabled (H8);
- `validate` on an Extism package with a missing descriptor (H9);
- two renders, a document update, then a click from the stale render (M1);
- `install()` then `statuses()` under a preset with an allowlist, asserting the
  host was not reopened (M2);
- a fake MCP server whose tools are named `getUser` (M3).

The existing suite is healthy where it looks (116 green here) but it mostly
exercises the happy path through the agent wrapper, which is exactly the one path
where enforcement is correct. The thin files (`signing.rs` 1 test, `wasm.rs` 1,
`mcp.rs` 1) are where the next regressions will land.

## 8. Fix status (same day, after the review)

Every code finding above is fixed in the working tree, with the regression tests
listed in §7 added where the behaviour is testable in this environment. The
suite is 127 extension tests green (the 9 process-spawning tests remain
environment-blocked here: nested `sandbox-exec` is denied inside this review's
own sandbox, not by the product).

| Finding | Fix |
|---|---|
| C1 | `McpRuntime` takes the policy and authorizes `process.execute` with program+args on every spawn |
| H1 | one seam (`capability::authorize_each` + `call_scope`) used by the agent wrapper, language-check, integrations, workflow steps and panel actions |
| H2 | `scan_installed` compares the recomputed hash with the registry record, re-verifies the signature, refuses a mismatch and demotes a removed publisher |
| H3 | scope paths are lexically normalized before storage and comparison; grant parsing normalizes too |
| H4 | WASM cancellation no longer blocks on the instance lock, host calls trap while cancelled, the flag is per call, and shutdown is a separate flag; Extism traps at the bridge and stops blocking cancel |
| H5 | skills count as an agent contribution |
| H6 | WASM and Extism enforce `max_output_bytes` |
| H7 | load-time process scope carries `args`, so `program:exe:args` activates |
| H8 | install no longer rewrites `enabled` |
| H9 | `validate` checks Extism/WASM entries and contribution files; a disk manifest may not claim `builtin` |
| M1 | the panel records the render revision and stamps actions with it |
| M2 | `HostConfig` equality ignores the preset allowlist; the preset filters `apply` only |
| M3 | MCP keeps the server's own tool name (sanitized id is local only) and reports collisions |
| M4 | the Grant button refuses to widen `secrets.read`/`network.http` to any; the denial keeps its scoped command |
| M5 | storage counts files on write, not only on list |
| M6 | market paths use the shared grammar; id/version/hash are compared with the index |
| M7-M10 | WASM subscribe is declared+authorized, path creation is authorization-first, host-initiated calls clamp to `max_execution_ms`, and WASM contributions load |
| M11 | process `commands/execute` resets the session and applies the output ceiling |
| M12 | declared `skills/<id>/SKILL.md` files are validated |
| M13 | a decoded JSON-RPC error is `Remote` and keeps the session |
| M14 | MCP calls run on the blocking pool under `min(caller, manifest)`; cancel kills the child by pid |
| M15 | `agent.skills.provide` is allowed at community/local as §6 says |
| M16 | skills, presets and both provider kinds are contribution kinds for uniqueness |
| M17 | images and panel icons render `asset/` files; `https` is refused (no loader) |
| M18 | install/enable/disable refresh commands, panels and themes |
| M19 | `extension.unloaded`, shell `config.changed` and `ui.action` are published |
| M20 | MCP subscribes to declared events and delivers `bsh/events/event` |
| M21 | `Failed -> Loading` exists and `activated` goes through the table |
| M22 | `reproduce` checks the installed hash |
| M23 | raw contribution files are parsed at load |
| M24 | covered by H9 |
| M25 | `--json`/`--root` are parsed anywhere in the argument list |
| M26 | long names keep a stable digest suffix instead of colliding |
| L1-L9 | TOCTOU hash compare, duplicate assignment, storage walk, grant-arg JSON, concurrent shutdown, hot paths off the host lock, workflow forwarding, corrupt-state default, skill ids |

**Residual, stated rather than hidden:**
1. A WASM guest that performs pure computation with no host call still cannot be
   interrupted by `cancel`; it stops at its epoch/fuel deadline. Trapping it
   mid-instruction needs an async Wasmtime store, which this synchronous design
   does not have. Host-call-heavy guests (fetch, storage, events, file IO) stop
   at the next call.
2. Remote images are refused rather than fetched; the contract now says so.
3. `install` still holds the host lock while `apply` reads skills and globs
   contexts. `publish` and `record_tool_call` no longer contend on that lock,
   but a build and another host operation still serialize.
4. The 9 process-runtime tests cannot run inside this review's own sandbox; they
   exercise the real `sandbox-exec` path and are expected to pass elsewhere.

