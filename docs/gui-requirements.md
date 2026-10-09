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
must never regress: tests **555/0 (63 suites)**, live verify 5/5.

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
| Terminal panel (persistent shell) | ✅ persistent bash/pwsh | ✅ owner-scoped shells | ❌ | M2 |
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
| M1 | **Incremental + virtualized transcript** | step 1 ✅ keyed patch renderer + headless harness ([`ui_selftest.sh`](../crates/gui/tests/ui_selftest.sh)): 5 000-row session repaints in **2.0 ms** vs **17.5 ms** for the old wipe-and-rebuild (9×), exactly one row rebuilt per frame, append/delete/reorder/tool-card/diff/non-diff checks green. step 2 (next): virtualization so repaint stops growing with row count (today 0.28 / 2.0 / 5.7 ms at 500 / 5k / 10k rows) + semantic scroll anchors | step 1 done, step 2 next |
| M2 | Shell: resizable/persisted 3-column, file sidebar, persistent terminal panel | layout persistence per workspace; terminal survives reload; tests + smoke | |
| M3 | Plan/todo + goal surfaces (UI for goals, budgets, blocked/complete) | goal visible live, editable, rounds/budget counters; todo list repaint from session log | |
| M4 | Approvals v2: allow-always with scoped rules, audit list, keyboard-first | rule matching tests; audit entries in session log | |
| M5 | Session intelligence: FTS search across sessions, titles, turn outline, stats, ZIP export | searchable index built incrementally; export round-trips | |
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

**Part II Phase 1 (Epic A) is underway; finish it before returning to M1 step 2.**

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
  rebuild at 5 000 rows (10× at 3 000), and 47 shell-contract checks.

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
|---|---|---|
| Left navigation | Brand, New Session, Plugins, Automation tasks, MCP, Workspaces, sessions, user profile | Navigation and resource management |
| Center workspace | Chat, Trajectory, agent output, task progress, tool calls, composer | Task interaction and execution monitoring |
| Right content panel | Start, file tabs, file path, Markdown/code view | Document viewing and editing |

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
|---|---|---|---|
| Permission | Dropdown | Workspace Write | Set the default permission mode for new sessions |
| Language | Dropdown | English | Choose the UI language |
| Appearance | Three-way segmented cards | Light | Light, Dark, System |
| Font size | Numeric input/stepper | 14 px | Change conversation-content font size |
| Work details | Dropdown | Detailed | Control how much tool-call detail is shown |
| Show coding view | Toggle | On | Show/hide trajectory, code diffs, and agent presets as applicable |
| Keyboard shortcuts | Button | Edit shortcuts | View and edit shortcuts |
| Open chat links in | Dropdown | Confirm against current product configuration | Configure how chat links open |

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
|---|---|
| Base spacing | Multiples of 8 px |
| Sidebar width | Approximately 280–340 px |
| Center workspace width | Approximately 400–520 px |
| Panel radius | 8–16 px |
| Settings content width | Responsive to available window size |
| Conversation font size | 14 px by default |
| Dividers | Low-contrast neutral gray |
| Primary action | High-contrast background and text |

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
|---|---|---:|
| Workbench foundation | GUI-01–03 | 6 person-days |
| Agent interaction | GUI-04–06 | 9 person-days |
| Document panel | GUI-07–09 | 8 person-days |
| Settings center | GUI-10–12 | 7 person-days |
| UX/accessibility | GUI-13 | 2 person-days |
| **Total** | **13 stories** | **32 person-days** |

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
|---|---|---|
| `SessionSelected` | Session ID | Load the session snapshot |
| `TaskStarted` | Task ID, session ID | Enter running state |
| `TaskProgress` | Task ID, phase/progress | Update task display |
| `ApprovalRequired` | Task ID, approval context | Show approval affordance |
| `TaskCompleted` | Task ID, result summary | Show completion state |
| `TaskFailed` | Task ID, error details | Show error and available recovery actions |
| `SettingsChanged` | Setting key, version/value | Update affected UI |
| `DocumentChanged` | File ID, version/content state | Refresh document or show conflict |

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
|---|---|---|
| Existing GUI framework differs from assumptions | Rework | Inspect framework and capabilities first |
| Duplicate or out-of-order events | Incorrect state | Validate task IDs and sequence/version numbers |
| Task lifetime tied to window lifetime | Tasks stop unexpectedly | Keep task ownership in the application/service layer |
| UI and persisted settings diverge | Preferences lost on restart | Define explicit persistence and recovery behavior |
| Document rendering blocks UI thread | UI freezes | Separate loading/parsing from presentation where supported |
| Permissions enforced only in UI | Security boundary failure | Enforce permissions in the actual execution layer |

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
| --- | --- |
| root folder, name, id | window layout, sidebar/doc geometry, theme |
| sessions (the whole conversation store) | keyboard shortcuts and shortcut profiles |
| LLM binding: model, base URL, API key, failover providers | the `[llm.<name>]` profile catalogue discovered from BOS config |
| tools: bash/file tools on/off, approval policy, context budget | approval policy *default* used when a workspace is created |
| skills directory, `AGENTS.md`-style instruction folding | telemetry/updates, default workspace id |
| MCP servers, memory store and its file | the workspace *list* itself (see 24.3) |

Rationale: every row in the left column is meaningless without a root folder, and
every row in the right column is meaningless *with* one. Sessions are the strongest
case — a conversation about a repo cannot move to another repo and stay coherent,
because its tool calls, approvals and instructions were that repo's.

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
