# BSH TUI — repository-backed Gap Analysis

- **Target commit:** working copy `@` in `/Users/gaosg/Projects/bos`
- **Scope:** `crates/bsh/src/tui`, `crates/bsh/src/runtime`, `crates/bsh/src/caps`, `crates/bsh/src/state`, `crates/bsh/src/runner`
- **Design inputs:** `docs/tui/rfc.md`, `docs/tui/requirements.md`, `docs/tui/ui-ux.md`, `docs/rfc/multi-interface-runtime.md`
- **Rule:** implementation claims below cite repository paths and tests. The RFC's "Non-goal" says it does not assert current implementation; this document is the evidence pass it requires.

## Summary

The repository already contains a Ratatui TUI client (`bsh tui`) of a standalone Runtime Host. The TUI is not a renderer over GUI-owned `AppState`; it speaks `runtime::contract` through `runtime::client::RuntimeClient`. The Host executor reuses the same BSH desktop agent construction path (`caps::build_agent_for_session_with_subagents_jobs_and_review`) with the same `Settings`, `Vault`, tools, skills, MCP, subagents, jobs, extensions and policy. The TUI keeps only presentation state (`tui::app::App`: navigation, focus, drafts, scroll, modal/detail state).

This change also fixes a narrow-rail layout defect, makes `Ctrl+Enter` an explicit submit chord, preserves composer draft on `Esc`, removes `Ctrl+C` cancellation, adds `/cancel [confirm]`, and changes `bsh tui` to start a **separate standalone `bsh host`** process when no Host owns the data directory and not to stop it on TUI exit.

### Shared application layer (this pass)

The terminal and the window now share every non-view layer the request named:

- **Session, history, trash, configuration.** The Host's session repository
  (`runtime::session_repo`) is the desktop's own `Store` + `history` + `trash`
  stack, rooted at the same `<workspace>/.bsh/sessions`. A terminal turn writes
  the session document and its Parquet rows, so `Store::open` in the window
  reads the chat; a window chat is imported into the Host's projections at open
  and can be resumed from the terminal. Titles, plans, goals, feedback, pins,
  archive flags and deletes use the same files, and the bin is one bin.
- **Context.** `runtime::context_prepare` builds and injects one typed snapshot
  (runtime facts, plan, workspace instructions, time context, memory recall,
  extension prompts) and the Host executor calls it, as the runner does.
- **Transcript replay.** `runtime::transcript` seeds the engine from the stored
  rows, including reasoning where the provider requires it, tool calls with
  their stored results, and images. Both interfaces replay one conversation.
- **Turn text, grammar, formatting, clock, audit.** `runtime::turn`,
  `runtime::grammar`, `runtime::format`, `runtime::clock` and
  `runtime::audit_view` replaced the copies the terminal held and the window
  now calls the same modules.

### Remaining hard gap

BSH desktop GUI is still not itself a client of the standalone Runtime Host.
`bsh tui` and `bsh host` speak `runtime::contract`; BSH desktop keeps using
`state::AppState` + `runner::Runner` + `caps::build_agent` for its own turns, and
its in-memory store does not observe a Host writing while the window is open.
Execution authority is therefore still split between the two processes, even
though the durable session state they both read and write is one. Requirement
TUI-R014 / TUI-038 (GUI+TUI same Application Contract conformance, one
authority) is therefore **not fully met**; the store and request layers are.

## Requirement matrix

| ID | Requirement | Current implementation | Evidence | Gap / status | Smallest issue |
|---|---|---|---|---|---|
| TUI-R001 | Independent client of standalone Runtime Host; TUI exit leaves Host-owned work running | `bsh tui` attaches to an existing Host or spawns a separate `bsh host`; no TUI-owned execution; TUI does not kill/stop a spawned Host | `crates/bsh/src/runtime/cli.rs::spawn_standalone_host`; `crates/bsh/src/runtime/cli.rs::tui_main`; `crates/bsh/src/tui/mod.rs` | Met for `bsh tui`. Explicit embedded `Authority` still exists for non-TUI callers, but `bsh tui` does not use it | Keep `bsh host` for shared multi-client use; test TUI-exit with a running turn under separate process |
| TUI-R002 | Domain mutations through shared Application Contract; no direct storage/Agent mutation from rendering | TUI calls `RuntimeClient` commands/queries; rendering reads projections. The Host writes the desktop's own `Store`/`history` documents through `runtime::session_repo`, and both interfaces call the shared `runtime` request layer | `crates/bsh/src/tui/app.rs` uses `crate::runtime::contract`; `crates/bsh/src/runtime/client.rs`; `crates/bsh/src/runtime/session_repo.rs`; `crates/bsh/src/runtime/context_prepare.rs` | Met for the TUI path; the window still mutates through `AppState`, but it writes the same session documents and calls the same shared modules | Add shared conformance fixtures |
| TUI-R003 | Host owns authorization and approval; forged/stale/expired rejected | Host derives Principal; approval digest/version/status checks; typed rejections | `runtime/contract.rs::RejectionCode`; `runtime/projection.rs`; `runtime/host.rs`; `runtime/authz.rs`; `runtime/audit.rs` | Met in TUI path. ApprovalView lacks requested scope/agent/risk fields | Extend `ApprovalView`/`ApprovalTarget` with scope/capability/requester/risk while keeping digest binding |
| TUI-R004 | Durable events cursor replay + gap-free live handoff | durable `EventEnvelope`, monotonic per stream, snapshot cursor, `CursorExpired`, replay-to-live | `runtime/journal.rs`; `runtime/contract.rs::CursorExpired`; `runtime/host.rs`; `tui/app.rs` `Response::CursorExpired` | Mostly met; full fault-injection proof is test coverage, not protocol gap | Run fault injection around commit/publish/reconnect |
| TUI-R005 | Idempotent commands + unknown outcomes | `command_id`, `client_instance_id`, `expected_version`, durable command ledger, `OutcomeUnknown`, retry same id | `runtime/contract.rs::CommandEnvelope`; `runtime/client.rs::ClientError::OutcomeUnknown`; `tui/app.rs::uncertain_send` | Met | Verify duplicate/mismatched key conformance |
| TUI-R006 | Typed semantic timeline blocks | `Message`, tool, delegation, approval, error, system, execution typed projections; no markdown inference | `runtime/contract.rs::SessionProjection`, `ToolCallView`, `ApprovalView`, `SubagentRunView`; `tui/app.rs` | Met. Reasoning summary only where policy-approved | Keep hidden reasoning excluded |
| TUI-R007 | Multiline Unicode + safe paste | multiline composer, Unicode cell width, bracketed paste, pasted controls as data, CJK/combining handling | `tui/term.rs::Key::Paste`, `EnableBracketedPaste`; `tui/app.rs` `newline`, cursor; `cells`, `truncate` | Met in code; platform matrix tests still needed | Run macOS/Linux/Windows terminal matrix |
| TUI-R008 | Adaptive narrow terminal | breakpoints <80/80–99/100–119/120–159/≥160; sidebar compact; inspector progressive; no horizontal clipping | `tui/app.rs` `sidebar`, `board::width`; `tui/nav.rs`; `tui/skin.rs` | Fixed `nav::item_row` overflow. Core at 80 must be smoke-tested live | Run `ocu` 80-col terminal check |
| TUI-R009 | Terminal restored on exit/panic | `ratatui::try_init`, panic hook, `try_restore`, Drop, disable bracketed paste/mouse, restore stderr | `tui/term.rs::Terminal`, `tui/mod.rs::StderrLog` | Met | Add fault-injection panic test if not already present |
| TUI-R010 | Job lifecycle independent of conversation | tasks pane independent; jobs queryable; navigation/disconnect not cancellation | `/tasks`, `TaskBoardReport`, `TaskFilter`; `tui/app.rs::tasks_command`; `runtime` jobs | Met | Verify after navigation/disconnect |
| TUI-R011 | Typed errors conflict/forbidden/offline/unknown | `RejectionCode`, `ClientError::{Conflict,Forbidden,OutcomeUnknown,Closed,...}`, `Response::CursorExpired` | `runtime/contract.rs`; `runtime/client.rs`; `tui/app.rs::client_error` | Met for TUI. Contract `RejectionCode` lacks separate `Forbidden`, `StaleVersion`, `CursorExpired`, `ResourceExhausted`, `HostUnavailable`, `ProtocolMismatch`, `ValidationFailed`; some are `ClientError`/response variants instead | Normalize typed error taxonomy |
| TUI-R012 | Approval modal binds immutable action/scope | digest + target + expiry + status; atomic Host transition; stale/expired/already-decided rejected; not optimistic | `ApprovalView`, `ApprovalTarget`, `ActionDigest`, `ControlPayload::DecideApproval`; `tui/app.rs::approval_card`, `decide_approval` | Partial. TUI renders a high-priority approval card, not a separate modal; requested scope/capability/requesting agent/risk are not in `ApprovalView`; ADR-03 modal not fully implemented | Add approval modal + extend contract fields |
| TUI-R013 | Slow clients cannot block Host | bounded subscriber queues; independent flow control; ephemeral coalescing; durable replay | `runtime/server.rs`, `runtime/client.rs`, `runtime/journal.rs`, `Response::CursorExpired` | Met in design; backpressure tests should be run | Run slow-consumer test |
| TUI-R014 | GUI/TUI shared operation semantics | TUI shares `runtime::contract` + `caps`; both interfaces call the shared `runtime` session store, context assembly, transcript replay, grammar and formatting | `runtime/agent_exec.rs`; `runtime/session_repo.rs`; `runtime/context_prepare.rs`; `runtime/transcript.rs`; `caps.rs`; `tui/app.rs` | **Partly met**: the request and durable session layer are shared; GUI still does not use `RuntimeClient`/`runtime::contract`, and there is no GUI+TUI conformance suite | Build shared conformance suite; make GUI a client or adapter |
| TUI-R015 | Activity and Audit separate | `/activity` operational, `/audit` security; separate panes | `tui/app.rs::audit_command`; nav `Activity`, `Audit`; `runtime/audit.rs` | Met, but `/activity` currently opens `Trajectory`, not a dedicated Activity projection | Add `SlashCommand::Activity` if literal match required |
| TUI-R016 | Jobs filters + detail timeline | filters All/Active/Waiting/Completed/Failed/Interrupted; rows + detail; cancellation gated | `TaskFilter`, `TasksPane`, `/tasks`, `task_detail`, cancellable Host state | Met | Verify `Interrupted`/`Recoverable`/`NeedsIntervention` states |
| TUI-R017 | Agent delegation hierarchy | `SubagentRunView.parent`, `depth`, status, assignment, output; `/subagents`; board agents | `runtime/contract.rs::SubagentRunView`; `tui/app.rs`; `tui/board.rs` | Partial. Status vocabulary is `running/done/failed`, not planning/working/waiting/completed/failed/idle; availability vs assignment vs execution not fully separated | Extend status + render tree |
| TUI-R018 | Inspector permission-filtered input/output/authorization + redaction | `/detail`, `/mcp`, inspector board; secret redaction in Host projections | `tui/board.rs`; `tui/app.rs::detail`; `runtime/projection.rs`; `docs/bsh-terminal-policy.md` | Partial. No Inspector tabs Overview/Input/Output/Timeline/Authorization/Diagnostics; `/detail` is a detail pane | Implement tabs |
| TUI-R019 | Command palette destinations + actions | Ctrl+P/configurable; fuzzy; all destinations; keyboard | `tui/nav.rs::entries`; `tui/prefs.rs::PaletteKey`; `tui/picker.rs`; `tui/menu.rs` | Met; help now matches `PaletteKey::parse` (`ctrl+p`, `ctrl+g`, `ctrl+o`, `ctrl+q`, `alt+g`, `alt+o`, `alt+q`) | No full keymap remapping beyond palette key |
| TUI-R020 | Capability browser scope/binding + states | `/capabilities`; granted scope; tools; extension capability views; approval statuses requested/approved/denied/expired/cancelled | `tui/app.rs::capabilities_body`; `runtime/contract.rs::ExtensionCapabilityView`; `CatalogueView` | Partial. No first-class `available/granted/requested/denied/expired` capability states for all capabilities | Add capability state projection |
| TUI-R021 | Theme + keymap configurable persist per client | `tui-preferences.json`; theme auto/ansi/mono/truecolor; palette key; `/tui` | `tui/prefs.rs`; `tui/skin.rs` | P2 met | Full keymap remapping beyond palette key is not implemented |
| TUI-R022 | Transport-neutral contract for Web/Mobile | no terminal types in contract; future HTTPS/SSE documented | `runtime/contract.rs`; `docs/rfc/multi-interface-runtime.md` | Met for contract. No Web/Mobile client (non-goal for TUI first release) | Keep contract tests independent of Ratatui |

## Backlog matrix

| Story | Status | Evidence / gap |
|---|---|---|
| TUI-001 | Done | `tui::app::App`, `RuntimeClient`, `runtime::host::RuntimeHost`, execution ownership map in `runtime/*` |
| TUI-002 | Mostly done | `Session`, `Turn`, `Execution`, `Task`, `ToolExecution`, `Approval`, `Subagent` identities; missing `Recoverable`/`NeedsIntervention` exposure in some projections |
| TUI-003 | Mostly done | `runtime/journal.rs`, `EventEnvelope`, `CursorExpired`; needs fault injection |
| TUI-004 | Mostly done | `runtime/authz.rs`, `runtime/audit.rs`, `ApprovalStatus`, atomic approval; needs race tests |
| TUI-005 | Mostly done | TUI maps to `runtime::contract`; both interfaces share the session store and the request/context layer in `runtime`; GUI transport is still `AppState` |
| TUI-006 | Done | Ratatui + crossterm; macOS verified; platform matrix pending |
| TUI-007 | Done | `CommandEnvelope`, `ControlEnvelope`, `QueryEnvelope`, `Response`, `PROTOCOL_VERSION` |
| TUI-008 | Done | `command_id`, `client_instance_id`, `OutcomeUnknown`, same-key retry; duplicate/mismatch tests should be run |
| TUI-009 | Mostly done | `Snapshot`, `Cursor`, `version`, `CursorExpired`; snapshot exact-sequence invariant needs test |
| TUI-010 | Mostly done | `RuntimeClient`, `ClientError`, `RejectionCode`; taxonomy needs normalization |
| TUI-011 | Mostly done | `Endpoint`, user-scoped socket, `Welcome`, peer identity, `PROTOCOL_VERSION`, bounded frames |
| TUI-012 | Mostly done | `EventEnvelope`, sequence, schema_version, classification; `ApprovalRequested` lacks requester/scope/risk |
| TUI-013 | Mostly done | `Snapshot`, cursor, `CursorExpired`; replay/live test needed |
| TUI-014 | Mostly done | gap-free handoff design; fault-injection test needed |
| TUI-015 | Mostly done | bounded queues + coalescing; backpressure test needed |
| TUI-016 | Mostly done | `CursorExpired`, `Response::CursorExpired`, `CursorExpired` UX; resync test needed |
| TUI-017 | Done | `tui::term::Terminal`, panic hook, restore, Drop |
| TUI-018 | Done | `tui::app::App`, nav/focus/back/detail/modal state |
| TUI-019 | Done | breakpoints in `tui/app.rs`, `tui/board.rs`; fixed `nav` overflow |
| TUI-020 | Mostly done | keymap + palette + menu; not full keymap remapping; help matches `PaletteKey::parse` |
| TUI-021 | Done | theme tokens + 16-color fallback + mono; contrast validation pending |
| TUI-022 | Done | data-in/rows-out `tui::nav`, `tui::board`, `tui::skin`, `term` tests; fake Host in `runtime::test_support` |
| TUI-023 | Done | typed `SessionProjection`, `ToolCallView`, `ApprovalView`, `SubagentRunView` |
| TUI-024 | Mostly done | streaming/coalescing + no duplicate committed blocks; reconnect test needed |
| TUI-025 | Done | multiline Unicode + bracketed paste + safe data; `Ctrl+Enter` added; `Esc` preserves draft; selection/undo not fully implemented |
| TUI-026 | Mostly done | draft preserved on nav/resize/Esc; persistence policy per client; draft persistence to disk not implemented |
| TUI-027 | Mostly done | slash commands + completion + command palette; mention completion; `/cancel [confirm]` added |
| TUI-028 | Done | inline tool execution cards + status + correlation + inspect |
| TUI-029 | Partial | `/detail`; no Inspector tabs; redaction in projections |
| TUI-030 | Done | `TaskFilter`, `TasksPane`, detail |
| TUI-031 | Partial | `SubagentRunView.parent/depth`; not full agents tree + status vocabulary |
| TUI-032 | Done for TUI | `/cancel [confirm]`; Ctrl-C not bound; Host confirms; job not cancelled on disconnect |
| TUI-033 | Partial | digest/target/expiry; not full requested scope/capability/risk/requester |
| TUI-034 | Mostly done | atomic Host approval; stale/expired/already-decided rejections; race test needed |
| TUI-035 | Partial | `/capabilities`; not full capability state matrix |
| TUI-036 | Done | `/activity` vs `/audit`; runtime audit |
| TUI-037 | Mostly done | secret redaction in projections/session store; no-secret UI fixture test needed |
| TUI-038 | Partial | shared session store, context assembly, transcript replay and request text; no shared conformance suite and no single execution authority |
| TUI-039 | Mostly done | transport-neutral contract; no Web/SSE checks |
| TUI-040 | Mostly done | `CursorExpired`, stale/offline read-only principles; mobile semantics not implemented |
| TUI-041 | Not met | macOS verified; macOS/Linux/Windows matrix not recorded |
| TUI-042 | Not met | no single end-to-end release gate; `runtime::embed` test + TUI tests separate |

## Live verification with Open Computer Use

- `sw_vers -productVersion`: macOS `27.2.1`.
- `open-computer-use doctor`: `accessibility=granted`, `screenRecording=granted`.
- `cargo build -p bsh`, then `bsh tui --echo --data-dir <scratch> --workspace <scratch-workspace>` in Terminal at 80×24.
- `open-computer-use call list_apps`, `get_app_state`, `press_key`.
- `Ctrl+T` created `Session 19:20`; `Host Connected`; 80-column layout; no horizontal clipping in snapshot.
- `Ctrl+D` exited `bsh tui`; `pgrep -fl 'target/debug/bsh tui'` empty; `pgrep -fl 'target/debug/bsh host'` still lists `bsh host ... --echo`; `<data-dir>/runtime.json` still present with protocol 1. This verifies TUI-R001 live.

## UX checklist status

- Keyboard-only: met in code; verify with `open-computer-use`.
- 80 columns: layout met; live verification pending.
- Draft survives resize/navigation/Esc: met; test/live pending.
- Tool vs prose: met.
- Job after leaving conversation: met.
- Approval exact target/scope/action/expiry: partial (scope/requester/risk missing in contract).
- Approval Host-confirmed: met.
- Disconnect not cancellation: met.
- Reconnect no duplicates: met in projection; test pending.
- Stale/offline marked: met.
- CJK/wide Unicode: met in `cells`/`truncate`; test pending.
- Terminal restore after exit/panic: met.
- `Ctrl+C` not job cancellation: met in TUI; `/cancel [confirm]` explicit.

## Changes made in this pass

1. `tui/nav.rs`: `item_row` reserves icon + key + gap before truncating label; narrow rail cannot overflow.
2. `tui/term.rs`: `Ctrl+Enter` maps to `Key::Enter`; test.
3. `tui/app.rs`: `Esc` closes completion + clears transient cancellation but preserves draft; `Ctrl+C` no longer cancels; `/cancel [confirm]` adds contextual confirmation; `cancel_pending`.
4. `runtime/cli.rs`: `bsh tui` starts a **separate `bsh host`** process when no Host exists and does not stop it on exit; help text updated; Ctrl-C help corrected.
5. `runtime/embed.rs`: embedded `Authority` retained but no longer used by `bsh tui`.
