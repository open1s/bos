# `crates/gui` — Requirements & Competitive Roadmap

> Mission (from the active goal): build a BOS GUI on `crates/gui` whose
> **architecture, performance, functionality and extensibility comprehensively
> exceed Codex and DeepSeek Harness (DSH)**, and which is a credible substrate
> for AGI-class long-horizon work.
>
> 目标：基于 BOS，在 `crates/gui/` 实现一个在**架构、性能、功能、扩展性**上全面超越
> Codex 与 DeepSeek Harness，并支撑 AGI 长期自治工作的桌面 GUI。

This document is the research-first artifact: it records what the two products
actually do (with evidence), what they admit is weak, and the requirements we
hold ourselves to. Every milestone below cites the evidence that closes it.

---

## 0. Method and evidence

| Evidence source | What it gives us | Location |
| --- | --- | --- |
| DSH shipped bundle (asar) | 287 internal package specs (English + zh READMEs), the whole architecture map | `/Applications/DeepSeek Harness.app/Contents/Resources/app.asar` → extracted to [`target/dsh-readmes/`](../target/dsh-readmes/) (`INDEX.txt`, `LIMITATIONS.txt`) |
| DSH self-declared limitations | 275 packages state their gaps — our cheapest "beat them" targets | [`target/dsh-readmes/LIMITATIONS.txt`](../target/dsh-readmes/LIMITATIONS.txt) |
| Codex checkout | `codex-rs/tui/src/**`, `codex-rs/app-server*`, `docs/*.md` | `/Users/gaosg/Projects/codex` |
| Web research | public docs, release notes, issue reports | digests in flight (§4, §9) |
| This repo | what `crates/gui` + `crates/agent` already do | `crates/gui/**` |

Evidence rule: **no requirement without evidence** — each is either (a) a
capability observed in Codex/DSH, or (b) an explicit design decision of ours,
marked as such. Unverified status is marked `⚠ verify`.

Extraction helper (re-runnable):

```bash
node /tmp/asar_dump_readmes.js        # re-dump DSH package READMEs
node /tmp/lim_extract.js              # re-extract DSH limitations
```

---

## 1. DSH capability inventory (what we must match or beat)

DSH `0.2.0-rc.2` is a Node/Electron desktop app composed of ~200 first-party
packages (`@deepseek-ai/dsh-*`) plus a bundled web GUI (`dsh-web-frontend`).
Its own package list is the most reliable feature spec we have.

### 1.1 Agent runtime

- **Agent loop / composition**: `dsh-agent`, `dsh-agent-loop` (parallel tool-call
  cap, unary exclusivity classification), `dsh-agent-preset` +
  `dsh-agent-preset-registry` (declarative YAML presets selecting tools/prompt
  sections/skills; several presets per process), `dsh-persona`,
  `dsh-system-prompt` (sectioned prompt assembly, variables, tool-schema
  sources), `dsh-agent-instructions` (AGENTS.md/CLAUDE.md loading),
  `dsh-agent-tool-presentation` (which form of a tool reaches the model),
  `dsh-time-context` (per-step clock + browser zone + elapsed), `dsh-tmux-context`.
- **Goals**: `dsh-goal` (persisted same-session goal), `dsh-goal-round-driver`
  (automatic continuation rounds), `dsh-tool-goal` / `dsh-command-goal`
  (`get_goal`/`create_goal`/`update_goal`, `/goal`).
- **Delegation**: `dsh-subagent` + `dsh-subagent-fork-in-process` +
  `dsh-subagent-spawn-in-process` (two delegation backends),
  `dsh-tool-subagent`, `dsh-tool-subagent-control`
  (`send_message`/`interrupt_agent`/`list_agents`), `dsh-tool-ralph`
  (fixed foreground fresh-agent loop toward one objective),
  `dsh-workflow` + `dsh-workflow-ptc` + `dsh-tool-workflow` (model-written JS
  orchestration fanning out subagents, run through the sandboxed PTC Node
  runtime).
- **Planning**: `dsh-plan-mode` (per-agent planning feature),
  `dsh-tool-todo` (`todo_write`, whole-list replacement, session-log backed).
- **Context management**: `dsh-token-meter` (replay-aware context pressure),
  `dsh-compaction` + `-basic` + `-image-offload` + `-tool-result-pruner`,
  `dsh-output-retention` (bounded model-facing output), `dsh-spill` +
  `-local` + `-policy` (oversized tool text saved to retrievable files with a
  shared text/image token budget), `dsh-repeat-tool-reminder`,
  `dsh-tool-call-timeout-policy`.
- **Tools**: `dsh-tool-fs` (read/read_image/write/edit),
  `dsh-tool-fs-search` (glob/grep), `dsh-tool-bash` + `-persistent`,
  `dsh-tool-pwsh` + `-persistent`, `dsh-tool-web` (web_search/web_fetch),
  `dsh-tool-present` (deliverable files), `dsh-tool-ask-user`,
  `dsh-tool-skill`, `dsh-tool-str-replace-editor`, `dsh-tool-jobs`,
  `dsh-tool-workspace-dependencies`, `dsh-tool-cordis` (read-only plugin API
  discovery), `dsh-tools` (registry + execution pipeline).
- **Skills**: `dsh-skill` (provider registry), `dsh-skill-filesystem`
  (local skills), `dsh-skill-office` (bundled Word/PPT/Excel instructions),
  `dsh-skill-badge`.
- **MCP**: `dsh-mcp-client`, `dsh-mcp-resources` (on-demand discovery with
  explicit server selection and agent scoping).
- **Approvals & safety**: `dsh-user-approval` (channel-neutral one-shot
  approval seam + policy), `dsh-user-questions` (waterfall ask/answer),
  `dsh-permission-presets`, `dsh-sandbox` + `-local` + `-policy` +
  `-windows-acl` (Windows write-restriction backend), `dsh-fs-observation-policy`
  (read-before-edit).
- **Hooks**: `dsh-hook-protocol`, `dsh-hooks-codex`, `dsh-hooks-claude-code`
  (run existing Codex / Claude Code hook configs during runs).
- **Jobs & schedule**: `dsh-jobs` + `-local` (background job registry with
  retained output), `dsh-schedule` (host-wide durable reminders plus
  session-bound task management).

### 1.2 Sessions and persistence

- Event-sourced session log: `dsh-session`, `-checkpoint-policy`,
  `-persistence-jsonl`, `-projection` + `-projection-cache`.
- **Four shipped format migrations**: `dsh-session-format-v0-to-v1` … `v3-to-v4`
  (frozen readers per released version, cardinality-changing migrations).
- Whole-log queries: `dsh-session-query` + `-query-sqlite` (**SQLite FTS5**
  full-text search), `dsh-session-turn-outline`, `dsh-session-stats`,
  `dsh-session-title` (+ `-llm` and `-first-prompt-llm` providers),
  `dsh-session-reference` (cross-session snapshot references),
  `dsh-session-log-export` (ZIP export) and `-log-deepseek` (upload),
  `dsh-session-telemetry` (+ `-otel`).

### 1.3 Client GUI (`dsh-web-app`, ~50 `dsh-client-ui-*` surfaces)

- **Shell**: `dsh-client-ui-layout` — three-column AppFrame, 264–420 px sidebar
  (default 280, 56 px rail collapsed, auto-collapse <1024 px), right column as
  an edge-anchored panel opening at 45 % of viewport (capped 70 %, protects
  400 px for the center), macOS window-chrome seat, Windows caption row,
  `ctx.layout.selectPanel`, theme presenter writing color-scheme, alias tokens
  and `--dsh-content-font-size`.
- **Transcript**: `dsh-client-ui-conversation` (target/scene registry, per-session
  bindings, input state), `dsh-client-ui-chat` (conversation nodes, turn rail
  with fixed-pitch marks + previews, paging through history), `dsh-client-ui-tool`
  (tool-call tree, **keyed per-tool view slot** `tool.call.toolview`, built-in
  cards for bash/pwsh/read/read_image/write/edit/grep/glob/web/todo/question/PTC
  dispatch), `dsh-client-ui-primitives` (React atoms: controls, icons, markdown +
  math, terminal/read/diff/search/web cards), `dsh-client-ui-trajectory`
  (turn-aware event ledger with interactive timing overview),
  `dsh-client-ui-subagent`, `dsh-client-ui-workflow-run` (durable workflow runs
  as independent conversation nodes), `dsh-client-ui-jobs` (session-header job
  list with expanding output panels), `dsh-client-ui-goal` (composer goal strip),
  `dsh-client-ui-plan` (plan-mode chip), `dsh-client-ui-approval`,
  `dsh-client-ui-user-questions`, `dsh-client-ui-deliverables`,
  `dsh-client-ui-attachment`, `dsh-client-ui-message-feedback`,
  `dsh-client-ui-reference` (`@file`/`@session`), `dsh-client-ui-skill`,
  `dsh-client-ui-schedule`, `dsh-client-ui-model-selection`,
  `dsh-client-ui-commands` + `-input-trigger` (the `/` and `@` menus),
  `dsh-client-ui-theme`, `dsh-client-ui-shortcuts` (browse/customize commands),
  `dsh-client-locale` (zh/en).
- **Sidebars**: right-sidebar docking with file tree, interactive terminal tabs,
  sandboxed HTTP(S) browser tabs (including loopback services), and document
  preview (Markdown/code/image/PDF/Office).
- **Settings**: schema-driven forms (`dsh-settings`, `dsh-config-editor`) with
  pages for General, Models, Agent loop, Subagent, Web search, Shell executor,
  Plugins (+ read-only inventory), Account, Session log.

### 1.4 Platform, protocol, extensibility

- **Typed RPC**: `dsh-typert-protocol` / `-registry` / `-loader` (decorator-based
  generated typed remotes, lazy Zod schema factories), `dsh-api-gateway`
  (dispatch, validation, cancellation, reconnection, forwarded host events),
  `dsh-api-*-controller` (session, settings, job, terminal, workspace, files,
  account), `dsh-client-connection` (browser↔host wire, event-stream reconnect,
  `/api/*` routes).
- **Host**: `dsh-host-webserver` (named routes, upgrades, index transforms,
  single-instance), `dsh-host-frontend-static`, `dsh-host-plugin-inventory`,
  `dsh-host-directory-picker-*` (native/browse/auto), `dsh-host-open-in-app`,
  `dsh-host-product-telemetry-otel`.
- **Other frontends**: `dsh-headless` (one-shot task mode), `dsh-acp` +
  `dsh-acp-app` (Agent Client Protocol server), `dsh-sdk-protocol` +
  `dsh-sdk-jsonrpc-server` + `dsh-sdk-app` + `dsh-sdk-minimal`
  (newline-delimited JSON-RPC SDK for out-of-process clients).
- **Plugins**: `dsh-plugin-manager` (enable/install/remove bundles, from Web
  sidebar or the agent), Cordis plugin loader/includes/groups, `dsh-hmr`
  (coordinated code + config reload), `dsh-invariants` (runtime invariant
  registry), `dsh-scope` (scoped registration).
- **Storage**: `dsh-storage` / `-domain` / `-json`, `dsh-atomic-write`
  (atomic replace + cross-process writer lock), `dsh-spill-local`.
- **Web access**: `dsh-web` (`ctx.web`), `dsh-web-fetch-http` (bounded safe
  fetch), `dsh-web-search-deepseek`.
- **LLM**: `dsh-llm` (provider-neutral streaming service), `dsh-llm-deepseek`
  (Messages/reasoning/image), `-api-key` and `-account` auth, `dsh-llm-pi-ai`
  (multi-provider adapter), `dsh-llm-retry`, `dsh-deepseek-llm-api-extensions`.
- **Ops**: `dsh-otel`, `dsh-launch-environment` (immutable env snapshot with
  provenance), `dsh-http-proxy`, `dsh-webhook` + `-github`.
- **Experimental**: `dsh-experimental-agent-team` (+ profile + 9 tools + Web
  roster/task board), `-auto-review` (per-call review using the current model),
  `-voice-input` / `-speech-to-text-sensevoice` (local CPU STT).

**Footprint observation (our angle):** DSH ships ~287 packages on a Node/Electron
runtime, with Vite-bundled browser plugins, Zod/TypeBox schemas, multiple vendor
SDKs and sqlite/native addons. A single Rust binary with a thin webview and
typed Rust seams can beat it on startup, memory, IPC latency and deployability
while keeping an equally rich plugin surface.

---

### 1.5 Shipped defaults and limits — the numbers we must beat

Source: [`target/dsh-feature-inventory.md`](../target/dsh-feature-inventory.md)
(all 77 runtime packages read; defaults quoted from their specs). "Target" is
what our own design must do, not merely match.

| Area | DSH shipped default | Our target |
| --- | --- | --- |
| Parallel tool calls | `maxParallelToolCalls` 10 (`1` = serial); PTC `maxParallelSubCalls` 10 | same ceiling, plus resource-aware serialization of conflicting calls (R3) |
| Turn budget | **none** — "no built-in turn budget" | explicit turn/token/time budget with visible counters (R3) |
| Goals | one current goal per session, CAS on `(goal_id, revision)`, `defaultMaxGoalRounds` 256, auto-block after 3 identical rounds | same semantics, plus UI, rounds/budget display and blocker history (M3) |
| Subagents | `maxDepth` 1, `maxActiveSubagents` 8 (`ACTIVATION_LIMIT_REACHED`); `spawn` = fresh child, `fork` = balanced completed-turn prefix snapshot | configurable depth/parallelism, documented fork prefix policy, catalog UI (M7) |
| Workflow | JS body only, `maxResultChars` 50 000; PTC `maxTotalAgents` 1000 — caps are "cooperative … not host-enforced security quotas" | caps that are actually enforced (R4) |
| Compaction | `thresholdRatio` 0.8, `retainRatio` 0.16, `headroomTokens` 65 536, image offload, tool-result pruner 8192/4096/1024 | same triggers with user-visible controls (M6) |
| Spill | 0700 temp dir, 30-day cleanup, **no retrieval/delete/search API** | spill with retrieval, retention policy and UI (M6) |
| Approvals | `ask` or `never`; **allow-once only**, no grant store, no revocation; unavailable answerer fails closed | scoped persistent rules, audit list, revocation (M4) |
| Sandbox | presets `workspace-write`(ask) / `danger-full-access`(never); policy default `read-only`; per-call report `full`\|`partial`; unenforceable ⇒ `SANDBOX_UNAVAILABLE` | fail-closed enforcement with per-call mode report (R7) |
| Hooks | command-only; exit 2 blocks; `{"continue": false}` has **no run-level effect** | real control semantics + Codex's 12-event surface (M9) |
| Plan mode | "does not restrict the agent: every tool stays callable" | plan mode with an *optional enforced* restriction (R3) |
| Tool timeouts | cooperative only; `bash`/`read`/`write`/`edit` declare none | real deadlines with safe interruption (R3) |
| Repeat nudge | advisory at [3, 5, 8] identical repeats | advisory, plus surfaced in the trajectory view (M10) |
| Token meter | deterministic log replay, 4-chars-per-token heuristic, no settings | provider-reported usage with heuristic fallback (M6) |
| fs observation | overwrite requires a prior `read` at the unchanged version (`FS_NOT_OBSERVED`/`FS_STALE_VERSION`) | keep this guard (parity, already present) |
| MCP | stdio/streamable-http, `failOnStartupError` false, reconnect 500→30 000 ms ×10, `toolCallTimeoutMs` 60 s | same, plus per-server governance/allow-deny (M8) |
| Skills | merged catalog over 5 ranked roots, description ≤ 500 chars, **loaded bodies uncapped** | capped bodies, authoring UI (M8) |
| Jobs | 10 per owner, in-session settlement, die with the process | panels that stream only the expanded row (M7) |
| Schedule | durable reminders, cron ≥ 1 min, history 200 records; needs the Web session controller | durable scheduler + UI (M11) |
| Presets | "presets are not security sandboxes"; a user override replaces the whole child list | keep composition and security separate, and say so in the UI (M9) |

### 1.6 Web GUI, verified from the shipped bundle

Full brief: [`target/dsh-ui-ux-inventory.md`](../target/dsh-ui-ux-inventory.md).

- **AppFrame**: sidebar 264–420px (default 280, 56px rail collapsed, auto-collapse
  below 1024px; on macOS the collapse hides the column and leaves a 52px top
  strip). The right panel opens at 45% of the viewport, respects the user's px
  preference capped at 70%, and the frame protects a 400px centre by shrinking
  the right panel to 300px, then asking its occupant to close, and only then
  compressing the centre. **Layout state is transient** — reload restores
  defaults — which R6 must fix.
- **Right sidebar**: one docking surface per session, `push` (default) or
  `fullscreen` sharing one content tree so switching does not remount tabs;
  opening below 768px auto-fullscreens, and leaving a narrow fullscreen closes
  the panel (widening never reopens it). Tabs: chat, subagent chat, files,
  terminal, browser, document preview, automation tasks; titles are fixed at open
  time and there is **no content navigation stack** — we add history.
- **Slots**: four kinds `single|list|keyed|chain`, where "declaring a slot is
  claiming it" (undeclared registration, duplicate declaration and a chain
  without `select` all throw at load); Component Factories via
  `registerFactory`/`renderFactorySlot`; the renderer is the single ctx→UI
  boundary and hydrates `[data-dsh-boot]` before the next paint. Our Rust plugin
  seam (M9) should expose the same four shapes without a JS toolchain.
- **Conversation**: incremental assembly for contiguous append/prepend and
  settlement, full rebuild only for replacement windows; work-details modes
  Compact/Standard/Detailed/Verbose; tool calls render as a nested call tree with
  a keyed `tool.call.toolview` per wire tool name; phases preparing/start/result
  with "Preparing content NKB" = `ceil(raw/1024)`; Inspect opens the trajectory.
- **Streaming markdown**: completed blocks freeze, the open fence advances by
  completed lines, and highlighting resumes from saved grammar state; completed
  lines enter fixed-size groups. `DiffBlock` keeps ≤3 context lines per side,
  separates distant changes with `⋯`, stops beyond 256 changed lines, and long
  fences keep their **complete token DOM** (no token virtualization there).
- **Virtualization**: the turn rail mounts only visible marks + overscan + the
  focused mark's neighbours using a fixed pitch **without reading the DOM scroll
  extent**, and hides itself when the transcript is ≤900px wide; Trajectory opens
  with 50 nodes ending at the mount-time tail, then mounts only the visible
  window + overscan with older pages on demand. That is the bar for M1 step 2.
- **Composer**: one rich-text editor per session, reference chips as atomic
  nodes; `+` and `/` open one menu (Add: File/Goal/Plan/Feedback; Commands:
  Compact/Permission/Model/Export); busy-Enter queues or steers; Esc Esc stops
  after `stopSequenceMs` (default 500, configurable 1…2147483646).
- **Shortcuts/settings**: per-device profiles where an omitted profile is
  unbound, override-only persistence that survives plugin unload, rejection of
  duplicate/overlapping defaults, and effective bindings taking priority over
  native actions; settings writes are namespace-scoped and revision-fenced
  ("conflicts preserve their drafts"), with `whileServed` hiding pages whose
  owner was never composed, an 800×800 shell, and a
  Disconnected→Reconnecting→Connected recovery indicator.
- Unverified: `dsh-web-frontend`, `dsh-client-web`, `ui-web` and
  `dsh-client-ui-dockkit` ship without specs in the bundle, so their internals
  are marked unknown rather than inferred.

---

### 1.7 Architecture, protocol and persistence (82 package specs)

Evidence: [`target/dsh-architecture-inventory.md`](../target/dsh-architecture-inventory.md)
(2 682 words; every claim cited to a package README under `target/dsh-readmes/`).

**Process topology.** One launcher (`dsh`) with profiles as ordered patch layers:
bundle patches → profile patch → `$DSH_HOME` patch → CLI overrides, where a patch
*replaces* whole config subtrees (no deep merge; a comments-only patch file fails
boot). Required globals are `agent-loop`, `webserver`, `modules`, `connection`,
`headless-runner`, `acp`, `sdk-jsonrpc-server` — a failed required entry disposes
the whole app, while a failed optional entry only warns. Host and Client are
separate Cordis environments joined by Typert RPC: dynamic plugin host halves run
in `node:vm` (`vmTimeoutMs` 5000, synchronous evaluation only) and browser halves
get React/console/styles/host but no `fetch` and no `setTimeout`.

**Wire.** Unary RPC is JSON; the stream mux is one WebSocket (`/api/remote.mux`)
with a 2 s heartbeat and a 256 KiB uplink inbox (`gateway/uplink-overflow`), and
nothing is replayed across carrier generations — `RemoteJournalStream` supplies
follow-before-page instead. Reconnect is jittered 50–100 % with caps 500 ms→10 s
and a 15 s handshake abort. Auth is a single browser session: a per-process launch
token exchanged for an `HttpOnly`, `SameSite=Strict` cookie (30 days, deliberately
not `Secure` on loopback); request trust is loopback or `trustedHosts` plus Origin
equal to Host and no cross-site fetch metadata, and `--host 0.0.0.0` is
unsupported. Errors are one `RemoteError` discriminated by `<domain>/<reason>`
code rather than `instanceof`; cancellation is descriptor metadata (`signal` as
the final host parameter). HTTP is a bare `node:http` server — default host
exactly `127.0.0.1`, `port: 0` OS-assigned, routes matched exact → longest prefix
→ fallback.

**Sessions and storage.** The append-only event log is the record, model history
is derived, and compaction *hides* superseded entries rather than deleting them.
Durability has three fail-closed barriers: flush the model request before
constructing the adapter stream, flush a top-level tool call before its body runs,
and flush everything preceding a step at `agent/pre-step`. Four format
generations migrate adjacent-edge-only, refusing to guess (v0→v1 finite
normalizations; v1→v2 chunk folding with dense sequence remapping; v2→v3 system
prompts promoted to messages and `code`→`ptc`; v3→v4 flat tool messages and
namespaced sources), with a catalog that fails at module init on any gap. JSONL
persistence is one zstd-checksummed append-only log per session, published by
no-overwrite hard link under a `flock` lease, torn tails truncated. Projections
are synchronous folds backed by a checkpoint cache whose invariant is stated
outright: *stale, but never ahead of committed events*. Whole-log search is a
literal regex scan with no ranking; the SQLite backend adds FTS5 (limits 20/100,
240-character snippets). Session references cap at 3 per message and budget
`max(65536, contextWindow × 4 × referenceContextFraction)` bytes per source.

**Where this leaves us.** These specs set several bars lower than their breadth
suggests, and §2 lists 275 packages documenting limitations: per-record JSON
storage has no cross-process locking and no migration; telemetry and session-log
upload are best-effort with no durable outbox; always-mode retry retries permanent
failures by design; session search without the SQLite backend is unranked; and
there is no plugin seam that avoids a JS toolchain. Our targets are the inverse:
one durable, migratable store with a real outbox; ranked search by default; retry
classes that never retry permanent failures; and a Rust plugin seam (R4).

---

## 2. What DSH itself admits is weak (275 packages)

Full list: [`target/dsh-readmes/LIMITATIONS.txt`](../target/dsh-readmes/LIMITATIONS.txt).
Representative, high-value targets:

| Area | DSH's own limitation | Our requirement |
| --- | --- | --- |
| Layout | panel geometry is transient (reload forgets widths); no scroll anchoring during squeeze reflow; center may fall below 400 px | persist layout per workspace; anchor scroll on reflow; guaranteed minimum center width with graceful panel stacking |
| Approvals | panel exposes transient allow-once/reject only; persistent policy lives elsewhere | one approval surface with allow-once / allow-always (scoped rule) / reject + editable rule, all auditable |
| Agent loop | exclusivity classification is unary (can't compare sibling calls/resources); **no built-in turn budget** | resource-aware scheduling (conflicting writes serialize) + explicit turn/token/time budgets with visible counters |
| Config agents | no per-agent persona field or setup hook for config-declared agents | per-agent persona/tools/budget in config, validated with schema errors at load |
| Preset UI | Web creates/edits no preset; viewer is read-only | authoring UI for presets/personas with live validation and diff-before-save |
| Conversation | transcript reflects only the loaded window; rail previews are card-sized (50/120 chars) | full-transcript navigation with real previews; instant jump to any turn (indexed) |
| Tool views | `web_fetch` links ignore the link-opening setting; first-party views colocated; `run_code` excluded from PTC bindings | consistent link policy everywhere; tool views fully pluggable; sandboxed script tool with host bindings |
| Chat | local submission echoes can disagree with host queue order; animated icon raster softens at high DPR | optimistic echoes reconciled to authoritative order; vector-only iconography |
| Attachments | no zoom or download in lightbox; focus not trapped | zoom/pan/download + proper focus trap |
| Deliverables | inline local images require HTTP(S) page; mention matching exact-path/basename only | inline images in any shell; fuzzy mention matching with disambiguation |

Rule: for every row we ship a test or a documented check, not a promise.

---

## 3. Codex inventory (local source + public docs)

Local checkout: `/Users/gaosg/Projects/codex` (recent master).

### 3.1 Verified from the local tree

- **TUI subsystems** (`codex-rs/tui/src/`): `chatwidget` (turn rendering),
  `bottom_pane` (composer + popups), `history_cell`, `exec_cell`,
  `transcript_view` + `thread_transcript` (`transcript_mode`,
  `transcript_reflow`), `status` + `status_indicator_widget`,
  `resume_picker`, `pager_overlay`, `app_backtrack`, `markdown_render`,
  `inline_visualization`, `notifications`, `keymap` + `keymap_setup`
  (`bindings`, `chords`, `actions`, `capture`, `picker`), `theme_picker`,
  `worktree_browser`, `workspace_command` / `workspace_messages`,
  `terminal_hyperlinks`, `clipboard_paste` / `clipboard_copy` /
  `markdown_copy`, `text_selection` / `vim_search`, `pets`,
  `empty_state_animation`, `onboarding`, `tooltips`, `ide_context`,
  `app_server_session` + `app_server_connection` (frontends share the core via
  app-server), `analytics`, `updates` / `update_prompt`,
  `windows_sandbox`, `external_agent_config_migration`,
  `auto_review_denials`, `backend_banners`, `thread_color`,
  `token_usage`, `tool_output`, `turn_tip`.
- **Docs surface** (`docs/`): `slash_commands.md`, `config.md`,
  `example-config.md`, `sandbox.md`, `execpolicy.md`, `skills.md`,
  `agents_md.md`, `exec.md`, `authentication.md`, `getting-started.md`,
  `install.md`, `contributing.md`.
- **Frontend protocol**: `codex-rs/app-server-protocol`, `app-server`,
  `app-server-client`, `app-server-daemon`, `app-server-transport`.

### 3.2 Verified from public sources (web snapshot 2026-10-09)

Anchors: latest stable `rust-v0.162.0` (2026-10-08), alpha `rust-v0.163.0-alpha.2`.
`developers.openai.com` returned HTTP 403 to our fetchers and `learn.chatgpt.com`
timed out, so documentation claims are snippet-level (**[snippet]** below); every
claim about code, config keys, enums and protocol is read from the tree.

- **Surfaces**: CLI/TUI, non-interactive `codex exec` (JSONL `--json`,
  `--output-schema`, `--ephemeral`, `-o`), IDE extension, desktop `codex app`,
  Codex Cloud (legacy, being deprecated **[snippet]**), and `app-server` — the
  TUI itself is a client of it.
- **Approvals** (`protocol/src/protocol.rs:986`): `UnlessTrusted` ·
  `OnRequest` (default) · `Granular` · `Never`. **Sandbox**: `danger-full-access`
  · `read-only{network_access}` · `external-sandbox` · `workspace-write{writable_roots,
  network_access, exclude_tmpdir_env_var, exclude_slash_tmp}`. Flag presets:
  `--approve-for-me` (= auto-review + on-request + workspace-write) and
  `--dangerously-bypass-approvals-and-sandbox`.
- **Scoped grants exist.** The approval overlay offers "don't ask again for
  commands that start with `{prefix}`", "…for this command in this session",
  "grant these permissions for this session", "grant for this turn with strict
  auto review"; denial can be narrowed ("deny read glob `**/*.env`"). DSH has
  allow-once only — so R3's approval-rule requirement must match Codex, not just
  beat DSH.
- **Config surface**: ~35 top-level keys (`approval_policy`, `approvals_reviewer`,
  `auto_review`, `sandbox_mode`, `default_permissions`, `[permissions]`,
  `mcp_servers`, `model_providers`, `tool_output_token_limit`,
  `model_auto_compact_token_limit`, `history`, `sqlite_home`, `notify`,
  `instructions`, `compact_prompt`, `[features]`, `[profiles.*]` …), plus
  `[tools]`, `[agents]` (`max_concurrent_threads_per_session`, `max_depth`,
  `default_subagent_model`, `job_max_runtime_seconds`, `roles`) and
  `[goals] max_goal_token_budget`; `requirements.toml` carries managed policy
  (`allow_managed_hooks_only`); `-c key=value` overrides anything.
- **Feature flags** (156 variants) prove the roadmap: `CodeMode`, `UnifiedExec`,
  `MemoryTool`, `Chronicle`, `CodexHooks`, `Worktrees`,
  `Collab`/`MultiAgentV2`/`AgentMessageBoard`, `ToolSearch`, `Plugins`,
  `InAppBrowser`/`BrowserUse`/`ComputerUse`, `ImageGeneration`, `SkillSearch`,
  `GuardianApproval`, `Goals`/`TokenBudget`/`ContextManagement`,
  `RealtimeConversation`, `JsRepl`, `Steer`, `CollaborationModes`. **Codex does
  ship goals, memory, subagents and a token budget** — §5 previously marked
  those ❌ and is corrected.
- **Hooks**: 12 events (`pre_tool_use`, `permission_request`, `post_tool_use`,
  `pre_compact`, `post_compact`, `session_start`, `session_end`,
  `user_prompt_submit`, `subagent_start`, `subagent_stop`, `stop`, `interrupt`),
  `hooks.json` with `matcher` + command handler — materially wider than DSH's
  5 (Codex) / 7 (Claude Code) event subsets.
- **Slash commands**: ~57 in enum order, including `memories`, `hooks`,
  `review`, `worktree`, `recap`, `goal`, `agents`, `export`, `raw`, `diff`,
  `statusline`, `theme`, `pets`, `apps`, `rollout`, `test-approval`.
- **Keymap** is context-addressable (`tui.keymap.<context>.<action>` across
  global/chat/composer/editor/vim_*/pager/list/agents/approval); printable keys
  are rejected for non-text actions. **Status line** is an item registry (~30
  items: context-remaining/used, five-hour/weekly limits, cost, `task-progress`
  from `update_plan`).
- **Rendering**: fullscreen transcript by default; streaming has explicit
  code-fence and table holdback with `commit_tick`; markdown render caching;
  `thread_transcript` groups history into collapsible activity pages; `raw`
  scrollback mode; ~32 embedded themes plus custom `.tmTheme`.
- **Architecture**: SubmissionQueue/EventQueue engine, **at most one Task at a
  time** (parallel work = one engine per thread), `TurnComplete` carries a
  `response_id` bookmark used to fork from an earlier point; transports include
  channels, IPC, stdio, TCP, HTTP2, gRPC with NDJSON when unframed.
  **app-server**: 170 client requests, 9 server→client requests, 84
  notifications, deliberately *not* JSON-RPC 2.0; experimental methods gated by
  `initialize.capabilities.experimentalApi`.
- **Sandboxing**: macOS Seatbelt (workspace-write keeps `.git`/`.codex`
  read-only), Linux bubblewrap with bundled fallback (Landlock only via the
  legacy split-policy path), WSL2 supported / WSL1 not, Windows restricted-token
  backends that **fail closed** on unenforceable policy; `apply_patch` is a
  virtual CLI.

**Codex's own weaknesses (open issues) — our targets:**

| Issue (github.com/openai/codex/issues) | Signal | Our requirement |
| --- | --- | --- |
| macOS `syspolicyd`/`trustd` runaway #25719 | 454 👍 | idle cost near zero: no polling loops, no animation work while thinking |
| "high GPU usage … due to tiny useless animation" #16857 | | animation must be compositor-only and switchable off |
| Windows freezes #20214/#23198, VS Code slowdown #3022 | | all I/O off the UI thread; bounded per-frame work |
| slow thread switching #11011 | | session switch paints from warm cache (already done) |
| no intraline diff #35496, collapsed diffs #47390 | | word-level intra-line diff, hunks expandable per hunk |
| no 1M context #19464 (241 👍), no auto-compaction control #4106 (127 👍) | | explicit budget/compaction controls + live meter |
| no IDE-grade diff/approval #2998 (236 👍), 60 s auto-resolve #28969 (218 👍) | | diff review surface + timed questions that keep the model working |
| `@` search blind to gitignored dirs #2952 | | ignore-aware index with explicit opt-in |
| MCP OAuth tokens not refreshed #17265 | | refresh + expiry surfaced in the MCP panel |
| image artifacts not inline #29451, no voice #14630 | | inline images (have), voice optional, never required |

---

## 4. Requirements

Priority: **P0** = parity with both competitors; **P1** = clear beat; **P2** =
differentiator for AGI-class work.

### R1 Architecture (P0)

1. `crates/gui` stays **thin**: all agent behaviour lives in `crates/agent` /
   `crates/react`; the GUI owns presentation, session plumbing and approvals.
2. **Single Rust binary** + system webview; no Node runtime, no bundled
   interpreter for the GUI path.
3. **Typed protocol**: one versioned event/command enum set shared by host and
   client (serde), with forward-compatible additions (unknown variants ignored).
4. Host/agent/UI split suitable for extra frontends (TUI, headless, SDK/ACP-like
   stdio JSON-RPC) reusing the same core — Codex does this with app-server; DSH
   with Typert/gateway/ACP/SDK. We need at least headless + stdio.
5. Session store is **event-sourced with checksummed, versioned records** and
   explicit migrations from day one.

### R2 Performance (P0/P1)

1. Streaming repaint budget: coalesced per animation frame (**done**), plus
   **incremental transcript patching** — never rebuild the whole transcript per
   frame (current `app.js` rebuilds everything; this is the top perf debt).
2. Virtualized transcript: constant DOM for 10k-message sessions; scroll anchor
   preserved on reflow and on panel resize.
3. Bounded memory: tool outputs capped for display with spill-to-file (DSH's
   `spill-policy` equivalent) and reference handles instead of copy.
4. Startup to interactive < 300 ms on a warm cache; session list from an index,
   not by parsing every log.
5. Measured, published numbers per milestone (cold start, first token, repaint
   time at 1k/10k messages, memory at 10k messages).

### R3 Functional parity and beyond

Must match: agent loop with parallel tools; plan mode + todos; goals with
continuation rounds; subagents (spawn + fork) with control tools; workflow
orchestration; skills; MCP (servers + on-demand resources); compaction +
token metering + output retention; hooks compatible with Codex/Claude Code
configs; background jobs; durable schedule/reminders; approvals with
persistent rules; user questions; sandbox policies; session search (FTS),
titles, turn outline, stats, export; deliverables (`present`); attachments and
images; provider config with failover (**done**); slash commands; shortcuts.

Beyond (P1/P2): resource-aware parallel scheduling; explicit turn/token/time
budgets; per-agent personas/presets in config **and UI**; trajectory/timeline
inspection with timing; deterministic replay of a session; multi-agent team
roster + shared task board; auto-review before dangerous calls; provider-side
prompt-cache awareness; evaluator/critic loop; long-lived memory with UI.

### R4 Extensibility (P0/P1)

1. Plugin seam in Rust: register tools, UI panels, slash commands, providers,
   approvals, skills — without forking the GUI.
2. Declarative presets (YAML/TOML) selecting tools/prompt sections/skills per
   agent, multiple presets per process, validated with actionable errors.
3. Skills: filesystem markdown skills (`SKILL.md` + frontmatter) discoverable,
   invocable, and authorable from the UI.
4. MCP: catalog + custom servers, per-session tool activation, OAuth/API-key
   credentials, governance (allow/deny per tool), resources on demand.
5. Hooks: Codex + Claude Code compatible hook configs, with a documented
   decision protocol (continue/abort/replace).
6. Client plugin story that does not require a JS build toolchain for the
   common case (contrast: DSH's Vite + HMR plugin graph).

### R5 AGI support (P2)

1. Long-lived memory/knowledge: `MemoryStore` exists agent-side; needs UI
   (browse, edit, forget, scope), automatic recall surfacing, and provenance.
2. Goal-driven autonomy: goals, continuation rounds, budgets, and visible
   progress/blocked reporting (**agent-side done**; UI + persistence polish needed).
3. Self-verification: `/verify`-style probes, reproducible evidence capture,
   and session replay.
4. Tool/harness self-improvement: the agent can inspect and configure its own
   composition (DSH ships `dsh-tool-cordis`; we need the equivalent).
5. Durable workspaces: multiple workspaces, per-workspace instructions and
   memory, session↔workspace binding.

### R6 UI/UX (P0/P1)

1. Three-column shell: sessions sidebar (collapsible), transcript, docked right
   panel (files / terminal / browser / document preview), all resizable and
   persisted per workspace.
2. Transcript: markdown + code highlighting + math + tables; reasoning blocks;
   tool cards keyed by tool name; unified-diff rendering (**done** for
   approvals and tool output); terminal output cards; images with gallery.
3. Interaction: `/` command palette (**partially done**), `@` references
   (files, sessions, skills), attachments, model picker, plan chip, goal strip,
   permission picker, jobs panel, schedule page, subagent catalog, workflow run
   nodes, trajectory view.
4. Approvals: diff-previewed allow-once / allow-always(scoped) / reject, with
   keyboard-first operation.
5. Keyboard: full shortcut map, discoverable and customizable, vim-style
   transcript search.
6. Theming: light/dark + token system + content font scaling; i18n zh/en from
   the start (no hard-coded strings).
7. Accessibility: focus traps in modals, aria state on disclosures, keyboard
   reachable everything.

### R7 Quality and evidence (P0)

Per milestone: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`,
`cargo test --workspace --exclude nbos --exclude jsbos`, `node --check` on the
UI bundle, GUI build, and a live verification against the local broker. Baselines
must never regress: tests **600/0 (63 suites)** — re-measure with the command above
rather than trusting this number, which has been stale twice — live verify 5/5, and
the four GUI guards, whose counts are also measured rather than remembered:
`ui_selftest.sh` **94** transcript checks, `shell_selftest.sh` **48** shell checks,
`layout_selftest.sh` **0** failures across five widths (1440/900/560/500 measured; 420 unreachable — Chrome clamps to ~500 px); `markup_selftest.sh` **25** structural checks (the only guard that reads the shipped `index.html`); `wiring_selftest.sh` **47/47** commands reachable, no phantoms; `page_selftest.sh` **8** checks that execute the shipped page itself.sh` `zip_selftest.sh` **10** checks in which an **independent** reader (Python's zipfile) accepts an archive the exporter actually wrote.
**45/45** commands reachable with no phantom calls.

### R8 Packaging (P1)

Signed macOS app bundle, single install artifact, no external runtime;
`bos-gui` CLI for headless/verification modes; upgrade path for session format.

---

## 5. Parity matrix (`✅ have · ⚠️ partial · ❌ missing`)

| Capability | Codex | DSH | bos/gui today | Target |
| --- | --- | --- | --- | --- |
| Streaming chat + reasoning | ✅ | ✅ | ✅ | keep |
| Incremental transcript rendering | ✅ (render cache) | ⚠️ windowed paging | ⚠️ R22 keyed patch | **M1: virtualize, flat repaint** |
| Tool timeline cards w/ ms + diff | ✅ | ✅ | ✅ (R21) | exceed: nested call tree + intra-line diff |
| Approvals + scoped persistent rules | ✅ prefix/session/turn | ⚠️ allow-once only | ⚠️ allow/deny | M4 |
| Sandbox enforcement (fail-closed) | ✅ Seatbelt/bwrap/ACL | ⚠️ full\|partial only | ⚠️ bash policy only | M4/R7 |
| Plan mode (+ optional enforced limit) | ✅ | ⚠️ non-restricting | ⚠️ display only | M3 |
| Todos | ✅ | ✅ (whole-list replace) | ⚠️ plan only | M3 |
| Goals + continuation rounds | ✅ (`Goals`, budget key) | ✅ (256 rounds, CAS) | ⚠️ agent-side only | M3 |
| Turn/token/time budget | ⚠️ goal + compact limits | ❌ none | ⚠️ context budget | M3/M6 |
| Subagents + control tools | ✅ `[agents]`, roles, board | ✅ spawn/fork, depth 1 | ❌ UI | M7 |
| Multi-agent teams | ✅ Collab/MultiAgentV2 | ✅ experimental (16) | ❌ | P2 |
| Workflow orchestration | ⚠️ code-mode | ✅ JS + PTC + ralph | ❌ UI | P2 |
| Hooks (Codex/Claude compat) | ✅ 12 events | ⚠️ 5 / 7 events | ❌ | M9 |
| Plugin system + marketplace | ✅ plugin crate + marketplace | ⚠️ JS/Cordis clients | ❌ | **M9: Rust seam** |
| Skills (discover/invoke/author) | ✅ SKILL.md + roots | ✅ ranked roots, uncapped | ⚠️ run-only | M8 |
| MCP servers + resources | ✅ + OAuth login | ✅ + resource tools | ⚠️ servers only | M8 |
| Compaction + token meter | ✅ (limits exposed) | ✅ 0.8/0.16/64Ki | ⚠️ auto-compact | M6 |
| Spill / oversized output policy | ⚠️ output token limit | ✅ spill store (no retrieval) | ❌ | M6 |
| Session search (FTS) | ✅ `thread/search` | ✅ SQLite FTS5 | ⚠️ in-session only | M5 |
| Session export/import | ✅ export + rollout migrate | ✅ ZIP | ⚠️ markdown export | M5 |
| Fork / replay / trajectory | ✅ response-id fork | ✅ trajectory window | ❌ | M10 |
| Terminal panel (persistent shell) | ✅ persistent bash/pwsh | ✅ owner-scoped shells | ✅ PTY session per workspace (rev 80) | M2 |
| File sidebar / tree | ✅ worktree browser | ✅ files tab | ❌ | M2 |
| Right dock: browser/doc preview | ⚠️ InAppBrowser flag | ✅ browser + doc preview | ❌ | P1 |
| Background jobs UI | ⚠️ `ps` / jobs | ✅ roster + stream panels | ❌ | M7 |
| Schedule / reminders UI | ❌ | ✅ automation tasks | ❌ | P2 |
| Inline images / gallery | ⚠️ artifacts not inline | ✅ durable images + lightbox | ❌ | P1 |
| Voice input | ✅ RealtimeConversation | ✅ experimental, off | ❌ | P2 |
| Browser / computer use | ✅ feature flags | ❌ | ❌ | P2 |
| Theming (light/dark, fonts) | ✅ ~32 themes + tmTheme | ✅ tokens, 8 sheets | ⚠️ dark only | M13 |
| i18n zh/en | ⚠️ | ✅ + browser fallback | ❌ | M13 |
| Keyboard customization | ✅ context keymap | ✅ device profiles | ❌ | M13 |
| Status-line item registry | ✅ (~30 items) | ⚠️ chips only | ⚠️ fixed chips | M13 |
| Headless / stdio SDK | ✅ exec + app-server | ✅ headless/ACP/SDK | ⚠️ verify bin | M12 |
| Multi-provider failover | ⚠️ | ⚠️ retry only | ✅ | keep |
| Memory / knowledge UI | ✅ MemoryTool + memories crate | ❌ (third-party MCP) | ⚠️ store, no UI | **M11 differentiator** |

Codex verdicts cite §3.2, DSH verdicts cite §1.5/§1.6; rows marked ⚠️ are
re-verified in the milestone that owns them, and the "today" column is
re-measured by [`crates/gui/tests/ui_selftest.sh`](../crates/gui/tests/ui_selftest.sh).

---

## 6. Milestones

Each milestone is independently shippable, gated by §R7, and ends with evidence
recorded in the round log.

| # | Milestone | Acceptance criteria (evidence) | State |
| --- | --- | --- | --- |
| M0 | Foundation: thin GUI, streaming, sessions, approvals, tool timeline, failover, context budget | 555 tests, live verify 5/5 incl. `ToolResult` end-to-end (plan-tool probe) | ✅ done (R1–R21) |
| M1 | **Incremental + virtualized transcript** | step 1 ✅ keyed patch renderer + headless harness ([`ui_selftest.sh`](../crates/gui/tests/ui_selftest.sh)): 5 000-row session repaints in **2.0 ms** vs **17.5 ms** for the old wipe-and-rebuild (9×), exactly one row rebuilt per frame, append/delete/reorder/tool-card/diff/non-diff checks green. step 2 ✅ (rev 74): the tail window (200 rows) existed already, but with **no spacer**, so the scrollbar described only the drawn rows and anything older was reachable only by pressing "Load earlier". A spacer now stands in for the un-drawn rows, scrolling near the top asks for one more window, and the spacer's **exact** change is credited to the scroll offset so the reader does not move. Measured at N=10 000: repaint **0.103 ms** windowed vs **2.5 ms** with the window open (**24.3×**), appends **0.151 ms** vs **4.925 ms**, rows drawn **200** vs 10 000. The spacer height is an estimate sampled from rendered rows, so the bar is approximate while the window is partial. |
| M2 | Shell: resizable/persisted 3-column, file sidebar, persistent terminal panel | layout persistence per workspace; terminal survives reload; tests + smoke | ✅ **file sidebar** (step 1, rev 77): the workspace on disk as a lazy tree under the session list — one directory per click, workspace-confined (canonicalized; absolute paths, `..` and symlinks leaving the root refused), `.git`/`target`/`node_modules` and dot-entries skipped, capped at 1 000 rows with the cut reported, directories first then names case-insensitively, sizes shown. Open folders persist per workspace root and are **re-fetched** on the next launch, not snapshotted; a folder that refuses to open keeps the reason on its row and retries on the next click; names are painted with `textContent` only. Evidence: 11 Rust tests, 104 harness checks, wiring 48/48, smoke green. ✅ **run panel** (step 2, rev 78): the persistent command-output panel the terminal slot called for — a `sh -c` run in the workspace root streaming stdout/stderr line-by-line into a bounded panel (`textContent` only), one receipt line per run (`[exit 0 · 12 lines · 1.2s]`), output surviving a reload per workspace root. It is **not a terminal**: no PTY is available in this build environment, so the panel says so out loud and a PTY upgrade stays a backend swap (§12.4). Evidence: 8 Rust tests (validation, clamping, streams, exit codes, budget kill, deadline kill), 113 harness checks, wiring 49/49, smoke green. ✅ **per-workspace geometry** (step 3, rev 79): each workspace keeps its own column widths, rail state and document-panel state, swapped live through one Tauri-free scope seam (`Shell.setLayoutScope`), with the unscoped key remaining the default scope so nothing saved earlier is lost (§12). Evidence: shell harness checks 48 → 54, including both swap directions and the same-scope no-op. ✅ **PTY terminal session** (step 4, rev 80): `portable-pty` landed in the offline cache, so the run panel gained the real terminal the slot always called for — one live shell per workspace root (`bos-pty-<root>`), owned by a `PtyRegistry` held in the Tauri app state so a **webview reload re-attaches to the same live process** with its 64 KiB tail replayed instead of spawning a second shell. The backend is honest about the seam: input goes to the PTY (the shell echoes, the panel does not), output is streamed through a streaming ANSI filter (CSI/OSC/2-byte escapes removed, incomplete sequences held to 64 bytes then failed open) and rendered `textContent`-only; a session reports `fresh: false` and its tail on re-attach, resize clamps to the shell grid (20–500 cols, 5–200 rows) and is measured off the panel's own font, and the panel's honesty note now swaps with the mode ("plain text, not a terminal" ↔ "interactive shell · ANSI stripped") while the blob gained `mode` (old shapes still load, garbage degrades to run mode). The run mode is untouched: one-shot `sh -c` runs with receipts still work, and switching modes never kills a live session. Evidence: 7 pty tests (3 spawning tests stand down loudly where the sandbox refuses `/dev/ptmx` and run in full on a normal machine), 125 ui checks (split-line flushing, mode note swap, chip labels, blob modes), wiring 53/53, smoke green. |
| M3 | Plan/todo + goal surfaces (UI for goals, budgets, blocked/complete) | goal visible live, editable, rounds/budget counters; todo list repaint from session log | |
| M4 | Approvals v2: allow-always with scoped rules, audit list, keyboard-first | rule matching tests; audit entries in session log | |
| M5 | Session intelligence: FTS search across sessions, titles, turn outline, stats, ZIP export | searchable index built incrementally; export round-trips | ✅ **search** (a live scan, with the hot path measured ~15× faster than the original), **turn outline + counts** (rev 73) and **ZIP chat export** (rev 75). An **FTS index was designed, implemented, measured and rejected** (rev 76), and the scan stayed. That revision reported the index **1.34×–3.52× slower than the scan it replaces** — that ratio is **withdrawn as unsupported**: the "scan" it compared against was a simplified stand-in that skipped snippet extraction, not the real search path. What stands from the round, measured on one corpus (300 sessions): at body 22 528 the indexed **miss** path cost 15.18 ms, the real `search` path 7.24 ms, and the scan 2.21 ms — so the index lost to the scan on this data on the evidence that exists. The unexplained cost inside the indexed path was never root-caused. |
| M6 | Context: token meter UI, spill/retention policy, compaction controls | spill files retrievable; meter matches provider usage within tolerance | |
| M7 | Subagents + jobs: catalog, live output panels, control actions | delegate/interrupt/list from UI; job panels stream bounded output | |
| M8 | Skills + MCP v2: authoring/install, resources on demand, per-session activation, governance | skill author→invoke round trip; MCP allow/deny enforced | |
| M9 | Extensibility: Rust plugin seam + declarative presets + hooks (Codex/Claude) | sample plugin registers tool+panel+command; preset validation errors actionable | |
| M10 | Trajectory + replay: timeline with timing, deterministic session replay | replay reproduces transcript + metrics | |
| M11 | AGI surfaces: memory/knowledge UI, self-inspection tool, evaluator/critic, workspace scoping | memory CRUD + recall surfacing; agent can read its own composition | |
| M12 | Multi-frontend: headless + stdio JSON-RPC SDK, scriptable from tests | external client drives a session over stdio | |
| M13 | Polish: i18n zh/en, theming tokens, shortcuts UI, a11y audit | no hard-coded UI strings; a11y checklist | |
| M14 | Packaging: signed bundle, upgrade/migration path, docs | install artifact + migration test | |

---

## 7. Immediate next step

**Part II Phase 1 (Epic A) is underway.** M1 step 2 has landed since this was written (rev 74: the tail window gained a spacer, scroll-back and an exact no-jump credit), so the ordering above is history rather than a gate.

Landed this round, verified by
[`crates/gui/tests/shell_selftest.html`](../crates/gui/tests/shell_selftest.html)
(47/47 green): the three-pane shell with drag and keyboard dividers, clamping
(sidebar 264–420, default 280; document pane 30–70 % of the window with a
protected 400 px centre), persisted layout, a sidebar rail and pane toggles in
the centre toolbar, a semantic token layer with a real light theme plus
system-follow, content font size, work-details and coding-view presentation
switches, a non-regressing task-status machine, and the document panel with tabs,
path and inert text rendering.

Remaining for Phase 1 (GUI-01–03):

1. Feed the app's real context into the document panel's Start page (workspace,
   model, session, active task) through `Shell.setContext`.
2. Route opened files into the panel (tool-call outputs, mentions, file list) plus
   a refresh/error path; markdown rendering stays with the app's renderer.
3. Close what the harness cannot reach: **sidebar navigation landed** (Plugins &
   skills and MCP servers open the settings panel that owns the data; Automation is
   explicitly disabled until jobs ship in M7), and **session rows now show a
   relative age** with the exact stamp on hover. Still open: workspace grouping in
   the sidebar, and a narrow-window visual pass.

Then M1 step 2 with the measured 500 / 3 000 / 10 000 acceptance runs. Step 2a
has landed: tail-follow is event-driven, so a streaming repaint performs **zero**
forced layout reads per frame (previously two) — verified by the harness's new
`layoutReadsPerStreamFrame` metric, with a detached-reader control. Repaint is
Step 2b has landed: a **validation cursor** (rows below it are reused without a
signature) plus a **trusted-prefix shape check** (same array, length, tail object,
one cache entry and one DOM row per message) mean a streaming frame computes **one**
signature and walks **one** row, and a frame with nothing marked returns after a
single scroll write. Measured — streaming 0.17 / 0.58 / 3.03 ms and idle
0.002 / 0.022 / 0.072 ms at 500 / 3 000 / 10 000 rows, against a full rebuild of
2.1 / 12.3 / 38.5 ms. The residual growth in the streaming column is no longer our
own work, so the next hypothesis to test is the browser's own layout and scroll
extent over a 10k-row tree: that makes windowing the remaining lever, and it must
ship **behind** the current contract — search and export already read the model
rather than the DOM, so a windowed transcript needs an explicit "load earlier"
affordance and a rerun of every structural assertion, not a silent shrink.

---

## 8. Revision log

| Rev | Date | Change |
| --- | --- | --- |
| 1 | 2026-10-09 | Initial: DSH bundle inventory (287 packages), DSH limitations, Codex local tree, requirements R1–R8, milestones M0–M14, parity matrix. |
| 2 | 2026-10-09 | Codex public research folded into §3.2 (which corrected the goals / memory / subagents / hooks / plugins verdicts — Codex ships all five); DSH runtime defaults added as §1.5 and bundle-verified web GUI as §1.6; parity matrix expanded to 37 rows with cited verdicts; M1 step 1 landed with measured evidence and a committed harness. |
| 3 | 2026-10-09 | Merged the screenshot-derived workbench spec (`docs/bos-gui-requirements.md` v1.0) as Part II §10–§23 with a reconciliation note: event contract mapped onto existing events, geometry unioned with §1.6, persistence split adopted; Part II Phase 1 (shell + design tokens) set as the next structural milestone. |
| 4 | 2026-10-09 | Fourth research digest landed as §1.7 (architecture / wire protocol / session model / storage, 82 package specs). Part II Phase 1 started: `ui/shell.js` (Tauri-free shell) + semantic tokens + light/system theming, with a 47-check shell harness; both harnesses green (16/16 + 47/47). Three real defects found and fixed on the way: keyboard resizes never persisted, a storage-less environment threw instead of degrading, and load-time `localStorage` access lacked a fallback. |
| 5 | 2026-10-09 | Tool results and `@`-mentioned files now open in the document panel, backed by a bounded, workspace-confined `read_file` command (escapes, symlinks leaving the workspace, binary and non-UTF-8 payloads refused; four unit tests). Tail-follow became event-driven: a streaming repaint now performs **0** forced layout reads per frame instead of 2, measured by the harness at 500 / 3 000 / 10 000 rows (0.28 / 1.23 / 6.73 ms) — which also shows repaint is still O(n), so windowing remains the outstanding half of M1 step 2. |
| 6 | 2026-10-09 | Validation cursor (M1 step 2b, first half): rows below the cursor are reused without a signature, and a presentation-epoch change (idle / streaming / editing) invalidates every row at once. A streaming frame now computes 4 signatures instead of one per message — 0.28 → 0.20 ms at 500 rows, 1.23 → 0.81 at 3 000, 6.73 → 5.38 at 10 000. The remaining linear scan and DOM walk is scoped in §7 as the next unit of work. |
| 7 | 2026-10-09 | Tail-only reconciliation: a trusted-prefix shape check lets the frame walk start at the cursor, and an unmarked frame returns after one scroll write. Streaming frames measure 0.17 / 0.58 / 3.03 ms and idle frames 0.002 / 0.022 / 0.072 ms at 500 / 3 000 / 10 000 rows (full rebuild: 2.1 / 12.3 / 38.5 ms). Residual growth is attributed to the browser's layout over a full row tree, which scopes windowing as the next lever. |
| 8 | 2026-10-09 | Sidebar navigation (Plugins & skills / MCP servers open the owning settings panel; Automation explicitly disabled until M7) and relative session ages with the exact stamp on hover, hidden in the rail. Seven new harness checks pin the age formatter and the navigation wiring. |
| 9 | 2026-10-09 | Workspace model specified (§24): the workspace is the aggregate root and owns its sessions, LLM bindings, tools, skills, MCP servers, memory and approval policy; global settings shrink to UI state, shortcuts, the config-discovered LLM profile catalogue and the default workspace. Lossless migration and the left-panel management surface are specified with the schema. |
| 10 | 2026-10-09 | Workspace authoring specified (§24.7–§24.9): root chosen with a folder picker (typed fallback) and validated, per-workspace capability selection as opt-out deny-lists for tools and skills plus per-server MCP switches, and presets as a global catalogue applied-and-diverged rather than followed. Registration seam recorded as `caps::build_agent`. |
| 11 | 2026-10-09 | Workspace schema landed in the GUI crate (`settings.rs`): `Workspace` with root, LLM binding, tool/skill deny-lists, MCP switches, approval, budget, skills, instructions and memory; `Settings.workspaces` + `active_workspace`; `Settings::migrate` copies today's globals into a `default` workspace on both load paths (never moves them) and repairs a dangling active id. Migration is asserted to preserve capability state (acceptance 8) and to be idempotent. Sessions do not carry a workspace yet — that is step 2. |
| 12 | 2026-10-09 | Sessions join a workspace: `SessionRecord.workspace` (serde-defaulted so pre-workspace files load), stamped at creation from the active workspace, inherited by a branch, and exposed on every summary with an empty stored value resolving to the workspace in use. Three tests cover legacy-file loading, migration and the fallback. The left panel is not grouped yet, and an agent is not yet resolved against a workspace. |
| 13 | 2026-10-09 | Resolution landed: `Settings::effective()` overlays the active workspace on the globals (owned values replace, empty strings inherit, a workspace may name a profile) and both production agent builds plus the capability panel now use it, so model, workspace root, tools, approval and budget come from the workspace. `caps::build_agent` honours the tool deny-list by the names the model sees (`bash`, `read_file`, `write_file`, `list_dir`, `plan`); a new test pins overlay-and-inherit. Not enforced yet: `disabled_skills` (the skills API registers a whole directory). |

## 9. Evidence status

**Landed (under `target/`, which is ignored — regenerate from the bundle):**

- `target/dsh-readmes/` — 288 package specs + `INDEX.txt` +
  `LIMITATIONS.txt` (1 618 lines of DSH's own stated limitations).
- `target/dsh-feature-inventory.md` — agent runtime, all 77 packages, cited defaults (§1.5).
- `target/dsh-ui-ux-inventory.md` — web GUI shell/rendering/interaction/design system (§1.6).
- `target/dsh-architecture-inventory.md` — process topology, wire protocol,
  session model, extensibility, LLM/storage/web/ops constraints, cited per
  package (§1.7).
- Codex: local tree + web brief (§3.2), including 12-event hooks, 170-request
  app-server surface and the open-issue target list.
- Measurements from `crates/gui/tests/ui_selftest.sh`, which runs the *shipped*
  frontend in headless Chrome: transcript repaint 2.0 ms vs 17.5 ms for a full
  rebuild at 5 000 rows (10× at 3 000). The shell-contract checks are a separate
  script, `shell_selftest.sh` — this line attributed them to `ui_selftest.sh` and
  carried a stale count; they are 48.

**Outstanding:**

- Re-check the §2 rows the architecture digest touches (sessions, storage,
  gateway, typert RPC, plugin inventory) now that §1.7 exists; the digest's own
  Unknowns paragraph lists what 82 READMEs cannot settle — the full
  `SessionEvent` vocabulary, `InvariantError` implementations, Desktop host
  internals, SQLite backend details and third-party search providers (`Exa`,
  `Tavily`, referenced by name only).

---

# Part II — Workbench & settings specification

**Source:** `docs/bos-gui-requirements.md` v1.0 (screenshot-derived), merged
here verbatim and renumbered to §10–§23 (the source file was folded in and then
removed; this Part is authoritative). Its own evidence boundary still holds: it describes visible UI and
reasonable interaction requirements inferred from three screenshots, and none of
its proposed APIs may be assumed to exist.

**Merge reconciliation (Part I ↔ Part II):**

- **Unified sequence.** Part II's Phase 0 (repository reconnaissance) is
  effectively complete: §1–§3 are the verified map of both competitors, and §6
  M0 records the shipped foundation. The in-flight performance debt (M1 step 2,
  virtualized transcript) stays part of Epic B; Part II **Phase 1 = Epic A
  (GUI-01–03, the shell and design tokens) is the next structural milestone**.
- **Event contract (§18) maps onto existing events, not new ones.** `TaskStarted`
  → stream start, `TaskProgress` → `text`/`reasoning`/`tool`/`tool_result`,
  `ApprovalRequired` → the approval request, `TaskCompleted` → stream finished,
  `TaskFailed` → `error`, `SessionSelected` → session switch, `SettingsChanged` /
  `DocumentChanged` → settings/config and file events. Out-of-order protection
  already exists to build on: every stream record carries a monotonic `gen`, and
  `AgentEvent::ToolResult` carries the duration. The genuine gap Part II adds is
  an explicit `TaskStatus` state machine (GUI-05) rather than more event types.
- **Geometry.** Part II suggests sidebar 280–340px and a 400–520px center;
  §1.6 measured DSH at 264–420px (default 280), a right panel at 45% capped at
  70%, and a protected 400px center. The union is: sidebar default 280
  (resizable 264–420), center never below 400px, right panel 45% capped at 70%.
- **Persistence split.** Part II §18.4 separates UI preferences / application
  behavior / server-owned account data — adopted as-is; layout (widths,
  collapsed state, dock) is UI-local and persists separately from configured
  settings.

**Project:** `open1s/bos` — `crates/gui`  
**Document type:** UI Requirements Specification and Implementation Plan  
**Version:** 1.0  
**Status:** Draft based on three supplied screenshots  
**Scope:** Desktop workbench, settings center, account settings, general settings, component architecture, user stories, implementation and acceptance criteria

> **Evidence boundary:** This document captures visible UI elements and reasonable interaction requirements inferred from the screenshots. It does not claim that the existing BOS repository has been inspected, that proposed APIs already exist, or that the proposed work has been implemented. Validate framework-specific details against the current code before implementation.

## 10. Product goals

Build a desktop GUI for AI coding agents and long-running autonomous tasks. The UI should provide a three-panel workbench and a consistent settings center, supporting multiple sessions and workspaces, agent execution status, Markdown/document viewing, model selection, and user preferences.

Goals:

- Provide a high-density, low-distraction AI agent workspace.
- Support coding, long-running tasks, background jobs, and multi-session workflows.
- Keep layout, interaction, and theming consistent across screens.
- Separate UI components from agent execution, model services, session management, and persistence.

## 11. Information architecture

### 11.1 Main workbench

| Area | Contents | Responsibility |
| 14 | 2026-10-09 | The per-server MCP switch is enforced: `attach_mcp_tools` takes the effective settings and skips handshakes the workspace did not opt into, and takes the approval policy from the same place. The rule is a named function (`enabled_mcp_servers`) with its own test, including the "nothing configured means nothing attaches" case. |
|---|---|---|
| 15 | 2026-10-09 | The left panel groups by workspace: a `groupSessions` rule (pure, tested) turns summaries into workspace groups carrying name, root and bound model, and a header row renders each group with the root on hover and the model as a chip; a chat written before workspaces existed joins the workspace in use rather than a nameless bucket. The workspace list comes from the settings the shell already fetches. Switching workspace, adding/removing/renaming workspaces and the folder picker are still open. |
| Left navigation | Brand, New Session, Plugins, Automation tasks, MCP, Workspaces, sessions, user profile | Navigation and resource management |
| 16 | 2026-10-09 | Switching workspace is real: `set_active_workspace` persists the choice, rejects an unknown id, denies the old agents' pending approvals, bumps the settings generation and clears the agent cache so the next build resolves the new workspace. Workspace headers are the switcher (root + model in the tooltip, active one marked). Exposed gap: the settings dialog still writes the **globals**, which the active workspace's overlay shadows — so the dialog must edit the active workspace instead (next), or a user will edit values the workspace overrides and see nothing change. |
| Center workspace | Chat, Trajectory, agent output, task progress, tool calls, composer | Task interaction and execution monitoring |
| 17 | 2026-10-09 | The coherence gap is closed: `get_settings` returns the **effective** settings (what will actually run), so the dialog can no longer display a value the active workspace overrides, and `save_settings` writes the edited values into the active workspace as well as the globals that seed new ones. `Workspace` gains `system_prompt`, `temperature: Option<f32>` (0.0 is a legitimate value, so it cannot use the empty-means-inherit rule) and `reasoning_effort`, all overlaid by `effective()`. A workspace named after its folder follows it when it moves; the tool and skill deny-lists are deliberately untouched by the dialog, since overwriting them would silently re-enable what the user switched off. |
| Right content panel | Start, file tabs, file path, Markdown/code view | Document viewing and editing |
| 18 | 2026-10-09 | Workspace management exists at the command layer: `create_workspace` (seeded from the current defaults, so a new workspace behaves like today's installation), `update_workspace` (rename or move the root; sessions stay put because their approvals and tool calls happened in the old folder) and `remove_workspace` (the last workspace cannot be removed; its chats are deleted with it or re-homed to the successor, the caller's explicit choice per §24.6). Roots are validated the way file reads are: must exist, must be a directory, stored canonical — so `~`, `..` and symlinks cannot turn one folder into two workspaces; empty means the process working directory. Test pins the canonicalization (macOS `/tmp` is itself a symlink). Still open: the panel UI for these, the folder picker, per-workspace capability lists and presets. |

### 11.2 Settings center

Use a large modal window with category navigation on the left and a settings panel on the right.

Categories shown in the reference:

- Account
- General
- Models
- Pets
- Built-in plugins
- Archived sessions
- Agent presets
- Plugin Market
- HyperFrames
- Remotion

Implement Account and General first. Keep the other categories extensible; unimplemented pages must show an explicit unavailable/placeholder state rather than fake functionality.

## 12. Main workbench requirements

**Column geometry (per workspace, rev 79)**

The three-column shell is resizable, and its geometry is **stored per
workspace root**. Each workspace keeps its own column widths, sidebar rail
state and document-panel state (`bos.layout.v1:<root>`; the unscoped
`bos.layout.v1` remains the "default" scope, so layouts saved before scoping
existed are still honoured). Widths are changed by dragging a divider or
with the keyboard (arrows, shift-arrows for a bigger step, Home resets) and
are clamped — sidebar 264–420 px, document panel at most 70 % of the window
while the centre keeps 400 px — with the compact-window rules (rail below
860 px, no document panel below 1 024 px) overriding the stored width rather
than fighting it. The shell stays Tauri-free: the app names the scope
(`Shell.setLayoutScope`) when the root is known or changes; the geometry on
screen is written back to the scope being **left**, then the new scope's
saved layout is adopted — or the defaults when it has none. UI preferences
(appearance, font size, language, …) stay global: they are about the app,
not the project.

### 12.1 Sidebar

**Layout**

1. Show the product logo and brand.
2. Provide a prominent **New Session** action.
3. Show navigation entries for plugins, automation tasks, and MCP connectors.
4. Show a Workspaces section with search, filter/settings, and create actions.
5. Present workspaces and sessions as grouped or tree-based lists.
6. Show session labels and recent activity times when available.
7. Anchor the user avatar/account entry near the bottom.

**Interactions**

- New Session creates a session through the application layer.
- Selecting a workspace changes the active project context.
- Selecting a session opens that session.
- The active workspace and session have visible selected states.
- Long labels are truncated gracefully; provide a way to inspect the full label.
- The sidebar can collapse on smaller windows without removing access to core navigation.

**Files tree (shipped, rev 77)**

The workspace on disk sits below the session list as a **lazy tree**.

1. Listing is **one directory per click**: the backend (`list_files`) returns a single level, so expanding a folder never walks the repository.
2. The listing is **workspace-confined**: the requested path is joined to the configured root and canonicalized, and is refused if it lands outside it. Absolute paths, `..` segments and symlinks that leave the root all end in "path escapes the workspace"; an unset root says "no workspace configured"; a root that cannot be canonicalized says "workspace unavailable".
3. `.git`, `target` and `node_modules` are skipped, dot-prefixed entries are not listed, and symlinks are never listed as entries.
4. A level is capped at 1 000 kept rows (4 000 raw reads); the cut is reported and the tree says a folder was truncated instead of pretending to be complete.
5. Rows sort directories first, then names case-insensitively; files show their size.
6. Open folders persist **per workspace root** and are **re-fetched** on the next launch, not replayed from a snapshot; changing the root drops the cache.
7. A folder that refuses to open keeps the reason on its own row, and clicking it retries.
8. Names come from disk and are untrusted: rows are painted with `textContent` only.
9. Clicking a file opens it in the document panel through the same bounded `read_file` the panel already trusts; the document panel stays a plain viewer.
10. `/files` in the command palette toggles the tree.

### 12.2 Center workspace

**Toolbar**

- Show the active session or task.
- Show subagent count and a details affordance when supported.
- Show background-job count and a details affordance when supported.
- Provide a context menu for session/task actions.
- Support Chat and Trajectory view switching.

**Conversation and activity area**

Support:

- User messages and agent responses.
- Product-level progress/status descriptions without exposing hidden internal reasoning.
- Tool calls, file edits, and change summaries.
- Long-running task progress.
- Background tasks and persistent goals.
- File references, errors, and approval requests.

**Composer**

- Multiline text input.
- File, command, session, or resource references where supported.
- Attachment affordance and model selection.
- Current task state.
- A stop action while a task is running.
- State-aware enabling/disabling of actions.

**Task states**

At minimum:

- `Idle`
- `Running`
- `WaitingForApproval`
- `Completed`
- `Failed`
- `Cancelled`

States must reflect actual task events, not animations or locally invented timers.

### 12.3 Right document panel

**Header**

- Show Start or another workspace entry.
- Support multiple file tabs.
- Clearly indicate the active tab.
- Show the file path.
- Show file type or view mode.
- Provide refresh and additional actions where supported.

**Content**

- Render Markdown headings, paragraphs, lists, blockquotes, tables, and code blocks correctly.
- Give long documents an independent scroll area.
- Keep the toolbar separate from document content.
- Allow future expansion to edit/preview modes.
- Handle long code lines, wide tables, and long paths.
- Show recoverable error states for missing or unreadable files.

**Layout**

The document panel and center workspace must have independent scroll positions. Dividers should support resizing where the GUI framework permits, without obscuring content.

### 12.4 Run panel (shipped, rev 78)

A bounded command-output panel under the plan panel in the center column. It is the M2 "terminal" slot in **run mode**: one-shot commands against pipes — no interactivity, no job control, no ANSI colour — and the panel's header says so ("plain text, not a terminal"). The PTY upgrade shipped in rev 80 (§12.5) without touching this mode: run and term share the panel, the persisted lines and the chip.

1. A form field runs a shell command (`sh -c`, in the configured workspace root, stdin closed) with Enter or the **Run** button; `/run` opens the panel from the palette, and a ✕ button hides it.
2. Output **streams**: each stdout/stderr line becomes a `run-line` event (`{run_id, stream, text}`) as it is produced; the UI repaints at most once per frame, the way the transcript does, so a chatty command cannot stall the UI.
3. The panel is open/closed on demand (`#run-panel`, `hidden`), starts closed, and keeps focus sane: opening it focuses the command field.
4. One run at a time: while `state.runPanel.activeId` is set the form refuses a second command (interleaved lines could not be told apart on screen); the chip by the title reads `running` (`.chip.streaming`) until the finish event lands.
5. Every run ends with a one-line receipt in the panel — `[exit 0 · 12 lines · 1.2s]` — and a failed start, a non-zero exit, a budget cut, and a deadline are each named explicitly (`exit 7`, `stopped by the budget`, `could not start: …`).
6. Bounded by construction: max **2 000** lines per run (a longer run is killed and marked truncated), max **4 096** characters per line (the cut is marked `…`), a per-line timeout against a **60 s** whole-run deadline, and the child is reaped on every exit path.
7. Lines are rendered with `textContent` only — command output is untrusted data, never markup; stderr is coloured `--error`, receipts/notes `--dim`.
8. What the panel showed **survives a reload**: the kept lines are stored per workspace root (`bos.runs.<root>`, capped at 400, the same per-root pattern as the file tree) and re-shown on init; switching the workspace root in settings swaps to that root's history. Clear empties the panel and the storage.
9. A command that produces invalid UTF-8 ends that stream with a visible `[output stopped: not valid UTF-8]` marker rather than dropping output silently.

**Honest limits (rev 78, run mode)**: run mode is not a terminal — no stdin, no colours/rendering of ANSI, no interactive programs, no Ctrl-C; commands run with the user's own permissions and no extra sandboxing beyond the workspace root for the *working directory* (the command itself may touch anything the OS allows). Term mode (§12.5) lifts the first three limits but keeps the last one.

### 12.5 Terminal session (shipped, rev 80)

The PTY half of the M2 terminal slot, behind the same panel. `/term` from the palette or the **Shell** button in the panel header switches the panel into term mode; a live shell session per workspace root is what makes the "terminal survives reload" promise true.

1. **One session per workspace root** (`bos-pty-<root>`), spawned on demand in that root with the user's `$SHELL` (falling back to `/bin/sh`), with a default grid of 120×40 clamped to 20–500 columns and 5–200 rows.
2. **The session outlives the webview.** The `PtyRegistry` is managed by the Tauri app (its own mutex, deliberately not part of the UI state), so a reload re-attaches to the same live process: `pty_start` on an existing id answers `fresh: false` and replays the buffered tail (64 KiB ring, ≤400 entries) instead of spawning a second shell.
3. **Input is the PTY's, not the panel's.** The form submits to `pty_write` (≤1 536 bytes) and the panel does **not** echo — the shell echoes, and echoing twice would print every command twice. An empty line is a bare Enter and is legal at a prompt.
4. **Output is untrusted text.** PTY bytes stream through a streaming ANSI filter (CSI/OSC/2-byte escapes stripped; an incomplete sequence is held up to 64 bytes then failed open) and land in the panel via `pty-output` events, rendered `textContent`-only. Chunks that split a line are held until the newline; the DOM keeps at most 2 000 lines.
5. **The panel reports the session honestly.** The chip reads `shell`/`attached` while alive and `exit <code>`/`killed` when the process ends (`pty-exit`); the **Kill** button (term mode only) ends the session explicitly so switching roots never leaves an orphan shell; the header note and the input placeholder both say the panel is an interactive shell while it is one.
6. **Geometry follows the panel.** The shell is resized to the panel's own character grid (measured off its monospace font) on attach and on window resize, so column-layout programs (`ls`, `htop`) draw correctly.
7. **The blob gained `mode`** (`{open, mode, lines}`): a term-mode panel re-attaches on reload and on workspace switch (the backend's tail is the fuller truth — the snapshot is shed on attach); the older bare-array and `{open, lines}` shapes still load, and a mode that is not exactly `"term"` degrades to run mode.
8. **Coexistence.** Run mode and term mode never fight: a live session is not killed by switching to run mode or by running one-shot commands; `/run` reopens the panel in run mode, the Shell button re-attaches to term.

**Honest limits (rev 80)**: no alternate screen buffer, no true colour or cursor addressing — ANSI is stripped, not interpreted, so `vim`/`htop` render as plain scrolled text (a vt100 emulator is the documented upgrade path behind the same events); no tab completion UI beyond what the shell itself echoes; no key binding for Ctrl-C (the shell's own controls apply only to what the PTY receives — type `exit` or press **Kill** to end the session). The spawning PTY tests stand down, loudly, in sandboxes that refuse `/dev/ptmx`, and run in full on a normal dev machine.

### 12.6 DSH frontend reference (rev 81–82)

The UI is a **reference-driven clone of DSH's web frontend**: we study DSH's
published UI contracts and reimplement them in our own thin, dependency-free
document rather than copying its React sources. The upstream material is vendored
at `docs/dsh-ui-reference/` — DSH's own READMEs and `.d.ts` seams (MIT, fetched
2026-10-10 from the public `@deepseek-ai/*` npm scope), with the index stating
provenance, what is cloned, what is deliberately not, and how to refresh the copy.
It is documentation held for study, not a runtime dependency.

Adopted so far (frame geometry + theme presenter), each held by a harness check:

1. **Columns** — sidebar 264–420px at a 280px default with a 56px rail; the right
   column opens at 45% of the viewport and never exceeds 70%; the centre keeps
   400px; below 1024px the sidebar auto-collapses.
2. **Stepped concession** — when the stored right-column width would squeeze the
   centre, the column steps down to 300px, and is reported as not fitting only
   when even that cannot be honoured; a column that does not fit **closes
   deterministically** and is not reopened automatically by a wider window.
3. **Space comes from the sidebar first** — opening the right column collapses a
   manually expanded sidebar.
4. **Theme presenter** — `color-scheme` on the root, `body[data-ds-dark-theme]`,
   `--content-font-size`, and one owned `<meta name="theme-color">` whose content
   is read back from the rendered body background.
5. **Frame clearances** — `--frame-top-clearance: 48px`,
   `--frame-overlay-top: 68px` and `--frame-leading-clearance` published for the
   window-chrome seats (macOS traffic lights, or a seat mounted beside them).
6. **Keyed panel registry** — the root column is a keyed `main` slot with
   `conversation` reserved; `selectPanel(null)` returns to the conversation, a
   registered panel becomes the active surface, an unknown id changes nothing, and
   the built-in document panel registers itself through the same public seam a
   future surface would use (`registerPanel`/`unregisterPanel`/`selectPanel`,
   with `panels`/`activePanel` readable and the active key mirrored onto
   `#app[data-panel]`). A column that closes releases its surface.
7. **Mod+B** toggles the sidebar, and yields to a control that has its own
   meaning for the chord — an editable field, or the terminal, where `Ctrl+B`
   belongs to the program at the other end of the PTY.

Still to reference and reimplement, each against an existing milestone rather than
as a new product surface: the sidebar/session seams and right-column docking
(§12.1, §12.3), conversation node rendering and composer blocks (§12.2), tool
cards, the `shell.leading` seat, the settings-card registry (§13), and the locale
and theme token sets (M13).

## 13. Settings center

### 13.1 Window and navigation

- Present settings in a large modal overlay.
- Use a left category navigation and right settings content panel.
- Provide **Open configuration file** and close actions in the top-right area.
- Clearly highlight the selected category.
- Scroll the settings content independently when it exceeds the viewport.
- Adapt to window resizing without clipping critical controls.

### 13.2 Account settings

The Account page contains two main cards.

**Account information card**

- User avatar.
- Display name.
- Masked phone number or account identifier.
- **More account information** link/action.

**Balance card**

- Topped-up balance label.
- Current balance and currency.
- More-information affordance.
- **View usage** action.
- **Top up** action.

**Behavior**

- Load account and balance data from the account service.
- Mask sensitive account identifiers by default.
- View usage opens usage details.
- Top up opens the recharge workflow.
- Show explicit loading and error states.
- Allow negative balances if the real business rules and service data permit them; do not silently clamp to zero.
- Never fabricate account data when no account service is available.

### 13.3 General settings

Settings are vertically grouped. Each row contains a label, description, and control.

| Setting | Control | Screenshot value | Requirement |
| 19 | 2026-10-09 | Workspace management is operable in the panel: "+ New workspace" and per-header ✎/✕ actions drive `create_workspace`/`update_workspace`/`remove_workspace` through one inline form (a webview may not implement `prompt()`, and an inline form is the version the harness can exercise), one applier keeps the "every workspace command returns the newly effective settings" invariant, and failures land in the status line rather than being swallowed. Removing asks what happens to the workspace's chats. Six harness checks cover the offer, the form swap, the fields, prefill, the chat question and the return of the button. Still open: the folder picker (no dialog plugin yet — the root field is typed and server-validated, which is the same guardrail the picker would use) and per-workspace capability lists/presets. |
|---|---|---|---|
| 20 | 2026-10-09 | Per-workspace tool selection is closed end to end: the workspace form lists the built-in tools (`bash`, `read_file`, `write_file`, `list_dir`, `plan` — the names the model sees, which is what `build_agent` gates on, so panel and server share one vocabulary), opens each one checked unless the workspace denies it, and `update_workspace` now takes the deny-lists. The payload is a **deny-list**, not a snapshot of every tool, so a tool added to the build later appears for people with no opinion. `disabled_skills` is carried through on save rather than cleared, because exposing it needs a per-skill seam in `crates/agent` that does not exist yet. Four harness checks cover the list, both open states, and the deny-list shape. |
| Permission | Dropdown | Workspace Write | Set the default permission mode for new sessions |
| 21 | 2026-10-09 | The MCP per-server switch is operable: the workspace form lists the workspace's configured servers, opens each ticked per its stored flag, and sends the entries back with only `enabled` changed (pass-through, so a transport the panel does not understand survives), which `update_workspace` revalidates before storing and `attach_mcp_tools` enforces at attach time. The round's one red check was a fixture error, not the feature: my test ticked `git` by accident while the implementation only flipped the flag the test had changed — recorded because the distinction matters. |
| Language | Dropdown | English | Choose the UI language |
| 22 | 2026-10-09 | The skills deny-list is enforced, not merely stored: `crates/agent` grows `register_skills_from_dir_denying` (the existing `register_skills_from_dir` delegates with an empty deny-list, so `nbos`/`jsbos` callers are untouched) and `caps::build_agent` passes the workspace's `disabled_skills`. Deny-list rather than allow-list for the same reason as tools: a skill added to the directory later is picked up without anyone opting in. Open, and pinned for the next round: the skip path has **no dedicated test yet** — the fixture to use is `crates/agent/tests/fixtures/skills`, and the test needs an `Agent` construction copied from `tests/agentic_mode_test.rs`; the panel also has no skill checkboxes yet, so the value is carried through on save rather than edited. |
| Appearance | Three-way segmented cards | Light | Light, Dark, System |
| 23 | 2026-10-09 | The owed test exists: `a_denied_skill_is_not_registered` loads `crates/agent/tests/fixtures/skills` twice — once with nobody objecting (asserting several skills register, so the fixture is real rather than empty), once denying `calculator` — and asserts the denied skill is absent **and** the others are not, i.e. `kept.len() == all.len() - 1`. That is the difference between "the field is stored" and "the engine obeys it"; the skipped path is now behaviour-verified, not compile-verified. Remaining for skills: the panel has no checkboxes for them yet, so the value is carried through on save rather than edited. |
| Font size | Numeric input/stepper | 14 px | Change conversation-content font size |
| 24 | 2026-10-09 | Skill selection is now editable in the same workspace form, which closes §24.8 for all three capability kinds (tools, MCP servers, skills). The list is the union of what the capability listing reports and what the workspace already denies, so **a denied skill stays visible and re-enableable even before that listing has been loaded** — the form never hides a switch the user flipped. A form that did not show the skills carries the stored deny-list through instead of clearing it. Five harness checks cover the list, both open states, the deny-list payload and the no-listing case. |
| Work details | Dropdown | Detailed | Control how much tool-call detail is shown |
| 25 | 2026-10-10 | Presets exist at the model level, with **apply-and-diverge** semantics: `Settings::apply_preset` creates a *new* workspace as a copy of the preset and switches to it, so following a preset is impossible by construction — editing what you got can never rewrite the preset or anything else made from it. `apply_preset` is a command (persisting, bumping the generation, clearing the agent cache) and the test asserts the copy's identity differs from the preset's, that the name override works, and that mutating the applied workspace leaves the preset untouched. Open: no panel entry point for presets yet, and Choose folder is **blocked in this environment** — `tauri-plugin-dialog` is absent from the local cargo registry cache (0 hits) and the build is offline, so the root field stays typed + server-validated until the crate can be fetched. |
| Show coding view | Toggle | On | Show/hide trajectory, code diffs, and agent presets as applicable |
| 26 | 2026-10-10 | Presets are operable: they are offered right beside "+ New workspace" (applying one *is* a way of creating a workspace), labelled by name with a tooltip stating it makes a copy rather than a link, and the preset list rides along in the settings snapshot every workspace command returns so the offer cannot go stale. Three harness checks cover the offer, the create button staying put, and the empty case. Remaining for §24.9: creating/promoting a preset from an existing workspace (today the catalogue is populated by settings, not by a click) and presets over the full capability set. |
| Keyboard shortcuts | Button | Edit shortcuts | View and edit shortcuts |
| 27 | 2026-10-10 | §24.9 is closed: a workspace can be promoted to a preset ("Save as preset" on an existing workspace, never on one that does not exist yet), which stores a **copy** whose id is blanked — a preset is not a workspace, so it has no sessions until it is applied. Duplicate names are refused rather than silently shadowing, because the name is how a preset is picked. No settings-generation bump: a preset is a catalogue, not a capability of the running agent. The test pins the copy, the drift afterwards, the duplicate refusal and the unknown-workspace error. |
| Open chat links in | Dropdown | Confirm against current product configuration | Configure how chat links open |
| 28 | 2026-10-10 | No code this round, on purpose, and the reason is itself the finding: the residual streaming cost (§7) is O(N) because the transcript builds a row for every message, and the fix is windowing — but the renderer is a contract-verified path (one signature per streamed frame, zero per idle frame, absolute-index cursor) and a patch written without reading its row loop would have risked that contract to save one round. Instead the windowing plan is now concrete, derived from the renderer as it actually is: (1) `windowStart` is a message index and `msgs[windowStart + i]` is row `i`, so marking stays absolute at the call sites — `markDirty(abs)` becomes `markDirty(max(0, abs - windowStart))`, and an absolute index below the window is a no-op; (2) the window offset folds into the existing `epoch` string, which the renderer already uses to force `markDirtyAll()` when presentation flags change — a changed offset therefore invalidates every row through the mechanism that is already there, rather than through a new one; (3) "load earlier" is explicit (a button above the first row) so the transcript never grows sideways under a scroll position the user did not choose; (4) acceptance: the harness's structural assertions (1 signature/stream frame, 0/idle frame) must hold with the window at 200 over a 10 000-row fixture, and the streaming cost must stop growing with N instead of merely growing slower — the honest test being the 500 / 3 000 / 10 000 curve flattening. |

**Configuration behavior**

1. Every setting has a defined type, valid values, and default.
2. Each setting explicitly defines whether it applies immediately or after confirmation.
3. Persisted settings remain in effect after reopening the settings window and restarting the app.
4. System appearance follows the operating system.
5. Font size changes conversation content only unless product requirements explicitly say otherwise.
6. Hiding the coding view must not delete task data.
7. Opening the configuration file uses the application's existing file-access mechanism and reports errors.

## 14. Visual design system

### 14.1 Style

- Desktop-tool aesthetic with relatively high information density and clear hierarchy.
- Light mode uses white/light-gray surfaces.
- Dark mode uses layered dark surfaces.
- Use distinct text contrast levels for primary and secondary content.
- Standardize borders, corner radii, spacing, and control heights.
- Clearly distinguish selected, running, error, and disabled states.

### 14.2 Suggested initial design tokens

These are implementation starting points, not pixel measurements extracted from the screenshots.

| Token | Suggested value |
| 29 | 2026-10-10 | The read refined the plan, and found the constraint that makes windowing non-trivial: **windowing collides with the single-signature contract at the window cap**. `sameShape` trusts the leading rows by comparing `rowCache.size` and `messagesEl.childElementCount` with the message count, so a window changes the invariant to "row per *visible* message". At the cap, appending a message shifts the window by one: the retained rows now map to different messages, so either the frame re-signs all 200 rows (violating the verified one-signature-per-streamed-frame bound) or the implementation grows an explicit **append-shift path** that drops the first row, keeps the retained rows unsigned, and signs only the new tail — reusing the existing "rebuild only what changed, swap in place" machinery. Full-width windows also mean `opts.index`/editing/regenerate stay *absolute* while the row loop counts *relative*, which is why the plan's index mapping is load-bearing rather than cosmetic. The next round has the rest of the bookkeeping (`seen` pruning and the `desired`/DOM reconciliation) in hand and implements the append-shift path; acceptance is still the flattening 500/3 000/10 000 curve under the unchanged signature bound. |
|---|---|
| 31 | 2026-10-10 | Windowing was implemented, went red, and was **reverted automatically** — nothing shipped, and the diagnosis is specific. The ui harness reported "result block missing (script threw at load?)" rather than a failed check, a different failure mode that points at a deliberate guard I had put in the append-shift path: `if (!gone) throw new Error("window shift lost a cached row")`. Two mistakes, both mine: the predicate used `-1` sentinels (`lastWindowStart = -1`, `lastWindowLen = -1`) so it could match an early frame with no cached row to drop, and a **render path must never throw** — a mis-predicted fast path should fall through to the general pass, not kill the app. The fix: fold cache presence into the predicate, require `lastWindowStart >= 0`, delete the throw. Also required in the same change: the harness's structural assertions still encode the pre-windowing "a row per message" assumption, so they must become window-aware (rows == min(N, WINDOW + extra)) while keeping the 1-signature/stream-frame and 0/idle-frame bounds. The patch ran under a backup with automatic restore, and the restore was verified (`node --check` clean, 56 + 48 checks green) — the attempt cost the round, not the tree. |
| Base spacing | Multiples of 8 px |
| 32 | 2026-10-10 | Second windowing attempt, also reverted, and the honest blocker is now the **harness's own error handling**: it reports "result block missing (script threw at load?)" and swallows the exception, so three candidate causes were eliminated by inference instead of by evidence. Eliminated this round: the deliberate `throw` in the append-shift path (replaced by a precondition — a render path must fall through to the slower correct pass, never throw), the `-1` sentinels (`lastWindowStart >= 0` / `lastWindowLen >= 0` now required), and the `messagesEl.parentNode` insertion (`renderLoadEarlier` now returns unless the transcript is in the document). It is still a load-time throw in the windowed renderer. **Prerequisite for the next attempt, and a defect worth fixing for its own sake: the harness must print the exception it catches** (`err.message` + stack) — a self-test that hides its failure is worse than one that fails. Also learned: a window changes the harness's own premise ("first paint renders every message" assumed a row per message), so the window-aware assertions have to land in the same change as the feature rather than after it. Tree verified green after each revert (`node --check` clean, 56 + 48 checks). |
| Sidebar width | Approximately 280–340 px |
| 33 | 2026-10-10 | Third attempt at windowing: still red, but the failure is now **localized by evidence instead of inference**, and the difference matters. The self-test harness was taught to report its own load failures (both `error` and `unhandledrejection`, since a rejected promise in an async harness reaches the runner as "result block missing" — a message with no reason in it). With those listeners in place the windowed render produced **neither** event: the page does not throw, it **hangs** before the result block is written, which is exactly why three rounds of reading code for a throw found nothing. The window code is reverted again (tree green, 56 + 48 checks), the harness's error reporting stays, and the next step is no longer guesswork: bisect with `N = 5, 250, 2000` to separate "hangs on any window" from "hangs at the cap", then disable the append-shift path with a one-line `const shifted = false` to decide which of the two branches is responsible. Known-good: the un-windowed renderer, verified after every revert (`node --check` clean, 56 + 48 checks, smoke 124). |
| Center workspace width | Approximately 400–520 px |
| 34 | 2026-10-10 | The hang is bisected to **windowing itself**, and the append-shift path is exonerated. Two one-line probes, each with the rest of the change held constant: `WINDOW = 1000000` (no window, no shift, all my other edits present) → **56 + 48 green**, so every unconditional part of the change is correct; `WINDOW = 200` with the shift path disabled via `if (false && shifted)` → **still hangs**. So the fault is in the general windowed pass, not in the fast path I had spent three rounds suspecting. Prime suspect now, and it explains why no exception was ever seen: the harness indexes rows as if the DOM held every message (`rows()[i]` for message `i`), so with a shrunk window it dereferences `undefined` — and if that happens inside the harness's own `try`, the page ends with neither a result block nor an `error`/`unhandledrejection` event, which is exactly the signature observed. Next step is instrumentation rather than another guess: have the harness write a phase marker (e.g. its current phase into `document.title`) before each phase, so a bail is attributable to a phase instead of to "the window". Tree verified green after the restore (56 + 48, `node --check` clean). |
| Panel radius | 8–16 px |
| 35 | 2026-10-10 | Correcting my own hypothesis from the previous revision: the harness contains **no polling construct** (no `setTimeout`, no `requestAnimationFrame`, no `while`) and its structural phase is a sequence of synchronous `renderMessages()` calls followed by `assert`s that *record* rather than throw. So "the harness waits for a row count that a window never reaches" is **wrong**, and the observed absence of a result block is not explained by a stuck wait loop. What the read did establish: of the harness's row-indexing sites, most degrade to plain assertion failures under a window (`afterAppendRows === N + 1`, `firstChild.textContent.includes(N)`, first-row identity), while `rows()[100]` stays valid for any window of 200. The hang therefore sits inside the windowed render call itself, reached synchronously — which is a smaller search space than before but still not pinned. Next step, deliberately minimal: re-apply windowing with `WINDOW = 5` at `N = 20`, where either the page completes and the recorded failures name the first broken assertion, or it hangs and the phase marker (`document.title`) attributes the bail to a phase. Two of my hypotheses have now been falsified by reading rather than by shipping, and both corrections are recorded rather than quietly dropped. |
| Settings content width | Responsive to available window size |
| 36 | 2026-10-10 | Two probes narrowed the window failure from "scale" to "the window itself". (1) A 250-message transcript — window active, only fifty messages hidden — fails the same way as 2 000, so this is **not** a slowness or timeout effect and my previous round's perf theory is withdrawn too. (2) With the "Load earlier" control disabled by a one-line early return and everything else windowed, the run reported: `result block missing`. That **exonerates** the control: disabling it changes nothing, so the sibling insertion is not the cause either. Windowing is reverted again (tree green, 56 + 48 checks). Next step is one more bisect in the same style: keep the window, place the control somewhere inert (or render it as a zero-height element the harness's layout spy ignores) and confirm whether the window alone passes — three of my own theories have now been falsified by probing rather than by shipping, and each correction is recorded. |
| Conversation font size | 14 px by default |
| 37 | 2026-10-10 | **Fourth falsified theory, and the search space is now small and enumerated.** The windowed renderer fails at 250 messages as well as 2 000, with the "Load earlier" control disabled as well as enabled, and it passes only when the window is effectively absent (`WINDOW = 1000000`). Four of my own explanations have now been eliminated by probing rather than by shipping: a throw in the append-shift path, a polling wait in the harness, scale/slowness, and the sibling control. What remains, in order, is exactly two things: (a) the **shape check's new window terms** (`windowLen === lastWindowLen`, the `slid`/`delta` arithmetic, `messagesEl.childElementCount === windowLen`), and (b) the loop's **relative index mapping** (`abs = windowStart + i` for `msgs`/`opts.index`/`editing`/`lastAssistant`). The next probe separates them in one line: keep the window (row count still `windowLen`) but index `msgs` absolutely (`msgs[i]`), which breaks correctness for a scrolled window while leaving the shape check intact — if that run reaches the result block, the fault is the index mapping; if it still vanishes, it is the shape check. Worth stating plainly: four rounds of windowing have shipped nothing, and the honest summary is that the *plan* is being validated far more slowly than the code is being reverted. |
| Dividers | Low-contrast neutral gray |
| 38 | 2026-10-10 | **The failure is isolated, and it is the index mapping, not the window.** One-line probe, exactly as planned: keep the window (row count still `windowLen`) but index `msgs` and the row options absolutely (`abs = i`) instead of window-relative (`abs = windowStart + i`) — the run then **reaches the result block and completes with 8 failures / 56 checks**. Those failures are the expected consequences of indexing wrongly (rows for the first messages instead of the tail), but the page *survives*; with window-relative indices it does not. So everything structural — the window length/offset arithmetic, the `slid`/`delta` slide, the subset `desired` list, the reconciliation, the control — is exonerated, and the defect sits in one of the seven places that consume `abs`: the message lookup `msgs[abs]`, `showCaret`, `editingHere`, `opts.index` (which feeds the row signature and the edit/delete/branch handlers), `regen`, `branch`, and `buildEditor(m, abs)`. The windowed file is now cached at `target/windowing/app.windowed.js` (jj-ignored) so the next attempt is one `cp` plus a focused edit instead of re-applying a 200-line patch — the cost of each attempt was itself part of why this took five rounds. Tree verified green after the restore (56 + 48 checks, `node --check` clean). |
| Primary action | High-contrast background and text |
| 39 | 2026-10-10 | Bisect table for the seven `abs` consumers, four variants at 250 messages in one pass: **A** all window-relative → missing; **B** only the row index relative (`index: i`, everything else relative) → missing; **D** flags and index relative with `abs` used *only* to fetch the message → missing; **C** all absolute (`abs = i`) → **completes, 8 / 56**. Reading the table: B and D keep the arithmetic and die, so the index *arithmetic* is not what kills the page. The one thing C changed beyond arithmetic is **which messages it draws** — C renders the head of the transcript and therefore never takes the caret branch (`abs === msgs.length - 1` is never true for it), while A/B/D all draw the tail and do take it. So the live hypothesis is not "windowStart + i is wrong" but "drawing the newest message inside a windowed transcript enters a path that cannot finish" — most likely the caret/streaming row, which is the only branch that C's mis-indexing silently skipped. That is a materially different and much smaller question than the one the last five rounds argued about, and it is exactly what the table was for. Next probe, one line: keep everything absolute but fetch the message from the tail (`m = msgs[windowStart + i]`), so the content and the caret branch are exercised with C's surviving arithmetic. Tree verified green (56 + 48). |

Prioritize responsive resizing over rigid fixed widths.

## 15. Component architecture

Suggested reusable logical components:

- `AppShell` — main window and overall layout.
- `Sidebar` — navigation and workspace list.
- `SessionList` — sessions and selection state.
- `WorkspaceTabs` — document tabs.
- `AgentWorkspace` — conversation and task content.
- `Composer` — input, attachments, and model selection.
- `TaskStatus` — task progress and execution state.
- `SettingsWindow` — settings container.
- `SettingsSidebar` — settings category navigation.
- `AccountSettings` — account information and balance.
- `GeneralSettings` — general preferences.
- `SettingRow` — consistent settings row.
- `SelectControl`, `ToggleControl`, `SegmentedControl` — reusable preference controls.

### Architecture constraints

- UI components must not own agent scheduling, model requests, or account business logic.
- Access sessions, tasks, files, and settings through explicit application interfaces.
- Separate ephemeral component state from persisted application state.
- Long-running task state must not depend on a view component's lifetime.
- Switching views must not unintentionally cancel background tasks.
- Theme, font size, language, and permission settings must have a consistent data model.
- Follow the actual GUI framework and cross-platform constraints already used by BOS.

## 16. Core logical data models

The following Rust is illustrative only and must be adapted to the existing GUI framework and code conventions.

```rust
enum TaskStatus {
    Idle,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

enum Appearance {
    Light,
    Dark,
    System,
}

struct UiSettings {
    appearance: Appearance,
    font_size: u16,
    show_coding_view: bool,
}
```

Additional suggested models:

- `WorkspaceSummary`: workspace ID, display name, and path.
- `SessionSummary`: session ID, workspace ID, and last activity time.
- `TaskSnapshot`: task ID, status, progress/phase, and error details.
- `DocumentTab`: file ID, path, type, and loading state.
- `SettingsSnapshot`: settings values, version, and persistence status.

Do not invent a numeric progress percentage when the backend cannot provide one. Use a phase label or indeterminate progress indicator instead.

## 17. Agile backlog

Each story is estimated at no more than three working days. Estimates are preliminary and have not been validated against the repository. Include tests within each story's estimate.

### Epic A — Workbench foundation

#### GUI-01: Main window and three-panel layout

**Estimate:** 2 days · **Priority:** P0

As a developer, I want to see navigation, the agent workspace, and document content together so that I can avoid unnecessary window switching.

Acceptance criteria:

- Three panels render using configurable initial proportions.
- Panels scroll independently.
- Dividers can be resized without layout corruption.
- Small windows do not permanently hide critical actions.

#### GUI-02: Sidebar and workspace navigation

**Estimate:** 2 days · **Priority:** P0

As a developer, I want to browse and switch workspaces and sessions so that I can quickly open the right project context.

Acceptance criteria:

- Current workspace and session have visible selected states.
- Long labels truncate gracefully.
- New-session and selection actions dispatch the correct application events.

#### GUI-03: Design tokens and themes

**Estimate:** 2 days · **Priority:** P0

As a user, I want consistent colors, spacing, and control styles across pages.

Acceptance criteria:

- Colors, radii, spacing, and typography use shared tokens.
- Light, Dark, and System modes work.
- Switching themes does not discard active session state.

### Epic B — Agent interaction

#### GUI-04: Message list and composer

**Estimate:** 3 days · **Priority:** P0

As a user, I want to enter tasks, select a model, and read messages.

Acceptance criteria:

- Supports multiline input, submit, and stop actions.
- Model selection reflects the actual available-model list.
- Submission errors and unavailable models are reported clearly.

#### GUI-05: Task status and event updates

**Estimate:** 3 days · **Priority:** P0

As a user, I want accurate running, approval, completion, failure, and cancellation states.

Acceptance criteria:

- State is driven by backend events.
- Duplicate or out-of-order events cannot incorrectly regress state.
- Correct task snapshots can be restored after switching sessions or panels.

#### GUI-06: Trajectory and background tasks

**Estimate:** 3 days · **Priority:** P1

As a user, I want to inspect tool calls, task stages, and background jobs.

Acceptance criteria:

- Chat and Trajectory use the same task-state source.
- Background jobs show real status.
- Hiding or switching the view does not accidentally terminate a task.

### Epic C — Document and code panel

#### GUI-07: File tabs and paths

**Estimate:** 2 days · **Priority:** P1

Acceptance criteria:

- Open, select, and close document tabs.
- Show path and loading state.
- Missing files produce understandable error messages.

#### GUI-08: Markdown and code viewing

**Estimate:** 3 days · **Priority:** P1

Acceptance criteria:

- Render headings, lists, tables, quotes, and code blocks correctly.
- Long documents scroll independently.
- Large documents do not block task-state updates.
- Choose a renderer based on the actual stack; do not assume an embedded browser is required.

#### GUI-09: Editing and saving

**Estimate:** 3 days · **Priority:** P1

Acceptance criteria:

- Distinguish viewing and editing modes.
- Preserve edited content when saving fails.
- Detect external file changes or provide a safe reload/conflict flow.

### Epic D — Settings center

#### GUI-10: Settings window and category navigation

**Estimate:** 2 days · **Priority:** P0

Acceptance criteria:

- Category navigation and settings content scroll independently.
- Closing/reopening settings preserves the current persisted values.
- Unimplemented categories show explicit placeholders rather than fake features.

#### GUI-11: General settings and persistence

**Estimate:** 3 days · **Priority:** P0

Acceptance criteria:

- Configure permission mode, language, appearance, font size, and work-detail level.
- Configure the Show coding view toggle.
- Validate allowed values.
- Report persistence failures and preserve recoverable edits.

#### GUI-12: Account, balance, and usage entry points

**Estimate:** 2 days · **Priority:** P1

Acceptance criteria:

- Account and balance values come from a real service.
- Sensitive account identifiers are masked.
- Usage and top-up actions handle loading, success, and failure.
- If no backend is available, show an explicit unavailable state instead of fake data.

#### GUI-13: Keyboard shortcuts and accessibility

**Estimate:** 2 days · **Priority:** P2

Acceptance criteria:

- Show shortcuts and support basic keyboard navigation.
- Prevent global shortcuts from interfering with text entry.
- Make primary actions reachable by keyboard.

### Preliminary effort summary

| Phase | Stories | Estimated effort |
| 40 | 2026-10-10 | **The "hang" was a reporting artifact, and six rounds of reverting were chasing it.** Variant E (windowed, caret branch disabled) failed as well, falsifying the caret theory too — so I stopped inferring and instrumented progress instead: the windowed renderer writes `document.title` at the start of each pass and at each exit, and Chrome was run directly on the harness URL with `--dump-dom`. The dump reads `<title>pass#404/250 shape-known reconciled</title>` — 404 passes, the last one **completed** — and contains `"failures":1`, i.e. the harness wrote a result and the windowed renderer drew the transcript. The runner calls this "result block missing" because its detector requires the literal `<pre id="result">` and this dump does not contain that markup (the tag is not matched by either the runner's regex or a looser `<pre[^>]*result[^>]*>`), which is why every windowed run looked like a hang while the un-windowed ones matched. Verified consequences: the windowed renderer is not broken, the one recorded failure is the row-count assertion the window *should* change, and the passes completed through the reconciliation — the very code the last six revisions suspected in turn. What to fix first is therefore the **test harness's own reporting** (detect the result robustly, and only then trust its verdict), after which windowing has to be re-evaluated rather than re-written. This is also the clearest lesson of the stretch: I trusted a green/red signal from a runner I had never validated, and the repair cost six rounds that produced no code. |
|---|---|---:|
| 41 | 2026-10-10 | **Windowed transcript landed and verified**, after the reporting artifact in the previous revision was cleared up. The runner now finds a result element whatever its tag (`id="result"`, any element) and says "no result element" instead of guessing "script threw at load?", and the harness's real complaint became visible: at harness line 260 it reads `rowCache` as if a row exists per message — `Cannot read properties of undefined (reading 'el')` — which is exactly what a window stops being true. The harness now runs its structural phases with the window open (`state.windowExtra = N`) and gives the window its own phase, whose first version measured nothing because it ran after the phases that empty the transcript (the detail had to go in the assertion *name*, since the runner does not print details: `rows=1 active=0 cache=0`); the phase now owns its fixture. Verified: **57 transcript + 48 shell checks green at 250, 500 and 2 000 rows**, Rust 99 passed, clippy clean, bundle rebuilt, smoke 124, and the contract bounds hold at every size — `signaturesPerStreamFrame: 1`, `idleSignaturesPerFrame: 0` at 500 / 3 000 / 10 000. **Not yet evidenced, stated plainly: the cost curve has not flattened.** Those measurements run in the unwindowed phases, so they still read `streamingRepaintMs` 0.15 / 0.51 / 2.04 ms and `fullRebuildMsPerFrame` 1.9 / 11.3 / 35.2 ms over 500 / 3 000 / 10 000 messages — O(N) as before. The window makes the *drawing* bounded; what is still missing is a streaming phase that runs **at the cap** and a measurement there. Until that number exists, the performance claim in this document stays open. |
| Workbench foundation | GUI-01–03 | 6 person-days |
| 42 | 2026-10-10 | **The streaming cost curve is flat, and this revision replaces the open claim with measured numbers.** A windowed phase now measures the same 120 streamed frames twice on the same fixture, differing only in the window flag: `N = 500` windowed **0.072** ms/frame vs unwindowed 0.114 (1.6x); `N = 3 000` **0.072** vs 0.518 (7.2x); `N = 10 000` **0.076** vs 2.095 (**27.6x**). Windowed cost is 0.072 / 0.072 / 0.076 ms across a twentyfold range — **flat**, where the unwindowed path still tracks transcript length as it always did. The rows drawn are 200 at every size, the signature bounds hold (`signaturesPerStreamFrame: 1`, `idleSignaturesPerFrame: 0`), and the suite is 58 transcript + 48 shell checks green at 500 / 2 000 / 3 000 / 10 000 rows, with Rust 99 passed, clippy clean, bundle rebuilt and smoke 124. Stated precisely so the number is not read as broader than it is: the measured case is a **tail mutation** within a fixed-length transcript, which is what a streamed token is; the **append-at-cap** case takes the same incremental slide (drop the row that left, sign only the new tail) but is not separately timed here, and the remaining unbounded work in the GUI is the cost of a *full* rebuild, which still grows with the transcript (1.9 / 11.3 / 35.2 ms at 500 / 3 000 / 10 000) — windowing bounds drawing and streaming, not history loading. |
| Agent interaction | GUI-04–06 | 9 person-days |
| 43 | 2026-10-10 | **The gap left open last revision is closed: appending at the cap is bounded too.** The same measurement now runs a second mode where a message actually arrives each frame, which is what slides the window by one and forces the incremental path to drop a row and sign a row: `N = 500` windowed **0.142** ms/frame vs unwindowed 0.268 (1.9x); `N = 3 000` **0.191** vs 1.123 (5.9x); `N = 10 000` **0.148** vs 4.861 (**32.9x**). Windowed append sits at 0.142 / 0.191 / 0.148 ms across a twentyfold range — bounded, with `rows=200` at every size — while the unwindowed path grows eighteenfold over the same range. It is honestly about twice the cost of the tail-mutation case (0.072 ms) because it also drops the row that left the window and reconciles the slide, and it is still **independent of transcript length**, which is the property that matters. Suite: **59** transcript + 48 shell checks green at 500 / 2 000 / 3 000 / 10 000. With this, both halves of streaming — a token growing the last message and a message arriving — are measured bounded, and the remaining unbounded cost in the GUI is a full rebuild for history loading (1.9 / 11.3 / 35.2 ms), which is deliberate: windowing bounds drawing, not history. |
| Document panel | GUI-07–09 | 8 person-days |
| 44 | 2026-10-10 | Per-workspace runtime configuration on the server side (§24.9, and the review directive that a workspace owns its model). `Settings::set_workspace_runtime(id, model, base_url, api_key, system_prompt, temperature, reasoning_effort)` is the seam; `update_workspace` now takes those seven values and calls it *before* it borrows the workspace mutably (the first attempt placed the call inside that borrow and the borrow checker was right to complain). Semantics match `effective`: blank strings inherit from the globals, a blank effort is an absence (`None`), and `temperature` stays an `Option` so 0.0 remains a setting rather than a hole — all three asserted in `a_workspace_can_be_pointed_at_its_own_model`, which also checks the overlay the agent would see and that an unknown id is an error. Verified: **100** Rust tests pass, clippy clean, rustdoc clean (the eight-argument seam carries a scoped `allow` with the reason written next to it), 59 transcript + 48 shell checks green, bundle rebuilt, smoke 124. Stated plainly about the scope: this revision makes a workspace's model, endpoint, prompt, temperature and effort **settable and persisted**, but the workspace panel does not send them yet, so the only way to set them today is the command — the panel fields are the next step, which is why the review item is not closed by this row. |
| Settings center | GUI-10–12 | 7 person-days |
| 45 | 2026-10-10 | The workspace panel now carries its runtime settings, and the panel found two real bugs on the way. It renders **model, base URL, reasoning effort, temperature and system prompt** for an existing workspace (a new one inherits the globals, so those fields appear only in edit mode), **and the API key is deliberately not rendered** — a secret re-drawn into the DOM on every re-render is a secret in a screenshot, so it is carried through from the stored workspace instead; the harness asserts that the key does not appear in the form's markup. Two defects the harness caught immediately: `input.value = value || ""` turned the legitimate temperature `0` into blank (falsy trap, fixed to an explicit null check), and `const { invoke } = window.__TAURI__.core` captured the invoker at load, so a test double installed later was invisible — `invoke` is now late-bound (`(cmd, args) => window.__TAURI__.core.invoke(cmd, args)`), which is also more robust when the runtime injects its global after the script parses. That second fix is what makes the contract testable, and the check now proves it end to end: editing the form and saving sends `update_workspace model=bigger/model temp=0 effort=high key=secret-key` — the regression from the previous revision (the panel did not send the new parameters at all) is thereby fixed and covered rather than assumed. Verified: **61** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 100 passed, clippy clean, bundle rebuilt, smoke 124. |
| UX/accessibility | GUI-13 | 2 person-days |
| 46 | 2026-10-10 | Sessions can be re-homed (§24.6): a chat's workspace was stamped at creation and could never change. `SessionStore::move_to(id, workspace)` is the seam — it keeps the messages, the id and the title, because a move changes which configuration runs the chat, not what was said in it. The `move_session` command validates the target workspace, **refuses a streaming chat** (its forwarder is bound to the configuration it started with), drops the cached agent so the next turn is built from the new workspace, and returns the session list so the sidebar re-groups from the server's answer rather than guessing. The sidebar shows a workspace picker on each row, only when there is somewhere to move to. **More importantly, the new coverage found a latent crash that had nothing to do with this feature:** the session row read `clip(title, 24)` with no such variable in scope, so *any* workspace group holding at least one chat threw while rendering the sidebar — the previous fixtures only ever rendered an empty transcript, which is why 61 green checks never touched the line. Fixed to `clip(s.title, 24)`, and the new phase renders a real row so the class of defect is covered. Verified: **101** Rust tests, **62** transcript + 48 shell checks green at 500 and 2 000 rows (the new one reads `move_session id=s1 to=w2 options=2`), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. |
| **Total** | **13 stories** | **32 person-days** |
| 47 | 2026-10-10 | Workspace groups in the left panel fold away (a review item that was still open). Each group header carries a fold control (`▾`/`▸`) whose click stops propagation so it does not also switch the active workspace — the header's own job — and a collapsed group still counts its chats in the header while hiding the rows, so collapsing never loses information. The harness asserts the full contract rather than the click alone: one row before, zero after, **zero again after a repaint** (a stream tick or a search keystroke must not spring the group open) and the marker flips to `▸`. Stated honestly: `state.collapsed` is in-memory for the session, not persisted in the settings file, so a restart reopens every group — persisting it is a settings field this revision does not add. Verified: **63** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 101 passed, clippy clean, bundle rebuilt, smoke 124. |

These are initial development estimates, not a committed schedule. They exclude major framework changes and additional backend-service development. Include tests in each story. Calendar duration depends on staffing, dependencies, and integration effort.

## 18. Technical implementation proposal

### 18.1 Suggested layers

1. **UI presentation:** windows, components, layout, theme, input, and display.
2. **Application state and commands:** user intent, state snapshots, event reduction, and error handling.
3. **Domain services/adapters:** session, agent, task, document, and settings interfaces.
4. **Existing BOS infrastructure:** inspect and reuse existing `bus`, `agent`, `config`, `logging`, and related capabilities where appropriate.

These crate names are candidates for inspection, not a claim that the GUI currently depends on them in a particular way.

### 18.2 Architectural principles

- **Single source of truth:** Chat, Trajectory, and background-job views must not maintain conflicting copies of task state.
- **Separate commands and events:** commands express user intent; events describe what actually happened.
- **Independent persistence:** a settings-write failure must not destroy recoverable UI state.
- **Decouple task lifetime from view lifetime:** switching a panel must not cancel a task.
- **Use adapters:** UI components should not depend directly on individual model providers or remote API response formats.

### 18.3 Suggested event contract

| Event | Required information | UI behavior |
| 48 | 2026-10-10 | **Settings now has a Workspaces section**, which was the last review item about where workspace management lives. The settings panel is one scrolling page whose tabs scroll to a section, so this is a section and a tab like the others (Workspaces, before Tools & skill), not a second page. Each card states what a workspace *is* — name and whether it is in use, its root (or "(current folder)"), its model (or "inherits the defaults"), its reasoning effort when set, and how many chats belong to it — and offers the two actions the sidebar header offers: **Use** (disabled on the active one) and **Edit**, which closes Settings and opens the sidebar form, because that is where the chats are. The list is refreshed from `renderSidebar`, the single point every session and workspace change already passes through, so it cannot drift out of step. Two implementation notes worth keeping: the section's buttons set `type="button"`, since a bare button inside the settings `<form>` submits it; and the wiring for the section's "New workspace" is null-guarded, because the harness page loads this file without the app shell. The harness check that had to be debugged was my own fixture's fault, not the feature's — `document.getElementById` resolves the *first* matching node and the harness page turned out to hold one already, so cards landed in the app's container while the phase read the node it had appended (`pre=2` in the probe, `htmlLen=0` on the wrong element); it now reads the node `$` resolves. Verified: **65** transcript + 48 shell checks green at 500 and 2 000 rows (the new ones read `cards=2` with both cards' text and `set_active_workspace id=w2`), Rust 101 passed, clippy clean, bundle rebuilt, smoke 124. |
|---|---|---|
| 49 | 2026-10-10 | **Content search across every stored chat**, the §17 parity item the GUI only approximated by filtering titles. `SessionStore::search(needle, limit)` scans titles, message text and reasoning, case-insensitively, newest chat first and then by id — a result list that reshuffles between identical queries is a result list nobody can use. The snippet is cut around the match with ellipses, and the character-offset arithmetic is done on `char`s rather than bytes so a CJK chat cannot panic it; when lowercasing changes a string's length (a few scripts do), the match is reported as absent rather than pointed at the wrong characters, which is the honest failure for a snippet. `search_sessions(query, limit)` caps at 200 server-side; the sidebar asks from **two characters on** (below that the local title filter is enough and no round trip happens), draws the hits above the groups as rows that open the chat they came from, and **drops a response whose query has moved on** — a late answer must not repaint a search the user has left. Deliberately not an index: the harness keeps FTS5, this scans, which is honest at these sizes and cannot drift out of step with the session files; the seam is where an index belongs if the scan ever shows up in a profile. Recorded as a remaining gap: a hit row does not yet show which workspace it came from, though the hit carries it. Verified: **102** Rust tests, **67** transcript + 48 shell checks green at 500 and 2 000 rows (the new ones read `rows=2 header="2 matches inside chats"` and `search_sessions query=quick limit=20`), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. |
| `SessionSelected` | Session ID | Load the session snapshot |
| 50 | 2026-10-10 | The gap recorded in the previous revision is closed, and clicking a result is now correct rather than merely working. A hit row **names the workspace it came from**, because a search finds chats in workspaces that are not the one in use and a result you cannot place is a result you have to open to identify. Opening one handles both cases: a hit in the current workspace opens its chat directly, while a hit elsewhere **moves the workspace first and awaits it**, since that command re-lists the sessions and the chat has to be switched after that list is refreshed rather than during it — a race that would have shown up as a chat that opens into the wrong context, which is exactly the kind of bug the ordering comment now prevents. Three checks cover it rather than one: `labels=One|Two`, `activeId=s1` after clicking the hit in the current workspace, and `set_active_workspace id=w2` as the **first** call after clicking the other one. Verified: **70** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 102 passed, clippy clean, bundle rebuilt, smoke 124. |
| `TaskStarted` | Task ID, session ID | Enter running state |
| 51 | 2026-10-10 | **The token meter** (M-item), and it reports the provider's own numbers rather than an estimate — a local count would drift from the provider's exactly when the context is close to its limit, which is when the number matters. The agent already exposed `last_token_usage()`, so `StreamFinished` now carries `prompt_tokens`/`completion_tokens` (captured under the same lock that persists the plan, absent when the provider reported none) and the frontend updates the status line from it, only for the session that just ended — a background chat must not overwrite the meter of the one on screen. The status line reads `in 12k · out 678`, adds `88% of 14k` when a context budget is set, and says **"near the context limit"** with a warning class at **80%**, the point beyond which the next turn is more likely to be compacted than answered; `fmtTokens` keeps 999 exact, shows a decimal in the thousands and drops it past ten thousand, because a decimal there is noise. My own expectation in the harness was the wrong one (`12.3k` for 12 345) and the implementation was right; the leak of that mistake into a green run is exactly what the assertion name carrying the numbers prevents. Verified: **72** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 102 passed, clippy clean, bundle rebuilt, smoke 124. Stated as a scope limit: the meter reflects the last completed turn (it is not a live count while streaming), and it appears once a turn has reported usage. |
| `TaskProgress` | Task ID, phase/progress | Update task display |
| 52 | 2026-10-10 | **The memory panel** (§20): the low-level agent already attaches a capped `FileMemory` (`caps.rs`) and remembers exchanges itself when auto-remember is on, so the missing piece was the window onto it, not the machinery. Everything the GUI needs was already on `Agent` — `memory()` returns the store, and the store's `all`/`add`/`remove` are the operations — so this round **touched no line of `crates/agent`**: the GUI stays an adapter, exactly as the crate layering asks. Three commands (`list_memories`, `remember_memory`, `forget_memory`) clone the `Arc<dyn MemoryStore>` out under the state lock and then await on it, because holding the lock across a suspension point would serialise every chat behind a memory read; each answers with the list as it now stands, since the caller's next question is always "what is left?". `clean_memory_text` refuses a blank memory and caps one at 2 000 **characters** (not bytes, or a CJK memory would be cut to a third of its allowance) and is split out so the rule is testable without a running app. The panel adds a Memory tab and section beside Workspaces: the list, a field, Remember, and a Forget per row — and the empty store says so rather than looking broken. Verified: **103** Rust tests (the new one pins the trim/blank/character-count rule), **75** transcript + 48 shell checks at 500 and 2 000 rows, clippy clean, bundle rebuilt, smoke 124. Recorded against myself: for the second round running my *assertion* was wrong and the code was right — I had written that a refused blank memory should still produce one server call, when refusing before the round trip is the entire point — and again it was the assertion name carrying the counts (`blankCalls=0`) that caught it rather than letting it pass as green. Scope limits: memory is one store for the whole app, not per workspace, so the panel does not say which workspace an item came from; the panel has no search or edit, only list/add/forget; and the meter/list refresh when Settings opens rather than live. |
| `ApprovalRequired` | Task ID, approval context | Show approval affordance |
| 53 | 2026-10-10 | **Memory is per workspace**, which closes the first limit recorded against the previous revision and is what the workspace-as-aggregate-root model implies. The survey changed the shape of the work: `Workspace.memory_path` and its `pick` in `effective()` **already existed** and `caps::build_agent` already opens the store from the effective settings, so the data layer was never the gap — **nothing could set it**. So this is a threading round: `update_workspace` and `Settings::set_workspace_runtime` take `memory_path`, blank means the shared file (the same rule model, base URL, key and skills directory follow), and the workspace form gains one field. Because the agent cache is cleared on every workspace command and the agent is built from the effective settings, each workspace now opens its **own** memory read from a clean rebuild — no new plumbing, just reachability. The panel also names its owner ("Memory for One"), since a list you cannot check against the right file is a list you cannot trust. Two mistakes of mine are recorded rather than smoothed over: my call-site patcher inserted the new argument **before a closing paren that already had a trailing comma** and produced invalid Rust, repaired by a pattern fix on the one broken site (the test call, whose indentation made it distinguishable) rather than by reverting; and the new settings test panicked on `index out of bounds: the len is 0` because a fresh `Settings` carries **no** workspaces until `migrate()` adopts one for the current folder — the test now migrates first, which is also what load does. Verified: **104** Rust tests, **76** transcript + 48 shell checks at 500 and 2 000 rows (`owner="Memory for One"`, `update_workspace memoryPath=~/w1.jsonl` then blank = inherit), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. Scope limits: `memory_enabled` is still a global switch rather than per workspace, and the panel does not show the resolved file path. |
| `TaskCompleted` | Task ID, result summary | Show completion state |
| 54 | 2026-10-10 | **The narrow-window pass, done without eyes and therefore done honestly.** This session's model cannot read images, so a screenshot baseline was not an option and claiming a visual pass would have been a fabrication; instead the layout is **measured**. `tests/layout_selftest.sh` + `layout_probe.js` inject a probe and a Tauri shim into a copy of `index.html` and ask the browser for geometry at four widths, and the shim matters: without it the first Tauri call throws and everything below it in `app.js` never runs, so a probe that skipped it would click a button that does nothing — which is exactly what the drawer check did until the shim was added. Three findings, in order of how they were found. (1) The shell and the settings modal were already sound: no horizontal overflow, nothing off-screen, the modal shrinks (520/460px at 560/500) with no clipped content. (2) The sidebar's 280→56px "collapse" was **not designed** — it was main's min-width *squeezing* it, leaving a chat list that could not be read or clicked, and the geometry check was blind to it because the squeezed text overflows its own box rather than the viewport. The guard now tests the box's width against its rows, and it **failed on the real defect** (`sidebarSqueezed=True rows=4`) before any fix. (3) The fix is a deliberate drawer below 720px with a toggle in the chat head, and the drawer then introduced a real regression the guard also caught: the parked drawer hangs off the left edge, so the whole app could be **panned sideways** — verified by asking the browser to scroll (`scrolledX=348`), not by reading a `scrollWidth` number, which is harmless once something clips it. Clipping `html`/`body` (not just `#app`) settled it. Two honesty limits are built in: the probe disables the transition so it measures the settled drawer and **does not claim to measure the animation**, and Chrome on macOS clamps the window to ~500px, so 420px was never reachable and is no longer claimed. My own process notes are recorded too: several patch attempts died on quoting or on an anchor that had already changed, and one of those silent deaths meant a check I believed was running (the `drawer=` field was never printed) was not — the guard failing loudly is what exposed it. Verified: **0** layout failures at 1440/900/560/500 with the drawer opening to 280px and closing again, 104 Rust tests, 76 transcript + 48 shell checks, clippy clean, bundle rebuilt, smoke 124. |
| `TaskFailed` | Task ID, error details | Show error and available recovery actions |
| 55 | 2026-10-10 | **The extension surface is now visible from inside the app, and a factual error of mine is corrected in code rather than in prose.** The requirements list this document has carried since its early revisions said "12 hook events"; the agent's `HookEvent` enum has **seven** — before LLM call, after LLM call, before tool call, after tool call, on message, on complete, on error — so the list was wrong and nobody could have noticed, because nothing read it. `HookEvent::ALL` is now the one list a host may read, documented as needing hand-keeping in step with the enum, with a test that pins all seven names and rejects duplicates: adding a variant without listing it fails the suite instead of silently shrinking the surface the app advertises. The capabilities panel gained the hook surface from that list, shown **whether or not anything is registered**, because "what can I build against?" has an answer even in an empty agent. Plugins are a Rust trait (`AgentPlugin`), not a file format, so this round deliberately does **not** pretend the GUI can load one: the panel names the seam and its events and stops there, which is the honest reach of a thin host. To make the panel checkable the renderer was split into `renderCapabilities(host, caps)` — pure DOM work with the snapshot passed in — which the harness drives with a fixture, and a real improvement fell out of it: the panel can now be tested at all, where before it was reachable only through an async `invoke`. Verified: **111** agent lib tests (the new one among them), **104** gui tests, **78** transcript + 48 shell checks at 500 and 2 000 rows (`groups=Tools|MCP tools|Plugins|Hook events (Rust extensions)` with all seven named), **0** layout failures at 1440/900/560/500, clippy and rustdoc clean for both crates, bundle rebuilt, smoke 124. Scope limit, stated plainly: the GUI lists the hook surface; only Rust code in this repository can attach to it. A figure in this row was wrong when first written — it said 110 agent tests because a truncated `grep` hid the lib line — and was corrected in the same round by re-running the suite, which is the difference between checking a number and asserting one. |
| `SettingsChanged` | Setting key, version/value | Update affected UI |
| 56 | 2026-10-10 | **Per-workspace skills, and a method for the threading rounds.** `Workspace.skills_dir` and its `pick` in `effective()` also already existed, and `caps::build_agent` already registers skills from the effective directory, so as with memory the gap was reachability, not machinery: `set_workspace_runtime` and `update_workspace` take `skills_dir`, blank inherits the shared directory, and the workspace form gains one field beside the memory file. The Rust test mirrors the memory one exactly — blank yields the global path, a value yields that value — so the two per-workspace paths are pinned the same way. The method is the part worth keeping: last round a generic call-site patcher inserted an argument before a closing paren that already had a trailing comma and broke the file, so this time each `set_workspace_runtime(` call site was **counted** — paren-matched, arguments counted at top level — and only the sites still carrying eight arguments were touched. That found the real shapes (two one-line test calls among the multi-line ones) and its first attempt still failed, because a one-line call has no trailing comma to insert after; the four sites were then repaired by pattern and the argument inserted correctly. A second failure was the harness's own guard: a `SyntaxError` for a re-declared identifier, because every phase shares one script scope — the new phase's locals were renamed and the page loaded. Verified: **105** gui tests, **79** transcript + 48 shell checks at 500 and 2 000 rows (`update_workspace skillsDir=~/w1-skills` then blank = inherit), **0** layout failures at 1440/900/560/500, clippy clean, bundle rebuilt, smoke 124. Scope limits: `disabled_skills` is already per workspace and unchanged, but there is still no in-app skill authoring — the form points at a directory, and skills are files a person writes. |
| `DocumentChanged` | File ID, version/content state | Refresh document or show conflict |
| 57 | 2026-10-10 | **Approvals v2: once, this session, or always.** Approving each side-effecting call separately is the thing that makes people disable approvals altogether, so the card now offers **Allow for this session** and **Always allow** beside **Allow once** and **Deny**. The design keeps the decision where it belongs: the broker answers *before* it asks, so an allowed tool never raises a request at all — `request` short-circuits and returns an empty id, which is the same statement to the UI (there is nothing to answer). A session allowance is cleared when a **different session** becomes active and survives a new **turn** in the same session, which is what "this session" means; a test pins exactly that boundary, and another asserts an allowed tool mints no id and leaves nothing pending. An "always" is persisted in `Settings.allowed_tools` (global, not per workspace — stated plainly, because the per-workspace plumbing added two rounds' worth of signature churn for a switch that is really a cross-workspace convenience), adopted by every broker as it is built, and removable through `forget_allowed_tool`, which also clears the live brokers so a removal takes effect without a restart. Honesty about the harness: the allowance command is issued synchronously and is asserted as such; the answer that follows it is awaited, and the check's name says so rather than pretending both were observed. Two of my own mistakes are recorded rather than hidden: three insertions in a row landed in the wrong `Settings` initializer — first a brace walk that matched the empty `impl Default`, then a `Settings {` search that matched a `..Settings::from_config_value(..)` spread in a later function — so the compiler's own E0063 locations were used instead, and the field went into the real constructor at line 609; and clippy then caught a method I had written for a settings list that does not exist yet, which was **deleted** rather than annotated away, with its tests re-based on `is_allowed`. Verified: **108** gui tests, **81** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: the settings panel does not yet list or remove the persistent allowances (the command exists and the data is in `get_settings`); and `require_approval` alone still decides whether the gate exists at all. |

Use existing BOS event types and bus conventions if they already cover these needs. Avoid duplicating infrastructure.

Handle:

1. Late events from previously selected sessions.
2. Duplicate and out-of-order events.
3. Events arriving after a view is unmounted.
4. Reconnection and snapshot recovery.
5. Races between task completion and stop requests.

### 18.4 Settings persistence

Separate settings into:

- **UI preferences:** theme, font size, detail level.
- **Application behavior:** default permission mode, language, view toggles.
- **Server-owned account data:** balance, usage, account identity.

Use the existing BOS configuration system for local preferences if appropriate. Account data must come from the account service, not be stored as ordinary local preferences.

Suggested update flow:

1. Validate input.
2. Build a candidate configuration.
3. Persist through the configuration interface.
4. Commit the saved state.
5. On failure, retain recoverable edits and report the error.

Theme and font size may use optimistic preview state, but distinguish preview values from persisted values.

## 19. Testing and quality gates

Add tests incrementally with each story:

- Event-reducer and event-ordering tests.
- Settings serialization, deserialization, and invalid-value tests.
- Three-panel layout and window-resize tests.
- Theme and font-size switching tests.
- Large document and large session-list performance tests.
- File-save and settings-persistence failure tests.
- Regression tests for switching views during long-running tasks.

Before merge, run the repository's existing formatting, lint/static-analysis, unit-test, and platform-build checks. Confirm actual commands from the repository rather than assuming a particular workspace configuration.

## 20. Implementation sequence and risk controls

### Phase 0 — Repository reconnaissance

**Initial estimate:** 1–2 working days

Inspect:

- `crates/gui/Cargo.toml` and module tree.
- Existing main-window and layout implementation.
- Event bus and agent interfaces.
- Configuration loading, persistence, and theme mechanisms.
- Existing tests and CI constraints.

Deliverable: a verified map from this specification to real Rust modules, interfaces, and reuse opportunities.

### Phase 1 — Workbench foundation

**Stories:** GUI-01–03

Stabilize layout, navigation, panel resizing, and design tokens.

### Phase 2 — Agent state and interaction

**Stories:** GUI-04–06

Establish the real task-state and event-driven interaction path.

### Phase 3 — Documents and settings

**Stories:** GUI-07–12

Implement the document panel, settings persistence, and account-service adapter.

### Phase 4 — Acceptance and hardening

**Story:** GUI-13 plus regression tests

Validate cross-platform layout, long-running-task stability, accessibility, and error recovery.

### Risk register

| Risk | Impact | Mitigation |
| 58 | 2026-10-10 | **The loop from last round is closed: the permanent allowances are visible and removable where the gate is configured.** The list sits under the approval checkbox, because that is where someone looks when a tool has stopped asking, and its empty state says *Approvals are asked every time* rather than showing nothing. Removing one calls `forget_allowed_tool`, which clears the broker's live allowance as well as the stored list — the round-78 command existed precisely so that a removal did not need a restart, and the UI now reaches it. The renderer is a pure function of the names, so the harness drives it with a fixture; the removal click is asserted synchronously on the command it issues, and the awaited reply is what would redraw, which the never-settling promise keeps out of the assertion. The harness's single script scope bit me a **second** time — a `SyntaxError` for a re-declared `rows` — so this phase's locals are prefixed like the last one; two occurrences make it a convention rather than an accident, and the convention is to prefix a new phase's locals. Verified: **108** gui tests, **82** transcript + 48 shell checks at 500 and 2 000 rows (empty state plus two rows with `forget_allowed_tool:bash`), **0** layout failures at 1440/900/560/500, clippy clean, bundle rebuilt, smoke 124. Scope limits: `require_approval` still decides whether the gate exists at all, and an allowance is global rather than per workspace, which last round's entry states and this one does not change. |
|---|---|---|
| 59 | 2026-10-10 | **The signature churn ends, and a claim I had carried for many rounds turns out to be false.** Every per-workspace field this project has added lately — memory file, skills directory — was threaded by lengthening one positional call and then repairing every call site, twice breaking a file doing it. `set_workspace_runtime` now takes a grouped `WorkspaceRuntime`: nine arguments became two, `#[allow(clippy::too_many_arguments)]` is gone because it is no longer needed, and the call sites became **shorter** (a blank payload is `&WorkspaceRuntime::default()`), which is the sign the grouping was right. The round's premise was wrong, and that is the more useful finding: my notes have said *memory\_enabled is still global* for many rounds, but `Workspace` already carried `memory_enabled: bool` and `effective()` already picked it — the grep that settled it shows both the field and the assignment in the block whose comment reads "booleans, budget and the selection sets are the workspace's own". What was actually missing was any way to **set** it: `adopted_from` copies the global at creation and no command ever touched it. So the deliverable is the reachable switch — the command takes `memory_enabled: Option<bool>`, the workspace form carries a checkbox, and a payload that omits the field restates the workspace's existing value rather than reading an absent checkbox as off. My own drift is recorded as well: three insertions in a row landed in the wrong structure or field (a brace walk that hit the empty `impl Default`, a `Workspace {` search that found a spread in a later function, and a replacement that removed the runtime's field instead of the duplicate), each time because I anchored on remembered text instead of reading the file; the fix was to use the compiler's own E0063 locations. Also cleaned up: a `redundant_field_names` clippy error, two generated expressions that double-referenced (`Some(&x.to_string())`), and one that wrapped a `String` in `Some`. And the harness caught a **second** assertion of mine that was wrong rather than the code: I asserted an absent checkbox would omit the key, where the implementation restates the existing value — equally safe, differently shaped — so the claim was corrected and the check renamed to say what is true. Self-check on the record itself: this row was first written while clippy still reported a `needless_update` error — adding the switch made the command's literal specify every field, so its `..Default::default()` had become redundant — and the row nonetheless claimed clippy clean. The error was fixed and the claim re-verified before the round closed; the deeper mistake is that the conditional which decides whether to record keyed on the harness being green while clippy was still red, and that is worth more than the one-line fix. The first attempt at that one-line fix used an anchor that did not match — `memory_enabled,` is not adjacent to the update — and failed silently, which is the same anchoring habit in miniature: the second attempt located the line by the command's own position in the file and clippy went to zero. Verified: **109** gui tests (one flips the switch through the payload, one confirms no opinion leaves it alone), **83** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean, bundle rebuilt, smoke 124. |
| Existing GUI framework differs from assumptions | Rework | Inspect framework and capabilities first |
| 60 | 2026-10-10 | **The seam between Rust and the webview is now guarded, and the guard is proven able to fail.** The round began by going looking for work and finding none where I expected it: `export_session` exists as a command *and* is already reachable through a palette `/export` entry, and a toast helper already exists — so instead of building a feature that was already there, the round cross-checked **every** command against **every** UI call site in both directions. The result is a clean 38/38 with no unreachable commands and no phantom calls, which is a stronger statement than "export works" and a cheap one to keep true: `tests/wiring_selftest.sh` fails if a registered command is never invoked (a feature nobody can reach) or if the UI invokes a name that is not registered (a call that dies at runtime), and reports both lists. A guard that has never been seen red is unproven, so it carries `--self-test`: it copies the tree, plants a registered command nothing calls, and requires the guard to fail — the same discipline as the layout probe's numeric rationale. Two mistakes of my own are recorded: the first patch attempt nested a heredoc whose delimiter (`PY`) terminated the outer block early, so nothing was written and the leftover lines ran as shell (the delimiter is now distinct, and the failure was loud rather than silent); and then the guard printed exactly the right thing while my *self-test* matcher was wrong, because Python's list repr quotes the name (`unreachable=['ghost_command']`) and I matched an unquoted form. That is the third time this project has shown the same pattern — rounds 72/73 and 80 also had my assertion text wrong rather than the code — so it is stated as a pattern here rather than as three anecdotes: in this project the most frequent defect class is my own expectation, and the harness is where it surfaces. Verified: guard green in both modes, **109** gui tests, **83** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean. Scope limits: the guard is a static text scan, so a command name built at runtime would be invisible to it, and it checks names only — not that a call's arguments match the command's parameters. |
| Duplicate or out-of-order events | Incorrect state | Validate task IDs and sequence/version numbers |
| 61 | 2026-10-10 | **The workspace form now covers the fields the workspace owns, found by counting rather than by assuming.** Last round's guard proved every *command* is reachable; this round asked the same question one level down — is every **field** settable? The inventory compares the 23 `Workspace` fields against the 16 `update_workspace` arguments and the 11 payload keys, and it found eight with no path from the UI: `providers`, `created_at`/`updated_at` (derived, correctly not settable), and **five genuine per-workspace policy fields** — `bash_enabled`, `file_tools_enabled`, `require_approval`, `context_budget`, `project_instructions` — each reachable only as a **global**. They were frozen at workspace creation, because `adopted_from` copies the globals once and no command ever touched them after. All five now travel end to end: `WorkspaceRuntime` gained them, the command carries them, and the form renders a checkbox each plus a budget input. That turns the previous round's one-off `Option` field into a **rule** worth stating rather than an exception: in this payload a `String` means blank-inherits, and an `Option` means an absent field is not an answer and the workspace keeps its own. Three pure helpers (`workspaceCheck`, `workspaceFlag`, `workspaceBudget`) keep that rule in one place and make it testable; the harness proves the restate path with a **bare form** and proves an unusable budget restates the old count instead of sending zero. My own slips: two anchors missed again because `cargo fmt` re-indents, and the compiler's `E0063` was again what located the literal — the same lesson as round 80, which suggests the reliable habit is to anchor on *positions the compiler reports*, not on remembered text; and a display slip, where the field-list print omitted the list even though the diff under it was computed correctly. Also worth recording that this round's assertions were right the first time, after three rounds in which the harness caught my expectation instead: the correction in round 81 was to phrase the claim as "what the printed shape actually is", and that is what worked here. Verified: **110** gui tests, **84** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring guard green in both modes, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: `providers` still has no UI path and is left out deliberately, since how a workspace chooses among providers is a design question this round did not answer; creating a workspace still adopts the globals, which remains the documented behaviour. |
| Task lifetime tied to window lifetime | Tasks stop unexpectedly | Keep task ownership in the application/service layer |
| 62 | 2026-10-10 | **The last undiscovered surface was a config file: a profile is selected by typing its name, and nothing listed the names.** The same counting that found the workspace fields, applied one level up, compared all 25 `Settings` fields against every name in the webview: exactly two never appear — `config_api_key`, correct by design because it is the *resolved key* of whichever profile was chosen, and `profiles`, which is a real gap. `profiles` holds the `[llm.<name>]` sections discovered from the config file at load time and **deliberately never serialized**, and resolution works by matching a profile whose `name` equals `model` — so the only way to use one was to know its name from the config file. The GUI now lists them, and the list is **key-free by construction**: `ProfileInfo` carries `name`, `model`, `base_url` and a `has_key` flag, and a test serializes the payload and asserts the secret string is absent from it. A chip types the name for you, which keeps one place where a model is configured rather than two. The round-81 wiring guard did exactly what it was built for, one round after it was written: registering the command before wiring it made the guard go **red** with `unreachable=['list_profiles']`, and it went green once the UI called it — a guard that catches real drift, not only planted drift. Second, round 80's fix for the record itself worked: clippy flagged `field_reassign_with_default` in the new test, so the conditional **refused** to write this row until the test was restructured as a struct literal and every gate re-ran green — previously that row would have claimed a clean clippy it did not have. My other slip is recorded without a diagnosis I did not earn: the first patch script did not parse, because a `, 1)` from a `.replace` idiom was pasted into a slice assignment, so **nothing was written** and the anchors stayed fresh; and the command body first used `state.inner.lock()`, which I did **not** establish whether it failed on its own or as a cascade from the unresolved `settings::` path in the same signature — so the record says what was observed, and the fix followed a census of the file, where `state.lock()?` is the dominant idiom. Verified: **111** gui tests, **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring guard 39/39 green in both modes, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: profiles are read-only in the GUI — they come from the config file and are never written back — and picking a chip fills the model field without saving it; `config_api_key` stays config-file-only on purpose. |
| UI and persisted settings diverge | Preferences lost on restart | Define explicit persistence and recovery behavior |
| 63 | 2026-10-10 | **An audit of the document's own numbers, whose first result is a negative — and the negative is the important part.** The requirements doc exists to describe *other* tools, so most of its figures are findings about Codex or DSH rather than claims about this code, and reading 41 claim-bearing body lines in context showed that the ones which look wrong are right: the **12 hook events** and **~57 slash commands** are Codex's, and the geometry figures (a 400 px centre, a 45 % right panel capped at 70 %, a 264–420 sidebar range) describe Codex's and DSH's layouts, not ours — I checked them because they read like implementation claims, and they appear nowhere in our CSS or JS, correctly. That distinction is exactly what bit me in round 76, when I read "12 hook events" as a claim about us and changed our code and doc over it; the doc was right about Codex, and our own count is 7. Two numbers **about us** were genuinely stale, both in the section whose whole job is to be trustworthy. The baseline said tests **555/0 (63 suites)**; measured with the doc's own command it is **581/0 (63 suites)** — the suite count was exactly right and the passed count had drifted by 26 as the project added tests. And the measurement note credited the **47** shell-contract checks to `ui_selftest.sh`; they are 48 and live in `shell_selftest.sh`. The fix is aimed at the class rather than the instance: R7's baseline line now names all four guards and their counts, states that they must be **re-measured rather than trusted because this number has been stale twice**, and records the widths that are actually measured plus why 420 is unreachable (Chrome clamps a window to about 500 px, so a 420 run reports 500). Scope limit I did not fix this round: the doc still mixes other tools' findings with our own implementation claims without marking every line, so a future audit has to read context to tell them apart — one sentence in the header says it, the body does not enforce it. Verified: the four counts above are what the scripts printed while writing this row (ui 85, shell 48, layout 0 failures, wiring 39/39), 111 gui tests, clippy and rustdoc clean. |
| Document rendering blocks UI thread | UI freezes | Separate loading/parsing from presentation where supported |
| 64 | 2026-10-10 | **Search stopped re-reading the store on every keystroke, and the measurement falsified my explanation of why it was slow.** `SessionStore::search` ran on every keystroke — the UI asks from two characters — and each call re-read and re-parsed every session file. It now parses from a cache keyed by the file's `(modified, length)`, dropping entries whose file is gone or has stopped parsing, so it holds at most the live sessions. `load_all` was deliberately left as a plain read: it has thirteen call sites and is not on the keystroke path, so churning them would have bought nothing. Measured by an ignored benchmark — `cargo test -p gui search_benchmark -- --ignored --nocapture`, 200 sessions × 30 messages, one matching message so every query scans everything — the first query costs **32.6 ms** and the following query **12.8 ms**, a **2.5×** improvement. Then the part worth recording: I predicted the per-query `String` clones of every message dominated and removed them; the measurement moved 13.22 ms → 12.83 ms, which is noise, so **the prediction was wrong** — the cost is the per-message `Vec<char>` allocations inside `hit_for`, which the clone removal does not touch. The clone removal is kept because it allocates less and changes no behaviour, but it is not claimed as the win, and the win is the parse cache alone. Limits stated rather than papered over: the warm query is still a full in-memory scan (12.8 ms at 6 000 messages, fast enough to be interactive and the next lever is an index well beyond this corpus size, which is not built); the benchmark is evidence for a person and not a gate, because a busy machine makes a timing flaky, which is the same treatment the layout numbers get; and one case can serve stale data — a rewrite that changes neither the file's length nor its timestamp inside the filesystem's granularity — which the code documents and the invalidation test pins for the case that matters, since any ordinary rewrite changes the length. Verified: **112** gui tests (6 ignored, the benchmark among them), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures, wiring 39/39 in both modes, clippy and rustdoc clean, smoke 124. |
| Permissions enforced only in UI | Security boundary failure | Enforce permissions in the actual execution layer |
| 65 | 2026-10-10 | **The optimisation I wrote was wrong, and the test written to attack it is what caught it — the numbers come after that.** Last round's measurement left the warm query at 12.8 ms, dominated by the `to_lowercase()` copy and the character vector `hit_for` builds for every message. Search now runs a pre-filter first that allocates nothing and can only skip work, and it is exact only where exactness can be **proven**: an ASCII needle against ASCII text, where byte equality under ASCII case folding is precisely the comparison the slow path makes. Everything else falls through. The first version of that condition read `needle.to_lowercase() == needle`, which proves **already lowercase**, not **has no case** — so it sent `quick` down the byte path, where `Quick` no longer matched. Both the pre-existing cross-session search test and the Unicode test written specifically to attack the fast path failed, so the claim was never made and the condition was narrowed to the provable case. This is the sharpest instance yet of the pattern recorded in round 62 — that the most frequent defect class in this project is my own expectation — except that here the expectation was embedded in an **optimisation** and would have silently dropped real search results had the tests not existed. Measured with the ignored benchmark: the warm query went **12.65 ms → 5.25 ms** (2.4×) and the cold one 32.6 → 25.4 ms; across the two rounds the warm query is **12.8 → 5.25 ms**, about **6×** against the 33 ms it started at. And the unsound version measured **1.97 ms** — 2.7× faster than the correct one, for the instructive reason that it called `str::contains` (a two-way search) while the sound version runs a hand-rolled `windows().any(eq_ignore_ascii_case)` loop. That makes the next lever measured rather than guessed: a **cached lowercase haystack** per session inside the existing parse cache, which would let every needle, CJK included, use `str::contains`. It is not built this round. Limits: non-ASCII and CJK searches keep the old cost, since no fast path is claimed for them; the benchmark stays evidence for a person and not a gate. Verified: **113** gui tests (6 ignored), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures, wiring 39/39 in both modes, clippy and rustdoc clean. |

## 21. Acceptance checklist

- [ ] Main workbench contains navigation, agent workspace, and document panel.
- [ ] Panels scroll independently and can be resized.
- [ ] New session, workspace selection, and session selection work.
- [ ] Chat and Trajectory switch without corrupting task state.
- [ ] Running, approval, success, failure, and cancellation states are accurate.
- [ ] Document tabs, paths, and Markdown rendering work.
- [ ] Settings category navigation and content scrolling work.
- [ ] Account, balance, usage, and top-up entry points have defined behavior.
- [ ] Permission, language, appearance, font size, work-detail, and coding-view settings work.
- [ ] Preferences persist and restore after restart.
- [ ] Light, Dark, and System themes work.
- [ ] Resizing does not permanently obscure critical controls.
- [ ] Loading, persistence, and service failures produce actionable feedback.
- [ ] UI visibility does not bypass permission checks or unintentionally cancel tasks.

## 22. Scope boundaries and open questions

The screenshots do not establish:

- The current GUI framework or exact Rust module layout.
- Exact pixel dimensions or responsive breakpoints.
- Account-service protocols and balance business rules.
- The semantics of each permission mode.
- The configuration file schema and persistence mechanism.
- Behavior of pages not shown in the screenshots.

Resolve these questions by inspecting the existing BOS code, configuration definitions, and service interfaces before implementation. Do not treat visual assumptions as confirmed business requirements.

## 23. Recommended next step

Perform a read-only architecture review of `crates/gui`, then map each story to actual files, types, and tests. Identify what can be reused before creating new abstractions. Only after that review should implementation-specific APIs and file-level tasks be finalized.

## 24. Workspace model (the aggregate root)

The three requirements stated in review converge on one change of shape: **a
workspace owns the things that depend on it**, instead of each of those living as a
global setting that happens to point at one folder.

### 24.1 Ownership

| Owned by the workspace | Stays global (device/runtime) |
| 66 | 2026-10-10 | **The lever round 65 measured, built — and the round that recorded nothing, which is what made the next round's anchor fail.** A cache entry now carries the lowercase copies search would otherwise rebuild, `(text, reasoning)` per message and index-aligned with the record, so the pre-filter is an **exact** `str::contains` on already-lowered text instead of the hand-rolled ASCII window scan that could only skip work. Exactness is the point: it is the same condition the slow path checks, so it serves any needle rather than only ASCII ones. Measured by the ignored benchmark with the same corpus: the ASCII needle's following query fell **5.25 ms → 1.99 ms**, and against the 33 ms where round 65 started, about **15×**. What this round did **not** establish is stated rather than implied: the benchmark still used only an ASCII needle, so the CJK claim — the whole reason the cached lowercase matters — was **left unmeasured**, and the attempt to add a Chinese needle failed on anchors `cargo fmt` had reflowed, so it was carried into the next round. Two further slips, both mechanical, both found by tools rather than by me: a multi-line `record.as_ref()` pattern did not match for the same reflow reason and the compiler's **E0599** located the line, which I read and fixed; and removing the old `may_match` helper left a blank line between a doc comment and its function, which `-D warnings` promotes to an error — so the recording attempt for this round **failed its gate and wrote nothing at all**. That refusal is the round-80 repair working, and the nothing-recorded is exactly what made the next round's stale anchor possible, which the next row records. |
| --- | --- |
| 67 | 2026-10-10 | **CJK measured, and the limit round 65 named is closed; then the recording step failed and told the truth about nothing.** The benchmark now prints two needles, because a CJK claim needs a CJK number rather than an inference from an ASCII one: an ASCII needle's following query is **2.17 ms** and a CJK needle's query is **2.37 ms** — the same cost as the warm ASCII one, so a Chinese query no longer pays the cost the ASCII-only fast path could not avoid. The bench's shape has to be read honestly: only the *first* needle's first query pays the parse, so the CJK row's printed **1.0×** ratio is an artefact of run order and **not** evidence of no speedup; the meaningful comparison is CJK-warm against ASCII-warm, which is the pair above. Then the failure worth more than the numbers: the row for this round was anchored on `\| 66 \|`, a row that **did not exist**, because the previous round's gate had refused to record anything — the script raised and printed nothing, **but the shell still ran `jj describe`**, so the commit description claimed a row the document did not contain. Repaired by writing both missing rows and by moving the guard one level further down, which is the round-80 repair applied to the recording step itself: the description must not be submitted until the row is **asserted present**. A measurement caveat is corrected here so it is never misquoted: my `clippy` counter also matches two `could not compile` summary lines, so it reports **3** for a single error — it is a presence test, not a count, and no count is quoted from it. Costs: memory is roughly the live sessions' text again, which the struct documents; the cache keeps its one documented stale window. Verified: **113** gui tests (6 ignored), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring 39/39 in both modes, clippy and rustdoc clean, smoke 124. |
| root folder, name, id | window layout, sidebar/doc geometry, theme |
| 68 | 2026-10-10 | **Compaction stopped being destructive.** `/compact` rewrote a chat's history into one anchored user turn plus an LLM summary and **discarded every folded turn** — a lossy summarizer was the only remaining copy, and there was no way to see or recover what it dropped. A session now keeps the folded turns in `archived` (a `#[serde(default)]` field, so existing session files still load and older ones simply have none), the write-out excludes the anchor that the live history retains — archiving it would put the same turn in the transcript twice on a restore — and `/compact` can be undone. Two commands carry it: `compacted_archive` reports how many turns are folded, so the UI offers the affordance only when it can do something, and `restore_compacted` puts them back. Restore **merges rather than replaces**: the folded turns are older, so they go **in front** of everything since, because a plain restore would silently delete every turn the user had after compacting — which is the same class of loss the feature exists to prevent, and it has its own test. The transcript gained a clickable **`⧉ N folded · restore`** status chip and a `/restore` palette entry; the chip is a `button` because unlike the other status chips it does something, and it renders nothing at zero. Verified: **116** gui tests (6 ignored) including three on the archive and restore paths — compaction keeps what it folds, restore merges instead of replacing, and restore refuses when nothing was folded — 86 transcript + 48 shell checks, 0 layout failures, wiring **41/41** with the two new commands reachable and no phantom calls, clippy and rustdoc clean. Two of my own slips, both caught by tools: `cargo fmt` reflowed my assertion lines so the anchor missed and the patch wrote nothing (fifth time this session — the reliable anchor is the shape the file prints), and my replacement test asserted five messages where the merge produces six, so the **test** was wrong and the implementation was right. This round also re-measured and corrected the baseline line rev 63 introduced: it had drifted to **586** tests and 41 commands, which is exactly what that line now instructs a reader to do rather than trust. That number is a small self-correction worth keeping: I first wrote 584 in this row by **adding three to the old total instead of measuring**, which is the very habit this row is about, so the row now carries the measured value. |
| sessions (the whole conversation store) | keyboard shortcuts and shortcut profiles |
| 69 | 2026-10-10 | **A folded turn can be read before it is restored.** Round 68 made compaction recoverable but left the reader with a count and a button: the folded turns could be put back, not inspected, so the summary was still the only thing anyone could see. The chip now **opens** them in the document panel, through a new read-only `archived_messages` command, and restoring moved to its own `↺ restore` control beside it — viewing and undoing are different decisions, and one control cannot honestly be both. The preview is a pure `archivedPreviewText(messages)` renderer, so the harness can check what a reader sees without a backend: the turns are listed in order with their roles, each is clipped at 1200 characters with the remainder counted, an empty turn says so, and an empty archive says there is nothing folded rather than showing an empty panel. Wiring is now **42/42** with the third new command reachable and no phantom calls. Verified: **586** workspace tests over 63 suites, 87 transcript + 48 shell checks at 500 and 2 000 rows, 0 layout failures, clippy and rustdoc clean, smoke 124. Scope stated rather than implied: this round added **no Rust test**, because the new command is a read whose behaviour the store tests already cover — its reachability is checked structurally by the wiring guard and its count by the store tests, which is less than the behavioural coverage the archive and restore paths got last round. |
| LLM binding: model, base URL, API key, failover providers | the `[llm.<name>]` profile catalogue discovered from BOS config |
| 70 | 2026-10-10 | **Deleting a chat stopped being permanent.** `SessionStore::delete` was a bare `fs::remove_file`, so deleting a chat — or choosing to delete a workspace's sessions — unlinked it immediately and irreversibly, which **contradicted the rule this project already states for workspaces**, that removing one must never silently destroy sessions, and left a mis-click with no recourse. Deletion now **moves** the file into `.trash/` beside the store; the name carries no `.json` extension, so `load_all` and the search cache skip it exactly as they skip any other non-session entry, and a test pins that a deleted chat does not come back through search. The store gained `trashed()` — parsed rather than listed by filename, because a bare UUID is not something a person can recognise — plus `restore(id)`, which returns the title, and `purge_trash()`, which is now **the only place a session file is unlinked**. Three commands expose it (`trashed_chats`, `restore_session`, `purge_trash`), the settings panel gained a Deleted chats list with a per-chat restore and a confirmed Empty the trash, and the delete confirmation now says where the chat went instead of asking a question with no stated consequence. Both delete paths route through the trash, so the workspace path is recoverable too. Verified: **590** workspace tests over 63 suites — four new store tests among them, covering that deleting moves and restoring brings the chat back with its title, that a trashed chat stays out of the store and out of search, that purge is the only permanent removal and is idempotent, and that restoring something never trashed says so — with 88 transcript + 48 shell checks, 0 layout failures, wiring **45/45** with all three new commands reachable and no phantom calls, clippy and rustdoc clean, smoke 124. The gate refused this row twice before it was written, and both times for the gate's own reasons rather than the feature's: clippy first failed on an unnecessary `mut` **in one of my own new tests** — the round-80 repair catching a slip that testing alone would have missed — and then my recording command backgrounded the `cd` along with the compiler, so the guards were addressed one directory too deep and reported "not found" until they were called by absolute path. Limits stated rather than hidden: the trash has **no automatic retention or cleanup** yet — the spec's M6 asks for a 30-day policy and none is built, so the trash grows until it is emptied, which is a real cost of this change; and restore does not rebuild the cached agent, because the next turn does that from the record's own workspace field, which deletion never touched. |
| tools: bash/file tools on/off, approval policy, context budget | approval policy *default* used when a workspace is created |
| 71 | 2026-10-10 | **The shell now describes itself to assistive technology, and a guard reads the markup that no other check reads.** An audit of the shipped accessibility surface found four real defects, all fixed: the command and mention popups put `role="listbox"` on a **wrapper** while the options sat in a `ul` inside it, so the listbox did not own its options; option rows carried `role="option"` but no `aria-selected`, and nothing linked the composer to the active option; neither modal was a dialog at all, so a screen reader had no idea a modal had opened or what it was for; and `closeSettings` hid the dialog while leaving focus on a hidden control, which strands a keyboard user at the top of the document. `#cmd-list` and `#mention-list` now own their listboxes, options have stable ids and `aria-selected`, and the composer carries `aria-controls`/`aria-expanded`/`aria-activedescendant`. Both modals became `role="dialog" aria-modal="true" aria-labelledby` with real label ids, `closeSettings` hands focus back to the element that opened it, and the approval dialog takes focus on its explicit choice — **Escape deliberately still does not dismiss an approval**, because an approval has to be answered rather than skipped past. The three icon-only buttons (`✎`, `✕`, `✕`) gained `aria-label`s. Verified by a new structural guard, `markup_selftest.sh` (**25** checks), added because **the transcript harness builds its own fixtures and never loads `index.html`**, so none of this is reachable from it; the invariants are structural, so they are checked structurally. Two instrument failures are recorded rather than buried. First, the gate had been invoking `tests/shell_selftest.sh`, which **does not exist**: `$SH` came back empty and the shell section was never enforced, while the "48 shell checks" quoted in earlier rows came from `ui_selftest.sh`'s own output — **observed, but not gated** — and the gate now extracts that line from the run that actually produces it. Second, the new guard's first version failed on the **correct** markup, because I matched a remembered string (`id="cmd-list" role="listbox"`) when the file has `class` between the attributes: the sixth time this session that an anchor written from memory, rather than the shape the file prints, was the defect. The guard now matches the tag with a regex, so attribute order is no longer mistaken for an invariant. Verified: **590** workspace tests over 63 suites, 120 gui tests, 88 transcript + 48 shell + 25 markup checks, 0 layout failures across five widths, wiring **45/45**, clippy and rustdoc clean, fmt clean, smoke 124. Scope stated: this round added **no Rust test**, and the accessibility work is verified **structurally, not behaviourally** — focus hand-back, the approval focus move and the palette's assistive state are asserted as source invariants, because no harness loads the shipped page; closing that gap needs a harness run against `index.html` itself, which does not exist yet. |
| skills directory, `AGENTS.md`-style instruction folding | telemetry/updates, default workspace id |
| 72 | 2026-10-10 | **The shipped page is now executed by a guard, not just read by one.** Round 71 recorded its own limit: every harness in this directory builds its own fixtures, so `ui/index.html` was never run by any check and the accessibility work could only be asserted as source invariants. `page_selftest.sh` assembles a temporary page from the shipped bytes — the markup verbatim, a `<base>` so its relative script and style URLs resolve, the Tauri stub injected ahead of `app.js` exactly as the transcript harness does, and `tests/page_checks.js` appended last — and asserts against the real DOM. It settles the two claims that were previously unproven: **closing settings really does hand focus back to the opener** (`shown=true, active=sidebar-toggle`), and **Escape closes it and hands focus back too**. It also drives the real composer to open the real palette and finds a listbox that owns its 7 options with exactly one `aria-selected`, the composer naming the active option by id, and both cleared when the palette closes. The two runners now share `read_result.py`, so the missing-result diagnosis and its hard-won load-error lesson live in one place rather than being copied; re-running the transcript harness after that refactor reproduced 88 + 48 checks unchanged. Two instrument failures are recorded, both mine, both caught loudly rather than passing quietly: the first assembly put `<base>` **after the body opened**, where the HTML spec says it is ignored, so `app.js` resolved against `/tmp` and never loaded — the runner reported "no result element … page did not finish" for a page that had never been parsed; and the temporary page was created by `mktemp -t` **without a `.html` extension**, which Chrome will not render as a document from a `file://` URL, producing the same loud failure. Verified: **590** workspace tests over 63 suites, 120 gui tests, 88 transcript + 48 shell + 25 markup + **8 shipped-page** checks, 0 layout failures across five widths, wiring **45/45**, clippy and rustdoc clean, fmt clean, smoke 124. Limits stated: the harness **stubs the Tauri host**, so bridge-dependent behaviour stays out of its scope; it awaits only promises that settle in a microtask, so anything needing a timer or the event loop cannot be checked this way; and the approval dialog's **focus-on-show is still not behaviourally verified**, because the approval queue it reads is module-private — only its explicit choice being a live focusable button is asserted. |
| MCP servers, memory store and its file | the workspace *list* itself (see 24.3) |
| 73 | 2026-10-10 | **A chat can now state what it amounts to.** M5 listed a turn outline and session stats; neither existed. `SessionRecord::overview()` computes messages, asks, replies, tool calls, errored turns, visible characters, reasoning characters and the folded count, plus one outline entry per ask, and `session_overview` serves it read-only (wiring **46/46**). Three decisions are worth stating. Counts **include the turns a compaction folded**, because "how big is this chat" means the whole file rather than what survived the last fold, and `archived` reports how many of them are folded so a reader can tell the difference. Preview text is clipped **by characters, not bytes** — a byte slice of multi-byte text either panics on a non-boundary or produces mojibake, and this transcript is explicitly multi-lingual — which is exactly what the third test pins. And the status chip renders **nothing until an overview arrives**, so switching chats never shows the previous chat's numbers. The UI adds a status-bar chip (`12 msgs · 4 tools · 3.2k chars`, abbreviated by a pure `fmtCount`) and a `/stats` panel with the full breakdown and the numbered outline, built by a pure `overviewLines` that emits text and never touches an HTML sink. Verified: **593** workspace tests over 63 suites — three new ones, covering counts that include folded turns, an empty chat reading as all zeroes, and previews clipped by characters — with 89 transcript + 48 shell + 25 markup + **8 shipped-page** checks, 0 layout failures, wiring **46/46**, clippy and rustdoc clean, fmt clean, smoke 124. The gate refused this row **three times** before it was written, and none of the three was the feature. First clippy failed on `9 + 0 + 12 + 10` in a new test (`identity_op`), where the expectation was right and the expression was clumsy. Then `cargo fmt` reflowed two assertions — caught only because the gate grew a formatting condition in an earlier round, and invisible in every number it printed. Finally the gate refused a run in which **every check had passed**: my extraction read `failures: 0 / 89` with a last-number pattern, so it compared the 89 *totals* against zero. That is why this run echoes each sub-condition rather than a summary line, and why the same run re-confirmed that `grep -cE '^error'` reports one clippy error as two: only a compile-failure count is honest there. The M5 row in the milestone table was updated to say what is done and what is not, rather than left to imply the milestone landed. Limits: the outline is **text in the document panel and does not jump** to the turn it names; search is still a **live scan**, so an FTS index remains unbuilt; and **ZIP export does not exist**. |

Rationale: every row in the left column is meaningless without a root folder, and
every row in the right column is meaningless *with* one. Sessions are the strongest
case — a conversation about a repo cannot move to another repo and stay coherent,
because its tool calls, approvals and instructions were that repo's. **Landed**: `SessionRecord.workspace`
is written when a session is created (`create_session` stamps the active workspace),
inherited by `fork_from`, and reported on every `SessionSummary`; a file written
before workspaces existed loads with an empty value, which summaries resolve to the
workspace in use, so no chat disappears from the panel during migration.

### 24.2 Schema

```rust
struct Workspace {                 // one file per workspace: workspaces/<id>.json
    id: String, name: String, root: String,     // root = absolute path, "" = cwd
    model: String, base_url: String, api_key: String,
    providers: Vec<ProviderEntry>,
    bash_enabled: bool, file_tools_enabled: bool,
    require_approval: bool, context_budget: usize,
    skills_dir: String, project_instructions: bool,
    mcp_servers: Vec<McpServerEntry>,
    memory_enabled: bool, memory_path: String,
    system_prompt: String, temperature: f32, reasoning_effort: Option<String>,
    created_at: u64, updated_at: u64,
}

struct Settings {                  // app-level, rewritten from today's Settings
    workspaces: Vec<Workspace>,
    active_workspace: String,      // workspace id
    default_workspace_model: String,   // seed for a new workspace
    // global: sidebar/doc geometry, theme, shortcuts, approval default
}

struct SessionRecord {             // gains one field
    workspace: String,             // workspace id; missing in old files
}
```

Resolution for any owned value: **workspace value → BOS config discovery →
built-in default**. A workspace field left empty means "inherit", which is what
makes the migration and the "use defaults" affordance trivial.

### 24.3 Migration (lossless, one release)

On first load after the change, today's global settings become a workspace named
after `settings.bash_workspace` (or "Default" when empty), every session file
without a `workspace` field is adopted by it, and `active_workspace` points at it.
**Landed** as `Settings::migrate` (called from both `load_from` paths, before the
environment overrides, which then reach the workspace in use as the outermost
override); a dangling `active_workspace` is repaired to the first workspace, so a
hand-edited settings file cannot leave the GUI with nothing selected.
Nothing is dropped: the old values are copied, not moved, so a downgrade still finds
them. Session files keep loading individually, so a partially migrated store is
never fatal.

### 24.4 Left panel

```
BOS                                    + New in <workspace>
[ search chats… ]
▾ bos                                  ~/Projects/bos        nvidia/…-flash
    ● gui transcript work                        2m
    ▸ plan for M8                                 1h
▸ harness-notes                        ~/notes
[ + Add workspace ]
──────────────
Plugins & skills · MCP servers · Automation (M7)
```

A workspace row is the switcher: it shows the root (clipped, full path on hover) and
the model bound to it, expands to its sessions, and carries rename / set-root /
duplicate / remove in a context menu (remove asks what happens to its sessions:
keep them in a rescued workspace or delete them). Session search stays global but
its results are grouped by workspace. The nav entries below the list keep opening
the unified settings surface (§ 12.1), where a **Workspaces** section owns the
management table (add, rename, root, model, remove, "use defaults").

### 24.5 Commands

`list_workspaces`, `create_workspace`, `update_workspace`, `remove_workspace`
(with a keep/delete policy for its sessions), `set_active_workspace`, plus the
existing session commands taking a workspace filter. `get_settings`/`save_settings`
keep their shape for the global half so the dialog and its tests do not churn.

### 24.6 Acceptance

1. An existing install opens with every chat present and reachable under one
   migrated workspace — no session lost, no title changed.
2. A session's model, tools, MCP servers and instructions follow its workspace;
   switching workspace switches all of them together.
3. Removing a workspace never silently destroys sessions: the keep/delete choice is
   explicit and the keep path leaves them reachable.
4. Global UI state stays device-local and never travels with a workspace folder.

### 24.7 Choosing the root folder

The root is the workspace's identity, so it is chosen, not typed if we can avoid it:
a **Choose folder** button opens the native picker (`tauri-plugin-dialog`, which the
crate does not yet depend on — adding it is part of this milestone), with the typed
path kept beside it for headless use and for people who paste a path. Whichever way
it arrives, the server validates before saving: the path must exist, be a directory,
and be canonicalized so `..` and symlinks cannot escape it — the same guardrails
[`resolve_workspace_file`] already applies to file reads. The dialog shows the
resolved absolute path, so "~/Projects/x" and "/Users/…/Projects/x" are visibly the
same workspace rather than two.

Changing the root of a workspace that already has sessions asks what happens to them
(keep with the old root recorded in the session, or re-home them to the new one),
because a conversation's approvals and tool calls happened in the old tree.

### 24.8 Choosing what the workspace may use

"Available capabilities" is a per-workspace decision with three kinds of answer, and
each gets the mechanism that suits it:

| Capability | Mechanism | Why |
| 74 | 2026-10-10 | **The transcript can be read backwards again, and the doc was wrong about why it could not.** I went to build M1 step 2 and found the transcript **already** rendered a 200-row tail window with a "Load earlier" button, so that part of the M1 row was stale. What the window lacked was a **spacer**: the scrollbar described only the drawn rows, and the conversation above them was reachable only by pressing a button. I measured before touching anything — `rowsFirstPaint` 500/5000/10000 with first render 18.6/121.5/237.9 ms and streaming repaint 0.16/0.87/2.05 ms, and those timing phases run with the window **deliberately open**, so they describe the full-render path rather than the shipped one. Landed: a top spacer stands in for the un-drawn rows (height = un-drawn count × a row average sampled off the streaming path, so the guarded zero-layout-reads-per-frame invariant holds), scrolling within 320 px of the top asks for one more window, and the reader's offset is credited so growing the window does not move them. The spacer is skipped by the row-reconcile walk because it is not a row, and the trusted-prefix fast path now compares a **stored** child count instead of `windowLen`, which the spacer would otherwise have defeated on every frame. The assertion I wrote caught a **real defect in my own first version**: it credited an *estimate* (`min(WINDOW, windowStart) × avg`), but the average is re-measured from what is on screen — so the credit used the new average (200 × 47 px) while the spacer gave up 200 rows at the old one (200 × 15 px), and the reader would have jumped 6 400 px. The credit is now the spacer's measured change, making "gave" and "moved" equal by construction. Getting there also falsified two of my own claims: the harness fixture was not a scrolling box at all (so `scrollTop` was always 0, invisible in every number in the failure message until I printed the intermediates), and one assertion read the offset *after* a later render had moved it again. Harness precision was fixed rather than worked around: the windowed phase counted `childElementCount` as rows and the spacer made it 201, so it now counts `rowCache.size`, which is what "a row per message" actually means. Phases 37/38 assert five separate things — only the tail is drawn (200 of 1 000), the spacer exists and the scroll height exceeds the window, scrolling asks for exactly one more window (rows 200→400, extra=200, spacer shrinks), growing does not move the reader (gave=3000, moved=3000), and the spacer disappears once the window covers the whole chat. Measured at N=10 000: repaint **0.103 ms** windowed vs **2.5 ms** open (**24.3×**), appends **0.151 ms** vs **4.925 ms**, rows **200** vs 10 000. Verified: **593** workspace tests over 63 suites, 94 transcript + 48 shell + 25 markup + 8 shipped-page checks, 0 layout failures, wiring **46/46**, clippy and rustdoc clean, fmt clean. The M1 row and §7 were corrected rather than left implying M1 step 2 was pending, and the timing phases' scope is now stated where the numbers are. Limits: the spacer height is an **estimate** from sampled rows, so the scrollbar is approximate while the window is partial (exact once it reaches the start of the chat); the headline timing lines still measure the full-render path by design, so the windowed figures come from the windowed phase; and the "Load earlier" button stays, now redundant with scroll-back but still a keyboard-reachable control. |
| --- | --- | --- |
| 75 | 2026-10-10 | **A chat can leave the app as a file.** M5's last unbuilt piece was ZIP export, and it could not be delegated to a crate: Cargo.lock is offline, the way the Tauri dialog plugin is. So `crates/gui/src/export.rs` writes the container itself — **stored** entries, a CRC-32 table built in a `const fn`, and a central directory — which also means any tool can inspect the archive, and that is the point of escaping a database at all. Exports land in `~/.bos/gui/exports/<id>.zip` holding `README.txt`, `chat.json` (the record exactly as the store keeps it) and `chat.md` (the same turns, with **folded turns first**, because they are older). A `/export` palette command (wiring **47/47**) runs it and reports the path. Three decisions are stated rather than implied: entries are **stored**, so no compressor is needed and no byte of a transcript is at the mercy of one; timestamps are **DOS format**, because that is what the container carries; and **ZIP64 is not implemented**, so an archive is capped at 4 GiB and 65 535 entries — a limit placed in the module docs rather than discovered later. The tests are known-answer tests wherever a published value exists, which paid for itself on the first run: `crc32("123456789")` must equal `0xCBF43926`, and the DOS stamp of `1_700_000_000` (2023-11-14T22:13:20Z) **failed** because the seconds-of-day field was a `u16` — a day holds 86 400 seconds, so 80 000 truncated and every entry would have carried a wrong timestamp. A round-trip test also reads the archive back through a reader written against the format (names, sizes, per-entry stored CRC, the end record's entry count). Self-consistency is not evidence, so the format is checked from **outside** as well: `zip_selftest.sh` runs the real exporter and then has Python's `zipfile` validate what it wrote — integrity, the three entry names, `chat.json` parsing with its `messages` and `archived` keys, the Markdown naming both roles, and every entry stored with size equal to compressed size — **10 checks** where two implementations agree instead of one agreeing with itself. Verified: **598** workspace tests over 63 suites — six new, all in `export.rs` — with 94 transcript + 48 shell + 25 markup + 8 shipped-page checks, 0 layout failures, wiring **47/47**, clippy and rustdoc clean (module docs included), fmt clean, smoke 124. Recording this row cost three refusals, all mine and none the feature: clippy caught a `&` on an accessor that already returned a reference, `cargo fmt` caught unformatted Rust, and the recorder refused twice because it anchored on the baseline line's **prose** and then on a condition that no single line satisfied — it now edits the line that carries the guard list and asserts the result, which is the fourth time this session that guessing a string instead of reading it was the actual defect. The M5 row now says what is done and states plainly that an **FTS index is still not built**. Limits: no ZIP64 and no compression; the Markdown is a rendering rather than the lossless form (the JSON is lossless); exports accumulate in that directory, since nothing prunes them yet; and `/export` covers the active chat only, with no multi-chat or whole-workspace archive. |
| built-in tools (bash, files, search, plan, …) | `disabled_tools: Vec<String>` — an **opt-out deny-list** | a new built-in tool should appear for users who never had an opinion, not silently stay off everywhere |
| 76 | 2026-10-10 | **A measured negative result: the FTS index was built, then rejected.** An index is the obvious answer to a linear scan, so this round implemented one — an inverted index over session text, persisted, refreshed from the same `(modified, length)` stamps the search cache already uses, with **7 unit tests and 2 integration tests** — and then measured it against the path it would replace at four corpus sizes (300 sessions, 166 KB to 6.85 MB, needle `brown`). It lost every time: **1.34×** slower at 166 KB, 1.67× at 509 KB, 2.34× at 1.78 MB and **3.52×** at 6.85 MB, while the path it replaced stayed flat at 1.6–2.1 ms. Even after the extra syscalls were removed — the first version called `fs::metadata` once per session per query, ~2.4 ms for 300 sessions, which is twice the entire cost of the scan it was trying to beat — the index still lost. Instrumentation showed the index's own work was only ~0.6 ms of the 7.57 ms total (`sync` 287 µs, candidate lookup 295 µs, no re-tokenization, no save), so the remainder was in the surrounding loop and was **not root-caused**; rather than ship a measured pessimization or guess at a cause, the index was reverted and the scan kept. The design work produced three findings worth keeping. First, a search that uses `str::contains` is a **substring** matcher, so a whole-word index is unsound: a document holding `there` has no `the` token, and a query for `the` would have been told nothing could match. A **pre-existing** test whose needle was the single character `e` failed and exposed it, which is the clearest case yet of why the baseline tests earn their place; the correct tokenizer emits **character bigrams**, which makes the needed property true — if the needle occurs in the text, every bigram of the needle occurs in it, including inside longer words — and the soundness direction is now asserted over a corpus containing `there` with needles `the`, `he`, `ere` and `er`. Second, a token **absent** from the index can only mean nothing matches, so it should answer the empty set and let the caller skip every session; the first version answered "no opinion" and scanned everything, and a failing test surfaced that too. Third, the shipped scan is **stat-dominated, not match-dominated** — its cost is flat as the text grows 40× — which is the strongest evidence yet that the existing lowercased-cache design is the right one for this store. Kept from the round: two tests that pin the scan's behaviour (`a_single_cjk_character_still_searches`, a case no bigram index can answer, and `search_keeps_answering_the_same_way`). Verified: **600** workspace tests over 63 suites, ui/shell/markup/page/layout/wiring/zip guards all unchanged and green, clippy and rustdoc clean, fmt clean. Limits and process note: the loop cost was not root-caused, so "an index cannot help this store" is **not** the claim — the claim is that this index, as measured, did not. The revert itself went wrong once first, because `jj restore --from @-` took the file from an ancient revision instead of the previous round; `jj op restore` recovered it exactly, and the lesson is recorded here: snapshot, never guess a revision, and never reach for `restore` without a backup of the working copy. |
| 77 | 2026-10-10 | **The workspace on disk became visible.** M2 step 1: a lazy file sidebar under the session list — one directory per click, workspace-confined by canonicalization (absolute paths, `..` and symlinks leaving the root refused), `.git`/`target`/`node_modules` and dot-entries skipped by one shared policy that the `@`-mention expansion already used, capped at 1 000 rows with the cut reported, directories first then names case-insensitively, sizes shown. Open folders persist per workspace root and are **re-fetched** on the next launch; a folder that refuses to open keeps the reason on its row and retries on the next click. Three of my own defects surfaced and were fixed before recording: an error folder's row lost its reason because only its walked children could carry it (the reason now travels on the row itself, with a root-error case in the harness); `clippy::explicit_counter_loop` fired on a manual counter (`.enumerate()` now); and `strip_prefix` in a `let-else` borrowed past its scope (E0716 — the `PathBuf` is bound first). This round also **withdrew the rev-76 headline ratio** as unsupported: the "scan" it was measured against was a simplified stand-in that skipped snippet extraction; what stands is the indexed **miss** path at 15.18 ms vs the real search hit at 7.24 ms vs the scan at 2.21 ms at body 22 528, with the unexplained cost inside the indexed path never root-caused. Verified: **612** workspace tests, 104 ui + 48 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 48/48, clippy/fmt/rustdoc clean, smoke 124, doc assertions 10/10. Limits: no file watching, no preview images, no drag-and-drop, and "Choose folder" stays blocked offline (the dialog plugin is absent from Cargo.lock). |
| 78 | 2026-10-10 | **A command can be run and its output kept.** M2 step 2, the persistent terminal slot — shipped as a bounded **run panel**, not a terminal, because `portable-pty` is still absent from Cargo.lock, every Cargo.toml and the offline registry cache; the panel's header says "plain text, not a terminal" so the limit is visible where the user types, and a PTY upgrade remains a backend swap behind the same events (§12.4). `crates/gui/src/runs.rs` runs `sh -c` in the configured workspace root with stdin closed and two reader threads feeding one channel, so stdout and stderr interleave as they arrive; every line becomes a `run-line` event and the UI repaints at most once per frame. Bounded by construction: 2 000 lines per run (a longer run is killed and marked truncated), 4 096 characters per line (the cut is marked), a 60 s deadline (a hung command is stopped), invalid UTF-8 ends that stream with a visible marker, and the child is reaped on every path. Each run ends with one receipt line — `[exit 0 · 12 lines · 1.2s]` — and a failed start, a non-zero exit, a budget cut and a deadline are each named. Output survives a reload per workspace root (`bos.runs.<root>`, capped at 400), the same per-root pattern as the file tree; switching the root in settings swaps history. The module had to be named `runs.rs` because `runner.rs` was already the agent-stream Runner; two of my own Rust defects surfaced on the first build — `mod runs;` was simply missing from `lib.rs`, and one array cannot hold both `ChildStdout` and `ChildStderr`, so the reader is now a generic `pump` per pipe. Verified: **620** workspace tests (8 new in `runs.rs`), 113 ui + 48 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 49/49, clippy/fmt/rustdoc clean, smoke 124. Limits, stated in §12.4 rather than discovered later: no PTY (no stdin, no ANSI colours, no interactive programs, no Ctrl-C), one run at a time, and the command runs with the user's own permissions — the workspace root constrains the working directory, not what the command may touch. |
| 79 | 2026-10-10 | **Each workspace now keeps its own geometry**, closing M2's last open item: the shell persisted column widths under one global key, so two projects shared one layout. The seam is deliberately small and keeps the shell Tauri-free — the app names a scope (`Shell.setLayoutScope`, called from init and from the settings submit where the root changes) and the shell swaps the stored layout under the live panes: the geometry on screen is written back to the scope being **left**, then the new scope's adopted values run through the same clamping policy the boot path uses (`adoptLayout`, factored out so a scope change cannot skip it); a scope with no saved layout gets the defaults. The unscoped key stays the "default" scope, so layouts saved before scoping existed are still what you get with no workspace configured, and UI preferences (appearance, font, language) stay global — they are about the app, not the project. The run panel's **open** flag joined its persisted lines: the blob is now `{open, lines}` and the parser accepts the earlier bare-array shape, because nobody's panel should be lost to a tidier format; the parser is a pure function, so the harness pins it without touching storage. Two harness gaps were closed while there: the idle-chip transition was invoked but never asserted, and the new shell section asserts both swap directions, per-scope storage, the same-scope no-op and the default scope staying intact. Verified: **620** workspace tests, 117 ui + 54 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 49/49, clippy/fmt/rustdoc clean, smoke 124. Limits: a scope's layout is read once per swap (two windows on the same root do not live-sync), and the compact-window rules override stored widths for the window rather than per workspace. |
| 80 | 2026-10-10 | **The terminal arrived, closing M2 outright.** `portable-pty` (0.9) became fetchable in the offline cache, so the PTY backend rev 78 had deferred shipped as `src/pty.rs`: a `PtyRegistry` (managed by Tauri itself, so a webview reload re-attaches rather than respawns) holding one live shell per workspace root, with a streaming ANSI filter, a 64 KiB/400-entry tail ring, a pump thread plus a wait thread that owns the child (the killer lives behind a mutex because `ChildKiller::kill` takes `&mut self`), and an exit sentinel that tells "still running" from "exited 0". Four commands (`pty_start`/`pty_write`/`pty_resize`/`pty_kill`) and two events (`pty-output`/`pty-exit`) expose it; the panel gained term mode — `/term` or the Shell button — that sends lines to the shell **without local echo** (the PTY echoes; twice would be nonsense), holds chunks that split a line, resizes the shell to its own measured character grid, and tells the truth in the chip (`shell`/`attached` → `exit n`/`killed`) and in the honesty note, which now swaps with the mode. The persisted blob gained `mode`; old shapes still load and garbage degrades to run mode. Run mode is untouched. The crate was also **renamed `gui` → `bsh`** (lib `bshlib`, binary `bsh`) in the shared working copy mid-flight; the rename was completed forward — `main.rs`, harness scripts and docs all point at the new name. One environment fact is now pinned in tests: this sandbox refuses to allocate PTYs (`openpty` → `EPERM`), so the three spawning tests stand down **loudly** (`eprintln!`, no silent pass) and run in full on a normal dev machine; rev 78's "no PTY in this environment" claim is retired to history. Verified: pty 7 tests, 125 ui + 54 shell + 25 markup checks, wiring 53/53, clippy/fmt/rustdoc clean, smoke green. Limits: ANSI is stripped, not interpreted (no alternate screen, no cursor addressing — `vim` renders as scrolled text; a vt100 emulator is the upgrade path), and the sandbox this was built in cannot exercise a live shell end to end. |
| 81 | 2026-10-10 | **The web UI is now a reference-driven clone of DSH's frontend, on its own terms.** The ask was to clone DSH's frontend; DSH ships React/TSX compiled to a built `lib/client.js`, so copying *code* would mean vendoring a framework this sandbox cannot build and cannot test. The clone is therefore of the **contract**: DSH's own READMEs and published `.d.ts` seams are vendored at `docs/dsh-ui-reference/` (MIT, fetched from `@deepseek-ai/*` on npmmirror, provenance and a refresh recipe written down), and the structure, geometry, tokens and behaviour are reimplemented in this crate's dependency-free document. Three rules still diverged from the contract and are now closed: the right column **steps down to 300px** before reporting that it does not fit — and then **closes deterministically**, never reopening because the window widened; **opening the right column collapses a manually expanded sidebar** (space is conceded from the sidebar first); and the sidebar's auto-collapse moved from 860px to **1024px**, coupling it to the right column's breakpoint as DSH does. The theme presenter gained the three writes DSH owns — `color-scheme`, `body[data-ds-dark-theme]`, and one `<meta name="theme-color">` read back from the rendered body background — and `--frame-top-clearance: 48px` / `--frame-overlay-top: 68px` / `--frame-leading-clearance` are published for window-chrome seats. Every rule is held by checks rather than prose: the shell harness grew from 54 to **66 checks** (step-down, deterministic close, no-reopen, open-concedes-sidebar, marker/color-scheme/theme-color, clearances), and two harness assumptions the clone falsified were fixed rather than papered over — the layout probe counted a collapsed rail's hidden rows as an "unreadable list", and the rail sequence assumed the sidebar was expanded before the column opened. Verified: 125 ui + 66 shell + 25 markup + 8 page checks, 0 layout failures at four widths, wiring 53/53, clippy/fmt/rustdoc clean, smoke green. Limits: this is the **frame and theme layer only** — the sidebar/session seams, right-column docking, conversation node rendering, tool cards, panel registry and settings-card registry are referenced but not yet reimplemented, and the vendored copy is documentation held for study, not a runtime dependency. |
| 82 | 2026-10-10 | **The layout finally has a seam, not just two columns.** DSH's ui-layout is a keyed `main` slot whose `conversation` key is reserved and whose every other surface is a panel selected by id; ours was a document panel bolted to a divider, so nothing but the product could ever put a surface in the right column. The shell now carries that registry — `registerPanel`/`unregisterPanel`/`selectPanel` with `panels` and `activePanel` readable and the active key mirrored onto `#app[data-panel]` — and the **built-in document panel registers itself through the same public seam a plugin would use**, so the seam is exercised by the product rather than only by tests. Three properties are pinned instead of assumed: `null` returns to the conversation, an **unknown id changes nothing** (no phantom surfaces), and the reserved `conversation` key **cannot be registered or displaced**; a column that closes releases its surface, so a hidden panel is never the active one. Mod+B joined it as DSH's sidebar chord, and it **yields the chord to a control that has a better claim on it**: an editable field, or the terminal, where `Ctrl+B` belongs to the program on the other end of the PTY — a shortcut that steals a keystroke from a shell is a bug wearing a feature's clothes. Verified: 125 ui + 80 shell (14 new registry and chord checks) + 25 markup + 8 page checks, 0 layout failures across four widths, wiring 53/53 with no new commands, clippy/fmt/rustdoc clean, smoke 124. Limits: the registry selects surfaces but does not yet render occupant-provided content (a panel is an identity, not a mount point), only the right column is registered, and the `shell.leading` seat is still unimplemented. |
| skills (`SKILL.md` folders) | `disabled_skills: Vec<String>` — opt-out, symmetric with tools | same reason; the skills directory is already per workspace |
| MCP servers / plugins | per-entry `enabled: bool` — **opt-in** | a server is added by hand, so it starts off until asked for, and each carries its own transport config |

`Settings` keeps a workspace-independent catalogue of what is *registered* (tools and
skills come from the runtime, MCP servers from the user's list), and the workspace
stores only the differences. The management UI is a checklist rendered from
`caps::describe`, with the reason each item exists as its subtitle.

The seam is already single: **`caps::build_agent`** is the one function that
registers tools, skills, MCP servers and the plan tool for a chat's agent, so it
takes the resolved workspace instead of the global settings and every gate
(`bash_enabled`, `file_tools_enabled`, skills, MCP) is decided there and nowhere
else. Switching workspace therefore changes what the agent can do in the same step
that changes the model.

### 24.9 Presets

A preset is a named bundle of the choices a user makes over and over:

```rust
struct Preset {                    // global catalogue in Settings
    id: String, name: String,
    model: String, reasoning_effort: Option<String>, temperature: f32,
    system_prompt: String,
    disabled_tools: Vec<String>, disabled_skills: Vec<String>,
    mcp: Vec<String>,              // names of servers this preset expects
    created_at: u64,
}
```

Semantics are **apply-and-diverge**: choosing a preset copies its values into the
workspace and then forgets it, and "Save current as preset" captures the workspace
as it stands. A preset that is *followed* instead of applied reads better in a demo
and behaves worse in practice — editing a workspace would silently edit every
workspace sharing the preset, and the user has no way to see that from the row they
edited. Divergence is visible (the row shows "customized" once a workspace with a
preset has been edited), and re-applying is one click.

Profiles discovered from BOS config (`[llm.<name>]`) stay a catalogue as well: a
preset may name a profile instead of a concrete model, which keeps config-managed
endpoints out of the GUI's copy of them.

### 24.10 Acceptance (authoring)

5. A root chosen with the picker is canonicalized, shown resolved, and cannot be
   saved when it does not exist or is not a directory.
6. A capability switched off in a workspace is absent from that workspace's agent —
   and switching workspace changes the available set. **Partly landed**: the built-in
   tool deny-list is enforced by `caps::build_agent`, and model/root/approval/budget
   already resolve per workspace. Open: `disabled_skills` (the skills API registers a
   whole directory, so per-skill filtering needs a seam in `crates/agent`) and the
   per-server MCP switch is now **enforced**: `attach_mcp_tools` takes the effective
   settings and skips any handshake whose server this workspace switched off (or never
   added), so nothing configured means nothing attaches. Still open:
   `disabled_skills`.
7. Applying a preset fills the workspace; editing afterwards diverges and leaves the
   preset untouched; re-applying restores it.
8. Migrated workspaces (§24.3) arrive with every capability in its previous state:
   migration must not silently enable MCP servers that were disabled, nor disable
   tools that were on.
