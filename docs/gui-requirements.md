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
| MCP | stdio/streamable-http, `failOnStartupError` false, reconnect 500→30 000 ms ×10, `toolCallTimeoutMs` 60 s | same, plus per-server governance — and **per-tool** allow/ask/deny enforced where tools are attached, with `ask` outranking a workspace that approves nothing and the workspace gate a floor no `allow` lowers (M8, rev 121) |
| Skills | merged catalog over 5 ranked roots, description ≤ 500 chars, **loaded bodies uncapped** | capped bodies and an authoring UI — the round trip is proven through DSH's own loader, not by trusting the writer (M8, rev 122) |
| Jobs | 10 per owner, in-session settlement, die with the process | panels that stream only the expanded row (M7) |
| Schedule | durable reminders, cron ≥ 1 min, history 200 records; needs the Web session controller | **implemented (M11, rev 119)**: a durable store plus the pane — and the differences are deliberate, see row 101 |
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
   **Memory is done (rev 116); the curated knowledge base is done (rev 120)** —
   a durable store the user edits and the agent searches *on demand* through a
   `knowledge_search` tool registered on every session's agent, rather than
   recall injected into every prompt. Still open: provenance (where a note came
   from), and scope (which workspace a note belongs to).
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

**GUI end-to-end suite (`crates/gui/e2e/`, tauri-driver + WebdriverIO 9):** five
specs drive the *real application window* — the shipped `ui/` against the real
Rust backend — through WebDriver, asserting what a user would see: the shell
chrome, M7 jobs, M6 context/compaction/spill, M4 rules with a live add→revoke
round trip, and M11/M10/M14 inspection, replay and bundle export. A `preflight.js`
fails fast when a precondition is missing, and `.github/workflows/gui-e2e.yml`
runs it on Linux under `xvfb`. It **cannot run on macOS**: tauri-driver speaks
WebDriver to the embedded webview, which means WebKitWebDriver on Linux and
msedgedriver on Windows and nothing on a Mac — so the same feature points are
also covered by `page_selftest.sh`, which runs anywhere and is the fast loop.

`package_selftest.sh` (rev 115) is the packaging layer: `cargo tauri
build` needs a network-plus-toolchain host, so what it checks instead is that
the config describes an artifact that exists — the identifier, the product/window
name agreement, `frontendDist` and its `index.html`, the absence of a build step,
and every icon's presence, size and **magic bytes** in one file-format test per
platform. Sixteen checks; the failure it catches is the silent one, where a
renamed or truncated icon still yields a bundle that merely has no icon.

On macOS the *live-backend* half of that intent is covered by
`live_selftest.sh` (2026-10-10): the shipped page runs with a **stateful,
asynchronous** bridge double (`tests/live/bridge.js` — state across commands,
events on timers, failure on demand) instead of page_selftest's settled
fixtures, so the behaviors that only exist against a live backend are
measured rather than asserted from source: boot into a seeded workspace,
send → streamed chunks → store reload on finish, a mid-stream follow-up
queued via **Enter** (the send button means Stop while streaming) and
flushed FIFO, Stop cancelling with the partial text kept, an approval that
must be answered before the tool result lands (Allow and Deny both), a
failed send rolling the pair back with the prompt restored, the model save
round-tripping, trash delete → restore returning the row, and content
search rendering its snippet, and a mid-stream `update_plan` repainting the
todo list from the session log (rev 90 — the check that exposed the
`$("#plan-list")` defect in row 90; its second assertion pins the geometry,
since a panel that never renders has never been laid out: top 404, bottom 518,
composer 602, inside 1440 px) — **86** checks, 0 failures (rev 91 added nine
that drive the plan editor end to end, from opening the draft to Enter and a
per-session restore; rev 92 added five that run the terminal against a real PTY
double — attach, write, stream back, re-attach and kill; rev 112 added seven
that drive the Context and Jobs panes against a double holding real state, and
rev 113 five that drive the Insight pane — the agent reading its own
composition, the goal it reports against the Goal card, the evaluator, replay
and the export note — rev 114 six that make a scoped approval rule settle a
call, revoke it, and watch the prompt come back, and rev 116 three that make the
Memory pane name the file behind the workspace it labels, list both memories
newest first, and actually forget one, and rev 118 one that ends a turn with the
store's own text — the streamed draft and the stored reply differ, so a row kept
across the reload would show the wrong one; rev 119 four that add a durable
reminder through the pane's own form, tick one that was already past due and
read how late it says it was, cancel it, and clear the delivery history; rev 120
six that curate a knowledge note end to end — add, refuse a blank title without
a round trip, search, edit in place, list again, delete; rev 121 four that set a
per-tool MCP policy from the pane and show the same decision reaching the tool's
own row, the server's badge and the Insight inventory — **86**). Two things the
first run caught and fixed: `restoreTrashed` never repainted the sidebar
(a restored chat stayed invisible — now `renderAll()`, the same treatment
`deleteSession` gives), and the runner itself had to run the app's
scheduled repaint directly, because headless Chrome under
`--virtual-time-budget` starves rAF after the first frames (measured: a
nested rAF never fires).

Per milestone: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`,
`cargo test --workspace --exclude nbos --exclude jsbos`, `node --check` on the
UI bundle, GUI build, and a live verification against the local broker. Baselines
must never regress: tests **746/0 (65 suites)** — re-measure with the command above
rather than trusting this number, which has been stale twice — live verify 5/5, and
the GUI guards, whose counts are also measured rather than remembered:
`ui_selftest.sh` **125** ui + **94** shell checks over the transcript and shell surfaces
(shell holds the rev 85 sidebar contract: the right column opens without railing
the session list, the ☰ toggle rails it, and layout schema v2 heals a pre-v2
concession rail once while keeping a rail the user chose), `layout_selftest.sh`
**0** failures across five widths (1440/900/560/500 measured;
420 unreachable — Chrome clamps to ~500 px); `markup_selftest.sh` **53** structural
checks (the only guard that reads the shipped `index.html`; rev 88 added three that
each caught a shipped defect — the third keeps the served `ui/icons/`
byte-identical to the `crates/gui/icons/` source — no `var()` may name a custom property the
stylesheet never defines, and no `data-i18n` mark may wrap elements, because
applying a translation replaces a mark's children; rev 90 added one that caught a
fourth shipped defect: no `ui/*.js` may pass a `#` selector to `$`, which is
`getElementById`, and which is exactly how the plan renderer silently returned
early on every call and kept the todo list off the screen); `wiring_selftest.sh`
**72/72** commands reachable, no phantoms; `page_selftest.sh` **59** checks that
execute the shipped page itself (including the Chat | Trajectory tab strip: click,
roving-key navigation, cross-tab visibility and the honest empty state; and the
M13 controls: the chrome translating to 中文 and back through the shell's own
language setting, the `?` reference with its focus hand-back, the theme control
cycling the shell's appearance, and the announced non-clickable keys) and the
**per-feature end-to-end sweep**: the bridge is replaced with a recording double
and every run surface (jobs, context/compaction/spill, rules/audit,
self-inspection/critic/replay/bundle) is driven through its real handler, so the
check sees both the arguments that crossed the bridge and the elements that
appeared — executed **styled**: the harness injects `<base>`
as the *first* child of `<head>` (rev 84; appending it after the stylesheet link
left every earlier run reading an unstyled document); `perf_selftest.sh` **19** work budgets measured on the
shipped page at scale (302 chats, one chat of 5 000 messages: a 400-event stream
burst queues **one** frame and one repaint, the streamed text lands in **2** DOM
mutations across **201** drawn rows, a full repaint draws **201 of 5 000**, a
query costs **one** round trip, and — rev 93 — a **2 000-chunk shell flood**
also queues **one** frame, keeps the transcript panel at its **400**-line cap in
state and in the DOM, and writes **0** `bos.runs` keys) — it counts frames and mutations rather than
milliseconds on purpose, because the virtual clock it needs for the bridge's
pushed events does not advance for synchronous work; `zip_selftest.sh` **10**
checks in which an **independent** reader (Python's zipfile) accepts an archive the
exporter actually wrote.

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
| Skills (discover/invoke/author) | ✅ SKILL.md + roots | ✅ ranked roots, uncapped | ✅ author in the pane → discovered by the loader → in the palette and in every open session (rev 122); BOS still caps a body where DSH does not | M8 |
| MCP servers + resources | ✅ + OAuth login | ✅ + resource tools | ✅ servers + per-tool policy (rev 121) + resources and prompts read on demand (rev 123); no `resources/subscribe`, no templated-resource listing, prompts listed but not fetched (no `prompts/get`), and the model cannot ask for a resource | M8 |
| Compaction + token meter | ✅ (limits exposed) | ✅ 0.8/0.16/64Ki | ⚠️ auto-compact | M6 |
| Spill / oversized output policy | ⚠️ output token limit | ✅ spill store (no retrieval) | ❌ | M6 |
| Session search (FTS) | ✅ `thread/search` | ✅ SQLite FTS5 | ⚠️ in-session only | M5 |
| Session export/import | ✅ export + rollout migrate | ✅ ZIP | ⚠️ markdown export | M5 |
| Fork / replay / trajectory | ✅ response-id fork | ✅ trajectory window | ❌ | M10 |
| Terminal panel (persistent shell) | ✅ persistent bash/pwsh | ✅ owner-scoped shells | ✅ PTY session per workspace (rev 80) | M2 |
| File sidebar / tree | ✅ worktree browser | ✅ files tab | ✅ workspace tree (rev 77) | M2 |
| Right dock: browser/doc preview | ⚠️ InAppBrowser flag | ✅ browser + doc preview | ❌ | P1 |
| Background jobs UI | ⚠️ `ps` / jobs | ✅ roster + stream panels | ❌ | M7 |
| Schedule / reminders UI | ❌ | ✅ automation tasks | ❌ | P2 |
| Inline images / gallery | ⚠️ artifacts not inline | ✅ durable images + lightbox | ❌ | P1 |
| Voice input | ✅ RealtimeConversation | ✅ experimental, off | ❌ | P2 |
| Browser / computer use | ✅ feature flags | ❌ | ❌ | P2 |
| Theming (light/dark, fonts) | ✅ ~32 themes + tmTheme | ✅ tokens, 8 sheets | ✅ tokens, light/dark/system + reduced-motion & contrast (rev 87) | M13 |
| i18n zh/en | ⚠️ | ✅ + browser fallback | ⚠️ zh/en for the chrome via one source of truth (rev 87); agent-generated strings still English | M13 |
| Keyboard customization | ✅ context keymap | ✅ device profiles | ⚠️ reference dialog over the 17 real bindings + `aria-keyshortcuts` (rev 87); no remapping yet | M13 |
| Status-line item registry | ✅ (~30 items) | ⚠️ chips only | ✅ registry + live region (rev 87) | M13 |
| Headless / stdio SDK | ✅ exec + app-server | ✅ headless/ACP/SDK | ✅ `bos-stdio` JSON-RPC 2.0, 4 methods, offline-deterministic (rev 87) | M12 |
| Multi-provider failover | ⚠️ | ⚠️ retry only | ✅ | keep |
| Memory / knowledge UI | ✅ MemoryTool + memories crate | ❌ (third-party MCP) | ✅ memory pane (rev 116) + a curated knowledge base the agent queries through `knowledge_search` (rev 120) | **M11 differentiator** |

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
| M3 | Plan/todo + goal surfaces (UI for goals, budgets, blocked/complete) | goal visible live, editable, rounds/budget counters; todo list repaint from session log | ✅ **goal surface** (step 1, rev 84): the goal is a **card directly above the composer** — the seat the DSH reference gives it — showing the objective, a state chip (`active`/`paused`/`completed`/`blocked`), rounds against the cap, the blocker, and an inline editor that refuses a blank objective and a non-numeric or negative cap. Advance (+1), pause/resume, complete, block-with-reason and delete all round-trip through `get_goal`/`set_goal`/`goal_action` over a `Goal` state machine in `crates/agent`, persisted on the session record. The card is **static markup bound once**, not a Shell panel (the reference gives that column to the conversation), with a `#goal-open` header chip and a `/goal` command as the entries while it is folded, and `refreshGoal` on every session switch, restore and fork. Evidence: 6 state-machine tests, 16 page checks (6 on the card: position above the composer, computed display when folded, the active/cap/blocked renderings, chip-and-cancel), wiring 56/56. Limits: `advance` is **explicit-only** — no backend auto-advance ticks rounds — and the objective carries no prompt-injection guard yet. ✅ **todo list repaint from the session log** (rev 90): the plan surface was written but **dead** — its renderer looked up `$("#plan-list")`/`$("#plan-count")`, and `$` is `getElementById`, so the lookups returned null, `if (!panel || !list) return` fired on every call, and the panel never appeared. Found by the surface's first coverage (no guard touched it before): a live check sends a message whose turn calls `update_plan` **mid-stream** and asserts the list repaints with no reload — `count=1/3 · 1 active`, 3 rows, statuses `completed`/`in_progress`/`pending` — which exercises both the tool-event re-pull (`refreshPlan`) and the finish-time `restore_plan`. `markup_selftest.sh` gained a check that no `ui/*.js` passes a `#` selector to `$` (53 checks, negative-tested). The **editor** followed in rev 91 and is now driven like a user: Edit opens the plan as a draft and hides itself, ↓ reorders in place, the status control cycles a step, × and `+ Add step` drop and append, **Save** persists the trimmed draft in order (asserted on the exact `set_plan` payload) and renders the backend's canonical answer, **Cancel**, **Escape** and **Enter** behave (the first two write nothing), and a session switch restores *that* chat's plan. Writing it found the editor drawing every row without its status class — so a done step lost its strikethrough and the active one its accent, and every step read as untouched. |
| M4 | Approvals v2: allow-always with scoped rules, audit list, keyboard-first | rule matching tests; audit entries in session log | ✅ **rule engine (rev 87): `agent::security::rules`** — `RuleSet`/`ToolRule`/`RuleScope{Global,Workspace,Session}`/`RuleEffect`/`ArgPredicate{Exact,Prefix,Contains}`, with **deny beating allow**, lexicographic specificity (scope rank, then a predicate outranking a bare tool), no-match ⇒ `Ask`, malformed JSON entries skipped with a warning rather than failing the load, and a **bounded audit ring** recording oldest→newest. ⬜ GUI compiler/host commands and the approval dialog's rule editor are the GUI half. | **UI verified end to end (rev 114):** the Rules pane adds a scoped rule (`write_*`, `global`, `allow` → `approval_rule_add` with `arg_key: null`, form cleared, confirmation `Added: write_* (global).`), the rule **settles the matching call without raising a prompt** (`dialog=hidden`, `respond_approval` count unchanged, the tool still ran), the ring records **which rule decided** (`allow` + `rule-1`, newest first, `limit: 20`), **Revoke** empties the book again, and framing it is exactly what brings the prompt back — the ring's newest entry then reads `ask` with no rule id. Found and fixed on the way: `addRule` posted its confirmation **before** `refreshRules`, whose redraw clears the note, so the confirmation never appeared (same ordering `compactNow` documents). Limits: only the `<tool>*` glob and the `allow` effect are exercised live; the argument predicate (`exact|prefix|contains`), `deny`, and the 200-entry ring cap are covered by the Rust tests (`approval.rs`: `ring is capped`), not here. |
| M5 | Session intelligence: FTS search across sessions, titles, turn outline, stats, ZIP export | searchable index built incrementally; export round-trips | ✅ **search** (a live scan, with the hot path measured ~15× faster than the original), **turn outline + counts** (rev 73) and **ZIP chat export** (rev 75). An **FTS index was designed, implemented, measured and rejected** (rev 76), and the scan stayed. That revision reported the index **1.34×–3.52× slower than the scan it replaces** — that ratio is **withdrawn as unsupported**: the "scan" it compared against was a simplified stand-in that skipped snippet extraction, not the real search path. What stands from the round, measured on one corpus (300 sessions): at body 22 528 the indexed **miss** path cost 15.18 ms, the real `search` path 7.24 ms, and the scan 2.21 ms — so the index lost to the scan on this data on the evidence that exists. The unexplained cost inside the indexed path was never root-caused. |
| M6 | Context: token meter UI, spill/retention policy, compaction controls | spill files retrievable; meter matches provider usage within tolerance | ✅ **verified end to end (rev 112)**: the Context pane reads `token_meter`/`compaction_status`/`spill_list` on tab activation and renders the split (`messages=4`, `estimated=90 tok`, `provider=—`, `live=3`, `folded=1`, `budget=8192 tok`); **Compact now** folds the live turns and the pane and the note agree (`live=0`, `folded=4`, `canCompact=no`, `Folded 4 turn(s); 0 live, 0 tok estimated.`); **Restore folded turns** brings them back (`live=4`, `folded=0`, `Restored 4 folded turn(s).`); and a spilled output opens with its truncation stated (`spill_read {id: spill-1, max_bytes: 8192}` → the file plus `(truncated)`). The retention half (a real spill/eviction policy under a live tool output) is still unverified — these checks drive the pane against a stateful double, not a real spill. |
| M7 | Subagents + jobs: catalog, live output panels, control actions | delegate/interrupt/list from UI; job panels stream bounded output | ✅ **verified end to end (rev 112)**: the Jobs pane lists each job with its state (`running,failed`, `2 shown`); opening one reads only a bounded **tail** (`job_output {id: job-1, max_bytes: 16384}` → `16.0 KiB of 28.2 KiB (tail)`, `12.2 KiB dropped`, and `test line 1999` present while line 0 is gone); **Stop** crosses as `job_interrupt {id: job-1}` and the list re-reads to show `interrupted`. Still unverified: a *delegated subagent* job (these are the shell/run job family) and live streaming into the panel while it stays open. |
| M8 | Skills + MCP v2: authoring/install, resources on demand, per-session activation, governance | skill author→invoke round trip; MCP allow/deny enforced | ✅ **library half (rev 87): `agent::mcp::policy` + `agent::skills::session`** — `McpPolicy` with deny-beats-allow and an explicit configurable default (a policy that grants nothing is expressible), and `SkillActivation` with per-session isolation plus `SkillAuthoringRecord::validate()` naming the offending field. ✅ **per-tool policy enforced (rev 121)**: `crates/gui/src/mcp_policy.rs` is a durable store (`~/.bos/gui/mcp-policy.json`, 200 rules, atomic write, hand-edited rules dropped on load, duplicate pairs collapsed to the strictest) consulted **where the tools are attached**, so `deny` means the tool is never built — absent from the registry, invisible to the model — while `ask` adds an approval gate a workspace that gates nothing else does not have; the workspace-wide `require_approval` stays a **floor** that no per-tool `allow` can lower, `*` matches either half, and one `decide()` answers for both the enforcement path and the pane. A policy change **rebuilds** every cached session agent through the same construction path a first build uses, because re-attaching onto a clone of a running agent can only ever add. Driven live: denying a tool updates its row, the server badge (`2 of 3 tools · 1 denied`) and the Insight inventory (`MCP tools 2`) — three surfaces, one decision. ✅ **skills author→invoke (rev 122)**: the pane writes a `SKILL.md` whose name and description rules are `SkillAuthoringRecord::validate`'s own, reads it back through `SkillLoader` so the pane's count and the agent's prompt are one number, and rebuilds every open session's agent so a skill saved now is one the model can be asked to use now. ✅ **MCP resources and prompts on demand (rev 123)**: the client's `resources/list`, `resources/read` and `prompts/list` — public since rev 87 and called by nothing — are now the MCP pane's catalog: per-server rows with a Read button, a bounded `<pre>` (`filesystem/file:///repo/schema.json · 17 chars · application/json`), a URI box for a resource the server never listed, and the bounds stated in the pane. A capability the server declared `false` is never asked about (and reports zero, not a handed-in count); one server's failed listing is its own line; readings are cut at 20 000 **characters**, counted not sliced; and a resource's text is never injected anywhere — reading is a gesture. ⬜ still open in M8: per-session skill activation in the UI (the workspace deny-list is the only scope), a rule editor for `agent::mcp::policy`'s config form (the GUI store is the GUI's own), and fetching a prompt (`prompts/get` does not exist in `agent::mcp`, so prompts are listed for reference only). |
| M9 | Extensibility: Rust plugin seam + declarative presets + hooks (Codex/Claude) | sample plugin registers tool+panel+command; preset validation errors actionable | ✅ **library half (rev 87): `agent::plugins` + `plugins::presets` + `agent::agent::hooks::external`** — `Plugin`/`PluginRegistry` (cross-kind name collisions rejected with an error naming the duplicate **and both owners**), `SamplePlugin` registering a tool, a panel and a command; `Preset` loading from JSON **and** TOML with errors that carry the **field path**; `ExternalHook`/`HookDispatcher` running an argv-matching hook (`/bin/echo` succeeds, a bad command fails cleanly with its exit status). ⬜ GUI: plugin/preset browser over these seams. |
| M10 | Trajectory + replay: timeline with timing, deterministic session replay | replay reproduces transcript + metrics | ✅ **step 1 (rev 85): the Chat \| Trajectory tab strip** — a `role=tablist` strip under the title (roving tabindex, click + ArrowLeft/ArrowRight) switches the centre between the conversation and a read-only **Trajectory** view of the run: steps with timings, tools summarised with durations and running states in words, roles/errors carried in text, the honest "No turns yet / Steps appear here as the conversation runs." empty state, and a **300-row cap** with a truncation note; `/trajectory` and `/chat` command the strip. Evidence: page **26/26** (5 tab checks), markup **37**, wiring **56/56**. Limits: tab choice is device-local (not persisted) and the trajectory is a view, not a replay. ⬜ timeline with timing.  **Step 2 (rev 113): deterministic replay runs from the pane** — `Deterministic replay` crosses as `replay_session {session_id}` and the report names the session (`Replay of Design the streaming harness`) with a metrics grid and up to 20 entries whose first row is `0. user…`; on the live transcript it reproduced all **18** turns. Still open: the timing half of the timeline, and a replay compared turn-for-turn against the original transcript. |
| M11 | AGI surfaces: memory/knowledge UI, self-inspection tool, evaluator/critic, workspace scoping | memory CRUD + recall surfacing; agent can read its own composition | partly ✅ **self-inspection + evaluator verified end to end (rev 113)**; ⬜ workspace scoping and memory/knowledge CRUD (M11 proper). The Insight pane reads `self_inspect {session_id}` and renders the whole composition (`tools=11`, `async tools=4`, `skills=2`, `plugins=1`, `hook events=5`, `MCP servers=1`, `MCP tools=3`, `memories=1`, `model=harness-model`, `workspace=/tmp/live`, `bash=on`, `file tools=on`, `skills dir=/tmp/live/skills`), and the goal it reports **agrees with the Goal card** about the same goal — the pane's `active · 3/12` against the card's `active` / `3 of 12 rounds`, two surfaces that reach the DOM through different commands (`self_inspect` vs `get_goal`). **Evaluate this chat** scores the transcript and explains itself (`excellent · 100`, one reason, `No problems were found in this transcript.`), with the grade band consistent with the score it printed. Open: workspace scoping has no live behavior (its commands are still `null` doubles) and memory/knowledge CRUD is only statically wired. | **Memory CRUD + the store's identity verified end to end (rev 116)**; ⬜ knowledge base, and recall surfacing beyond the inclusion the evaluator already scores. The gap here was never the wiring — `list_memories`/`remember_memory`/`forget_memory` were implemented and the pane's add/list path was already live-checked — it was that **two facts a person cannot check were hidden**. First: an empty `memory_path` (the default) puts *every* workspace on one shared file, while the pane's own hint said "One store serves every workspace" and its row header said "Memory for <workspace>", so the label implied an isolation the default does not have. The resolution now lives in exactly one place (`settings::memory_file`), `caps` opens the store from it, and the pane renders the **file** next to the workspace it labelled: `<path> · shared by N workspaces · N of 500 kept`. Second: the store's item bound (500, past which the oldest memory is evicted) and the per-note bound (2000 chars) were invisible; the line now carries the cap and says `at the cap, so the oldest memory goes next` when it is reached. `Settings::effective_for(id)` was split out of `effective()` so "which workspaces share this file?" is answerable for every workspace, not just the shown one. Evidence: **ui 129/0** (4 checks on the pure status line), **page 60/0** (the pane the page guard opened but never looked at now has a fixture and an assertion), **live 71/0** (the file/sharing/cap line, both rows newest-first, and **Forget** — previously uncovered — removing that memory and only it while the count follows to `1 of 500 kept`), plus a Rust test that the default is one shared file and that a workspace opting out leaves it. Falsified: hiding the sharing in the UI reddens ui + page + live; a harness that claims a private store reddens live; `memory_file` losing its empty→default rule reddens the Rust test.  **Durable reminders (rev 119)**: the scheduler DSH has and BOS did not — `crates/gui/src/schedule.rs` plus a Schedule pane, with the clock handed in by the caller, `cron ≥ 1 min` as a stated rule, a 200-delivery ring, and a tick that delivers a missed period once (row 101). **The curated knowledge base (rev 120)**: `crates/gui/src/knowledge.rs` — a durable store (500 entries, titles 120 chars, bodies 4000, 8 tags), a pane that curates it, and a `knowledge_search` tool registered on **every session's agent** so the model pulls a note when it needs one instead of memory being injected into every prompt. The registration is tested against a real agent, through the registry's own dispatch (row 102). ⬜ still open in M11: workspace scope for notes and their provenance, recall surfacing, the OS-level delivery a reminder needs to reach a closed app. |
| M12 | Multi-frontend: headless + stdio JSON-RPC SDK, scriptable from tests | external client drives a session over stdio | ✅ **library half (rev 87): `bos-stdio`** — newline-delimited JSON-RPC 2.0 on stdin/stdout in `crates/agent/src/bin/bos-stdio.rs`, methods `session.create`/`session.list`/`message.send`/`shutdown`, responses in **deterministic offline echo** shape (`echoed:true, deterministic:true, llm:"none"`) so a test client needs no network and no key. Error handling never panics and never drops the stream: unknown session `-32602`, unknown method `-32601`, malformed line `-32700` (with `id:null`) — the stream continues after each. **Independently driven, not just unit-tested**: a hand-written 7-line session reproduced `reply:"echo: hello"`, `count:1`, all three error codes in order, `{"ok":true}`, and exit **0**. Integration test `tests/stdio_driver_test.rs` spawns the binary via `CARGO_BIN_EXE_bos-stdio`; `cargo test -p agent` = **258 passed / 0 failed** (baseline 189). ⬜ GUI wiring (a settings toggle that launches `bsh --headless` is not built yet). |
| M13 | Polish: i18n zh/en, theming tokens, shortcuts UI, a11y audit | no hard-coded UI strings; a11y checklist | ✅ **step 1 (rev 87): the polish layer** — `ui/polish.js` translates *marks* (`data-i18n`/`-placeholder`/`-aria`/`-title`, 12 chrome keys, en/zh) and never keeps its own copy of a preference: it watches the attributes `shell.js` writes, so the Settings rows and the header toggles cannot disagree. A `?` reference dialog lists the **17 bindings the code actually implements** (composer, palette, tabs, dividers, Mod+B) with `aria-modal`, focus hand-back, Escape to close and a field guard so typing `?` in the composer is not hijacked. The header gains a theme control cycling the shell's light/dark/system, and the a11y pass adds a skip link, `aria-live` on the stream chip, `aria-keyshortcuts` on the two non-clickable keys, plus `prefers-reduced-motion` and `prefers-contrast: more` support. The markup guard grew to **49** checks, and one of the new ones — "no id may appear twice" — **caught a defect that had shipped**: `index.html`'s static `#sidebar-toggle` (app.js's narrow-window drawer button) and the one `shell.js` injected for the desktop rail both claimed that id, so `app.js` bound its handler to whichever element it found first (the injected one) and the static button was dead while the id selector showed *both* under 720 px. The shell now **adopts** the markup's button — verified by a page check asserting exactly one element holds the id (it read `2` before the fix, `1` after) — and the guard was itself negative-tested: injecting a dropped `aria-live`, a typo'd `btn.sennd` key and a duplicate `tab-chat` produced exactly three failures, and restoring produced zero. One further edge: `?` no longer stacks the reference over an already-open dialog. The footer also became a **registry** rather than a fixed pair of chips: `Shell.addStatusItem`/`updateStatusItem`/`removeStatusItem` keep one node per id in insertion order (a poller refreshes in place instead of stacking spans) inside a polite live region, so a panel can own a status item without knowing the footer's layout. Evidence: page **40/40** (13 new checks), ui **125**, shell **94**, markup **49**, layout **0**. One bug this round found and fixed by insisting on the stronger assertion: `shell.js` publishes `const Shell`, a global *lexical* binding that is **not** a `window` property, so `polish.js` had been silently falling back to painting `data-theme`/`lang` directly — the colours looked right and the preference was never persisted. The loader now resolves either binding and the harness asserts the choice reached `Shell.state` *and* the stored `bos.ui.v1` record. Limits: translations cover the chrome, not the JS-generated transcript strings; no key remapping; the reference is descriptive, not editable. ⬜ M13 proper (i18n of every generated string, custom keymap, full a11y audit). |
| M14 | Packaging: signed bundle, upgrade/migration path, docs | install artifact + migration test | partly ✅ **artifact guard + versioned migration (rev 115)**; ⬜ signing/notarization, a real `cargo tauri build` round trip, CI wiring. `tests/package_selftest.sh` validates the artifact the config describes — **16 checks**: the config parses, `identifier` is reverse-DNS and not the Tauri placeholder, the window title and `productName` agree, `frontendDist` is a real directory holding a non-empty `index.html`, `withGlobalTauri` is on **because `ui/*.js` actually reads `window.__TAURI__`** (the flag is load-bearing, so the guard fails if either side moves), no `beforeBuildCommand` (the artifact assembles with no toolchain, network or package manager), `bundle.active`, every listed icon exists, is non-empty and carries **its format's magic bytes** (`89 50 4E 47`, `00 00 01 00`, `icns`), one icon per platform, and a declared version parses as numeric-dotted. The migration path is now **versioned**: `SETTINGS_SCHEMA_VERSION = 1` is written into `settings.toml` with the field-level serde default deliberately `UNVERSIONED_SETTINGS = 0`, so a pre-version file cannot look already-migrated; `migrate()` runs the shape steps and stamps the version **last**, returning whether it advanced. Two Rust tests pin it: an unversioned file loads at the current version, keeps its own values, gains its workspace, saves the stamp, and advances nothing on reload; and a file with no key reads as `0` rather than as current — the test that would have caught the container-level serde fallback silently reporting the newest version. Deliberate: reading is side-effect-free, so the stamp becomes durable on the next **save**. |

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
| 14 | 2026-10-09 | The per-server MCP switch is enforced: `attach_mcp_tools` takes the effective settings and skips handshakes the workspace did not opt into, and takes the approval policy from the same place. The rule is a named function (`enabled_mcp_servers`) with its own test, including the "nothing configured means nothing attaches" case. |
| 15 | 2026-10-09 | The left panel groups by workspace: a `groupSessions` rule (pure, tested) turns summaries into workspace groups carrying name, root and bound model, and a header row renders each group with the root on hover and the model as a chip; a chat written before workspaces existed joins the workspace in use rather than a nameless bucket. The workspace list comes from the settings the shell already fetches. Switching workspace, adding/removing/renaming workspaces and the folder picker are still open. |
| 16 | 2026-10-09 | Switching workspace is real: `set_active_workspace` persists the choice, rejects an unknown id, denies the old agents' pending approvals, bumps the settings generation and clears the agent cache so the next build resolves the new workspace. Workspace headers are the switcher (root + model in the tooltip, active one marked). Exposed gap: the settings dialog still writes the **globals**, which the active workspace's overlay shadows — so the dialog must edit the active workspace instead (next), or a user will edit values the workspace overrides and see nothing change. |
| 17 | 2026-10-09 | The coherence gap is closed: `get_settings` returns the **effective** settings (what will actually run), so the dialog can no longer display a value the active workspace overrides, and `save_settings` writes the edited values into the active workspace as well as the globals that seed new ones. `Workspace` gains `system_prompt`, `temperature: Option<f32>` (0.0 is a legitimate value, so it cannot use the empty-means-inherit rule) and `reasoning_effort`, all overlaid by `effective()`. A workspace named after its folder follows it when it moves; the tool and skill deny-lists are deliberately untouched by the dialog, since overwriting them would silently re-enable what the user switched off. |
| 18 | 2026-10-09 | Workspace management exists at the command layer: `create_workspace` (seeded from the current defaults, so a new workspace behaves like today's installation), `update_workspace` (rename or move the root; sessions stay put because their approvals and tool calls happened in the old folder) and `remove_workspace` (the last workspace cannot be removed; its chats are deleted with it or re-homed to the successor, the caller's explicit choice per §24.6). Roots are validated the way file reads are: must exist, must be a directory, stored canonical — so `~`, `..` and symlinks cannot turn one folder into two workspaces; empty means the process working directory. Test pins the canonicalization (macOS `/tmp` is itself a symlink). Still open: the panel UI for these, the folder picker, per-workspace capability lists and presets. |
| 19 | 2026-10-09 | Workspace management is operable in the panel: "+ New workspace" and per-header ✎/✕ actions drive `create_workspace`/`update_workspace`/`remove_workspace` through one inline form (a webview may not implement `prompt()`, and an inline form is the version the harness can exercise), one applier keeps the "every workspace command returns the newly effective settings" invariant, and failures land in the status line rather than being swallowed. Removing asks what happens to the workspace's chats. Six harness checks cover the offer, the form swap, the fields, prefill, the chat question and the return of the button. Still open: the folder picker (no dialog plugin yet — the root field is typed and server-validated, which is the same guardrail the picker would use) and per-workspace capability lists/presets. |
| 20 | 2026-10-09 | Per-workspace tool selection is closed end to end: the workspace form lists the built-in tools (`bash`, `read_file`, `write_file`, `list_dir`, `plan` — the names the model sees, which is what `build_agent` gates on, so panel and server share one vocabulary), opens each one checked unless the workspace denies it, and `update_workspace` now takes the deny-lists. The payload is a **deny-list**, not a snapshot of every tool, so a tool added to the build later appears for people with no opinion. `disabled_skills` is carried through on save rather than cleared, because exposing it needs a per-skill seam in `crates/agent` that does not exist yet. Four harness checks cover the list, both open states, and the deny-list shape. |
| 21 | 2026-10-09 | The MCP per-server switch is operable: the workspace form lists the workspace's configured servers, opens each ticked per its stored flag, and sends the entries back with only `enabled` changed (pass-through, so a transport the panel does not understand survives), which `update_workspace` revalidates before storing and `attach_mcp_tools` enforces at attach time. The round's one red check was a fixture error, not the feature: my test ticked `git` by accident while the implementation only flipped the flag the test had changed — recorded because the distinction matters. |
| 22 | 2026-10-09 | The skills deny-list is enforced, not merely stored: `crates/agent` grows `register_skills_from_dir_denying` (the existing `register_skills_from_dir` delegates with an empty deny-list, so `nbos`/`jsbos` callers are untouched) and `caps::build_agent` passes the workspace's `disabled_skills`. Deny-list rather than allow-list for the same reason as tools: a skill added to the directory later is picked up without anyone opting in. Open, and pinned for the next round: the skip path has **no dedicated test yet** — the fixture to use is `crates/agent/tests/fixtures/skills`, and the test needs an `Agent` construction copied from `tests/agentic_mode_test.rs`; the panel also has no skill checkboxes yet, so the value is carried through on save rather than edited. |
| 23 | 2026-10-09 | The owed test exists: `a_denied_skill_is_not_registered` loads `crates/agent/tests/fixtures/skills` twice — once with nobody objecting (asserting several skills register, so the fixture is real rather than empty), once denying `calculator` — and asserts the denied skill is absent **and** the others are not, i.e. `kept.len() == all.len() - 1`. That is the difference between "the field is stored" and "the engine obeys it"; the skipped path is now behaviour-verified, not compile-verified. Remaining for skills: the panel has no checkboxes for them yet, so the value is carried through on save rather than edited. |
| 24 | 2026-10-09 | Skill selection is now editable in the same workspace form, which closes §24.8 for all three capability kinds (tools, MCP servers, skills). The list is the union of what the capability listing reports and what the workspace already denies, so **a denied skill stays visible and re-enableable even before that listing has been loaded** — the form never hides a switch the user flipped. A form that did not show the skills carries the stored deny-list through instead of clearing it. Five harness checks cover the list, both open states, the deny-list payload and the no-listing case. |
| 25 | 2026-10-10 | Presets exist at the model level, with **apply-and-diverge** semantics: `Settings::apply_preset` creates a *new* workspace as a copy of the preset and switches to it, so following a preset is impossible by construction — editing what you got can never rewrite the preset or anything else made from it. `apply_preset` is a command (persisting, bumping the generation, clearing the agent cache) and the test asserts the copy's identity differs from the preset's, that the name override works, and that mutating the applied workspace leaves the preset untouched. Open: no panel entry point for presets yet, and Choose folder is **blocked in this environment** — `tauri-plugin-dialog` is absent from the local cargo registry cache (0 hits) and the build is offline, so the root field stays typed + server-validated until the crate can be fetched. |
| 26 | 2026-10-10 | Presets are operable: they are offered right beside "+ New workspace" (applying one *is* a way of creating a workspace), labelled by name with a tooltip stating it makes a copy rather than a link, and the preset list rides along in the settings snapshot every workspace command returns so the offer cannot go stale. Three harness checks cover the offer, the create button staying put, and the empty case. Remaining for §24.9: creating/promoting a preset from an existing workspace (today the catalogue is populated by settings, not by a click) and presets over the full capability set. |
| 27 | 2026-10-10 | §24.9 is closed: a workspace can be promoted to a preset ("Save as preset" on an existing workspace, never on one that does not exist yet), which stores a **copy** whose id is blanked — a preset is not a workspace, so it has no sessions until it is applied. Duplicate names are refused rather than silently shadowing, because the name is how a preset is picked. No settings-generation bump: a preset is a catalogue, not a capability of the running agent. The test pins the copy, the drift afterwards, the duplicate refusal and the unknown-workspace error. |
| 28 | 2026-10-10 | No code this round, on purpose, and the reason is itself the finding: the residual streaming cost (§7) is O(N) because the transcript builds a row for every message, and the fix is windowing — but the renderer is a contract-verified path (one signature per streamed frame, zero per idle frame, absolute-index cursor) and a patch written without reading its row loop would have risked that contract to save one round. Instead the windowing plan is now concrete, derived from the renderer as it actually is: (1) `windowStart` is a message index and `msgs[windowStart + i]` is row `i`, so marking stays absolute at the call sites — `markDirty(abs)` becomes `markDirty(max(0, abs - windowStart))`, and an absolute index below the window is a no-op; (2) the window offset folds into the existing `epoch` string, which the renderer already uses to force `markDirtyAll()` when presentation flags change — a changed offset therefore invalidates every row through the mechanism that is already there, rather than through a new one; (3) "load earlier" is explicit (a button above the first row) so the transcript never grows sideways under a scroll position the user did not choose; (4) acceptance: the harness's structural assertions (1 signature/stream frame, 0/idle frame) must hold with the window at 200 over a 10 000-row fixture, and the streaming cost must stop growing with N instead of merely growing slower — the honest test being the 500 / 3 000 / 10 000 curve flattening. |
| 29 | 2026-10-10 | The read refined the plan, and found the constraint that makes windowing non-trivial: **windowing collides with the single-signature contract at the window cap**. `sameShape` trusts the leading rows by comparing `rowCache.size` and `messagesEl.childElementCount` with the message count, so a window changes the invariant to "row per *visible* message". At the cap, appending a message shifts the window by one: the retained rows now map to different messages, so either the frame re-signs all 200 rows (violating the verified one-signature-per-streamed-frame bound) or the implementation grows an explicit **append-shift path** that drops the first row, keeps the retained rows unsigned, and signs only the new tail — reusing the existing "rebuild only what changed, swap in place" machinery. Full-width windows also mean `opts.index`/editing/regenerate stay *absolute* while the row loop counts *relative*, which is why the plan's index mapping is load-bearing rather than cosmetic. The next round has the rest of the bookkeeping (`seen` pruning and the `desired`/DOM reconciliation) in hand and implements the append-shift path; acceptance is still the flattening 500/3 000/10 000 curve under the unchanged signature bound. |
| 30 | 2026-10-10 | **(row lost before rev 84 — an earlier round's append spliced the log into the section tables below §8 and row 30 vanished; content was never recorded. The interleaving was repaired in rev 84.)** |
| 31 | 2026-10-10 | Windowing was implemented, went red, and was **reverted automatically** — nothing shipped, and the diagnosis is specific. The ui harness reported "result block missing (script threw at load?)" rather than a failed check, a different failure mode that points at a deliberate guard I had put in the append-shift path: `if (!gone) throw new Error("window shift lost a cached row")`. Two mistakes, both mine: the predicate used `-1` sentinels (`lastWindowStart = -1`, `lastWindowLen = -1`) so it could match an early frame with no cached row to drop, and a **render path must never throw** — a mis-predicted fast path should fall through to the general pass, not kill the app. The fix: fold cache presence into the predicate, require `lastWindowStart >= 0`, delete the throw. Also required in the same change: the harness's structural assertions still encode the pre-windowing "a row per message" assumption, so they must become window-aware (rows == min(N, WINDOW + extra)) while keeping the 1-signature/stream-frame and 0/idle-frame bounds. The patch ran under a backup with automatic restore, and the restore was verified (`node --check` clean, 56 + 48 checks green) — the attempt cost the round, not the tree. |
| 32 | 2026-10-10 | Second windowing attempt, also reverted, and the honest blocker is now the **harness's own error handling**: it reports "result block missing (script threw at load?)" and swallows the exception, so three candidate causes were eliminated by inference instead of by evidence. Eliminated this round: the deliberate `throw` in the append-shift path (replaced by a precondition — a render path must fall through to the slower correct pass, never throw), the `-1` sentinels (`lastWindowStart >= 0` / `lastWindowLen >= 0` now required), and the `messagesEl.parentNode` insertion (`renderLoadEarlier` now returns unless the transcript is in the document). It is still a load-time throw in the windowed renderer. **Prerequisite for the next attempt, and a defect worth fixing for its own sake: the harness must print the exception it catches** (`err.message` + stack) — a self-test that hides its failure is worse than one that fails. Also learned: a window changes the harness's own premise ("first paint renders every message" assumed a row per message), so the window-aware assertions have to land in the same change as the feature rather than after it. Tree verified green after each revert (`node --check` clean, 56 + 48 checks). |
| 33 | 2026-10-10 | Third attempt at windowing: still red, but the failure is now **localized by evidence instead of inference**, and the difference matters. The self-test harness was taught to report its own load failures (both `error` and `unhandledrejection`, since a rejected promise in an async harness reaches the runner as "result block missing" — a message with no reason in it). With those listeners in place the windowed render produced **neither** event: the page does not throw, it **hangs** before the result block is written, which is exactly why three rounds of reading code for a throw found nothing. The window code is reverted again (tree green, 56 + 48 checks), the harness's error reporting stays, and the next step is no longer guesswork: bisect with `N = 5, 250, 2000` to separate "hangs on any window" from "hangs at the cap", then disable the append-shift path with a one-line `const shifted = false` to decide which of the two branches is responsible. Known-good: the un-windowed renderer, verified after every revert (`node --check` clean, 56 + 48 checks, smoke 124). |
| 34 | 2026-10-10 | The hang is bisected to **windowing itself**, and the append-shift path is exonerated. Two one-line probes, each with the rest of the change held constant: `WINDOW = 1000000` (no window, no shift, all my other edits present) → **56 + 48 green**, so every unconditional part of the change is correct; `WINDOW = 200` with the shift path disabled via `if (false && shifted)` → **still hangs**. So the fault is in the general windowed pass, not in the fast path I had spent three rounds suspecting. Prime suspect now, and it explains why no exception was ever seen: the harness indexes rows as if the DOM held every message (`rows()[i]` for message `i`), so with a shrunk window it dereferences `undefined` — and if that happens inside the harness's own `try`, the page ends with neither a result block nor an `error`/`unhandledrejection` event, which is exactly the signature observed. Next step is instrumentation rather than another guess: have the harness write a phase marker (e.g. its current phase into `document.title`) before each phase, so a bail is attributable to a phase instead of to "the window". Tree verified green after the restore (56 + 48, `node --check` clean). |
| 35 | 2026-10-10 | Correcting my own hypothesis from the previous revision: the harness contains **no polling construct** (no `setTimeout`, no `requestAnimationFrame`, no `while`) and its structural phase is a sequence of synchronous `renderMessages()` calls followed by `assert`s that *record* rather than throw. So "the harness waits for a row count that a window never reaches" is **wrong**, and the observed absence of a result block is not explained by a stuck wait loop. What the read did establish: of the harness's row-indexing sites, most degrade to plain assertion failures under a window (`afterAppendRows === N + 1`, `firstChild.textContent.includes(N)`, first-row identity), while `rows()[100]` stays valid for any window of 200. The hang therefore sits inside the windowed render call itself, reached synchronously — which is a smaller search space than before but still not pinned. Next step, deliberately minimal: re-apply windowing with `WINDOW = 5` at `N = 20`, where either the page completes and the recorded failures name the first broken assertion, or it hangs and the phase marker (`document.title`) attributes the bail to a phase. Two of my hypotheses have now been falsified by reading rather than by shipping, and both corrections are recorded rather than quietly dropped. |
| 36 | 2026-10-10 | Two probes narrowed the window failure from "scale" to "the window itself". (1) A 250-message transcript — window active, only fifty messages hidden — fails the same way as 2 000, so this is **not** a slowness or timeout effect and my previous round's perf theory is withdrawn too. (2) With the "Load earlier" control disabled by a one-line early return and everything else windowed, the run reported: `result block missing`. That **exonerates** the control: disabling it changes nothing, so the sibling insertion is not the cause either. Windowing is reverted again (tree green, 56 + 48 checks). Next step is one more bisect in the same style: keep the window, place the control somewhere inert (or render it as a zero-height element the harness's layout spy ignores) and confirm whether the window alone passes — three of my own theories have now been falsified by probing rather than by shipping, and each correction is recorded. |
| 37 | 2026-10-10 | **Fourth falsified theory, and the search space is now small and enumerated.** The windowed renderer fails at 250 messages as well as 2 000, with the "Load earlier" control disabled as well as enabled, and it passes only when the window is effectively absent (`WINDOW = 1000000`). Four of my own explanations have now been eliminated by probing rather than by shipping: a throw in the append-shift path, a polling wait in the harness, scale/slowness, and the sibling control. What remains, in order, is exactly two things: (a) the **shape check's new window terms** (`windowLen === lastWindowLen`, the `slid`/`delta` arithmetic, `messagesEl.childElementCount === windowLen`), and (b) the loop's **relative index mapping** (`abs = windowStart + i` for `msgs`/`opts.index`/`editing`/`lastAssistant`). The next probe separates them in one line: keep the window (row count still `windowLen`) but index `msgs` absolutely (`msgs[i]`), which breaks correctness for a scrolled window while leaving the shape check intact — if that run reaches the result block, the fault is the index mapping; if it still vanishes, it is the shape check. Worth stating plainly: four rounds of windowing have shipped nothing, and the honest summary is that the *plan* is being validated far more slowly than the code is being reverted. |
| 38 | 2026-10-10 | **The failure is isolated, and it is the index mapping, not the window.** One-line probe, exactly as planned: keep the window (row count still `windowLen`) but index `msgs` and the row options absolutely (`abs = i`) instead of window-relative (`abs = windowStart + i`) — the run then **reaches the result block and completes with 8 failures / 56 checks**. Those failures are the expected consequences of indexing wrongly (rows for the first messages instead of the tail), but the page *survives*; with window-relative indices it does not. So everything structural — the window length/offset arithmetic, the `slid`/`delta` slide, the subset `desired` list, the reconciliation, the control — is exonerated, and the defect sits in one of the seven places that consume `abs`: the message lookup `msgs[abs]`, `showCaret`, `editingHere`, `opts.index` (which feeds the row signature and the edit/delete/branch handlers), `regen`, `branch`, and `buildEditor(m, abs)`. The windowed file is now cached at `target/windowing/app.windowed.js` (jj-ignored) so the next attempt is one `cp` plus a focused edit instead of re-applying a 200-line patch — the cost of each attempt was itself part of why this took five rounds. Tree verified green after the restore (56 + 48 checks, `node --check` clean). |
| 39 | 2026-10-10 | Bisect table for the seven `abs` consumers, four variants at 250 messages in one pass: **A** all window-relative → missing; **B** only the row index relative (`index: i`, everything else relative) → missing; **D** flags and index relative with `abs` used *only* to fetch the message → missing; **C** all absolute (`abs = i`) → **completes, 8 / 56**. Reading the table: B and D keep the arithmetic and die, so the index *arithmetic* is not what kills the page. The one thing C changed beyond arithmetic is **which messages it draws** — C renders the head of the transcript and therefore never takes the caret branch (`abs === msgs.length - 1` is never true for it), while A/B/D all draw the tail and do take it. So the live hypothesis is not "windowStart + i is wrong" but "drawing the newest message inside a windowed transcript enters a path that cannot finish" — most likely the caret/streaming row, which is the only branch that C's mis-indexing silently skipped. That is a materially different and much smaller question than the one the last five rounds argued about, and it is exactly what the table was for. Next probe, one line: keep everything absolute but fetch the message from the tail (`m = msgs[windowStart + i]`), so the content and the caret branch are exercised with C's surviving arithmetic. Tree verified green (56 + 48). |
| 40 | 2026-10-10 | **The "hang" was a reporting artifact, and six rounds of reverting were chasing it.** Variant E (windowed, caret branch disabled) failed as well, falsifying the caret theory too — so I stopped inferring and instrumented progress instead: the windowed renderer writes `document.title` at the start of each pass and at each exit, and Chrome was run directly on the harness URL with `--dump-dom`. The dump reads `<title>pass#404/250 shape-known reconciled</title>` — 404 passes, the last one **completed** — and contains `"failures":1`, i.e. the harness wrote a result and the windowed renderer drew the transcript. The runner calls this "result block missing" because its detector requires the literal `<pre id="result">` and this dump does not contain that markup (the tag is not matched by either the runner's regex or a looser `<pre[^>]*result[^>]*>`), which is why every windowed run looked like a hang while the un-windowed ones matched. Verified consequences: the windowed renderer is not broken, the one recorded failure is the row-count assertion the window *should* change, and the passes completed through the reconciliation — the very code the last six revisions suspected in turn. What to fix first is therefore the **test harness's own reporting** (detect the result robustly, and only then trust its verdict), after which windowing has to be re-evaluated rather than re-written. This is also the clearest lesson of the stretch: I trusted a green/red signal from a runner I had never validated, and the repair cost six rounds that produced no code. |
| 41 | 2026-10-10 | **Windowed transcript landed and verified**, after the reporting artifact in the previous revision was cleared up. The runner now finds a result element whatever its tag (`id="result"`, any element) and says "no result element" instead of guessing "script threw at load?", and the harness's real complaint became visible: at harness line 260 it reads `rowCache` as if a row exists per message — `Cannot read properties of undefined (reading 'el')` — which is exactly what a window stops being true. The harness now runs its structural phases with the window open (`state.windowExtra = N`) and gives the window its own phase, whose first version measured nothing because it ran after the phases that empty the transcript (the detail had to go in the assertion *name*, since the runner does not print details: `rows=1 active=0 cache=0`); the phase now owns its fixture. Verified: **57 transcript + 48 shell checks green at 250, 500 and 2 000 rows**, Rust 99 passed, clippy clean, bundle rebuilt, smoke 124, and the contract bounds hold at every size — `signaturesPerStreamFrame: 1`, `idleSignaturesPerFrame: 0` at 500 / 3 000 / 10 000. **Not yet evidenced, stated plainly: the cost curve has not flattened.** Those measurements run in the unwindowed phases, so they still read `streamingRepaintMs` 0.15 / 0.51 / 2.04 ms and `fullRebuildMsPerFrame` 1.9 / 11.3 / 35.2 ms over 500 / 3 000 / 10 000 messages — O(N) as before. The window makes the *drawing* bounded; what is still missing is a streaming phase that runs **at the cap** and a measurement there. Until that number exists, the performance claim in this document stays open. |
| 42 | 2026-10-10 | **The streaming cost curve is flat, and this revision replaces the open claim with measured numbers.** A windowed phase now measures the same 120 streamed frames twice on the same fixture, differing only in the window flag: `N = 500` windowed **0.072** ms/frame vs unwindowed 0.114 (1.6x); `N = 3 000` **0.072** vs 0.518 (7.2x); `N = 10 000` **0.076** vs 2.095 (**27.6x**). Windowed cost is 0.072 / 0.072 / 0.076 ms across a twentyfold range — **flat**, where the unwindowed path still tracks transcript length as it always did. The rows drawn are 200 at every size, the signature bounds hold (`signaturesPerStreamFrame: 1`, `idleSignaturesPerFrame: 0`), and the suite is 58 transcript + 48 shell checks green at 500 / 2 000 / 3 000 / 10 000 rows, with Rust 99 passed, clippy clean, bundle rebuilt and smoke 124. Stated precisely so the number is not read as broader than it is: the measured case is a **tail mutation** within a fixed-length transcript, which is what a streamed token is; the **append-at-cap** case takes the same incremental slide (drop the row that left, sign only the new tail) but is not separately timed here, and the remaining unbounded work in the GUI is the cost of a *full* rebuild, which still grows with the transcript (1.9 / 11.3 / 35.2 ms at 500 / 3 000 / 10 000) — windowing bounds drawing and streaming, not history loading. |
| 43 | 2026-10-10 | **The gap left open last revision is closed: appending at the cap is bounded too.** The same measurement now runs a second mode where a message actually arrives each frame, which is what slides the window by one and forces the incremental path to drop a row and sign a row: `N = 500` windowed **0.142** ms/frame vs unwindowed 0.268 (1.9x); `N = 3 000` **0.191** vs 1.123 (5.9x); `N = 10 000` **0.148** vs 4.861 (**32.9x**). Windowed append sits at 0.142 / 0.191 / 0.148 ms across a twentyfold range — bounded, with `rows=200` at every size — while the unwindowed path grows eighteenfold over the same range. It is honestly about twice the cost of the tail-mutation case (0.072 ms) because it also drops the row that left the window and reconciles the slide, and it is still **independent of transcript length**, which is the property that matters. Suite: **59** transcript + 48 shell checks green at 500 / 2 000 / 3 000 / 10 000. With this, both halves of streaming — a token growing the last message and a message arriving — are measured bounded, and the remaining unbounded cost in the GUI is a full rebuild for history loading (1.9 / 11.3 / 35.2 ms), which is deliberate: windowing bounds drawing, not history. |
| 44 | 2026-10-10 | Per-workspace runtime configuration on the server side (§24.9, and the review directive that a workspace owns its model). `Settings::set_workspace_runtime(id, model, base_url, api_key, system_prompt, temperature, reasoning_effort)` is the seam; `update_workspace` now takes those seven values and calls it *before* it borrows the workspace mutably (the first attempt placed the call inside that borrow and the borrow checker was right to complain). Semantics match `effective`: blank strings inherit from the globals, a blank effort is an absence (`None`), and `temperature` stays an `Option` so 0.0 remains a setting rather than a hole — all three asserted in `a_workspace_can_be_pointed_at_its_own_model`, which also checks the overlay the agent would see and that an unknown id is an error. Verified: **100** Rust tests pass, clippy clean, rustdoc clean (the eight-argument seam carries a scoped `allow` with the reason written next to it), 59 transcript + 48 shell checks green, bundle rebuilt, smoke 124. Stated plainly about the scope: this revision makes a workspace's model, endpoint, prompt, temperature and effort **settable and persisted**, but the workspace panel does not send them yet, so the only way to set them today is the command — the panel fields are the next step, which is why the review item is not closed by this row. |
| 45 | 2026-10-10 | The workspace panel now carries its runtime settings, and the panel found two real bugs on the way. It renders **model, base URL, reasoning effort, temperature and system prompt** for an existing workspace (a new one inherits the globals, so those fields appear only in edit mode), **and the API key is deliberately not rendered** — a secret re-drawn into the DOM on every re-render is a secret in a screenshot, so it is carried through from the stored workspace instead; the harness asserts that the key does not appear in the form's markup. Two defects the harness caught immediately: `input.value = value || ""` turned the legitimate temperature `0` into blank (falsy trap, fixed to an explicit null check), and `const { invoke } = window.__TAURI__.core` captured the invoker at load, so a test double installed later was invisible — `invoke` is now late-bound (`(cmd, args) => window.__TAURI__.core.invoke(cmd, args)`), which is also more robust when the runtime injects its global after the script parses. That second fix is what makes the contract testable, and the check now proves it end to end: editing the form and saving sends `update_workspace model=bigger/model temp=0 effort=high key=secret-key` — the regression from the previous revision (the panel did not send the new parameters at all) is thereby fixed and covered rather than assumed. Verified: **61** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 100 passed, clippy clean, bundle rebuilt, smoke 124. |
| 46 | 2026-10-10 | Sessions can be re-homed (§24.6): a chat's workspace was stamped at creation and could never change. `SessionStore::move_to(id, workspace)` is the seam — it keeps the messages, the id and the title, because a move changes which configuration runs the chat, not what was said in it. The `move_session` command validates the target workspace, **refuses a streaming chat** (its forwarder is bound to the configuration it started with), drops the cached agent so the next turn is built from the new workspace, and returns the session list so the sidebar re-groups from the server's answer rather than guessing. The sidebar shows a workspace picker on each row, only when there is somewhere to move to. **More importantly, the new coverage found a latent crash that had nothing to do with this feature:** the session row read `clip(title, 24)` with no such variable in scope, so *any* workspace group holding at least one chat threw while rendering the sidebar — the previous fixtures only ever rendered an empty transcript, which is why 61 green checks never touched the line. Fixed to `clip(s.title, 24)`, and the new phase renders a real row so the class of defect is covered. Verified: **101** Rust tests, **62** transcript + 48 shell checks green at 500 and 2 000 rows (the new one reads `move_session id=s1 to=w2 options=2`), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. |
| 47 | 2026-10-10 | Workspace groups in the left panel fold away (a review item that was still open). Each group header carries a fold control (`▾`/`▸`) whose click stops propagation so it does not also switch the active workspace — the header's own job — and a collapsed group still counts its chats in the header while hiding the rows, so collapsing never loses information. The harness asserts the full contract rather than the click alone: one row before, zero after, **zero again after a repaint** (a stream tick or a search keystroke must not spring the group open) and the marker flips to `▸`. Stated honestly: `state.collapsed` is in-memory for the session, not persisted in the settings file, so a restart reopens every group — persisting it is a settings field this revision does not add. Verified: **63** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 101 passed, clippy clean, bundle rebuilt, smoke 124. |
| 48 | 2026-10-10 | **Settings now has a Workspaces section**, which was the last review item about where workspace management lives. The settings panel is one scrolling page whose tabs scroll to a section, so this is a section and a tab like the others (Workspaces, before Tools & skill), not a second page. Each card states what a workspace *is* — name and whether it is in use, its root (or "(current folder)"), its model (or "inherits the defaults"), its reasoning effort when set, and how many chats belong to it — and offers the two actions the sidebar header offers: **Use** (disabled on the active one) and **Edit**, which closes Settings and opens the sidebar form, because that is where the chats are. The list is refreshed from `renderSidebar`, the single point every session and workspace change already passes through, so it cannot drift out of step. Two implementation notes worth keeping: the section's buttons set `type="button"`, since a bare button inside the settings `<form>` submits it; and the wiring for the section's "New workspace" is null-guarded, because the harness page loads this file without the app shell. The harness check that had to be debugged was my own fixture's fault, not the feature's — `document.getElementById` resolves the *first* matching node and the harness page turned out to hold one already, so cards landed in the app's container while the phase read the node it had appended (`pre=2` in the probe, `htmlLen=0` on the wrong element); it now reads the node `$` resolves. Verified: **65** transcript + 48 shell checks green at 500 and 2 000 rows (the new ones read `cards=2` with both cards' text and `set_active_workspace id=w2`), Rust 101 passed, clippy clean, bundle rebuilt, smoke 124. |
| 49 | 2026-10-10 | **Content search across every stored chat**, the §17 parity item the GUI only approximated by filtering titles. `SessionStore::search(needle, limit)` scans titles, message text and reasoning, case-insensitively, newest chat first and then by id — a result list that reshuffles between identical queries is a result list nobody can use. The snippet is cut around the match with ellipses, and the character-offset arithmetic is done on `char`s rather than bytes so a CJK chat cannot panic it; when lowercasing changes a string's length (a few scripts do), the match is reported as absent rather than pointed at the wrong characters, which is the honest failure for a snippet. `search_sessions(query, limit)` caps at 200 server-side; the sidebar asks from **two characters on** (below that the local title filter is enough and no round trip happens), draws the hits above the groups as rows that open the chat they came from, and **drops a response whose query has moved on** — a late answer must not repaint a search the user has left. Deliberately not an index: the harness keeps FTS5, this scans, which is honest at these sizes and cannot drift out of step with the session files; the seam is where an index belongs if the scan ever shows up in a profile. Recorded as a remaining gap: a hit row does not yet show which workspace it came from, though the hit carries it. Verified: **102** Rust tests, **67** transcript + 48 shell checks green at 500 and 2 000 rows (the new ones read `rows=2 header="2 matches inside chats"` and `search_sessions query=quick limit=20`), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. |
| 50 | 2026-10-10 | The gap recorded in the previous revision is closed, and clicking a result is now correct rather than merely working. A hit row **names the workspace it came from**, because a search finds chats in workspaces that are not the one in use and a result you cannot place is a result you have to open to identify. Opening one handles both cases: a hit in the current workspace opens its chat directly, while a hit elsewhere **moves the workspace first and awaits it**, since that command re-lists the sessions and the chat has to be switched after that list is refreshed rather than during it — a race that would have shown up as a chat that opens into the wrong context, which is exactly the kind of bug the ordering comment now prevents. Three checks cover it rather than one: `labels=One|Two`, `activeId=s1` after clicking the hit in the current workspace, and `set_active_workspace id=w2` as the **first** call after clicking the other one. Verified: **70** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 102 passed, clippy clean, bundle rebuilt, smoke 124. |
| 51 | 2026-10-10 | **The token meter** (M-item), and it reports the provider's own numbers rather than an estimate — a local count would drift from the provider's exactly when the context is close to its limit, which is when the number matters. The agent already exposed `last_token_usage()`, so `StreamFinished` now carries `prompt_tokens`/`completion_tokens` (captured under the same lock that persists the plan, absent when the provider reported none) and the frontend updates the status line from it, only for the session that just ended — a background chat must not overwrite the meter of the one on screen. The status line reads `in 12k · out 678`, adds `88% of 14k` when a context budget is set, and says **"near the context limit"** with a warning class at **80%**, the point beyond which the next turn is more likely to be compacted than answered; `fmtTokens` keeps 999 exact, shows a decimal in the thousands and drops it past ten thousand, because a decimal there is noise. My own expectation in the harness was the wrong one (`12.3k` for 12 345) and the implementation was right; the leak of that mistake into a green run is exactly what the assertion name carrying the numbers prevents. Verified: **72** transcript + 48 shell checks green at 500 and 2 000 rows, Rust 102 passed, clippy clean, bundle rebuilt, smoke 124. Stated as a scope limit: the meter reflects the last completed turn (it is not a live count while streaming), and it appears once a turn has reported usage. |
| 52 | 2026-10-10 | **The memory panel** (§20): the low-level agent already attaches a capped `FileMemory` (`caps.rs`) and remembers exchanges itself when auto-remember is on, so the missing piece was the window onto it, not the machinery. Everything the GUI needs was already on `Agent` — `memory()` returns the store, and the store's `all`/`add`/`remove` are the operations — so this round **touched no line of `crates/agent`**: the GUI stays an adapter, exactly as the crate layering asks. Three commands (`list_memories`, `remember_memory`, `forget_memory`) clone the `Arc<dyn MemoryStore>` out under the state lock and then await on it, because holding the lock across a suspension point would serialise every chat behind a memory read; each answers with the list as it now stands, since the caller's next question is always "what is left?". `clean_memory_text` refuses a blank memory and caps one at 2 000 **characters** (not bytes, or a CJK memory would be cut to a third of its allowance) and is split out so the rule is testable without a running app. The panel adds a Memory tab and section beside Workspaces: the list, a field, Remember, and a Forget per row — and the empty store says so rather than looking broken. Verified: **103** Rust tests (the new one pins the trim/blank/character-count rule), **75** transcript + 48 shell checks at 500 and 2 000 rows, clippy clean, bundle rebuilt, smoke 124. Recorded against myself: for the second round running my *assertion* was wrong and the code was right — I had written that a refused blank memory should still produce one server call, when refusing before the round trip is the entire point — and again it was the assertion name carrying the counts (`blankCalls=0`) that caught it rather than letting it pass as green. Scope limits: memory is one store for the whole app, not per workspace, so the panel does not say which workspace an item came from; the panel has no search or edit, only list/add/forget; and the meter/list refresh when Settings opens rather than live. |
| 53 | 2026-10-10 | **Memory is per workspace**, which closes the first limit recorded against the previous revision and is what the workspace-as-aggregate-root model implies. The survey changed the shape of the work: `Workspace.memory_path` and its `pick` in `effective()` **already existed** and `caps::build_agent` already opens the store from the effective settings, so the data layer was never the gap — **nothing could set it**. So this is a threading round: `update_workspace` and `Settings::set_workspace_runtime` take `memory_path`, blank means the shared file (the same rule model, base URL, key and skills directory follow), and the workspace form gains one field. Because the agent cache is cleared on every workspace command and the agent is built from the effective settings, each workspace now opens its **own** memory read from a clean rebuild — no new plumbing, just reachability. The panel also names its owner ("Memory for One"), since a list you cannot check against the right file is a list you cannot trust. Two mistakes of mine are recorded rather than smoothed over: my call-site patcher inserted the new argument **before a closing paren that already had a trailing comma** and produced invalid Rust, repaired by a pattern fix on the one broken site (the test call, whose indentation made it distinguishable) rather than by reverting; and the new settings test panicked on `index out of bounds: the len is 0` because a fresh `Settings` carries **no** workspaces until `migrate()` adopts one for the current folder — the test now migrates first, which is also what load does. Verified: **104** Rust tests, **76** transcript + 48 shell checks at 500 and 2 000 rows (`owner="Memory for One"`, `update_workspace memoryPath=~/w1.jsonl` then blank = inherit), clippy clean, rustdoc clean, bundle rebuilt, smoke 124. Scope limits: `memory_enabled` is still a global switch rather than per workspace, and the panel does not show the resolved file path. |
| 54 | 2026-10-10 | **The narrow-window pass, done without eyes and therefore done honestly.** This session's model cannot read images, so a screenshot baseline was not an option and claiming a visual pass would have been a fabrication; instead the layout is **measured**. `tests/layout_selftest.sh` + `layout_probe.js` inject a probe and a Tauri shim into a copy of `index.html` and ask the browser for geometry at four widths, and the shim matters: without it the first Tauri call throws and everything below it in `app.js` never runs, so a probe that skipped it would click a button that does nothing — which is exactly what the drawer check did until the shim was added. Three findings, in order of how they were found. (1) The shell and the settings modal were already sound: no horizontal overflow, nothing off-screen, the modal shrinks (520/460px at 560/500) with no clipped content. (2) The sidebar's 280→56px "collapse" was **not designed** — it was main's min-width *squeezing* it, leaving a chat list that could not be read or clicked, and the geometry check was blind to it because the squeezed text overflows its own box rather than the viewport. The guard now tests the box's width against its rows, and it **failed on the real defect** (`sidebarSqueezed=True rows=4`) before any fix. (3) The fix is a deliberate drawer below 720px with a toggle in the chat head, and the drawer then introduced a real regression the guard also caught: the parked drawer hangs off the left edge, so the whole app could be **panned sideways** — verified by asking the browser to scroll (`scrolledX=348`), not by reading a `scrollWidth` number, which is harmless once something clips it. Clipping `html`/`body` (not just `#app`) settled it. Two honesty limits are built in: the probe disables the transition so it measures the settled drawer and **does not claim to measure the animation**, and Chrome on macOS clamps the window to ~500px, so 420px was never reachable and is no longer claimed. My own process notes are recorded too: several patch attempts died on quoting or on an anchor that had already changed, and one of those silent deaths meant a check I believed was running (the `drawer=` field was never printed) was not — the guard failing loudly is what exposed it. Verified: **0** layout failures at 1440/900/560/500 with the drawer opening to 280px and closing again, 104 Rust tests, 76 transcript + 48 shell checks, clippy clean, bundle rebuilt, smoke 124. |
| 55 | 2026-10-10 | **The extension surface is now visible from inside the app, and a factual error of mine is corrected in code rather than in prose.** The requirements list this document has carried since its early revisions said "12 hook events"; the agent's `HookEvent` enum has **seven** — before LLM call, after LLM call, before tool call, after tool call, on message, on complete, on error — so the list was wrong and nobody could have noticed, because nothing read it. `HookEvent::ALL` is now the one list a host may read, documented as needing hand-keeping in step with the enum, with a test that pins all seven names and rejects duplicates: adding a variant without listing it fails the suite instead of silently shrinking the surface the app advertises. The capabilities panel gained the hook surface from that list, shown **whether or not anything is registered**, because "what can I build against?" has an answer even in an empty agent. Plugins are a Rust trait (`AgentPlugin`), not a file format, so this round deliberately does **not** pretend the GUI can load one: the panel names the seam and its events and stops there, which is the honest reach of a thin host. To make the panel checkable the renderer was split into `renderCapabilities(host, caps)` — pure DOM work with the snapshot passed in — which the harness drives with a fixture, and a real improvement fell out of it: the panel can now be tested at all, where before it was reachable only through an async `invoke`. Verified: **111** agent lib tests (the new one among them), **104** gui tests, **78** transcript + 48 shell checks at 500 and 2 000 rows (`groups=Tools|MCP tools|Plugins|Hook events (Rust extensions)` with all seven named), **0** layout failures at 1440/900/560/500, clippy and rustdoc clean for both crates, bundle rebuilt, smoke 124. Scope limit, stated plainly: the GUI lists the hook surface; only Rust code in this repository can attach to it. A figure in this row was wrong when first written — it said 110 agent tests because a truncated `grep` hid the lib line — and was corrected in the same round by re-running the suite, which is the difference between checking a number and asserting one. |
| 56 | 2026-10-10 | **Per-workspace skills, and a method for the threading rounds.** `Workspace.skills_dir` and its `pick` in `effective()` also already existed, and `caps::build_agent` already registers skills from the effective directory, so as with memory the gap was reachability, not machinery: `set_workspace_runtime` and `update_workspace` take `skills_dir`, blank inherits the shared directory, and the workspace form gains one field beside the memory file. The Rust test mirrors the memory one exactly — blank yields the global path, a value yields that value — so the two per-workspace paths are pinned the same way. The method is the part worth keeping: last round a generic call-site patcher inserted an argument before a closing paren that already had a trailing comma and broke the file, so this time each `set_workspace_runtime(` call site was **counted** — paren-matched, arguments counted at top level — and only the sites still carrying eight arguments were touched. That found the real shapes (two one-line test calls among the multi-line ones) and its first attempt still failed, because a one-line call has no trailing comma to insert after; the four sites were then repaired by pattern and the argument inserted correctly. A second failure was the harness's own guard: a `SyntaxError` for a re-declared identifier, because every phase shares one script scope — the new phase's locals were renamed and the page loaded. Verified: **105** gui tests, **79** transcript + 48 shell checks at 500 and 2 000 rows (`update_workspace skillsDir=~/w1-skills` then blank = inherit), **0** layout failures at 1440/900/560/500, clippy clean, bundle rebuilt, smoke 124. Scope limits: `disabled_skills` is already per workspace and unchanged, but there is still no in-app skill authoring — the form points at a directory, and skills are files a person writes. |
| 57 | 2026-10-10 | **Approvals v2: once, this session, or always.** Approving each side-effecting call separately is the thing that makes people disable approvals altogether, so the card now offers **Allow for this session** and **Always allow** beside **Allow once** and **Deny**. The design keeps the decision where it belongs: the broker answers *before* it asks, so an allowed tool never raises a request at all — `request` short-circuits and returns an empty id, which is the same statement to the UI (there is nothing to answer). A session allowance is cleared when a **different session** becomes active and survives a new **turn** in the same session, which is what "this session" means; a test pins exactly that boundary, and another asserts an allowed tool mints no id and leaves nothing pending. An "always" is persisted in `Settings.allowed_tools` (global, not per workspace — stated plainly, because the per-workspace plumbing added two rounds' worth of signature churn for a switch that is really a cross-workspace convenience), adopted by every broker as it is built, and removable through `forget_allowed_tool`, which also clears the live brokers so a removal takes effect without a restart. Honesty about the harness: the allowance command is issued synchronously and is asserted as such; the answer that follows it is awaited, and the check's name says so rather than pretending both were observed. Two of my own mistakes are recorded rather than hidden: three insertions in a row landed in the wrong `Settings` initializer — first a brace walk that matched the empty `impl Default`, then a `Settings {` search that matched a `..Settings::from_config_value(..)` spread in a later function — so the compiler's own E0063 locations were used instead, and the field went into the real constructor at line 609; and clippy then caught a method I had written for a settings list that does not exist yet, which was **deleted** rather than annotated away, with its tests re-based on `is_allowed`. Verified: **108** gui tests, **81** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: the settings panel does not yet list or remove the persistent allowances (the command exists and the data is in `get_settings`); and `require_approval` alone still decides whether the gate exists at all. |
| 58 | 2026-10-10 | **The loop from last round is closed: the permanent allowances are visible and removable where the gate is configured.** The list sits under the approval checkbox, because that is where someone looks when a tool has stopped asking, and its empty state says *Approvals are asked every time* rather than showing nothing. Removing one calls `forget_allowed_tool`, which clears the broker's live allowance as well as the stored list — the round-78 command existed precisely so that a removal did not need a restart, and the UI now reaches it. The renderer is a pure function of the names, so the harness drives it with a fixture; the removal click is asserted synchronously on the command it issues, and the awaited reply is what would redraw, which the never-settling promise keeps out of the assertion. The harness's single script scope bit me a **second** time — a `SyntaxError` for a re-declared `rows` — so this phase's locals are prefixed like the last one; two occurrences make it a convention rather than an accident, and the convention is to prefix a new phase's locals. Verified: **108** gui tests, **82** transcript + 48 shell checks at 500 and 2 000 rows (empty state plus two rows with `forget_allowed_tool:bash`), **0** layout failures at 1440/900/560/500, clippy clean, bundle rebuilt, smoke 124. Scope limits: `require_approval` still decides whether the gate exists at all, and an allowance is global rather than per workspace, which last round's entry states and this one does not change. |
| 59 | 2026-10-10 | **The signature churn ends, and a claim I had carried for many rounds turns out to be false.** Every per-workspace field this project has added lately — memory file, skills directory — was threaded by lengthening one positional call and then repairing every call site, twice breaking a file doing it. `set_workspace_runtime` now takes a grouped `WorkspaceRuntime`: nine arguments became two, `#[allow(clippy::too_many_arguments)]` is gone because it is no longer needed, and the call sites became **shorter** (a blank payload is `&WorkspaceRuntime::default()`), which is the sign the grouping was right. The round's premise was wrong, and that is the more useful finding: my notes have said *memory\_enabled is still global* for many rounds, but `Workspace` already carried `memory_enabled: bool` and `effective()` already picked it — the grep that settled it shows both the field and the assignment in the block whose comment reads "booleans, budget and the selection sets are the workspace's own". What was actually missing was any way to **set** it: `adopted_from` copies the global at creation and no command ever touched it. So the deliverable is the reachable switch — the command takes `memory_enabled: Option<bool>`, the workspace form carries a checkbox, and a payload that omits the field restates the workspace's existing value rather than reading an absent checkbox as off. My own drift is recorded as well: three insertions in a row landed in the wrong structure or field (a brace walk that hit the empty `impl Default`, a `Workspace {` search that found a spread in a later function, and a replacement that removed the runtime's field instead of the duplicate), each time because I anchored on remembered text instead of reading the file; the fix was to use the compiler's own E0063 locations. Also cleaned up: a `redundant_field_names` clippy error, two generated expressions that double-referenced (`Some(&x.to_string())`), and one that wrapped a `String` in `Some`. And the harness caught a **second** assertion of mine that was wrong rather than the code: I asserted an absent checkbox would omit the key, where the implementation restates the existing value — equally safe, differently shaped — so the claim was corrected and the check renamed to say what is true. Self-check on the record itself: this row was first written while clippy still reported a `needless_update` error — adding the switch made the command's literal specify every field, so its `..Default::default()` had become redundant — and the row nonetheless claimed clippy clean. The error was fixed and the claim re-verified before the round closed; the deeper mistake is that the conditional which decides whether to record keyed on the harness being green while clippy was still red, and that is worth more than the one-line fix. The first attempt at that one-line fix used an anchor that did not match — `memory_enabled,` is not adjacent to the update — and failed silently, which is the same anchoring habit in miniature: the second attempt located the line by the command's own position in the file and clippy went to zero. Verified: **109** gui tests (one flips the switch through the payload, one confirms no opinion leaves it alone), **83** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean, bundle rebuilt, smoke 124. |
| 60 | 2026-10-10 | **The seam between Rust and the webview is now guarded, and the guard is proven able to fail.** The round began by going looking for work and finding none where I expected it: `export_session` exists as a command *and* is already reachable through a palette `/export` entry, and a toast helper already exists — so instead of building a feature that was already there, the round cross-checked **every** command against **every** UI call site in both directions. The result is a clean 38/38 with no unreachable commands and no phantom calls, which is a stronger statement than "export works" and a cheap one to keep true: `tests/wiring_selftest.sh` fails if a registered command is never invoked (a feature nobody can reach) or if the UI invokes a name that is not registered (a call that dies at runtime), and reports both lists. A guard that has never been seen red is unproven, so it carries `--self-test`: it copies the tree, plants a registered command nothing calls, and requires the guard to fail — the same discipline as the layout probe's numeric rationale. Two mistakes of my own are recorded: the first patch attempt nested a heredoc whose delimiter (`PY`) terminated the outer block early, so nothing was written and the leftover lines ran as shell (the delimiter is now distinct, and the failure was loud rather than silent); and then the guard printed exactly the right thing while my *self-test* matcher was wrong, because Python's list repr quotes the name (`unreachable=['ghost_command']`) and I matched an unquoted form. That is the third time this project has shown the same pattern — rounds 72/73 and 80 also had my assertion text wrong rather than the code — so it is stated as a pattern here rather than as three anecdotes: in this project the most frequent defect class is my own expectation, and the harness is where it surfaces. Verified: guard green in both modes, **109** gui tests, **83** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, clippy and rustdoc clean. Scope limits: the guard is a static text scan, so a command name built at runtime would be invisible to it, and it checks names only — not that a call's arguments match the command's parameters. |
| 61 | 2026-10-10 | **The workspace form now covers the fields the workspace owns, found by counting rather than by assuming.** Last round's guard proved every *command* is reachable; this round asked the same question one level down — is every **field** settable? The inventory compares the 23 `Workspace` fields against the 16 `update_workspace` arguments and the 11 payload keys, and it found eight with no path from the UI: `providers`, `created_at`/`updated_at` (derived, correctly not settable), and **five genuine per-workspace policy fields** — `bash_enabled`, `file_tools_enabled`, `require_approval`, `context_budget`, `project_instructions` — each reachable only as a **global**. They were frozen at workspace creation, because `adopted_from` copies the globals once and no command ever touched them after. All five now travel end to end: `WorkspaceRuntime` gained them, the command carries them, and the form renders a checkbox each plus a budget input. That turns the previous round's one-off `Option` field into a **rule** worth stating rather than an exception: in this payload a `String` means blank-inherits, and an `Option` means an absent field is not an answer and the workspace keeps its own. Three pure helpers (`workspaceCheck`, `workspaceFlag`, `workspaceBudget`) keep that rule in one place and make it testable; the harness proves the restate path with a **bare form** and proves an unusable budget restates the old count instead of sending zero. My own slips: two anchors missed again because `cargo fmt` re-indents, and the compiler's `E0063` was again what located the literal — the same lesson as round 80, which suggests the reliable habit is to anchor on *positions the compiler reports*, not on remembered text; and a display slip, where the field-list print omitted the list even though the diff under it was computed correctly. Also worth recording that this round's assertions were right the first time, after three rounds in which the harness caught my expectation instead: the correction in round 81 was to phrase the claim as "what the printed shape actually is", and that is what worked here. Verified: **110** gui tests, **84** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring guard green in both modes, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: `providers` still has no UI path and is left out deliberately, since how a workspace chooses among providers is a design question this round did not answer; creating a workspace still adopts the globals, which remains the documented behaviour. |
| 62 | 2026-10-10 | **The last undiscovered surface was a config file: a profile is selected by typing its name, and nothing listed the names.** The same counting that found the workspace fields, applied one level up, compared all 25 `Settings` fields against every name in the webview: exactly two never appear — `config_api_key`, correct by design because it is the *resolved key* of whichever profile was chosen, and `profiles`, which is a real gap. `profiles` holds the `[llm.<name>]` sections discovered from the config file at load time and **deliberately never serialized**, and resolution works by matching a profile whose `name` equals `model` — so the only way to use one was to know its name from the config file. The GUI now lists them, and the list is **key-free by construction**: `ProfileInfo` carries `name`, `model`, `base_url` and a `has_key` flag, and a test serializes the payload and asserts the secret string is absent from it. A chip types the name for you, which keeps one place where a model is configured rather than two. The round-81 wiring guard did exactly what it was built for, one round after it was written: registering the command before wiring it made the guard go **red** with `unreachable=['list_profiles']`, and it went green once the UI called it — a guard that catches real drift, not only planted drift. Second, round 80's fix for the record itself worked: clippy flagged `field_reassign_with_default` in the new test, so the conditional **refused** to write this row until the test was restructured as a struct literal and every gate re-ran green — previously that row would have claimed a clean clippy it did not have. My other slip is recorded without a diagnosis I did not earn: the first patch script did not parse, because a `, 1)` from a `.replace` idiom was pasted into a slice assignment, so **nothing was written** and the anchors stayed fresh; and the command body first used `state.inner.lock()`, which I did **not** establish whether it failed on its own or as a cascade from the unresolved `settings::` path in the same signature — so the record says what was observed, and the fix followed a census of the file, where `state.lock()?` is the dominant idiom. Verified: **111** gui tests, **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring guard 39/39 green in both modes, clippy and rustdoc clean, bundle rebuilt, smoke 124. Scope limits: profiles are read-only in the GUI — they come from the config file and are never written back — and picking a chip fills the model field without saving it; `config_api_key` stays config-file-only on purpose. |
| 63 | 2026-10-10 | **An audit of the document's own numbers, whose first result is a negative — and the negative is the important part.** The requirements doc exists to describe *other* tools, so most of its figures are findings about Codex or DSH rather than claims about this code, and reading 41 claim-bearing body lines in context showed that the ones which look wrong are right: the **12 hook events** and **~57 slash commands** are Codex's, and the geometry figures (a 400 px centre, a 45 % right panel capped at 70 %, a 264–420 sidebar range) describe Codex's and DSH's layouts, not ours — I checked them because they read like implementation claims, and they appear nowhere in our CSS or JS, correctly. That distinction is exactly what bit me in round 76, when I read "12 hook events" as a claim about us and changed our code and doc over it; the doc was right about Codex, and our own count is 7. Two numbers **about us** were genuinely stale, both in the section whose whole job is to be trustworthy. The baseline said tests **555/0 (63 suites)**; measured with the doc's own command it is **581/0 (63 suites)** — the suite count was exactly right and the passed count had drifted by 26 as the project added tests. And the measurement note credited the **47** shell-contract checks to `ui_selftest.sh`; they are 48 and live in `shell_selftest.sh`. The fix is aimed at the class rather than the instance: R7's baseline line now names all four guards and their counts, states that they must be **re-measured rather than trusted because this number has been stale twice**, and records the widths that are actually measured plus why 420 is unreachable (Chrome clamps a window to about 500 px, so a 420 run reports 500). Scope limit I did not fix this round: the doc still mixes other tools' findings with our own implementation claims without marking every line, so a future audit has to read context to tell them apart — one sentence in the header says it, the body does not enforce it. Verified: the four counts above are what the scripts printed while writing this row (ui 85, shell 48, layout 0 failures, wiring 39/39), 111 gui tests, clippy and rustdoc clean. |
| 64 | 2026-10-10 | **Search stopped re-reading the store on every keystroke, and the measurement falsified my explanation of why it was slow.** `SessionStore::search` ran on every keystroke — the UI asks from two characters — and each call re-read and re-parsed every session file. It now parses from a cache keyed by the file's `(modified, length)`, dropping entries whose file is gone or has stopped parsing, so it holds at most the live sessions. `load_all` was deliberately left as a plain read: it has thirteen call sites and is not on the keystroke path, so churning them would have bought nothing. Measured by an ignored benchmark — `cargo test -p gui search_benchmark -- --ignored --nocapture`, 200 sessions × 30 messages, one matching message so every query scans everything — the first query costs **32.6 ms** and the following query **12.8 ms**, a **2.5×** improvement. Then the part worth recording: I predicted the per-query `String` clones of every message dominated and removed them; the measurement moved 13.22 ms → 12.83 ms, which is noise, so **the prediction was wrong** — the cost is the per-message `Vec<char>` allocations inside `hit_for`, which the clone removal does not touch. The clone removal is kept because it allocates less and changes no behaviour, but it is not claimed as the win, and the win is the parse cache alone. Limits stated rather than papered over: the warm query is still a full in-memory scan (12.8 ms at 6 000 messages, fast enough to be interactive and the next lever is an index well beyond this corpus size, which is not built); the benchmark is evidence for a person and not a gate, because a busy machine makes a timing flaky, which is the same treatment the layout numbers get; and one case can serve stale data — a rewrite that changes neither the file's length nor its timestamp inside the filesystem's granularity — which the code documents and the invalidation test pins for the case that matters, since any ordinary rewrite changes the length. Verified: **112** gui tests (6 ignored, the benchmark among them), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures, wiring 39/39 in both modes, clippy and rustdoc clean, smoke 124. |
| 65 | 2026-10-10 | **The optimisation I wrote was wrong, and the test written to attack it is what caught it — the numbers come after that.** Last round's measurement left the warm query at 12.8 ms, dominated by the `to_lowercase()` copy and the character vector `hit_for` builds for every message. Search now runs a pre-filter first that allocates nothing and can only skip work, and it is exact only where exactness can be **proven**: an ASCII needle against ASCII text, where byte equality under ASCII case folding is precisely the comparison the slow path makes. Everything else falls through. The first version of that condition read `needle.to_lowercase() == needle`, which proves **already lowercase**, not **has no case** — so it sent `quick` down the byte path, where `Quick` no longer matched. Both the pre-existing cross-session search test and the Unicode test written specifically to attack the fast path failed, so the claim was never made and the condition was narrowed to the provable case. This is the sharpest instance yet of the pattern recorded in round 62 — that the most frequent defect class in this project is my own expectation — except that here the expectation was embedded in an **optimisation** and would have silently dropped real search results had the tests not existed. Measured with the ignored benchmark: the warm query went **12.65 ms → 5.25 ms** (2.4×) and the cold one 32.6 → 25.4 ms; across the two rounds the warm query is **12.8 → 5.25 ms**, about **6×** against the 33 ms it started at. And the unsound version measured **1.97 ms** — 2.7× faster than the correct one, for the instructive reason that it called `str::contains` (a two-way search) while the sound version runs a hand-rolled `windows().any(eq_ignore_ascii_case)` loop. That makes the next lever measured rather than guessed: a **cached lowercase haystack** per session inside the existing parse cache, which would let every needle, CJK included, use `str::contains`. It is not built this round. Limits: non-ASCII and CJK searches keep the old cost, since no fast path is claimed for them; the benchmark stays evidence for a person and not a gate. Verified: **113** gui tests (6 ignored), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures, wiring 39/39 in both modes, clippy and rustdoc clean. |
| 66 | 2026-10-10 | **The lever round 65 measured, built — and the round that recorded nothing, which is what made the next round's anchor fail.** A cache entry now carries the lowercase copies search would otherwise rebuild, `(text, reasoning)` per message and index-aligned with the record, so the pre-filter is an **exact** `str::contains` on already-lowered text instead of the hand-rolled ASCII window scan that could only skip work. Exactness is the point: it is the same condition the slow path checks, so it serves any needle rather than only ASCII ones. Measured by the ignored benchmark with the same corpus: the ASCII needle's following query fell **5.25 ms → 1.99 ms**, and against the 33 ms where round 65 started, about **15×**. What this round did **not** establish is stated rather than implied: the benchmark still used only an ASCII needle, so the CJK claim — the whole reason the cached lowercase matters — was **left unmeasured**, and the attempt to add a Chinese needle failed on anchors `cargo fmt` had reflowed, so it was carried into the next round. Two further slips, both mechanical, both found by tools rather than by me: a multi-line `record.as_ref()` pattern did not match for the same reflow reason and the compiler's **E0599** located the line, which I read and fixed; and removing the old `may_match` helper left a blank line between a doc comment and its function, which `-D warnings` promotes to an error — so the recording attempt for this round **failed its gate and wrote nothing at all**. That refusal is the round-80 repair working, and the nothing-recorded is exactly what made the next round's stale anchor possible, which the next row records. |
| 67 | 2026-10-10 | **CJK measured, and the limit round 65 named is closed; then the recording step failed and told the truth about nothing.** The benchmark now prints two needles, because a CJK claim needs a CJK number rather than an inference from an ASCII one: an ASCII needle's following query is **2.17 ms** and a CJK needle's query is **2.37 ms** — the same cost as the warm ASCII one, so a Chinese query no longer pays the cost the ASCII-only fast path could not avoid. The bench's shape has to be read honestly: only the *first* needle's first query pays the parse, so the CJK row's printed **1.0×** ratio is an artefact of run order and **not** evidence of no speedup; the meaningful comparison is CJK-warm against ASCII-warm, which is the pair above. Then the failure worth more than the numbers: the row for this round was anchored on `\| 66 \|`, a row that **did not exist**, because the previous round's gate had refused to record anything — the script raised and printed nothing, **but the shell still ran `jj describe`**, so the commit description claimed a row the document did not contain. Repaired by writing both missing rows and by moving the guard one level further down, which is the round-80 repair applied to the recording step itself: the description must not be submitted until the row is **asserted present**. A measurement caveat is corrected here so it is never misquoted: my `clippy` counter also matches two `could not compile` summary lines, so it reports **3** for a single error — it is a presence test, not a count, and no count is quoted from it. Costs: memory is roughly the live sessions' text again, which the struct documents; the cache keeps its one documented stale window. Verified: **113** gui tests (6 ignored), **85** transcript + 48 shell checks at 500 and 2 000 rows, **0** layout failures at 1440/900/560/500, wiring 39/39 in both modes, clippy and rustdoc clean, smoke 124. |
| 68 | 2026-10-10 | **Compaction stopped being destructive.** `/compact` rewrote a chat's history into one anchored user turn plus an LLM summary and **discarded every folded turn** — a lossy summarizer was the only remaining copy, and there was no way to see or recover what it dropped. A session now keeps the folded turns in `archived` (a `#[serde(default)]` field, so existing session files still load and older ones simply have none), the write-out excludes the anchor that the live history retains — archiving it would put the same turn in the transcript twice on a restore — and `/compact` can be undone. Two commands carry it: `compacted_archive` reports how many turns are folded, so the UI offers the affordance only when it can do something, and `restore_compacted` puts them back. Restore **merges rather than replaces**: the folded turns are older, so they go **in front** of everything since, because a plain restore would silently delete every turn the user had after compacting — which is the same class of loss the feature exists to prevent, and it has its own test. The transcript gained a clickable **`⧉ N folded · restore`** status chip and a `/restore` palette entry; the chip is a `button` because unlike the other status chips it does something, and it renders nothing at zero. Verified: **116** gui tests (6 ignored) including three on the archive and restore paths — compaction keeps what it folds, restore merges instead of replacing, and restore refuses when nothing was folded — 86 transcript + 48 shell checks, 0 layout failures, wiring **41/41** with the two new commands reachable and no phantom calls, clippy and rustdoc clean. Two of my own slips, both caught by tools: `cargo fmt` reflowed my assertion lines so the anchor missed and the patch wrote nothing (fifth time this session — the reliable anchor is the shape the file prints), and my replacement test asserted five messages where the merge produces six, so the **test** was wrong and the implementation was right. This round also re-measured and corrected the baseline line rev 63 introduced: it had drifted to **586** tests and 41 commands, which is exactly what that line now instructs a reader to do rather than trust. That number is a small self-correction worth keeping: I first wrote 584 in this row by **adding three to the old total instead of measuring**, which is the very habit this row is about, so the row now carries the measured value. |
| 69 | 2026-10-10 | **A folded turn can be read before it is restored.** Round 68 made compaction recoverable but left the reader with a count and a button: the folded turns could be put back, not inspected, so the summary was still the only thing anyone could see. The chip now **opens** them in the document panel, through a new read-only `archived_messages` command, and restoring moved to its own `↺ restore` control beside it — viewing and undoing are different decisions, and one control cannot honestly be both. The preview is a pure `archivedPreviewText(messages)` renderer, so the harness can check what a reader sees without a backend: the turns are listed in order with their roles, each is clipped at 1200 characters with the remainder counted, an empty turn says so, and an empty archive says there is nothing folded rather than showing an empty panel. Wiring is now **42/42** with the third new command reachable and no phantom calls. Verified: **586** workspace tests over 63 suites, 87 transcript + 48 shell checks at 500 and 2 000 rows, 0 layout failures, clippy and rustdoc clean, smoke 124. Scope stated rather than implied: this round added **no Rust test**, because the new command is a read whose behaviour the store tests already cover — its reachability is checked structurally by the wiring guard and its count by the store tests, which is less than the behavioural coverage the archive and restore paths got last round. |
| 70 | 2026-10-10 | **Deleting a chat stopped being permanent.** `SessionStore::delete` was a bare `fs::remove_file`, so deleting a chat — or choosing to delete a workspace's sessions — unlinked it immediately and irreversibly, which **contradicted the rule this project already states for workspaces**, that removing one must never silently destroy sessions, and left a mis-click with no recourse. Deletion now **moves** the file into `.trash/` beside the store; the name carries no `.json` extension, so `load_all` and the search cache skip it exactly as they skip any other non-session entry, and a test pins that a deleted chat does not come back through search. The store gained `trashed()` — parsed rather than listed by filename, because a bare UUID is not something a person can recognise — plus `restore(id)`, which returns the title, and `purge_trash()`, which is now **the only place a session file is unlinked**. Three commands expose it (`trashed_chats`, `restore_session`, `purge_trash`), the settings panel gained a Deleted chats list with a per-chat restore and a confirmed Empty the trash, and the delete confirmation now says where the chat went instead of asking a question with no stated consequence. Both delete paths route through the trash, so the workspace path is recoverable too. Verified: **590** workspace tests over 63 suites — four new store tests among them, covering that deleting moves and restoring brings the chat back with its title, that a trashed chat stays out of the store and out of search, that purge is the only permanent removal and is idempotent, and that restoring something never trashed says so — with 88 transcript + 48 shell checks, 0 layout failures, wiring **45/45** with all three new commands reachable and no phantom calls, clippy and rustdoc clean, smoke 124. The gate refused this row twice before it was written, and both times for the gate's own reasons rather than the feature's: clippy first failed on an unnecessary `mut` **in one of my own new tests** — the round-80 repair catching a slip that testing alone would have missed — and then my recording command backgrounded the `cd` along with the compiler, so the guards were addressed one directory too deep and reported "not found" until they were called by absolute path. Limits stated rather than hidden: the trash has **no automatic retention or cleanup** yet — the spec's M6 asks for a 30-day policy and none is built, so the trash grows until it is emptied, which is a real cost of this change; and restore does not rebuild the cached agent, because the next turn does that from the record's own workspace field, which deletion never touched. |
| 71 | 2026-10-10 | **The shell now describes itself to assistive technology, and a guard reads the markup that no other check reads.** An audit of the shipped accessibility surface found four real defects, all fixed: the command and mention popups put `role="listbox"` on a **wrapper** while the options sat in a `ul` inside it, so the listbox did not own its options; option rows carried `role="option"` but no `aria-selected`, and nothing linked the composer to the active option; neither modal was a dialog at all, so a screen reader had no idea a modal had opened or what it was for; and `closeSettings` hid the dialog while leaving focus on a hidden control, which strands a keyboard user at the top of the document. `#cmd-list` and `#mention-list` now own their listboxes, options have stable ids and `aria-selected`, and the composer carries `aria-controls`/`aria-expanded`/`aria-activedescendant`. Both modals became `role="dialog" aria-modal="true" aria-labelledby` with real label ids, `closeSettings` hands focus back to the element that opened it, and the approval dialog takes focus on its explicit choice — **Escape deliberately still does not dismiss an approval**, because an approval has to be answered rather than skipped past. The three icon-only buttons (`✎`, `✕`, `✕`) gained `aria-label`s. Verified by a new structural guard, `markup_selftest.sh` (**25** checks), added because **the transcript harness builds its own fixtures and never loads `index.html`**, so none of this is reachable from it; the invariants are structural, so they are checked structurally. Two instrument failures are recorded rather than buried. First, the gate had been invoking `tests/shell_selftest.sh`, which **does not exist**: `$SH` came back empty and the shell section was never enforced, while the "48 shell checks" quoted in earlier rows came from `ui_selftest.sh`'s own output — **observed, but not gated** — and the gate now extracts that line from the run that actually produces it. Second, the new guard's first version failed on the **correct** markup, because I matched a remembered string (`id="cmd-list" role="listbox"`) when the file has `class` between the attributes: the sixth time this session that an anchor written from memory, rather than the shape the file prints, was the defect. The guard now matches the tag with a regex, so attribute order is no longer mistaken for an invariant. Verified: **590** workspace tests over 63 suites, 120 gui tests, 88 transcript + 48 shell + 25 markup checks, 0 layout failures across five widths, wiring **45/45**, clippy and rustdoc clean, fmt clean, smoke 124. Scope stated: this round added **no Rust test**, and the accessibility work is verified **structurally, not behaviourally** — focus hand-back, the approval focus move and the palette's assistive state are asserted as source invariants, because no harness loads the shipped page; closing that gap needs a harness run against `index.html` itself, which does not exist yet. |
| 72 | 2026-10-10 | **The shipped page is now executed by a guard, not just read by one.** Round 71 recorded its own limit: every harness in this directory builds its own fixtures, so `ui/index.html` was never run by any check and the accessibility work could only be asserted as source invariants. `page_selftest.sh` assembles a temporary page from the shipped bytes — the markup verbatim, a `<base>` so its relative script and style URLs resolve, the Tauri stub injected ahead of `app.js` exactly as the transcript harness does, and `tests/page_checks.js` appended last — and asserts against the real DOM. It settles the two claims that were previously unproven: **closing settings really does hand focus back to the opener** (`shown=true, active=sidebar-toggle`), and **Escape closes it and hands focus back too**. It also drives the real composer to open the real palette and finds a listbox that owns its 7 options with exactly one `aria-selected`, the composer naming the active option by id, and both cleared when the palette closes. The two runners now share `read_result.py`, so the missing-result diagnosis and its hard-won load-error lesson live in one place rather than being copied; re-running the transcript harness after that refactor reproduced 88 + 48 checks unchanged. Two instrument failures are recorded, both mine, both caught loudly rather than passing quietly: the first assembly put `<base>` **after the body opened**, where the HTML spec says it is ignored, so `app.js` resolved against `/tmp` and never loaded — the runner reported "no result element … page did not finish" for a page that had never been parsed; and the temporary page was created by `mktemp -t` **without a `.html` extension**, which Chrome will not render as a document from a `file://` URL, producing the same loud failure. Verified: **590** workspace tests over 63 suites, 120 gui tests, 88 transcript + 48 shell + 25 markup + **8 shipped-page** checks, 0 layout failures across five widths, wiring **45/45**, clippy and rustdoc clean, fmt clean, smoke 124. Limits stated: the harness **stubs the Tauri host**, so bridge-dependent behaviour stays out of its scope; it awaits only promises that settle in a microtask, so anything needing a timer or the event loop cannot be checked this way; and the approval dialog's **focus-on-show is still not behaviourally verified**, because the approval queue it reads is module-private — only its explicit choice being a live focusable button is asserted. |
| 73 | 2026-10-10 | **A chat can now state what it amounts to.** M5 listed a turn outline and session stats; neither existed. `SessionRecord::overview()` computes messages, asks, replies, tool calls, errored turns, visible characters, reasoning characters and the folded count, plus one outline entry per ask, and `session_overview` serves it read-only (wiring **46/46**). Three decisions are worth stating. Counts **include the turns a compaction folded**, because "how big is this chat" means the whole file rather than what survived the last fold, and `archived` reports how many of them are folded so a reader can tell the difference. Preview text is clipped **by characters, not bytes** — a byte slice of multi-byte text either panics on a non-boundary or produces mojibake, and this transcript is explicitly multi-lingual — which is exactly what the third test pins. And the status chip renders **nothing until an overview arrives**, so switching chats never shows the previous chat's numbers. The UI adds a status-bar chip (`12 msgs · 4 tools · 3.2k chars`, abbreviated by a pure `fmtCount`) and a `/stats` panel with the full breakdown and the numbered outline, built by a pure `overviewLines` that emits text and never touches an HTML sink. Verified: **593** workspace tests over 63 suites — three new ones, covering counts that include folded turns, an empty chat reading as all zeroes, and previews clipped by characters — with 89 transcript + 48 shell + 25 markup + **8 shipped-page** checks, 0 layout failures, wiring **46/46**, clippy and rustdoc clean, fmt clean, smoke 124. The gate refused this row **three times** before it was written, and none of the three was the feature. First clippy failed on `9 + 0 + 12 + 10` in a new test (`identity_op`), where the expectation was right and the expression was clumsy. Then `cargo fmt` reflowed two assertions — caught only because the gate grew a formatting condition in an earlier round, and invisible in every number it printed. Finally the gate refused a run in which **every check had passed**: my extraction read `failures: 0 / 89` with a last-number pattern, so it compared the 89 *totals* against zero. That is why this run echoes each sub-condition rather than a summary line, and why the same run re-confirmed that `grep -cE '^error'` reports one clippy error as two: only a compile-failure count is honest there. The M5 row in the milestone table was updated to say what is done and what is not, rather than left to imply the milestone landed. Limits: the outline is **text in the document panel and does not jump** to the turn it names; search is still a **live scan**, so an FTS index remains unbuilt; and **ZIP export does not exist**. |
| 74 | 2026-10-10 | **The transcript can be read backwards again, and the doc was wrong about why it could not.** I went to build M1 step 2 and found the transcript **already** rendered a 200-row tail window with a "Load earlier" button, so that part of the M1 row was stale. What the window lacked was a **spacer**: the scrollbar described only the drawn rows, and the conversation above them was reachable only by pressing a button. I measured before touching anything — `rowsFirstPaint` 500/5000/10000 with first render 18.6/121.5/237.9 ms and streaming repaint 0.16/0.87/2.05 ms, and those timing phases run with the window **deliberately open**, so they describe the full-render path rather than the shipped one. Landed: a top spacer stands in for the un-drawn rows (height = un-drawn count × a row average sampled off the streaming path, so the guarded zero-layout-reads-per-frame invariant holds), scrolling within 320 px of the top asks for one more window, and the reader's offset is credited so growing the window does not move them. The spacer is skipped by the row-reconcile walk because it is not a row, and the trusted-prefix fast path now compares a **stored** child count instead of `windowLen`, which the spacer would otherwise have defeated on every frame. The assertion I wrote caught a **real defect in my own first version**: it credited an *estimate* (`min(WINDOW, windowStart) × avg`), but the average is re-measured from what is on screen — so the credit used the new average (200 × 47 px) while the spacer gave up 200 rows at the old one (200 × 15 px), and the reader would have jumped 6 400 px. The credit is now the spacer's measured change, making "gave" and "moved" equal by construction. Getting there also falsified two of my own claims: the harness fixture was not a scrolling box at all (so `scrollTop` was always 0, invisible in every number in the failure message until I printed the intermediates), and one assertion read the offset *after* a later render had moved it again. Harness precision was fixed rather than worked around: the windowed phase counted `childElementCount` as rows and the spacer made it 201, so it now counts `rowCache.size`, which is what "a row per message" actually means. Phases 37/38 assert five separate things — only the tail is drawn (200 of 1 000), the spacer exists and the scroll height exceeds the window, scrolling asks for exactly one more window (rows 200→400, extra=200, spacer shrinks), growing does not move the reader (gave=3000, moved=3000), and the spacer disappears once the window covers the whole chat. Measured at N=10 000: repaint **0.103 ms** windowed vs **2.5 ms** open (**24.3×**), appends **0.151 ms** vs **4.925 ms**, rows **200** vs 10 000. Verified: **593** workspace tests over 63 suites, 94 transcript + 48 shell + 25 markup + 8 shipped-page checks, 0 layout failures, wiring **46/46**, clippy and rustdoc clean, fmt clean. The M1 row and §7 were corrected rather than left implying M1 step 2 was pending, and the timing phases' scope is now stated where the numbers are. Limits: the spacer height is an **estimate** from sampled rows, so the scrollbar is approximate while the window is partial (exact once it reaches the start of the chat); the headline timing lines still measure the full-render path by design, so the windowed figures come from the windowed phase; and the "Load earlier" button stays, now redundant with scroll-back but still a keyboard-reachable control. |
| 75 | 2026-10-10 | **A chat can leave the app as a file.** M5's last unbuilt piece was ZIP export, and it could not be delegated to a crate: Cargo.lock is offline, the way the Tauri dialog plugin is. So `crates/gui/src/export.rs` writes the container itself — **stored** entries, a CRC-32 table built in a `const fn`, and a central directory — which also means any tool can inspect the archive, and that is the point of escaping a database at all. Exports land in `~/.bos/gui/exports/<id>.zip` holding `README.txt`, `chat.json` (the record exactly as the store keeps it) and `chat.md` (the same turns, with **folded turns first**, because they are older). A `/export` palette command (wiring **47/47**) runs it and reports the path. Three decisions are stated rather than implied: entries are **stored**, so no compressor is needed and no byte of a transcript is at the mercy of one; timestamps are **DOS format**, because that is what the container carries; and **ZIP64 is not implemented**, so an archive is capped at 4 GiB and 65 535 entries — a limit placed in the module docs rather than discovered later. The tests are known-answer tests wherever a published value exists, which paid for itself on the first run: `crc32("123456789")` must equal `0xCBF43926`, and the DOS stamp of `1_700_000_000` (2023-11-14T22:13:20Z) **failed** because the seconds-of-day field was a `u16` — a day holds 86 400 seconds, so 80 000 truncated and every entry would have carried a wrong timestamp. A round-trip test also reads the archive back through a reader written against the format (names, sizes, per-entry stored CRC, the end record's entry count). Self-consistency is not evidence, so the format is checked from **outside** as well: `zip_selftest.sh` runs the real exporter and then has Python's `zipfile` validate what it wrote — integrity, the three entry names, `chat.json` parsing with its `messages` and `archived` keys, the Markdown naming both roles, and every entry stored with size equal to compressed size — **10 checks** where two implementations agree instead of one agreeing with itself. Verified: **598** workspace tests over 63 suites — six new, all in `export.rs` — with 94 transcript + 48 shell + 25 markup + 8 shipped-page checks, 0 layout failures, wiring **47/47**, clippy and rustdoc clean (module docs included), fmt clean, smoke 124. Recording this row cost three refusals, all mine and none the feature: clippy caught a `&` on an accessor that already returned a reference, `cargo fmt` caught unformatted Rust, and the recorder refused twice because it anchored on the baseline line's **prose** and then on a condition that no single line satisfied — it now edits the line that carries the guard list and asserts the result, which is the fourth time this session that guessing a string instead of reading it was the actual defect. The M5 row now says what is done and states plainly that an **FTS index is still not built**. Limits: no ZIP64 and no compression; the Markdown is a rendering rather than the lossless form (the JSON is lossless); exports accumulate in that directory, since nothing prunes them yet; and `/export` covers the active chat only, with no multi-chat or whole-workspace archive. |
| 76 | 2026-10-10 | **A measured negative result: the FTS index was built, then rejected.** An index is the obvious answer to a linear scan, so this round implemented one — an inverted index over session text, persisted, refreshed from the same `(modified, length)` stamps the search cache already uses, with **7 unit tests and 2 integration tests** — and then measured it against the path it would replace at four corpus sizes (300 sessions, 166 KB to 6.85 MB, needle `brown`). It lost every time: **1.34×** slower at 166 KB, 1.67× at 509 KB, 2.34× at 1.78 MB and **3.52×** at 6.85 MB, while the path it replaced stayed flat at 1.6–2.1 ms. Even after the extra syscalls were removed — the first version called `fs::metadata` once per session per query, ~2.4 ms for 300 sessions, which is twice the entire cost of the scan it was trying to beat — the index still lost. Instrumentation showed the index's own work was only ~0.6 ms of the 7.57 ms total (`sync` 287 µs, candidate lookup 295 µs, no re-tokenization, no save), so the remainder was in the surrounding loop and was **not root-caused**; rather than ship a measured pessimization or guess at a cause, the index was reverted and the scan kept. The design work produced three findings worth keeping. First, a search that uses `str::contains` is a **substring** matcher, so a whole-word index is unsound: a document holding `there` has no `the` token, and a query for `the` would have been told nothing could match. A **pre-existing** test whose needle was the single character `e` failed and exposed it, which is the clearest case yet of why the baseline tests earn their place; the correct tokenizer emits **character bigrams**, which makes the needed property true — if the needle occurs in the text, every bigram of the needle occurs in it, including inside longer words — and the soundness direction is now asserted over a corpus containing `there` with needles `the`, `he`, `ere` and `er`. Second, a token **absent** from the index can only mean nothing matches, so it should answer the empty set and let the caller skip every session; the first version answered "no opinion" and scanned everything, and a failing test surfaced that too. Third, the shipped scan is **stat-dominated, not match-dominated** — its cost is flat as the text grows 40× — which is the strongest evidence yet that the existing lowercased-cache design is the right one for this store. Kept from the round: two tests that pin the scan's behaviour (`a_single_cjk_character_still_searches`, a case no bigram index can answer, and `search_keeps_answering_the_same_way`). Verified: **600** workspace tests over 63 suites, ui/shell/markup/page/layout/wiring/zip guards all unchanged and green, clippy and rustdoc clean, fmt clean. Limits and process note: the loop cost was not root-caused, so "an index cannot help this store" is **not** the claim — the claim is that this index, as measured, did not. The revert itself went wrong once first, because `jj restore --from @-` took the file from an ancient revision instead of the previous round; `jj op restore` recovered it exactly, and the lesson is recorded here: snapshot, never guess a revision, and never reach for `restore` without a backup of the working copy. |
| 77 | 2026-10-10 | **The workspace on disk became visible.** M2 step 1: a lazy file sidebar under the session list — one directory per click, workspace-confined by canonicalization (absolute paths, `..` and symlinks leaving the root refused), `.git`/`target`/`node_modules` and dot-entries skipped by one shared policy that the `@`-mention expansion already used, capped at 1 000 rows with the cut reported, directories first then names case-insensitively, sizes shown. Open folders persist per workspace root and are **re-fetched** on the next launch; a folder that refuses to open keeps the reason on its row and retries on the next click. Three of my own defects surfaced and were fixed before recording: an error folder's row lost its reason because only its walked children could carry it (the reason now travels on the row itself, with a root-error case in the harness); `clippy::explicit_counter_loop` fired on a manual counter (`.enumerate()` now); and `strip_prefix` in a `let-else` borrowed past its scope (E0716 — the `PathBuf` is bound first). This round also **withdrew the rev-76 headline ratio** as unsupported: the "scan" it was measured against was a simplified stand-in that skipped snippet extraction; what stands is the indexed **miss** path at 15.18 ms vs the real search hit at 7.24 ms vs the scan at 2.21 ms at body 22 528, with the unexplained cost inside the indexed path never root-caused. Verified: **612** workspace tests, 104 ui + 48 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 48/48, clippy/fmt/rustdoc clean, smoke 124, doc assertions 10/10. Limits: no file watching, no preview images, no drag-and-drop, and "Choose folder" stays blocked offline (the dialog plugin is absent from Cargo.lock). |
| 78 | 2026-10-10 | **A command can be run and its output kept.** M2 step 2, the persistent terminal slot — shipped as a bounded **run panel**, not a terminal, because `portable-pty` is still absent from Cargo.lock, every Cargo.toml and the offline registry cache; the panel's header says "plain text, not a terminal" so the limit is visible where the user types, and a PTY upgrade remains a backend swap behind the same events (§12.4). `crates/gui/src/runs.rs` runs `sh -c` in the configured workspace root with stdin closed and two reader threads feeding one channel, so stdout and stderr interleave as they arrive; every line becomes a `run-line` event and the UI repaints at most once per frame. Bounded by construction: 2 000 lines per run (a longer run is killed and marked truncated), 4 096 characters per line (the cut is marked), a 60 s deadline (a hung command is stopped), invalid UTF-8 ends that stream with a visible marker, and the child is reaped on every path. Each run ends with one receipt line — `[exit 0 · 12 lines · 1.2s]` — and a failed start, a non-zero exit, a budget cut and a deadline are each named. Output survives a reload per workspace root (`bos.runs.<root>`, capped at 400), the same per-root pattern as the file tree; switching the root in settings swaps history. The module had to be named `runs.rs` because `runner.rs` was already the agent-stream Runner; two of my own Rust defects surfaced on the first build — `mod runs;` was simply missing from `lib.rs`, and one array cannot hold both `ChildStdout` and `ChildStderr`, so the reader is now a generic `pump` per pipe. Verified: **620** workspace tests (8 new in `runs.rs`), 113 ui + 48 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 49/49, clippy/fmt/rustdoc clean, smoke 124. Limits, stated in §12.4 rather than discovered later: no PTY (no stdin, no ANSI colours, no interactive programs, no Ctrl-C), one run at a time, and the command runs with the user's own permissions — the workspace root constrains the working directory, not what the command may touch. |
| 79 | 2026-10-10 | **Each workspace now keeps its own geometry**, closing M2's last open item: the shell persisted column widths under one global key, so two projects shared one layout. The seam is deliberately small and keeps the shell Tauri-free — the app names a scope (`Shell.setLayoutScope`, called from init and from the settings submit where the root changes) and the shell swaps the stored layout under the live panes: the geometry on screen is written back to the scope being **left**, then the new scope's adopted values run through the same clamping policy the boot path uses (`adoptLayout`, factored out so a scope change cannot skip it); a scope with no saved layout gets the defaults. The unscoped key stays the "default" scope, so layouts saved before scoping existed are still what you get with no workspace configured, and UI preferences (appearance, font, language) stay global — they are about the app, not the project. The run panel's **open** flag joined its persisted lines: the blob is now `{open, lines}` and the parser accepts the earlier bare-array shape, because nobody's panel should be lost to a tidier format; the parser is a pure function, so the harness pins it without touching storage. Two harness gaps were closed while there: the idle-chip transition was invoked but never asserted, and the new shell section asserts both swap directions, per-scope storage, the same-scope no-op and the default scope staying intact. Verified: **620** workspace tests, 117 ui + 54 shell + 25 markup + 8 page + 0 layout + 10 zip checks, wiring 49/49, clippy/fmt/rustdoc clean, smoke 124. Limits: a scope's layout is read once per swap (two windows on the same root do not live-sync), and the compact-window rules override stored widths for the window rather than per workspace. |
| 80 | 2026-10-10 | **The terminal arrived, closing M2 outright.** `portable-pty` (0.9) became fetchable in the offline cache, so the PTY backend rev 78 had deferred shipped as `src/pty.rs`: a `PtyRegistry` (managed by Tauri itself, so a webview reload re-attaches rather than respawns) holding one live shell per workspace root, with a streaming ANSI filter, a 64 KiB/400-entry tail ring, a pump thread plus a wait thread that owns the child (the killer lives behind a mutex because `ChildKiller::kill` takes `&mut self`), and an exit sentinel that tells "still running" from "exited 0". Four commands (`pty_start`/`pty_write`/`pty_resize`/`pty_kill`) and two events (`pty-output`/`pty-exit`) expose it; the panel gained term mode — `/term` or the Shell button — that sends lines to the shell **without local echo** (the PTY echoes; twice would be nonsense), holds chunks that split a line, resizes the shell to its own measured character grid, and tells the truth in the chip (`shell`/`attached` → `exit n`/`killed`) and in the honesty note, which now swaps with the mode. The persisted blob gained `mode`; old shapes still load and garbage degrades to run mode. Run mode is untouched. The crate was also **renamed `gui` → `bsh`** (lib `bshlib`, binary `bsh`) in the shared working copy mid-flight; the rename was completed forward — `main.rs`, harness scripts and docs all point at the new name. One environment fact is now pinned in tests: this sandbox refuses to allocate PTYs (`openpty` → `EPERM`), so the three spawning tests stand down **loudly** (`eprintln!`, no silent pass) and run in full on a normal dev machine; rev 78's "no PTY in this environment" claim is retired to history. Verified: pty 7 tests, 125 ui + 54 shell + 25 markup checks, wiring 53/53, clippy/fmt/rustdoc clean, smoke green. Limits: ANSI is stripped, not interpreted (no alternate screen, no cursor addressing — `vim` renders as scrolled text; a vt100 emulator is the upgrade path), and the sandbox this was built in cannot exercise a live shell end to end. |
| 81 | 2026-10-10 | **The web UI is now a reference-driven clone of DSH's frontend, on its own terms.** The ask was to clone DSH's frontend; DSH ships React/TSX compiled to a built `lib/client.js`, so copying *code* would mean vendoring a framework this sandbox cannot build and cannot test. The clone is therefore of the **contract**: DSH's own READMEs and published `.d.ts` seams are vendored at `docs/dsh-ui-reference/` (MIT, fetched from `@deepseek-ai/*` on npmmirror, provenance and a refresh recipe written down), and the structure, geometry, tokens and behaviour are reimplemented in this crate's dependency-free document. Three rules still diverged from the contract and are now closed: the right column **steps down to 300px** before reporting that it does not fit — and then **closes deterministically**, never reopening because the window widened; **opening the right column collapses a manually expanded sidebar** (space is conceded from the sidebar first); and the sidebar's auto-collapse moved from 860px to **1024px**, coupling it to the right column's breakpoint as DSH does. The theme presenter gained the three writes DSH owns — `color-scheme`, `body[data-ds-dark-theme]`, and one `<meta name="theme-color">` read back from the rendered body background — and `--frame-top-clearance: 48px` / `--frame-overlay-top: 68px` / `--frame-leading-clearance` are published for window-chrome seats. Every rule is held by checks rather than prose: the shell harness grew from 54 to **66 checks** (step-down, deterministic close, no-reopen, open-concedes-sidebar, marker/color-scheme/theme-color, clearances), and two harness assumptions the clone falsified were fixed rather than papered over — the layout probe counted a collapsed rail's hidden rows as an "unreadable list", and the rail sequence assumed the sidebar was expanded before the column opened. Verified: 125 ui + 66 shell + 25 markup + 8 page checks, 0 layout failures at four widths, wiring 53/53, clippy/fmt/rustdoc clean, smoke green. Limits: this is the **frame and theme layer only** — the sidebar/session seams, right-column docking, conversation node rendering, tool cards, panel registry and settings-card registry are referenced but not yet reimplemented, and the vendored copy is documentation held for study, not a runtime dependency. |
| 82 | 2026-10-10 | **The layout finally has a seam, not just two columns.** DSH's ui-layout is a keyed `main` slot whose `conversation` key is reserved and whose every other surface is a panel selected by id; ours was a document panel bolted to a divider, so nothing but the product could ever put a surface in the right column. The shell now carries that registry — `registerPanel`/`unregisterPanel`/`selectPanel` with `panels` and `activePanel` readable and the active key mirrored onto `#app[data-panel]` — and the **built-in document panel registers itself through the same public seam a plugin would use**, so the seam is exercised by the product rather than only by tests. Three properties are pinned instead of assumed: `null` returns to the conversation, an **unknown id changes nothing** (no phantom surfaces), and the reserved `conversation` key **cannot be registered or displaced**; a column that closes releases its surface, so a hidden panel is never the active one. Mod+B joined it as DSH's sidebar chord, and it **yields the chord to a control that has a better claim on it**: an editable field, or the terminal, where `Ctrl+B` belongs to the program on the other end of the PTY — a shortcut that steals a keystroke from a shell is a bug wearing a feature's clothes. Verified: 125 ui + 80 shell (14 new registry and chord checks) + 25 markup + 8 page checks, 0 layout failures across four widths, wiring 53/53 with no new commands, clippy/fmt/rustdoc clean, smoke 124. Limits: the registry selects surfaces but does not yet render occupant-provided content (a panel is an identity, not a mount point), only the right column is registered, and the `shell.leading` seat is still unimplemented. |
| 83 | 2026-10-10 | **The panel seam became a mount point — and closing a panel never really worked.** rev 82 gave the column a registry, but a panel was only an identity: nothing could render into it. It is now a docking host — a panel may carry `render(host)`, its surface is created on first activation and handed to that function **exactly once** (re-selecting reuses the DOM rather than rebuilding under the reader), selecting a right-docked panel **surfaces it** through the same concession path the documents panel uses, and unregistering removes the surface with it. Building it falsified something I had already shipped: `hidden` on the column **never hid anything**, because `.doc-panel { display: flex }` outranks the user agent's `[hidden] { display: none }`. It looked correct only because closing also zeroes `--doc-w`, so the panel collapsed to nothing by width while still being laid out — and the new host, which does not zero its own width, made the defect visible as an element hanging 146px past the viewport. The rule is now explicit (`.doc-panel[hidden] { display: none }`), so a closed column is genuinely out of the layout. That is the second time a new surface exposed an old lie rather than adding one. Verified: 125 ui + 87 shell (7 new mount-point checks) + 25 markup + 8 page checks, 0 layout failures across four widths, wiring 53/53, clippy/fmt/rustdoc clean, smoke 124. Limits: only the right column is a docking host; panels cannot yet request a full-width or full-window presentation, and the `shell.leading` seat is still unimplemented. |
| 84 | 2026-10-10 | **The goal card landed — and its first honest check falsified three things that were already shipped.** M3's goal surface is now a card directly above the composer (§12.6.8): objective, state chip (`active`/`paused`/`completed`/`blocked`), rounds against the cap, blocker, inline editor, with `#goal-open` and `/goal` as the entries while it is folded and `refreshGoal` on switch/restore/fork — backed by a `Goal` state machine in `crates/agent` and `get_goal`/`set_goal`/`goal_action` over the session record, as **static markup bound once** rather than a Shell panel (the reference gives that column to the conversation). Then the checks: (1) **the page harness had been reading the shipped page unstyled — every round since `<base>` existed**, because it was appended at the *end* of `<head>` and URL resolution happens as each element parses, so the stylesheet link before it resolved against the temp file and 404'd; every structural check passed on the unstyled document, and only a computed-style assertion could see it — `<base>` is now the first child of `<head>`. (2) **No global `.hidden` rule ever existed**: only components with their own `.x.hidden` rule hid anything (`rev 83` fixed one of these by hand), `.queue-list{display:flex}` carried the class and was never hidden, and 36 `classList` toggles were mostly no-ops — one `.hidden{display:none!important}` rule now closes every surface, and the goal check reads computed display, not the class. (3) **Focus hand-back was a lie the unstyled page was hiding**: the check opened settings with `#sidebar-toggle` as the opener, which is `display:none` above 720px, so `focus()` silently no-oped and the recorded opener was `body`; a probe showed `closeSettings` leaving `activeElement` on `set-model`, a field that is itself gone. The tests now assert their anchor really takes focus (on `#open-settings`) and cover the no-opener path, and `closeSettings` detects stranded focus (body, or an element with no client rects) and falls back to an on-screen anchor. Three instances of the round-62 pattern — my own expectation — in one round, each caught only because a check insisted on reading the real thing. Verified: **633** gui tests (63 suites, 6 new state-machine tests), page **16/16** (6 on the card, 2 on focus), ui **125** + shell **87**, markup **25**, wiring **56/56**, layout **0**, zip **10**, clippy/fmt/rustdoc clean, smoke 124. Limits: `advance` is explicit-only (the backend has no auto-advance), the objective carries no prompt-injection guard, only navigation/restore repaints from `get_goal` (no push event), and M3's todo repaint is still open. **Also this round: the log itself was repaired.** Every round row from 14 on had been spliced into the *section tables below* §8 (14–18 into §11.1, 19–28 into §11.2, 29–39 into the token table, …, 66–73 into §24.1, 74–84 into §24.8), so the round log read as interleaved garbage exactly where it mattered — a defect every prior round's append had walked past. All 84 rows now sit contiguously under `| Rev | Date | Change |`, each section table reassembled as header + separator + its own rows, and row 30 — which vanished in the same mess before this round — is a marked placeholder rather than an invented record. |
| 85 | 2026-10-10 | **The centre gained a Trajectory view (M10 step 1).** A `role=tablist` strip under the title switches the centre between the conversation and a read-only **Trajectory** view of the run — steps with timings, tools summarised with durations and running states in words, roles/errors carried in text rather than colour, the honest "No turns yet" empty state, and a **300-row cap with a truncation note** instead of silent growth; roving tabindex (click or ArrowLeft/ArrowRight), `/trajectory` and `/chat` command the strip, and `renderMessages` returns early on the trajectory tab so the conversation does not re-render underneath it. Files: `ui/app.js`, `ui/index.html`, `ui/style.css`, `tests/page_checks.js`, `tests/markup_selftest.sh`, plus the §7 baseline refresh (633 workspace tests, ui 125) that was measured in this round. Evidence: page **26/26** (5 tab checks), markup **37**, wiring **56/56**, ui **125**. Limits: the tab choice is device-local (not persisted) and the trajectory is a view of one run, not a deterministic replay — M10 proper. This revision sits beneath the published `main` bookmark and carries no description of its own, so the record is kept here; it was deliberately **not** rewritten, because describing it would have rewritten published history and forced a force-push. |
| 86 | 2026-10-10 | **The open code-scanning and dependency alerts were cleared — and the sidebar-rail fix rode along in the same working copy.** Security: `SECURITY.md` with private reporting, `permissions: contents: read` plus a concurrency group on all three workflows, `pyo3` 0.28→0.29 with `pyo3-async-runtimes` in `bus` and `nbos`, and refreshed `Cargo.lock`, `jsbos/yarn.lock`, and the `video/bos-intro` manifest + lockfile. Because this revision was the working copy while the rest of the rev-85 round was written, it also carries that round's second half: the **sidebar-rail root cause fixed**. Opening the right column forced *and persisted* `sidebarCollapsed=true`, so every later launch railed the session list to 56 px — proven by the user's stored WebKit record (`{"sidebar":280,"doc":329,"sidebarCollapsed":true,"docOpen":true}`) — although the rail was geometrically redundant at ≥1024 px (264+400+280+16 = 976 ≤ 1024, so sidebar + centre + column always fit). `openRightColumn` now concedes **width, not the column** (the sidebar narrows to a 300 px floor, never below `SIDEBAR_MIN` 264) and never rails it; the manual ☰ rail and the <1024 auto-rail are untouched; and layout schema **v2** heals a pre-v2 concession rail exactly once while keeping a rail the user chose — the old selftest assertion that *encoded the bug* ("open rails the sidebar") was replaced by "open keeps the sidebar column/session list" plus two migration checks (shell **89**). Also carried: the docs edits that name that contract (the §7 sentence, the M10 row, the §12 compact-window prose, the §12.6 heading, and §12.6.3's documented divergence from the DSH reference's line 30 — "opening the right panel collapses a manually expanded sidebar" is deliberately not cloned). Verified: fmt/clippy/rustdoc clean, **633** tests / 0 failed / 63 suites, ui **125**, shell **89**, markup **37**, page **26/26**, wiring **56/56**, layout **0**, zip **10**, smoke 124. This revision is published (`main` = `@origin`), so it was not rewritten. |
| 87 | 2026-10-10 | **The six remaining backend milestones got their UI, every registered command became reachable, and the feature points are now proved end to end.** Three streams landed in this one working copy. **The backend**: 16 new commands — M4 scoped approval rules that outlive a call (`approval_rules_list`/`_add`/`_revoke`, with a 200-entry `approval_audit` ring; precedence is a rank tuple so “deny beats allow” and “more specific beats broader” both hold, and a malformed rules file is skipped per entry and reported rather than fatal), M6 `token_meter` reconciled against the provider's own count, `spill_list`/`spill_read` over the tool outputs too large to keep inline, `compaction_status`/`compact_now`/`restore_compacted` on the existing fold path, M7 a job registry (`jobs_list`/`job_output`/`job_interrupt`) with a bounded tail, M10 a **deterministic** replay with no wall-clock fields so two runs are byte-identical, M11 `self_inspect` plus a deterministic transcript critic, and M14 a ZIP bundle whose manifest lists every entry including itself (its own row carries null size/CRC with a note saying why) verified entry-by-entry by CRC-32. Also from the same push: the security package — 18 prompt-injection rules with most-specific-deny precedence, plugin presets, external hook dispatch, an MCP policy layer, session-scoped skills, and a headless **stdio driver** (`bos-stdio`) driven by hand through JSON-RPC (echo round trip, `-32601`, `-32602`, `-32700` then stream-continues, `session.list`, clean exit 0). **The UI**: four new Settings sections — Jobs, Context, Rules, Inspect — and the sidebar's **Automation item enabled at last** (it had shipped `disabled` with the promise “arrive with the jobs milestone”; this is that milestone). Each panel is a pure `render(host, data)` plus a refresh that funnels failure into its own note instead of throwing into an unlistened handler, so the guard can drive it with a fixture. **The verification**: `wiring_selftest.sh` went **56/56 → 72/72** with zero unreachable and zero phantom — the red row the backend left behind is exactly what the harness exists to catch. `page_selftest.sh` went **40 → 59** by adding a *recording bridge double*: every run surface is now driven through its real handler, asserting both the arguments that crossed the bridge (`job_output{id,max_bytes}`, `approval_rule_add`'s whole predicate, `token_meter{session_id,provider_tokens:null}`) and the elements that appeared, plus one honest negative — a session-scoped panel must ask for a chat rather than call with an empty id. That assertion earned its place immediately by catching a defect the fixture could not: `compact_now` wrote its outcome into the panel note and then called the refresh, which **cleared the note as it redrew**, so the user would have seen the work happen and no word about it; the note is now written after the refresh. **The E2E layer**: `crates/gui/e2e/` drives the real window through tauri-driver + WebdriverIO 9 (five specs: shell chrome, M7, M6, M4 with a live add→revoke round trip, M11/M10/M14), with `preflight.js` failing fast when a precondition is missing and `.github/workflows/gui-e2e.yml` running it under xvfb. It **cannot run on macOS** — tauri-driver speaks WebDriver to the embedded webview, which means WebKitWebDriver on Linux and msedgedriver on Windows and nothing on a Mac — so that limit is stated in the README and the preflight rather than discovered by a cryptic connection error, and the same feature points stay covered on any machine by the page harness. Both npm and cargo work were done under the sandbox's rules (private npm cache, `--offline` for cargo). Evidence: **746** tests / 0 failed / 65 suites, ui **125**, shell **94**, markup **49**, page **59**, layout **0**, zip **10**, wiring **72/72**, wdio config validated by the real CLI (9.32.0, 5 specs discovered) and the suite's failure mode exercised on macOS. Limits: the WDIO specs are written against the real backend and are **unrun on this machine** by construction (their Linux CI run is the evidence they exist for); translations still cover the chrome, not generated strings). |
| 88 | 2026-10-10 | **The left panel and the Settings dialog were rebuilt to the reference the user supplied, the chrome's icons became one system, and three shipped defects fell out of the work.** Both screenshots came from the user. The sidebar now has the reference's shape: a brand row (the provided icon, the wordmark, a HARNESS pill), one full-width **New session** action, icon+label nav rows in the reference's order (Plugins & skills, Automation tasks, MCP connectors, Files), a **Workspaces** header that owns its search / reload / add actions — the search field is revealed by its action instead of permanently holding a row — a marker per chat (hollow while idle, filled while streaming, and the state *also* spelled out in text, because a ring is decoration and not information), and an identity row in the foot. Settings became a real two-pane dialog: a header row (title plus ✕ on the same close path, so focus hand-back is unchanged), a vertical `role=tablist` rail with roving arrows/Home/End, `aria-selected`/`aria-controls` and a roving tabindex, exactly one `role=tabpanel` pane at a time with card-styled groups, and a ≤720 px rule that turns the rail into a wrapped strip so the panes keep the width a label beside its control needs. **Icons and logo**: `crates/gui/icons/bos-harness-{logo,icon}.svg` are used directly (copied into the served `ui/icons/`, because `frontendDist` is `ui` and `../icons` cannot be served), the icon is the brand mark and the favicon, the lockup heads the empty document column, and `bundle.icon` lists the 512² PNG plus a generated multi-size `.ico` — `cargo build -p bsh` is what validates that config and it builds clean. Every remaining chrome glyph became a stroke SVG inheriting `currentColor`, so no icon is a colour emoji and none is out of step with its label. **Three defects surfaced, each caught by a check rather than by eye:** (1) **five custom properties were used but never defined** (`--muted`, `--fg`, `--panel`, `--line`, `--input-bg` — 27 declarations), each carrying a dark-only hard-coded fallback, so light mode painted near-white text on white in the file tree, the settings tabs and the workspace rows; they are mapped to tokens that exist in both palettes, and a new check fails on any `var()` naming an undefined property (negative-tested by injecting `--panell`); (2) **a `data-i18n` mark on a wrapper deletes that wrapper's children** when a translation applies — it had silently removed four rail icons and the new-session ⊕, and switching to 中文 would have removed them for real; the marks moved onto text spans and a new check rejects any mark that wraps elements (it caught `btn.new` on its first run); (3) **the rail inherited `flex-wrap: wrap`** from the old horizontal pill strip, and as a column that means *column-wrap*, so ten 198 px columns laid out **1998 px** wide — the layout probe named it exactly (`set-nav:2098>208`, six elements outside the viewport) and it was the only thing that would have. Evidence: tests **746/0** (measured: 52 of the 65 test binaries report cases; the rest are empty harnesses — the passed count is the number to hold), ui **125**, shell **94**, markup **52** (was 49), page **59**, live **34**, layout **0** across five widths, zip **10**, wiring **72/72**, `cargo build -p bsh` clean with the new bundle icons, fmt/clippy/rustdoc clean. Limits: the icon set covers the chrome only (generated strings still carry emoji), each pane keeps its whole DOM (no virtualisation), the reference's **Open configuration file** action is absent rather than decorative because no command backs it, and the WDIO suite is still unrun on macOS by construction. |
| 89 | 2026-10-11 | **The performance axis got its first guard, and writing it produced three silent false passes worth recording.** Two central claims about this UI were comments rather than facts — *"coalesce streaming repaints to one per animation frame"* and *"the streaming repaint is the hot path"* — so neither could regress loudly. `perf_selftest.sh` now runs the **shipped page** with the live bridge asked for a heavy workspace (`window.__PERF_SCALE__`: 302 chats, one chat of 5 000 messages), replaces rAF with a queue so frames are deterministic, and **counts the work instead of timing it**. Measured: a burst of **400 `agent-event` deltas → 0 repaints during the burst, exactly 1 frame queued, 1 repaint on that frame**, and the streamed text lands in **2 DOM mutations across 201 drawn rows** (a tail patch, not a rebuild); streaming causes **0 mutations of `#session-list`**; a full repaint draws **201 rows of 5 000** and mutates nothing for rows that did not change; a content query costs **1 `search_sessions` round trip** across 302 chats; boot at that scale completes and lists all 302. **Three of the guard's own bugs would each have reported green while proving nothing**, and an assertion caught every one: (1) the scale hook seeded its long chat *before* the bridge's own chats, so the app booted into a different chat and the burst took the **background-session** branch — the "0 repaints" that looked like perfect coalescing was really "no work happened", caught only because the check asserts the boot-active chat **is** the seeded long one (which is why `row.dataset.id` was added to the session row); (2) the bridge delivers an event **one macrotask later** ("like a real IPC push"), so a count taken straight after the emit loop was zero for a second, subtler reason — the harness now awaits the push; (3) a `MutationObserver` deposits its records on a **microtask**, so a counter read synchronously after the repaint was zero — it is now drained with `takeRecords()`, and drained **after** the frame rather than before it. That ordering mistake is the one to remember: a mutation count taken before the repaint measures the burst, which is supposed to touch nothing. **Why counts and not milliseconds:** the guard needs `--virtual-time-budget` for the bridge's pushed events to land, and under virtual time the clock advances only on timers, so a synchronous 201-row repaint measures **0.00 ms** — a lie about cost; wall-clock numbers therefore stay with `ui_selftest.sh`, which runs a static fixture without virtual time (M1: 0.103 ms windowed, 2.5 ms with the window open, at N=10 000). Evidence: perf **8/0**, whole suite re-run green (markup 52, ui 125, shell 94, layout 0, page 59, live 34, zip 10, wiring 72/72, tests 746/0). Limits: the guard counts work on one machine and sets no absolute time budget for the shipped page; it does not measure the terminal or run-panel streams (they share `scheduleRunPaint`, the same rAF-coalescing mechanism, and the live guard covers their behavior); and 400 events is a burst, not a soak — memory growth over a long session is still unmeasured. |
| 90 | 2026-10-11 | **The todo list had never rendered — the `#` in `$("#plan-list")`.** Giving the plan surface its first coverage (M3's other half: **0 of 8 guards** touched it) turned up a shipped defect that every existing guard had passed over. `$` in `app.js` is `(id) => document.getElementById(id)`, but the plan renderer called `$("#plan-list")` and `$("#plan-count")` — three call sites, all on the plan path — so the lookups returned **null**, `if (!panel || !list) return;` fired on every render, and the panel, its rows and its count never appeared. The commands (`get_plan`/`set_plan`/`restore_plan`), the markup, the editor and the tool-event re-pull were all present and all correct; the feature died on its first line. It is **pre-existing**, not a sidebar-round regression: `@-` carries the same `$("#plan-list")`. The diagnosis is the part worth keeping — five hypotheses died on evidence (*`refreshPlan` never fired*; *`get_plan` returned `[]`*; *`renderPlan` threw and `refreshPlan`'s `catch (_)` swallowed it*; *the edit-state branch returned early*; *a mutation record arrived too late*), and what settled it was a **temporary trace inside the function itself**, printing `panel=true list=false` — the panel resolved and the list did not, which is only possible if the argument carries a `#`. Evidence: live **36/0** (a turn whose scripted `update_plan` lands mid-stream repaints the list with no reload — `count=1/3 · 1 active`, 3 rows, `status-completed,status-in_progress,status-pending` — and the rendered panel is measured in the viewport above the composer: top 404, bottom 518, composer 602, inside 1440 px, a geometry that had never been exercised because the panel never drew), markup **53/0** with a new static check that no `ui/*.js` passes a `#` selector to `$` (negative-tested: inject one, watch it go red), and the whole suite re-run green (ui 125, shell 94, layout 0, page 59, zip 10, wiring 72/72, perf 8/0). The rendered panel was also measured rather than eyeballed: top 404, bottom 518, composer 602, inside 1440 px — geometry that had never been exercised, because the panel had never drawn. One harness inaccuracy fell out too: the bridge's seeded plan used `status: "done"`, which is not in the vocabulary the planning tool's own schema publishes (`pending`, `in_progress`, `completed`) — the seed now uses `completed`, so the icon and the count mean what they say. Limits: the plan **editor** (add/remove/reorder/save/cancel through `set_plan`) and `restore_plan` across a session switch still have no guard; the status vocabulary lives in both a JS constant and the Rust tool schema with nothing pinning them together. |
| 91 | 2026-10-11 | **The plan editor is now driven like a user, and it was hiding status.** Row 90 brought the todo list to life; row 91 covers the half that edits it, which had also never been exercised (the editor is only reachable once the panel renders, and the panel had never rendered). The live guard now: opens Edit (asserting it appears **only once the chat is idle** and hides itself while editing), reorders with the ↓ control, cycles a step's status through `.plan-status-btn`, removes with ×, appends with `+ Add step` and retitles the blank row, clicks **Save** and asserts the **exact** `set_plan` payload (order, trimmed text, per-step status) plus the rendered canonical result (`0/3 · 2 active`), then proves **Cancel** and **Escape** write nothing while **Enter** in a step saves, and finally that switching to another chat restores *that* chat's plan (`store.plan[sEmpty.id]`) rather than keeping the one on screen. Writing the coverage found a second shipped defect in the same surface: `renderPlanEditor` built rows as `"plan-item plan-edit-row"` with **no `status-*` class**, so inside the editor a completed step lost its `line-through`/faint styling and an active step lost its accent — every step read as untouched exactly when the user is deciding what to change. Fixed to `"plan-item plan-edit-row status-" + it.status`. Also pinned a cross-crate vocabulary that had two independent copies: `PLAN_ICONS` in `ui/app.js` and the planning tool's JSON `enum` in `crates/agent/src/tools/plan.rs`; `wiring_selftest.sh` now compares the two sets, so a status the UI cannot name (silently falling back to the pending glyph) or one the backend never emits fails the guard. Evidence: live **45/0** (9 new checks), wiring green with the new pin (negative-tested both ways: `done` planted in `PLAN_ICONS` fails with `never_emitted=['done']`; the panel's own defect was caught by the guard before it was announced), whole suite re-run green (markup 53, ui 125, shell 94, layout 0, page 59, zip 10, perf 8/0). Limits: the editor has no keyboard coverage for the Alt+↑/↓ reorder shortcut (only the buttons), no drag affordance, and no undo — a Cancel is the only way back; the plan's `pending`/`in_progress`/`completed` glyphs are text characters (`☐ ◐ ☑`), so they take their weight from the UI font rather than the icon set the rest of the chrome now uses. |
| 92 | 2026-10-11 | **The terminal's live path had never run; its backend double was a stub that could only fail.** The bridge answered `pty_start` with `null`, so `for (const chunk of status.tail)` threw on every attach and the only end-to-end outcome reachable was the `[shell unavailable]` branch — coverage existed in the *fixture* (`ui_selftest.html` renders the panel's pure functions) and nowhere in the shipped page. Replaced with a real PTY double that keeps a shell per session id (tail, shell name, liveness) and emits `pty-output` / `pty-exit` on timers the way the Rust side does. Five live checks now drive the terminal through the shipped page: the Shell button attaches with `sessionId="bos-pty-"+workspaceRoot` and `shell:null`, replays the tail, and shows the `shell` chip; a submitted line crosses as `data:"echo hi\n"` with the composer cleared and **no local echo** (the line appears exactly twice — once as the shell's echo, once in its answer) and the answer streams back through `pty-output`; a second attach adopts the *same* live shell (`fresh:false`), shows `attached`, and replays the accumulated tail instead of starting over; Kill ends it and the narration comes from the `pty-exit` event (`chip=killed`, `[session ended · killed · the Shell button starts a new one]`), not from the click; and an injected `pty_start` failure ends with `chip=no shell` plus `[shell unavailable: Error: no shell here]` rather than looking attached. Two harness details had to be right first: the run panel paints on its **own** rAF, which `--virtual-time-budget` starves exactly as it starves the transcript's, so the checks run the panel's frame work (`window.renderRunPanel()`) on every poll; and an injected rejection reaches the UI as `Error: <message>`, so the refusal is matched on `shell unavailable:.*no shell here` rather than a literal. Evidence: live **50/0** (5 new), whole sweep green. Limits: `pty_resize` is still answered with a constant, so the character-size math (`syncTermSize`) is exercised only for "does not throw"; `/term` and the run-mode chip's other labels are covered by the fixture's contract tests, not end to end; there is no check that the panel's scroll follows the tail. |
| 93 | 2026-10-11 | **The run/terminal stream got the budgets row 89 said it lacked.** Row 89 closed with the limit "it does not measure the terminal or run-panel streams" — that gap is now closed with five more budgets, and writing them was mostly about *not* letting the harness lie. The bridge gained `LIVE.ptyFlood(sessionId, n)`, which pushes `n` `pty-output` events on timers like the real backend, and the shipped page is asked to swallow **2 000** lines: **0** run-panel paints happen during the flood, exactly **1** frame is queued, and **1** paint runs on it (so `scheduleRunPaint`'s `runPaintQueued` guard coalesces a chatty process exactly as the transcript's does); the panel keeps **400** lines of the 2 003 offered (`MAX_RUN_LINES`) **in state and in the DOM** — the memory bound holds, which is the part that would otherwise grow without limit in a long-lived shell; **0** `bos.runs` keys are written during the flood, because `pty-output` deliberately does not call `persistRunLines` (a per-chunk `localStorage` write would be a jank source, not a correctness one); and the tail the user sees is the **newest** output (`flood 1600..1999` present, line 0 dropped) rather than the first lines that arrived. Both guarantees were negative-tested by breaking them in `ui/app.js` and watching the budget go red — with `scheduleRunPaint` painting synchronously the same check reports `2000 chunks → 2000 painted during the flood, 0 frame, 0 paint on it`, and with `pushRunLine`'s `trimRunLines` removed it reports `2003 lines kept of 2003, 2003 drawn` — then restored to green. Evidence: perf **13/0**, whole suite re-run green. Limits: this counts work, not time, and still sets no absolute wall-clock budget for the shipped page; a 2 000-chunk flood is a burst, not a soak, so memory over a session measured in hours remains unmeasured; `pty_resize` still answers a constant, so the character-size path (`syncTermSize`) is only exercised for "does not throw"; and the panel's scroll-follow is not asserted (the paint writes `scrollTop` once per frame, which the budgets here do not distinguish from any other write). |
| 94 | 2026-10-11 | **M6 and M7 were implemented, statically wired, and behaviourally dead in the harness.** Both milestone rows sat blank while the page guard proved their *wiring* with real fixtures; what nobody had done is run them against a backend that holds state. The live bridge answered `token_meter`, `compaction_status` and `compact_now` with `null` and `restore_compacted` with `0` — and that last stub took `sessionId` while the command it doubles takes `session_id`, so restoring folded turns could never work, only look like it. That is the same failure mode as the terminal's `pty_start: () => null` (row 92) and the plan panel's `$("#plan-list")` (row 90): **a surface no guard drives is a surface nothing is true about.** The bridge now keeps a live/folded split per session and a job registry with a 28.2 KiB output body and a 20 KB spill, and seven checks drive the shipped page through its own controls: the settings tab's click (which is what fires `refreshSection`) reads `token_meter`/`compaction_status`/`spill_list` and renders `messages=4, estimated=90 tok, provider=—, live=3, folded=1, budget=8192 tok`; **Compact now** folds and both the card and the note agree (`live=0, folded=4, canCompact=no`, `Folded 4 turn(s); 0 live, 0 tok estimated.`); **Restore folded turns** brings them back (`live=4, folded=0`, `Restored 4 folded turn(s).`); a spill row opens its file with `(truncated)` stated; the Jobs pane lists `running,failed` (`2 shown`); opening a job reads only a bounded **tail** — `job_output {id: job-1, max_bytes: 16384}` → `16.0 KiB of 28.2 KiB (tail)`, `12.2 KiB dropped`, newest line present and line 0 gone; and **Stop** crosses as `job_interrupt` with the list re-read showing `interrupted`. Two expectation corrections are recorded rather than papered over: the compaction note reports the **archive size** (`Folded 4 turn(s)` after folding 3 with 1 already folded) and reads consistently with the card above it; and the meter's `provider`/`delta`/`within tolerance` rows show `—` on this path because the pane itself asks with `provider_tokens: null` — the reconciliation is only exercised when a caller supplies usage. Evidence: live **57/0** (7 new), seen red first — against the unfixed doubles the restore check times out (`waitFor: the folded count to clear`) — and the app-side negative test (deleting the job detail's `(tail)` marker) stops the run at **30 checks, 1 failure**, restoring to 57/0; whole sweep green. Limits: the retention/spill *policy* is not exercised (no real tool output overflows into a spill here); the jobs are the shell/run family, not a delegated subagent; and a job's live streaming into an open pane is not covered. |
| 95 | 2026-10-11 | **The Insight pane had never been driven.** `self_inspect`, `evaluate_transcript`, `replay_session` and `export_bundle` were all `() => null` doubles, so the only reachable branch of every one of them was the `catch`. Five checks now drive the pane through the markup's own buttons: the composition inventory (twelve rows, all asserted), the goal the agent reports about itself set against the Goal card, the evaluator's score card, deterministic replay, and the export note. The one worth the most is the second: `self_inspect` and `get_goal` are different commands reaching the DOM by different routes, so `active · 3/12` matching `3 of 12 rounds` is not a tautology — and it was **falsified by making the double lie** (`max_rounds - rounds - 1` → `active · 3/11` against the card's `3 of 12 rounds`), which is the evidence that the check compares rather than echoes. The evaluator's check was falsified app-side too (dropping the `renderScore` call stops the run at 27 checks, 1 failure). Also found, and left for its own round: the M4 **scoped-rule commands are stateless doubles** (`approval_rules_list`/`approval_rule_add`/`approval_rule_revoke` all return `{rules: [], warnings: [], audit_cap: 200}` without touching state), so the Rules pane cannot hold a rule on the live path — the same shape as rows 92–94. A shared `rulesState` seam now feeds both those handlers and `self_inspect`'s `approval_rules`, so the count cannot disagree once they are made stateful. Evidence: live **62/0** (5 new); whole sweep green. Limits: the evaluator's check asserts coherence (grade band vs printed score, a non-empty reason) rather than a pinned score, because the live transcript's length varies with the round that produced it; replay is not compared turn-for-turn against the DOM transcript; and the export is a double, so no real ZIP is opened here. |
| 96 | 2026-10-11 | **The Rules pane could not hold a rule.** `approval_rules_list`, `approval_rule_add`, `approval_rule_revoke` and `approval_audit` were all stateless or `() => []` doubles — and `approval_audit` was defined **twice**, the later empty one shadowing the ring, so the audit list could only ever be empty on the live path. The bridge now carries a rule book with a bounded audit ring and, more to the point, **consults it before a user is asked**, exactly as the backend does: a matching rule settles the call, and every decision lands in the ring whether or not a rule matched. Six checks drive it: the empty book's own explanation, adding a rule through the form, the rule suppressing the prompt (`dialog=hidden`, `respond_approval` count unchanged, the tool still ran), the ring naming the rule (`allow` + `rule-1`), Revoke, and — the pair that makes it a *consequence* rather than a listing — revoking the rule bringing the prompt back with the ring's newest entry reading `ask` and no rule id. Two defects surfaced and one was fixed here: `addRule` posted its confirmation **before** `refreshRules`, whose redraw clears the note, so `Added: write_* (global).` never appeared (the ordering `compactNow` already documents avoiding); and the first version of these checks matched `Done — notes/a.txt is written.` once, which the *previous* Allow turn had already put in the transcript, so it passed before the rule's turn had streamed — the fix was to count occurrences, and that shape is worth remembering for any check that follows a turn with similar text. Evidence: live **68/0** (6 new); the note-ordering regression was re-injected and the check went red (`note=""`, 1/68); and making the bridge ignore its rule book stopped the run at 19 checks with `waitFor: the rule-allowed turn`. Limits: the glob/`deny`/argument-predicate matrix and the 200-entry cap are verified in Rust, not live. |
| 97 | 2026-10-11 | **M14 was the only milestone with an empty status column, and the honest reason was that none of it could be reached from here: `cargo tauri build` wants a network-plus-toolchain host and a signing identity.** What *can* be checked is that the artifact the config describes exists, so the packaging layer became a guard — 16 checks over `tauri.conf.json` and `icons/`, including the ones that would fail silently otherwise: `identifier` still reverse-DNS, the window title and `productName` one name, `frontendDist` a real directory with a non-empty `index.html`, **`withGlobalTauri` on because `ui/*.js` reads `window.__TAURI__`** (a one-sided change now fails), no `beforeBuildCommand`, and every icon present, non-empty and carrying its format's magic bytes, one per platform. The guard found a bug **in itself** on first falsification: it read the config's values space-split, so the moment a `beforeBuildCommand` existed (`pnpm build --release`) every later field shifted and it reported a bogus `version is not numeric-dotted (build -)`; it now reads one value per line. The other half of the milestone — the upgrade path — was a real gap: `migrate()` was shape-only (`if self.workspaces.is_empty()`), so a future migration that is *not* shape-detectable had nothing to sequence on and no file recorded what wrote it. `settings.toml` now carries `schema_version`, with the field-level serde default deliberately `0` (**not** the current version) so a pre-version file cannot look migrated; the shape steps run first and the stamp is written **last**, and `migrate()` reports whether it advanced. Two tests pin it, one of which exists purely to catch the container-level serde fallback that would have hidden the whole thing. Limits, stated plainly: nothing here builds or signs a bundle, the guards that constitute this evidence are **not wired into CI at all** (`.github/workflows/gui-e2e.yml` drives only the WDIO suite), and the version stamp lands on the next save rather than on load, by choice. |
| 98 | 2026-10-11 | **The last functional half of M11 was not missing wiring — it was missing truth.** `list_memories`/`remember_memory`/`forget_memory` existed, and the add/list path was already live-checked. What no one could check were two facts the UI hid. The first: an empty `memory_path` — the default — puts **every** workspace on one shared file, yet the pane's hint read "One store serves every workspace" (accurate) while its row header read "Memory for <workspace>" and a comment in the renderer claimed "the store follows the workspace" (both implying an isolation that is not there by default). The resolution now lives in one place, `settings::memory_file`, which `caps` also opens the store from, so the agent and the pane cannot disagree; the pane prints the **file** it is showing and who else lands on it (`<path> · shared by N workspaces · N of 500 kept`), and `Settings::effective_for(id)` was split out of `effective()` so that question is answerable per workspace rather than only for the shown one. The second: the item bound (500, past which the oldest memory is silently evicted) and the note bound (2000 chars) were invisible; the line carries the cap and names eviction when it is reached. Coverage grew where it was actually thin: the page guard **opened this dialog and never looked at the pane** (no `list_memories` or `memory_status` fixture, so the invoke rejected inside the pane's own catch and the guard stayed green), so it gained both fixtures and an assertion; the ui guard gained four checks on the pure status line, which is the lowest layer that can see the copy; and **Forget** — real, wired, and untested — is now driven live, asserting that only the forgotten memory leaves and the count follows. Falsified on all three axes: hiding the sharing reddens ui, page **and** live; a harness that claims a private store reddens live; `memory_file` losing its empty→default rule reddens the Rust test. Limits: no knowledge-base surface, and recall is surfaced only where the evaluator scores it. |
| 99 | 2026-10-11 | **The perf guard's own counter was silently dropping most of what it was written to measure — and a soak is what exposed it.** Row 89 recorded the fix for a synchronous mutation read ("drained with `takeRecords()`, after the frame"), but that fix is only half a counter: `takeRecords()` returns what is *still queued*, and the observer's empty callback discards everything delivered to it at each microtask checkpoint. Anything spanning an `await` — a turn, a flood, a repaint after a tick — was therefore invisible, and several budgets were passing on a number that was always zero. A `MutationObserver` now accumulates in **both** places (the callback counts what is delivered; `takeRecords()` counts what is still queued), which is the only shape that is exact. With real numbers, the guard gained the soak row 89 named as a limit: **ten turns into a 5 000-message session**, whose measured cost is **406, 406, 407, … 607 DOM mutations per turn** — so a streamed turn into a saturated window rebuilds the whole 200-row window as it slides, and the cost does **not** grow with session length (5 000 → 5 020 changes nothing per turn). The soak also pins that the drawn window stays at its cap (200 messages + the scroll spacer) across the turns, that the newest turn is drawn while the row that was on top has slid out, that the window's DOM is bounded by the **cap** rather than by the rows drawn (a ratio passed even when the window grew to hold all 5 000 messages and 43 332 nodes — a bound that cannot fail is not a bound), and that a send costs one round trip per turn, not per chunk. The terminal's flood number also moved from an undercounted 2 to a real 3. Falsified: a window set to 5 000 reddens the soak (5 000 messages drawn), and restoring the callback-only counter reddens the per-turn cost as a flat row of zeros. Limits: still no absolute wall-clock budget, and the ~400-mutations-per-turn slide is now **measured** but not yet optimised — a row-recycling slide would be the next step, with this check as the before/after. |
| 100 | 2026-10-11 | **The soak's number was real, and chasing it produced a ~50× win plus one bug the new check caught in the same round.** Row 99 measured a turn into a saturated window at ~406 DOM mutations. Two causes, in that order. First, the row actions (Copy/Regenerate/Edit/Delete/Branch) were gated on the chat being idle, and that flag is part of the renderer's per-row signature — so the idle flip at the end of every turn rewrote the whole drawn window (the 607 spikes at the end of the soak). They are now built from the message alone, with the gate moved to the click and to a single `#messages.busy` class write, so ending a turn no longer changes any row's shape. Second, and the actual 406: a finished turn re-reads the session, which hands back **fresh objects** for messages already on screen — `msgKey` is per-object, so every row missed its cache entry and was rebuilt. `reuseIdentical` now pairs the drawn tail with the objects already in hand, and a turn costs **6–10 mutations instead of 405–409** — the perf guard's per-turn claim became an absolute budget (24), not a ratio. **The bug this introduced is the interesting part.** The first version paired by `messageSignature` and the live check added in the same round went red: a backend that finalizes a reply hands back text of the *same length* whose *last characters* match the streamed draft, and the renderer's signature is deliberately lossy (`length` + the last 24 characters — a full hash per frame would make streaming O(n²)). So the draft object was kept, the row's signature matched, and the store's finalized text never appeared. Pairing is now exact (`sameMessage`, including tool `open`/`ms`, which are rendered), and only over `WINDOW + windowExtra + 8` messages, so it stays proportional to the window rather than the session (0.3 ms for 5 000 messages, against 2.5 ms for a whole-session scan). That also closed a guard gap the injection exposed: **no live check asserted that a reload's content wins over what was streamed** — the double's compaction never rewrote the transcript, and its `finish` wrote exactly the streamed text. The double now finalizes (streams one reply, stores another) and live asserts the store's version is what is drawn. Coverage: **ui 134/0** (five checks: tail identity kept, the window — not the session — is scanned, a rewrite that **collides with the lossy signature** is not kept, and a 5 000-message adoption inside 25 ms), **live 72/0**, **perf 19/0**. Falsified three ways, each reddening the lowest layer that can see it: reverting the merge reddens perf (405–409 of 24) **and** ui; pairing by position reddens ui **and** live; pairing by the lossy signature reddens live and — once the probe was made a true collision (same length, same last 24 characters) — ui too. Limits: ~7 mutations per turn is the new baseline, not zero; the transcript's *first* paint at 10 000 messages is still timed only in the fixture guard. |
| 101 | 2026-10-11 | **The scheduler DSH has and BOS did not: durable reminders with a UI, and the clock handed in rather than read.** The requirement list calls it a must-match ("durable schedule/reminders") and the gap table named the Web session controller as the missing piece; nothing in `crates/agent` or `crates/gui` implemented any of it. It is now a store (`crates/gui/src/schedule.rs`) plus a Settings pane, and the design choices are the interesting part. **Every command takes `now_ms` from the caller**, so the pure logic reads no clock: `add`/`cancel`/`tick`/`report` are testable without waiting, the double can make a reminder due without sleeping, and a tick is reproducible. **The tick is the UI's**, on a 30 s interval and once at boot, because that is exactly the "Web session controller" the requirement asked for: the store is durable, the delivery is what needs a running app. A tick returns **what it delivered** (`report.fired`) rather than letting a caller diff two histories — a reminder that came due while the app was closed is announced once, and the 200 deliveries already on disk are history, not a backlog of notifications. **A missed period is not a missed reminder**: a repeat that should have fired five times while the app was off delivers once, and `next_ms` advances arithmetically past `now` (`now - next)/step + 1`), so a month of downtime costs one step and the next due is always in the future. A delivery keeps both `at_ms` (when it was due) and `noticed_ms` (when a tick saw it), which is what lets the pane say `· 5 min late` instead of pretending it was on time; a one-shot leaves the list when it fires, because its record is the delivery. Bounds are stated where they apply, the same lesson as the memory pane: `2 of 50 reminders · 2 of 200 deliveries kept · a repeat needs at least 1 min`, the cap naming itself (`— at the cap, cancel one first`). DSH's "cron ≥ 1 min" becomes a rule with a number in it, enforced in the store and refused in the pane before the round trip. A corner closed twice over: a hand-edited file with a sub-minute repeat is **dropped on load** (it would otherwise fire on every tick), and a corrupt or missing file is an empty store rather than a failure to start. The list is shown ordered by creation, deliveries newest first, and the pane is repainted **only while it is the open tab** (plus on boot and on a tab switch), so a hidden pane costs nothing per interval. Evidence: **14 Rust tests** (fired-once, missed periods, the minute rule, text trimming and the 500-char bound, the 50-reminder cap, the 200-delivery ring, a restart round trip, corrupt and hand-edited files, `report.fired`, unknown-id cancel, clearing history, id collisions inside one millisecond), **ui 140/0** (six checks on the formatters — cadence, due-by-kind, `untilText` past due, the reminder line, the late label, the bounds line), **page 63/0** (the pane lists a reminder with its next due and bounds; a tick announces a delivery **without repainting a closed pane**; opening the tab catches up), **live 76/0** (four: added through the pane's form and listed with its cadence, a past-due reminder delivered and reading `5 min late`, cancelled while its delivery stays, history cleared), plus the wiring guard's new commands and `cargo test --workspace` at **764 passed / 0 failed**. Falsified 14 ways, each reddening the layer that owns the rule: eight Rust injections (no minute rule, `missed = 1`, one-shots kept, `fired` always empty, `noticed = due`, no load sanitizing, unbounded history, no cap) each failed exactly its own test; and four UI + one page + one live injection (due read by the wrong field, past due as "in 0 min", the run count dropped, lateness never said, a tick repainting a closed pane, a double that delivers nothing) each failed exactly its own check. Limits: the pane offers an interval, not a calendar picker, so a one-shot is "in N minutes" (the store takes an absolute time, and the Rust tests drive that path); reminders are delivered while the app runs only — there is no OS-level notification, so a reminder due while BOS is closed arrives at the next launch; and the 30 s interval is not adaptive. |
| 102 | 2026-10-11 | **The knowledge base: the half of M11 that was not a store with a better label, and the split that makes it different from memory.** After rev 116 the memory pane was honest, but "memory" and "knowledge" were the same thing in this app — recall injected into the system prompt, keyword-ranked, bounded, evicted oldest-first. That is the wrong shape for a project's conventions and decisions: they are long, they are precise, and injecting all of them into every prompt is both expensive and noisy. So the second surface is defined by its opposite: `crates/gui/src/knowledge.rs` is a durable store of **named** notes (title, tags, body) that is **never** injected, and `knowledge_search` is a tool registered on every session's agent, so the model pulls a note when it needs one. The corpus can be large because nothing is paid for until it is asked for. The store is `~/.bos/gui/knowledge.json` written atomically (temp + rename), bounded at 500 entries, a title required and trimmed to 120 characters, a body to 4000, at most 8 tags normalized to lowercase and deduplicated, and caps that travel to the UI in every report — the same lesson as the memory and schedule panes: a cap nobody can see surprises someone. Ranking is stated so it can be argued with: a query token scores 4 in the title, 3 in the tags, 1 in the body, a **substring** match counts (recall over precision, since a curated corpus is small and intentional), ties go to the most recently edited and then to the id, and an empty query is a list of everything newest-first. Ranking, storage and the tool all read one `matches()`; the pane and the model cannot drift. **The tool is the interesting half.** It is a `Tool` (sync, no awaits to hold a lock across), its schema requires `query`, `limit` is clamped to 1..20 and defaults to 5, `"*"` lists the newest entries, each body comes back clamped to 1200 characters with a per-match `truncated` flag and a store-level one, and the note states the corpus: `no knowledge entry matches "x"; the store holds 12` rather than an empty array that looks like a bug. `attach_knowledge` is called by `Inner::agent` for **every** fresh build, so the tool is on a session's agent and not just on one built by hand — and that path is tested, not asserted: the test builds the real agent, reads the composition the Insight pane reads (one sync tool more), lists the registry, and calls the tool **through the registry's own `execute`**, which validates the arguments against the schema first. The pane is the curator — search, add, edit in place, delete — and a filtered view still says how many notes exist overall (`2 of 500 entries` while one row is drawn), because a filter that hides the size of what it filtered is the same defect as a hidden cap. Evidence: **21 Rust tests** (20 in `knowledge.rs`: the title requirement and the blank-title refusal, both character bounds at and past the limit, tag normalization/deduplication and both tag caps, the entry cap, ranking by field, case-insensitive substring matching, recency tie-break, newest-first listing and an honoured limit, a restart round trip that keeps `created_ms`, a missing or corrupt file as an empty store, a hand-edited entry dropped on load, unknown-id update and delete, the report's totals and bounds, unique ids inside one millisecond, and four on the tool — its name/schema/description, a match with its body clamped and flagged, a short body left alone with the default limit, an absurd limit clamped, and an empty query refused with the store's size in the note; plus the agent test), **ui 143/0** (three on the pure formatters: an edit age from the caller's clock with a future timestamp reading "just now", a one-line body preview that says when it was cut, and the bounds line including the search cap it lowers to the tool's own maximum), **page 70/0** (seven: the pane's first paint with tags, age, body preview and bounds; searching asking for matches and drawing only them; clearing back to the list; adding sending exactly what the form holds; editing filling the form and turning Add into Save; saving an edit sending the id it is editing and clearing the form; deleting sending the row's own id), **live 82/0** (six: a note added in the pane and listed with its tags and age, a blank title refused **here** rather than by the backend, a search drawing only matches while the line still counts the whole store, editing filling the form from the row, the edit rewritten in place with the form back to Add, and delete removing that note and only it), the wiring guard green for all five new commands, and `cargo test --workspace` at **785 passed / 0 failed**. Falsified **17 ways**, each reddening the layer that owns the rule: nine Rust injections (no title requirement, a title hit no longer outranking, case-sensitive matching, ties ignoring the edit time, the cap unenforced, load without sanitation, the tool sending a whole body and reporting it untruncated, the tool accepting an empty query, and `Inner::agent` not attaching the tool — the last failing the session-agent test, which is exactly its job), three ui (no cap clause, no future guard, no truncation), two page (editing adding instead of updating; an edit that never remembers the id), and three live (a delete that does not redraw, a double that ignores the query, and a blank title allowed to reach the backend). Two defects were found in my own work and are worth the ink: a store test that asserted a **count** did not delete its temp file first, so it passed alone and failed after any earlier run — the falsification batch made it fail in the *restored* state, which is how it was caught; and the markup guard's tag regex (`[a-zA-Z]+`) read `<h3 data-i18n=...>` as tag `h`, which both reported a false nested mark and would have missed a genuine one in any heading, so it now accepts digits in tag names. Limits: an entry has no provenance and no workspace scope — the store is one per user, like memory's default file — there is no edit history or versioning, the pane has no tag filter or autocomplete, and the tool only reads: the agent cannot write knowledge, which is deliberate (curation is the user's) but means a good answer cannot yet be saved as a note. |

| 103 | 2026-10-11 | **MCP had a gate for the workspace and nothing for a tool: the policy seam `agent::mcp::policy` had no callers at all.** The library carried `McpPolicy`, `McpRule` and `McpEffect` (deny beats allow, an explicit default) from rev 87, and the GUI attached **every** tool a connected server advertised under a single `settings.require_approval` — so enabling a server granted all of it, and the only alternative to "everything, gated" was "everything, ungated". The DLL half of M8 was real and unreachable. `crates/gui/src/mcp_policy.rs` now makes it reachable: a durable store (`~/.bos/gui/mcp-policy.json`, versioned, 200 rules, atomic temp+rename) whose `decide(server, tool) -> Decision { effect, by }` is **the one place** the answer is computed, so the pane that prints a rule and the code that applies it cannot drift — the defect this project keeps finding is a surface that reports a rule nobody enforces. Precedence is stated once and tested: **deny > ask > allow**, either half of a rule may be `*`, a pair covered by several rules takes the strictest (then the first in document order, so the printed reason is stable), and an uncovered pair takes the policy default. The `/` in a rule's key is what the pane shows as the reason: `by rule filesystem/write`, or `follows the default`. **Enforcement is at the attach site**, not in a wrapper someone can forget: `attach_tools_with_policy` is handed the advertised names and a factory, skips `Deny` without ever building the tool (so it is absent from `registry().async_tool_names()` — the model cannot see or call it), and passes `gate_for(workspace_gate, decision)` to the factory so an `ask` rule adds a gate in a workspace that approves nothing while a per-tool `allow` **cannot** remove the workspace-wide one: a policy surface must not be able to weaken a setting made somewhere else. The workspace gate is a floor, and that is two separate assertions rather than one. Three design consequences worth the ink. First, **the double-check that a rule takes effect is a live one**: the pane's row, the server badge (`2 of 3 tools · 1 denied`) and the Insight pane's `MCP tools` count all move together when one tool is denied, because the double's `mcp_status` and `self_inspect` both consult its policy the way the backend does — three surfaces, one decision, and any of them derived independently would have been the defect. Second, **an unreadable policy attaches nothing** (a poisoned lock fails closed and says so; a corrupt file is the default policy rather than a reason not to start, and a hand-edited rule that no longer validates is dropped). Third, and this is the finding: **re-attaching by cloning a running agent can only ever add.** The first version of `reattach_mcp_agents` cloned each cached session agent and attached the policy's survivors, which left a tool the policy had just denied sitting in the registry — it was written, documented as "a policy change reaches the agents that already exist", and would have shipped silently, because nothing drove it. A test written for exactly that promise caught it (`a denied tool must leave the running session: ["srv_read", "srv_write", …]`), and the fix was structural rather than local: `Inner::build_agent_for` is now **the** construction path (base agent → MCP tools under the policy → knowledge), used by a first build, by a policy change and by the connect pass, so a re-attach cannot disagree with a first build. The connect pass had the same clone-and-add hole and now shares the helper, which also means a server that *drops* a tool between passes no longer keeps its old wrapper. Evidence: **24 Rust tests** (21 in `mcp_policy.rs`: the default, a rule outranking it, `*` on either half, deny>ask>allow with the reason naming the winning rule, a repeated pair replacing its rule, both required halves and the length bound, the 200-rule cap refused with the number named, clearing a pair, a restart round trip, missing/corrupt/nonsense documents, four invalid rules dropped on load, duplicates collapsed to the strictest, the report's counts and bounds, the workspace gate as a floor, a denied tool never built and never registered, an `ask` gating exactly one tool, a catch-all deny leaving the agent without the server, the wire shape, the version stamp, and a decision not changing when the policy does; plus three in `app.rs` — a denied tool absent from a session agent's registry while the report agrees, and a policy change leaving the running session's agent too), **ui 146/0** (three on the pure formatters: the effect spelling, the reason line, the summary's counts including a file whose version this build does not write, and a server line that counts tools rather than survivors), **page 76/0** (six: the pane's first paint with the deciding rule on every row, a click sending that pair and effect, a half-written rule refused **here**, a complete rule sent and the form cleared, Clear sending the stored pair, and the default select following into the summary), **live 86/0** (four: the pane's listing, a denial reaching row + badge + inventory, an `ask` keeping the tool and recording who asked, and clearing a rule then a deny default leaving exactly the unasked tools), the wiring guard green for all four new commands, and `cargo test -p bsh` at **264 passed / 0 failed**. Falsified **18 ways**, each reddening the layer that owns the rule, and the count is the batch's own output rather than a recollection: eleven Rust (the ask rule dropped from `gate_for`; the workspace floor dropped; `decide` taking the first rule instead of the strictest; no wildcard matching; a denied tool still attached; duplicates keeping the last; the cap off by one; the reason no longer naming the rule; the report forgetting a denied tool; `attach_mcp_tools` handed a permissive store; the re-attach skipping the swap), two ui (`policyReason` losing the rule name, the summary losing the denied count), three page (the rules list not rendering, a click sending the wrong effect, and the add button losing its id, which takes the whole guard down), and two live (the server badge forgetting `tool_names`, the inventory pinned at 3). One detector bug is worth recording because it is the shape of a false negative: the first batch reported the two ui injections as **not** reddening, because the ui guard prints two totals and the check read the *shell* section's `failures: 0 / 94` — the injections had failed all along (`[ui] failures: 1 / 146`), and re-running them with the right line confirmed it. Limits: the policy is per user, not per workspace (a workspace may switch a server off, but two workspaces cannot disagree about a tool); the pane edits a rule only as `server/tool`, so there is no argument-level predicate and no per-session activation; `agent::mcp::policy`'s own config form is still not surfaced; and the deny/ask matrix is verified in Rust and in a double, never against a real MCP server, because none is available offline. |

| 104 | 2026-10-11 | **The agent could not be handed a new skill from the GUI, and the seam that validates one had no caller.** `agent::skills::SkillAuthoringRecord` is a public authoring record with `validate()` — and since rev 87 nothing in the GUI had touched it. The Skills half of M8 was "run-only": the pane listed the names the loader had discovered, the palette could mention one, and the only way to add a skill was to hand-write a directory. `crates/gui/src/skills.rs` now closes the round trip, and the round trip is the point: a skill is only real if the loader that feeds the agent's prompt can read it back. So the name and description rules are the agent's own (`validate()`'s messages are what the pane shows: `name: must not contain whitespace`, `description: must be at most 1024 characters`), the file is written as YAML frontmatter plus the instruction body, and every listing is produced by calling `SkillLoader::discover`/`load` and counting `SkillContent::instructions` — the string the model receives, not the file size — so a skill the loader cannot parse is absent from the pane rather than listed next to a success message. Two rules are the GUI's, because the loader has neither: a body must be non-empty and at most 12 000 characters (a skill is an instruction, not a corpus; past that the right shape is a knowledge entry the agent queries on demand), and **a name must be one path segment**. That second one is a real defect that the agent's validator cannot see and this round had to invent: `validate()` rejects whitespace and length and nothing else, so `../evil` passes it — writing that name verbatim into a directory would put a `SKILL.md` outside the skills directory, in `~/.bos/` or anywhere else a `..` reaches. The fix is in the name-to-directory mapping (`slug`): no separator, no NUL, and neither `.` nor `..`, with the name otherwise used **verbatim** rather than slugified, because a lossy rewrite could make two distinct skills share one directory. The refusal is proven by a test that also asserts nothing was created (`!dir.exists()`), and the same test covers `a/b`, `a\\b` and the empty name. Three more decisions worth the ink. First, **the scalars are JSON-quoted**, which makes them valid YAML double-quoted scalars: a description containing `:`, `#`, quotes or a leading `-` then survives the round trip instead of splitting the document — the test authors `Step 1: do "it" — #now (see: notes)` and reads it back byte-for-byte. Second, **saving rebuilds the agents that already exist** (the helper from rev 121, renamed `reattach_cached_agents` because it now serves two callers for the same reason): a session's agent read the directory when it was built, so without a rebuild a skill written now would be invisible to the session it was written for, and the test asserts the *same* cached session agent gains the skill with the authored body — including that a refused second save changes neither the file nor that agent. Third, **the refusal is by name and the replacement is explicit**: a second save of an existing name fails with the path (`…/pr-review/SKILL.md already exists — replace it to overwrite`) and the pane has a Replace checkbox, so clobbering a skill someone edited by hand is never a side effect of pressing Save twice. On the UI side the new block sits in the Tools & skills pane beside the capability list, with one gesture that matters: **Use** prefills the composer with `Use the <name> skill: `, the same prefill the palette's skill entry produces, so the row that names a skill is one click from asking for it. Saving clears the palette's own cache (`state.skills = []`) — without that the new skill is in the directory and in the pane and *not* in the `/` list, which is the difference between a skill that exists and a skill the model can be asked to use — and re-reads the capability list, so the two readings of one directory are compared on every save. The markup guard earned its keep here: it rejected the new styles for using three custom properties that no stylesheet in the tree defines (`--fg-dim`, `--line`, `--mono`), which is the class of defect that otherwise shows as an invisible border in one theme only. Evidence: **9 Rust tests** — 7 in `skills.rs` (the write→discover→load round trip with the body compared verbatim; a description full of YAML metacharacters; six names that would leave the directory, each refused with nothing created; the agent's own rules for the blank, spaced, over-long name/description/body; a second save refused by path and then replaced on request; a listing that counts instructions and marks the workspace's deny-list; a missing directory discovering nothing without error) and 2 in `app.rs` (a skill reaching the session that was already open, and an unconfigured directory refused instead of writing into the working directory) — plus **ui 148/0** (two on the formatters: a row naming the instruction size, the file and the workspace state; a summary counting skills, saying how many are off, and naming the directory or saying it is missing), **page 83/0** (seven: the pane listing what the loader would hand the agent, Use prefilling the composer, the palette caching the list *before* the save, a blank body refused *here* with no round trip, a save sending name/description/body/overwrite and clearing the form, both readings of the directory re-read after a save — the capability list and the palette's cache, measured as call deltas — and Replace reaching the command as `overwrite: true`), **live 91/0** (five: the pane's listing against the double's directory, Use as the palette's own gesture, a saved skill appearing in the pane *and* the capability list *and* on the double's disk, a second save refused by path, and Replace overwriting in place instead of adding a second skill), the wiring guard green for both new commands, and `cargo test -p bsh` at **273 passed / 0 failed**. Falsified **26 ways**, each reddening the layer that owns the rule: 12 Rust (the body check dropped, the body bound ignored, a separator allowed in a name, `..` allowed, unquoted scalars, a silent replacement, a replace request refused, no frontmatter, the listing counting bytes, the deny-list ignored, the save never reaching a running session, an empty directory written into anyway), 4 ui (the row losing its instruction count, the summary losing the off count, the summary losing the skill count, the missing directory no longer named), 7 page (Replace never sent, the palette cache kept, the list not redrawn, the form not cleared, the capability list not refreshed, the row loop dropped, and the palette's cached list never filled — see below) and 3 live (the double never replacing, the double not counting instructions, the capability list dropping skills). Three of the page injections *stayed green* on the first run and had to be rewritten before they counted, which is the most useful thing this batch found: a **settled fixture answers the same thing before and after**, so a check that a save invalidated a cache cannot compare the render — every read returns the fixture's own list either way. The check now fills the palette's cache *before* the save and then asserts the round trips increased (`2→3 then 3→3` when the invalidation is removed), and the capability-list refresh is asserted the same way. The third green was the injection's own precondition: with the palette never opened before the save there was no cache to keep, so the check could not have failed for the right reason. The batch also left the world changed, and the *next* full run found it: removing the containment rule made `save` write a real `SKILL.md` at `../evil`, and removing the empty-directory guard made it write one into `crates/gui/nowhere`, so the two tests that assert those paths stay empty failed in the workspace run and passed when the crate was run alone. The residue was deleted and both assertions now name a process-id-carrying path they remove first — the same lesson as the temp-file trap below, applied to a path instead of a count, and a reminder that a falsification batch is a mutation of the machine and not only of the source. Limits: the record's `schema` half is passed an empty object because `SKILL.md` has no slot for a tool schema, so an authored skill is instructions the model follows rather than a callable tool; there is no rename or delete in the pane (the file is the user's, and the pane names it so they can edit it directly); scope is the directory plus the workspace deny-list, so there is no per-session activation; and the live double holds one directory, so the workspace-scoped path is exercised in Rust only. |
| 105 | 2026-10-11 | **An MCP server could publish resources and prompts, and this app could not see either — the same shape as the two rounds before it.** `McpClient` has spoken `resources/list`, `resources/read` and `prompts/list` for as long as it has spoken `tools/list`; nothing in `crates/gui` had ever called them. So a server whose whole point is a resource — a schema, a runbook, a database catalog — could be configured, handshaked, listed in the pane with its tool count, and still be unreadable from the app. Rev 121 was `agent::mcp::policy` with no caller, rev 122 was `SkillAuthoringRecord::validate` with no caller, this is the resource half of the client: **an agent-side seam with no GUI caller is a feature the user cannot reach, and the third instance in a row says the review should look for the shape rather than wait to trip over it.** `crates/gui/src/mcp_view.rs` is the pane's side, and three rules live in it rather than in the command that calls it. First, **a capability the server never declared is not asked about**: `resources`/`prompts` deserialize to `false` when omitted, `supported()` is the one place that reads that, and a server that says `false` is drawn as "no resources capability" instead of being sent a `resources/list` that can only end in an error path — while an unsupported capability reports a total of **zero, not the count of rows the caller happened to hand in**. That second half is not decoration: the first version gated the rows but computed `resources_total` from the list it was given, and my own test caught it — the pane would have said "3 resources" under a heading that said the server does not do resources. Second, **one server's failure cannot hide another's rows**: `server_view` takes `Result<Vec<McpResource>, String>` and keeps the `Err` as *that server's own line*, because a catalog is a summary of several servers and `resources/list` is not implemented by every server — the normal case must not read like a crash. Third, **everything a server says is bounded**, and the bound is in characters, not bytes (`clip` counts `chars()`: 2000 emoji are 2000 characters and 8000 bytes, and a byte-bound cut both under-reports the source and can slice a character in half). The bounds travel to the pane in every catalog (`max_rows: 200`, `max_text_chars: 20 000`, `max_desc_chars: 240`) for the same reason the memory, schedule, knowledge and policy panes state theirs: a cap nobody can see surprises someone. **Reading is a gesture, never a side effect of being connected** — a resource's text goes to the pane and nowhere else, so a server cannot put words into the model's prompt by being enabled; the source of the text is a row's Read button, the picker plus a typed URI (a templated resource a server did not list), or nothing at all. `mcp_catalog` and `mcp_read_resource` are the commands, and the pieces that decide anything are testable without I/O: `catalog_plan` (configured servers in settings order, each with the live client when this pass has one — so `connected: false` is a line rather than an omission), `read_target` (the client *named*, or a refusal naming it), `read_uri` (trimmed, non-empty, ≤ 2000 characters, checked **before** any server sees it), and `fetch_catalog` bound by `LIST_TIMEOUT` (10 s) per call because a hung server must cost its own line and a bounded wait, not the pane. An honest limit inside that: `list_prompts()` swallows its own errors in the agent crate, so a failing `prompts/list` is indistinguishable from an empty one and the timeout is the only failure this side can observe — which is why the prompt half is drawn **for reference only**. `agent::mcp` has no `prompts/get`, so a prompt can be listed and cannot be fetched; that is recorded as an open item rather than papered over with a disabled button. On the pane the catalog sits at the bottom of the MCP tab: one block per server headed `<name> · 2 resources · 1 prompt` (or `no resources capability`, or `not connected this pass`), a Read button per resource, a prompt row that counts its arguments and how many are required, a picker holding **only connected** servers, a `<pre>` for the body, and a line under it reading `filesystem/file:///repo/schema.json · 17 chars · application/json` or `… · 20000 of 41000 chars · 2 parts · truncated`. Every refusal and every failure **clears** that `<pre>` first: text left from an earlier read would otherwise sit under an error as if it were this read's result, which is the kind of lie a pane tells when it only appends. Evidence: **11 Rust tests** — 9 in `mcp_view.rs` (the negative capability spellings and the positive ones; the character-not-byte cut with a 10×-é source; an unsupported capability showing no rows *and* no total; one server's error leaving another's rows; the row cap holding at 200 while the total keeps the real count, and a long name/description clipped with `clipped` set; a prompt row counting arguments and the required subset, with `arguments: None` reading as none; a read joining two parts with a blank line, and a 20 500-character body cut to exactly 20 000 with the source reported; a resource whose only content has no text reading `empty` rather than as an empty string; the catalog carrying its own bounds) and 2 in `app.rs` (a catalog pass listing every configured server in settings order while holding the live client for the one that has one; a read refused by name, with the named client chosen over the first on the list and an oversized URI refused before any server is consulted) — plus **ui 152/0** (four on the pure formatters: a server line that distinguishes "1 resource" from "no prompts capability" from "not connected this pass"; a resource row naming its type and saying when the name was cut; a prompt row counting arguments and the required ones; a read line reporting the size sent, the source size when it was cut, the parts and the no-text case), **page 90/0** (seven: the pane asking for the catalog when the tab opens, four servers drawn with what each publishes including one that is not connected and one whose listing failed on its own line, only connected servers in the picker, a row's Read sending **its own server** and URI and drawing the text and the size line, a blank URI refused here with no round trip, a typed URI trimmed and sent with the picker's server, and a fourth server's row still reading from that server while the picker names another), **live 96/0** (five: the pane's listing against the double's catalog, a row read into the `<pre>` with the line naming the size and type, a 30 000-character body arriving cut to 20 000 with both numbers, a URI the server does not hold reporting `resources/read: no resource at …` **and leaving no stale text**, and a resource with no text saying `no text (application/octet-stream)` instead of drawing an empty box), the wiring guard green for both commands, and `cargo test -p bsh` at **284 passed / 0 failed** with `cargo test --workspace --exclude nbos --exclude jsbos` at **829 passed / 0 failed**. Falsified **21 ways**, each reddening the layer that owns the rule: **10 Rust** (a byte-based `clip`; rows drawn under an unsupported capability; the total leaking with it; the row cap off by one; parts glued with nothing; a no-text body reading as non-empty; every prompt argument counting as required; a catalog pass dropping a server it cannot ask; a read taking the first client instead of the named one; an oversized URI not refused), **3 ui** (a server line hiding a missing capability; a read line hiding that it was cut; a prompt row hiding its required arguments), **4 page** (the tab never asking for the catalog; resource rows never drawn; a row reading from the picker instead of its server; a listing failure swallowed) and **4 live** (the double's cap four times what the pane was told; a no-text body reading as non-empty; an unknown URI answering instead of failing; a failed read leaving the previous text on screen). Three findings from the batch are worth more than the count. **The first is a check that could not fail:** injecting `read_target` to ignore the name it was handed left the Rust suite **green**, because the only test for a refusal built an `Inner` with no clients at all — where "the one that was named" and "the first one there" are the same answer. The test now seeds **two** live clients (and asserts the second is returned for the second name), and the injection goes red; the same blind spot appeared on the page side, where a row that read from the picker instead of from its server was invisible with a single connected server, so the fixture grew a fourth server and the check now clicks *its* row while the picker names another. **The second is that a falsification batch mutates the machine, and a killed batch keeps mutating it:** the first run of this batch was started with `nohup … &` inside a normal tool call, the harness reaped that process group when the call returned, and the last two injections it had applied — `take(MAX_ROWS + 1)` in `mcp_view.rs` and `readResource(row.uri)` in `app.js` — stayed in the tree; the *next* batch then ran against a baseline that already had three defects (the third, `.find(|_| true)`, was applied moments before the kill), which made its Rust results a mixture of the injection and the residue, and its own "restored clean: yes" line was **vacuous** because it grepped `jj status` for a phrase that never appears there. The driver now snapshots the bytes of every file it touches and compares them after the run, prints the names of any that differ, and *that* is what caught it. Long-running work belongs in a managed background job, and a residue check has to compare state rather than search for a reassuring string. **The third is a fixture that spoke the wrong dialect:** the live double's first prompt was seeded as `{arguments: 2, required: 1}` — the *view's* shape — while the protocol (and `viewPrompt`) takes a list of `{name, required}` arguments, so the pane honestly rendered "no arguments" and the guard, not the code, was wrong. The double now seeds the protocol's shape. Limits: prompts can be listed and not fetched (`prompts/get` does not exist in `agent::mcp`, so a listed prompt cannot be turned into a message from here); there is no `resources/subscribe` and no `resources/templates/list`, so a templated resource must be typed by hand; the model still cannot ask for a resource — the agent's MCP adapter exposes tools only, so a read is the user's gesture and never the agent's; the capability gate is enforced twice (in `server_view` and in `fetch_catalog`) and only the view's copy is covered by a test, the command's being belt-and-braces that no unit test can see; and the live double holds one connected server, so the multi-server listing states (an unsupported capability, a failing listing, a server that is not connected) are proven in the page guard and in Rust rather than live. |

| 106 | 2026-10-11 | **Adding a workspace was a Settings pane, and that is the wrong room for it: it is the one gesture that points the app at something it has never been trusted with.** The folder-plus in the Workspaces section head opened the Settings table, and the only way to configure a new workspace was to create it and then edit it field by field — an installation that is briefly configured wrong, which is exactly the state a first chat must not be handed. Creation is now a flow of its own, and four decisions define it. **The folder chooser is ours, not the platform's.** `crates/gui/src/folders.rs` + `list_folder` walk one level at a time and answer with sub-folders only — never a file's contents, never a file at all — resolving every path so `~`, `..` and symlinks cannot make one folder look like two. Two rules are deliberately the opposite of `crate::filetree`'s: this reads **outside** the current workspace (the sidebar refuses to leave a root; choosing a root happens before there is one to leave) and it does **not** apply the sidebar's skip list, because someone pointing the app at a folder inside `target/` is making a choice rather than making noise. A native file dialog was rejected for the reason the whole harness exists: the driver cannot drive one and a webview may not implement one, so an in-app browser is the only version of this feature that is testable and the only one that behaves the same on every platform. **One payload, refused whole.** `create_workspace` gained `config: Option<WorkspaceDraft>` (model, system prompt, the three gates, the three deny-lists, and `mcp_enabled: Option<Vec<String>>`), and `apply_draft` runs *before* the workspace is pushed, so `no MCP server named "ghost"` or a root that is not a folder leaves nothing behind: the caller gets a workspace that matches the form or an error. `Some` means exactly those servers are on — a server ticked off arrives off even though the global it was seeded from was on — and `None` keeps each seeded switch, which is what every older caller's payload means, so the command's existing callers are unchanged in behaviour. **Plugins are governed by rebuilding.** A workspace carries `disabled_plugins`, and `PluginRegistry` has `register` and `clear` and no per-name switch, so `caps::apply_plugin_deny` re-registers the survivors — the same shape as `reattach_cached_agents`, for the same reason: a registry that is only ever added to cannot forget anything. Honest scope, written into §24.11 rather than implied: nothing in this build registers a plugin, so the section reads "No plugins are registered" and the field is stored, enforced and tested while waiting for something to govern. **Flat is the mechanism, not only the look.** The dialog carries no shadow, gradient, blur or transition past a 90 ms hover colour; pointer hover and keyboard focus are the *same* colour change; every size comes from the shell's scale (`--space-*`, `--radius-*`, `--control-h`, `--font-ui`); `contain: content` sits on the card and on each of its three scroll regions, so ticking a box cannot invalidate layout outside it; the inventory is read **once per open** and the MCP list costs **zero** round trips because it is read from the workspace the sidebar already holds; and no `innerHTML` is used, so a folder named `<img onerror=…>` is a folder name. Two defects of my own surfaced while making the flow work, and both are recorded because the guards now own them. A **stale `state.error`** made a fresh verdict unknowable — an error left by anything earlier kept the dialog open after a *successful* creation and blamed the folder for it; the status is now cleared before the attempt and the verdict is this attempt's (the live guard went red on exactly this). And **a workspace with no chats had no group at all**, because `groupSessions` derived its groups from sessions: a folder just added was invisible until someone happened to start a chat in it, so the seeded-from-workspaces rule is now a promise with a ui-fixture check of its own. **The falsification batch found one check that could not fail, and the fix was to make the redundancy visible.** 18 injections, 17 red — and one stayed green: weakening `normalize_workspace_root`'s lookup to fall back to another directory left "a missing root is refused" green, because `canonicalize` refuses a missing path by itself. Two refusals guard the missing case, so no test using a missing path can tell which one fired; the app test now also hands it a path that **exists and is a file**, which only the directory rule refuses. The batch's residue check confirms every touched file came back byte-identical. Separately, the active workspace's status dot was a `::before` on the count — invisible to any assertion and to a screen reader — so it is now an element with `aria-current` on the header, and the page guard asserts exactly one header carries it (seen red when the attribute is removed). Evidence: **8 Rust tests** (3 in `folders.rs` — folders only and sorted case-insensitively with resolved paths, a file and a missing path refused with the path named while a blank one opens home, and the 500-entry cap reported rather than silently dropped; 3 in `settings.rs` — a draft landing whole including trimming and only the ticked server on, a draft naming an unknown server refused, and an absent choice keeping the seeded switches; 1 in `caps.rs` — a denied plugin rebuilt away while the others stay; 1 in `app.rs` — a workspace born configured, a bad draft adding nothing, and a missing root refused), taking the lib suite to **308 passed / 0 failed**, plus **page 128/0** (13 new: the dialog is its own labelled modal and not inside the settings modal, the folder-plus opens it with **no** `get_settings` read, the MCP switches and the inventory rows match the fixtures with an empty plugin section saying so, home → descend → Use-this-folder filling the resolved path, a creation with no folder refused locally with no round trip, one `create_workspace` carrying folder + name + model + deny-lists + `mcp_enabled` where a server that started on and was ticked off arrives absent, the dialog closing on success, Escape closing it and returning focus to its opener, and `+ New workspace` opening the same dialog instead of an inline form, plus exactly one header carrying the active dot and `aria-current`), **live 120/0** (8 new, against a double whose `create_workspace` keeps the workspace, whose `list_folder` is a real small tree and whose `set_active_workspace` now returns the whole settings instead of a partial snapshot that blanked the sidebar), **ui 162/0** (a workspace with no chats is still a group — the fix for a defect this round created: a folder just added was invisible until someone started a chat in it), **markup 61/0**, **layout**, **wiring 98/98** and `cargo build -p bsh` clean. Limits: the chooser starts at `$HOME` and shows every folder (no filter box and no recent-folders list); presets still capture only the fields they always did, so a preset cannot carry its MCP switches yet; `disabled_plugins` has no plugin to act on in this build; and the dialog is one long card rather than a step-by-step wizard, which is a deliberate bet that a single screen beats three clicks for seven fields. |

| 107 | 2026-10-11 | **The folder picker should have been the system's, and the honest reason it was not is a dependency that is not in this machine's offline crate cache.** Round 106 shipped an in-app browser as *the* chooser and wrote the trade-off down as if it were a design decision; the review says otherwise, and the review is right: a workspace root is chosen once and trusted for a long time, so the gesture that picks it should be the one every desktop user already knows. `tauri-plugin-dialog` and `rfd` are both absent from `~/.cargo/registry`, and that is a reason to reach for the panel the OS already ships, not a reason to build a weaker one. So `crates/gui/src/picker.rs` builds the platform's own command line — macOS `osascript`'s `choose folder`, Linux `zenity --file-selection --directory` falling back to `kdialog`, Windows PowerShell's `FolderBrowserDialog` — and reads the answer back. **The outcome is a type, not a `String`, because two of the three answers are not failures.** `ChooseOutcome::Picked` is a folder; `Cancelled` is a decision (macOS reports `-128`, which the script catches; the others exit non-zero with nothing said) and must leave the field exactly as it was and write no error; `Unavailable` is a machine with no panel (a container, a headless session, a Linux box without zenity) and carries a reason the UI can show. Collapsing those into one string is how "changed my mind" ends up rendered as a red failure — the field keeps what it had, and the app never invents a verdict for a dismissal. **A modal cannot be driven, so the modal call is the only thing not under test — and it is swapped rather than skipped.** Every decision around the panel is pure and unit-tested (`command`, `outcome`, `quote_applescript`, `quote_powershell`): a chosen path comes back as given, a dismissal is not a failure in any of the four shapes the three platforms produce (`""` with exit 0, exit 1 with no output, `-128` in stderr, exit 0 with no output on Windows), a real failure keeps the program's own words, a path printed beside a non-zero exit is **not** trusted, and the escaping is proven in the direction that bites — the backslash is escaped before the quote (otherwise `C:\tmp` gains a `t` escape) and control characters are dropped, because a newline cannot live inside an AppleScript literal at all. The one test that touches the real interpreter runs `the same argv with only the panel statement replaced by a reference to a folder that exists`: the `-e` list, the quoting, `POSIX path of` and the `on error number -128` handler all execute against `osascript`, so a typo in the wrapper cannot hide behind "a human has to click". **The in-app browser is kept, as the fallback, and only the fallback.** One field, two doors, one rule: whichever of the panel, the browser or the keyboard produced the path, `folders::resolve` is the single place it is canonicalized and checked, so a root cannot be trusted differently depending on how it was chosen. A guard caught a real defect while this was being wired: the unavailable path wrote the reason into the hint and *then* revealed the browser, whose reveal overwrote the hint — the user was told the fallback had opened and never why, which is precisely the half of the promise that matters. The reason now travels into the reveal. Evidence: **10 new Rust tests** (8 pure — `command`, the four dismissal shapes, a chosen path, a real failure's words, a path beside a non-zero exit, both escaping directions, and the picked-path resolution with its dismissal — plus two that touch `osascript`: the script-shape test that swaps only the panel verb, and one that hands the **real** verb to `osacompile`, which validates the AppleScript and writes a compiled script *without opening a panel*; **318 lib tests pass**), **page 132/0** (four new: the panel's path lands, a dismissal changes nothing and writes no error, an absent panel opens the fallback *with the reason* — the check that caught the overwrite — and a path from the panel closes a fallback that was open), **live 123/0** (the same three against a double that answers a path, a dismissal or an absence on demand: `LIVE.answerNext("choose_folder", …)`), plus `wiring` (**99 registered ↔ 99 invoked**, no unreachable, no phantom, now including `choose_folder`), `markup`, `layout`, `perf`, `package`, `zip`, clippy `-D warnings` and `cargo fmt` clean. A falsification batch of **12 injections against the new rules returned 11 red and 1 green**, and the green one was a finding about the *injection*, not the check: making the success path reveal the browser and then adopt the path stays green because `adoptRoot` closes it again — a compensating call in the same function neutralizes it. The check that *can* fail is the one with the precondition: it now opens the fallback first, and removing `hideInAppBrowser()` from `adoptRoot` reddens it (verified). The panel verb itself has a targeted red of its own: `choose folder` → `choose folders` fails the compile check and nothing else. Batch residue: every touched file byte-identical to its snapshot. Limits: macOS reaches `osascript` by sending an Apple Event, so a **sandboxed** bundle needs `com.apple.security.automation.apple-events` in its entitlements (the dev binary is not sandboxed and does not) — the fallback is what makes that a packaging task rather than a dead button; the panel is only as good as the platform's program being present (hence the fallback), the start folder is passed only when it exists (a panel opened at a missing path reports an error instead of opening), there is no "recent folders" memory yet, and the panel's own click is the one step nothing here performs: the statement is compiled and the surrounding logic is executed against the real interpreter, but *showing* the panel and pressing Choose is left to a human and is reported as such — never claimed as automated. |
| 108 | 2026-10-11 | **Flat stopped being a rule for one dialog and became a rule for the UI, and the difference is that a check now owns it.** Rev 106 wrote "flat is the mechanism, not only the look" about the Add-workspace dialog alone; the rest of the chrome still painted seven `box-shadow`s (a row menu, a modal, two popovers, a tooltip, the mobile drawer, the new-session button), a `filter: brightness(1.1)` on the accent hover, a `--shadow-panel` token that was **defined twice and used nowhere**, and **nine different corner radii** (3/4/5/6/7/8/9/10/12/14 px, 53 literal declarations) instead of the three-token scale the shell claimed to own. The pass itself is mechanical and was done by a script that reports every change: the seven shadows are gone (an overlay takes `--border-strong` on its 1px border instead — on the dark theme a shadow over `--card` was doing almost nothing, while its edge was already there), the brightness filter became a solid `--accent-hover` token per theme so pointer hover and keyboard focus are the same colour change, the dead shadow token was deleted, and all 53 radii were mapped onto a scale that is now `--radius-xs/sm/md/lg` = **4/6/8/12 px** (`999px` pills and `50%` circles are shapes and are allowed). What makes it a requirement rather than a coat of paint is the enforcement, and it is deliberately **two checks because each is blind to what the other sees**. `markup_selftest.sh` scans every declaration in `ui/style.css` and rejects a `box-shadow`/`text-shadow`/`backdrop-filter`/`filter` that is not `none`, any `gradient(`, any literal px radius, and any non-`transform` transition over 120 ms — with the two exemptions written into the guard so a third cannot be added quietly: the streaming caret's `steps(1)` blink (a text caret, one paint per second) and the mobile drawer's `transform` slide (a motion, not a surface). `layout_selftest.sh` now also asks **every element the shipped page lays out** what it actually paints (`painted=[…]`), because a text scan cannot see a shadow JavaScript sets on an element and a rendered probe cannot see a rule that never applies: at 1440/900/560/500 the line reads `painted=[]`. Both layers were falsified before they were trusted — injecting one `box-shadow` into `.modal-card` reddened the scan (`834:box-shadow=0 18px 60px rgba(0,0,0,0.5)`) *and* the probe (`painted=[modal-card(shadow)]`), which is the whole argument for having both. Evidence: **markup 62/0**, **page 132/0**, **live 123/0**, **ui 162/0 + shell 94/0**, **layout 0** at four widths, **wiring** (both directions), **package 16/0**, **zip 10/0**, **perf 19/0**, `cargo test -p bsh` **321 passed / 0 failed / 6 ignored**, clippy `-D warnings` clean. A screenshot of the shipped page under the live double (1440×900) is the visual half of the evidence: solid fills, 1px edges, no depth. Limits: the rendered probe covers the shipped page, so a fixture that carried its own inline styles would be outside it (none does — `tests/*.html` have no shadow or gradient); the blink and the drawer slide stay by design; `--border-strong`'s two values were chosen for legibility rather than measured against a contrast ratio; and `cargo fmt --check` currently fails in `crates/gui/src/mcp_view.rs` / `app.rs` on a concurrent session's in-progress `prompts/get` work, which this revision did not touch. |
| 109 | 2026-10-11 | **A three-way review of the GUI (external parity research, then performance and features/architecture) was commissioned, and its first two survivors were defects that had shipped under a green suite.** The external half is recorded as a capability matrix against the two products; the local halves are the ~52 findings this row acts on. Two were not style questions. **The token meter never painted.** `renderUsage()` appends `.usage-meter` to `#status-right`, and `renderStatus()` cleared that element before filling it — so the meter was built and thrown away on every streaming frame, and the ui fixture could not see it because its check called `renderUsage()` *directly*, bypassing the pass that owns the element. The first fix was still wrong and the new check caught it: `renderArchived` clears the host it is handed too, so a meter rendered between the two clears is lost by the second writer — the same lost-update shape as rev 107's overwritten hint. It is now rendered after **both** clears, and the check goes through `renderStatus()`: `text="none"` before, `88% of 14k` after. **One flick grew the transcript to the whole chat.** The scroll handler grew the drawn window per *scroll event* and a gesture fires dozens per frame; measured in the ui fixture, 50 events took `windowExtra` to 10 000 and the drawn transcript from 200 rows to all 1 000 — the documented `WINDOW` cap held only until the first scroll-up, a bound this document claimed and the perf soak cannot see because it measures streaming, not scrolling. Growth is now granted once per **drawn frame** (the render re-opens the gate, which also makes the gesture self-limiting: the growth prepends rows above the reader and the render restores their offset, so the next growth needs another deliberate scroll), capped at the message count, and reset on every session switch, since the widened window belonged to the chat that was open. Both checks were red before the fix: `the token meter survives the status line's own clear (text="none")` and `a burst of scroll events grows the window once (events=50 extra=10000 rows=1000)`. Also landed: `schedule_tick` saved unconditionally on a 30 s timer that runs whether or not a reminder exists — 2 880 serialize + temp + rename cycles a day for an unchanged file — so `schedule::Store::tick_due` persists **only when something was delivered**, with a test that owns its temp path and asserts both halves (an idle tick leaves the bytes identical; a delivery is written back, so a restart cannot deliver the same reminder twice). Evidence: **ui 164/0** (the two new checks among them), **markup 62/0**, **page 132/0**, **live 123/0**, **shell 94/0**, **layout 0** at four widths, **wiring**, **package 16/0**, **zip 10/0**, **perf 19/0**, `cargo build -p bsh` and clippy on the lib clean. The reviewed findings that are **not** acted on are listed in §24.14 rather than dropped; the highest-value ones are a swallowed `respond_approval` (the modal closes although the answer never reached the backend), `/export` appearing twice so the ZIP row is unreachable by typing, `load_all` answering a by-id question by parsing every session file, and the unbounded `brokers` vector that grows on every agent rebuild. |
| 110 | 2026-10-11 | **The sidebar carried four rows the user asked to remove, and three of them were a second door into a room that already had one.** The screenshot's red box said "Remove from left panel" over **Plugins & skills**, **Automation tasks**, **MCP connectors** and **Files**, and the first three were literally shortcuts: rev 88 drew them as rows that called `navTo("cap-list")`, `navTo("jobs-section")` and `navTo("set-mcp")`, each opening a Settings rail tab that already existed with the same icon and the same content — two front doors, one room, and the sidebar paid for it in height while the sessions list lost it. They are **deleted, not hidden**: the `<nav class="sidebar-nav">` block, its four rows, its three JS bindings, `#files-toggle`, and every stylesheet rule they owned (`.sidebar-nav`, `.nav-item`, `.files-head`, the `[data-sidebar="rail"]` overrides for them) are gone, so the sidebar is now brand → New session → Workspaces → sessions → foot, as the rail mode always was. The fourth row was not a duplicate: **Files** was the only door to the workspace tree outside the palette's `/files`. It was not deleted but moved, into the Settings rail **beside its three siblings** — Tools & skills, MCP servers and Jobs already live there, and this app's convention is that the sidebar lists conversations while every other surface is a Settings section. Moving it removed a piece of state rather than adding one: the tree had an `open` flag and a `hidden` attribute of its own *and* a pane that can be active or not, which is two places that can disagree about whether the tree is showing — the same one-element-two-writers shape rev 107's overwritten hint and rev 109's token meter were, so here it was deleted: visibility belongs to the pane (`focusSection` toggles `.active`), and `renderFileTree` only paints rows. Loading became the section-selection gesture itself (`focusSection` → `ensureFilesLoaded`), idempotent on a filled cache, because the pane is the only thing that needs the listing; a workspace-root change refetches only while that pane is the one on screen, and otherwise leaves the empty cache for the next visit. `/files` now calls `navTo("files-section")`. **The change exposed a check that could never fail.** The ui fixture auto-creates every id app.js binds (`document.getElementById` is shimmed), so its "sidebar navigation is wired" assertion — `!!getElementById("nav-plugins") && !!getElementById("nav-mcp")` — was true whatever the shipped markup said; it had been green since rev 88 while proving nothing about the sidebar. Structural facts now live where they can be read: `markup_selftest.sh` scans the shipped page and stylesheet for a resurrected `sidebar-nav`/`nav-item`/`files-head`/`files-toggle`, asserts the rail tab and the pane point at each other (`role="tab"` + `aria-controls` / `role="tabpanel"` + `aria-labelledby`, matched as two halves because this file wraps a pane's opening tag), asserts the tree host sits **inside** `#files-section` (a `sed`-scoped block, so it is an assertion about the pane rather than about the document), and asserts `/files` names that section. The fixture keeps the half it can see: the palette entry, plus a rail tab and pane it now builds *before* app.js loads so the real click path runs. **And the tree gained the live coverage it never had.** The bridge's `list_files` was a stub returning `[]` and no live check had ever opened the tree; it now answers **per path** (the root holding a folder and a file, the folder holding a child), so the new check drives the whole loop — select the section, read the root's rows with the file's size, expand the folder (which must ask for *that* folder: a single-level fixture cannot tell "expanded" from "re-listed the root"), then open the file and find it in the document panel, with the child's indent asserted because depth is what says the row is inside the folder rather than beside it. Evidence: **markup 71/0** (nine new checks), **page 132/0**, **ui 164/0** + **shell 94/0**, **live 133** with the three new Files checks green (the single red is the concurrent session's image-bubble check, not this revision — it aborts the harness after those three have already passed), **layout 0** at 1440/900/560/500, **wiring 0**, **package 16/0**, **zip 10/0**, **perf 19/0**, `node --check` clean on every edited script. Falsified **five ways, against a copy** of `ui/`, `tests/` and `icons/` under `/tmp` rather than the shared tree — another session is editing `ui/app.js` in this working copy, and a batch that restores app.js from its own snapshot would silently revert their work, so the batch cannot touch the tree at all and its residue check is byte identity inside the copy. Each injection reddened what it should: a utility row put back (`no nav-item rows may remain`); the tree host moved to the sidebar (`the file tree must sit after the sidebar` *and* `the tree host must be inside #files-section`); `/files` retargeted (`/files must open the Files section`); the `focusSection` hook removed (ui: `pane=true rows=0 calls=0`); the cache guard removed (ui: `calls=2`). Limits: the rail is thirteen tabs now and wraps below 720 px; the Files pane does not repeat which workspace it is browsing (Settings → General still names it, and the tree is per-root); `renderFileTree`'s truncation note stays English while the pane's title and hint are translated; the removed rows' `title` tooltips went with them, so the only place those surfaces are named on hover is the rail; and the e2e spec that clicked `#nav-automation` was updated to open Settings and the Jobs tab, but tauri-driver has no macOS backend, so that path is verified by reading rather than by running. |
| 111 | 2026-10-11 | **The sidebar's most prominent control was "New session", and the user said it should be "Add Workspace"; asked where the old control goes, they said "move to here as an icon".** Two instructions, one redesign, and the redesign is a rename of *intent* rather than of a label. A chat is started from the workspace it belongs to — the row's `+` — or from the keyboard (Cmd/Ctrl+N) or from the palette (`/new`), so the top of the sidebar is free for the gesture that creates the thing the sidebar is a list *of*. The old `.side-primary` "New session" button, its `#new-chat` id and its `.new-session` class are gone; the new control is a folder-plus whose name and tooltip travel as `data-i18n-aria`/`data-i18n-title="btn.addWorkspace"`, because a glyph has no text for the translator to find. **One chat-start, three doors**: the row `+` (`newSessionIn(group.id)`, with `aria-keyshortcuts`), Cmd/Ctrl+N and the palette's `/new` all reach the same creation path, which is what makes removing the big button a *move* rather than a loss of a capability. Evidence: **markup 81/0** (the guard's `STALE_NAMES` scan asserts no `#new-chat`, `.new-session` or `side-primary` survives in the shipped bytes; three scoped greps assert the three doors, one of them scoped to the row's own `+` block because its `…` menu also creates), **page 132/0** (the Add-workspace opener is the sidebar's own control and leaves Settings shut), **live 135** with both M9 Add-workspace checks green — the single red is the concurrent session's image-bubble check, which aborts the harness after they have passed — **ui 164/0 + shell 94/0**, **layout 0** at 1440/900/560/500, **wiring 0**, **package 16/0**, **zip 10/0**, **perf 19/0**. Falsified **7 ways**, each against a copy of `ui/`, `tests/` and `icons/` under `/tmp` rather than the shared tree (another session is editing `ui/app.js` in this working copy), each injection reddening the check that owns it with byte-identical residue. Limits: for exactly one round the icon sat in the brand row of `.sidebar-head`, which **rev 112 reverses** — the design puts it in the header of the section it adds to; the row's `+` and the icon are distinct gestures (a chat in *that* workspace versus a new workspace), so neither replaces the other; and the icon is not itself a keyboard shortcut (the palette and Cmd+N are). |
| 112 | 2026-10-11 | **The design redraws the Workspaces header into three controls and hides three settings behind the middle one.** The mockup's grey band holds a magnifier, a sliders glyph and a folder-plus; its arrows say magnifier → the search field, sliders → a menu, folder-plus → a workspace root out of the system panel. The menu is the new thing, and it is the whole of the requirement's "search, filter/settings, and create actions" §12.1 item 4 already asked for: **Group by** WorkSpace / Workspace Tree (ticked) / In one list, **Order by** Manual / Last updated (ticked), **Filter sessions** Hide archived (ticked, counted) / All conversations (show archived) / Archived only (counted), with the header's lost reload folded in as the menu's one acting item. The design is silent about the brand row above the band (a redraw with nothing there), so the reading is written down: the folder-plus **returns to the section header** and the brand row is the brand alone again — **rev 111's placement is reversed**, and this row is where to disagree with it. **One menu, one writer.** `state.showArchived` is deleted, `state.view = {group, order, filter}` is the only state, and the `⧉ Archived (n)` disclosure is deleted with it: which archived rows are drawn is the filter's decision, made in `groupSessions`, and the counts live in the labels that make the decision. `openRowMenu` — the repo's single menu — gained the two shapes this needs: `item.group` draws a `.row-menu-head` only when the group changes, and `item.checked` makes an entry a `role="menuitemradio"` with `aria-checked` and a tick. A pick redraws the list and **reopens the menu** (the three choices are usually set together), which is why the button stops propagation — the document closes any menu a click lands outside of, so without it the menu opened and shut inside one gesture. **That defect is this revision's most useful finding**: it was written, the page guard's click went red on it (`menu=false, items=0`), and the guard's diagnostic step — not a reading — named it. **The three modes are three shapes of one list, not three lists.** `tree` is the default shape (a header per workspace, all expanded); `workspace` is every workspace as a header with only the **active** one expanded, and the fold is drawn only on the active group, because a fold that folds nothing is a dead control; `list` has no headers at all — one ordered list, every row carrying a `.sess-ws-chip`, since a mode that drops the workspace name would drop a fact the grouping was carrying. `order` is `updated` (default) or `manual` (the store's order; the sort is stable, so ties stay where the backend put them). `filter` is `hide` (default), `all` (archived rows drawn beside their own workspace's chats) or `archived` (only archived, and no empty group for a workspace with nothing archived). **Every label in it is translated like the row menus' are** — thirteen new `tr()` keys — and widening the completeness check from the markup's `data-i18n` marks to **every** key `tr()` is asked for found seven *pre-existing* keys that had an English fallback and no dictionary entry (the Add-workspace dialog's hints and the active-workspace label), so those rendered English in a Chinese UI while the check stayed green: a check that reads only one of the two places a label can live is the same shape as rev 110's check that could never fail. The choice is persisted under **one key** (`bos.sessionView`) through the same `parse…Blob` guard the run panel uses, so a hand-edited or truncated blob is the default rather than an exception at boot. Evidence: **markup 90/0** (9 new: the header holds search, the menu and add in that order with the menu announcing `aria-haspopup="menu"`; the brand row holds the brand alone; no `#new-chat`, `#workspace-add`, `.new-session`, `side-primary`, `sessions-refresh`, `archived-toggle` or `showArchived` may survive in the shipped bytes; the six mode/order/filter strings and the one key exist; the reload's item **and** its handler exist, because either half alone is a control that does nothing), **page 140/0** (8 new, driven through the button's own click: the menu is nine radio entries under four headings with `group:tree,order:updated,filter:hide` ticked and both counts in their labels; "In one list" has no `.ws-head` and chips reading `bos,bos,trino`; "WorkSpace" draws two headers, one fold and only the active group's rows; "Manual" keeps a fixture's array order where "Last updated" reorders it (`o2,o1` versus `o1,o2` — a fixture whose array order *is* its recency order could not tell them apart); "All conversations" puts the archived row back between `s2` and `s4` in its own group; "Archived only" draws one row and one header; the blob is read back and `parseViewBlob` refuses nonsense), **live 135** (the archive flow now drives the design end-to-end — the count in the label, its `aria-checked`, "All conversations" revealing the row, Restore putting the chat back; the single red is the concurrent session's image-bubble check, which aborts the harness after mine have passed), **ui 164/0 + shell 94/0** (four fixture call sites now pass the view explicitly — `groupSessions` reads it, and a fixture that silently inherited a default would be testing a mode the app never had), **layout 0** at 1440/900/560/500, **wiring 0**, **package 16/0**, **zip 10/0**, **perf 19/0**. Falsified **12 ways**, each in a `/tmp` copy and each reddening its own layer, with byte-identical residue: the header losing the menu (markup 2), the folder-plus returning to the brand row (markup 3), the storage key renamed (markup 1), the reload's handler dropped while its item stayed (markup 1), "Manual" ignored (page 1), the click let through to the document (page 2 — the defect above, reproduced on demand), "Archived only" no longer excluding the live chats (page 4), the flat chip emptied (page 1), a choice no longer redrawing the list (live 1), a view-menu key removed from the dictionary (markup 1, naming `view.filterArchived`), a *pre-existing* key app.js asks for removed (markup 1, naming `ws.active`), and the reload item losing its `tr()` wrapper (markup 1). The Chinese screenshot of the same menu is the visual half of that: every group heading, choice and count renders translated, and taking it is what found **two `New workspace` labels with no i18n mark at all** — the sidebar's own add row and the manager's button — which no text check could see, because a label that opts into nothing is invisible to a scan of the marks. Both are marked (`ws.add`) now, and the sidebar's is drawn through `tr()`. Limits: the e2e specs were retargeted to the doors that exist (`newChat()` clicks the active workspace row's `+`; `shell.spec.js` asserts `#workspace-add-primary`) but tauri-driver has no macOS backend, so that path is read rather than run; the menu is opened by click and is not reachable from the list beside it by keyboard (Escape, arrows, Home/End and focus hand-back work once it is open); "Manual" order is the backend's and cannot be edited (no drag); the counts in the labels count the whole store, not the filtered view; and `workspace` mode's fold exists only on the active group, so a non-active workspace cannot be folded by hand. |

| 113 | 2026-10-11 | **A workspace's configuration moved next to the code: adding one creates `<root>/.bos/workspace.toml`, the way a project carries `.vscode`.** The registry (`~/.bos/gui/settings.toml`) still answers "which workspaces exist"; the folder now answers "how this one runs", so the settings can be read, edited, diffed, committed and carried to another machine with the code. `crates/gui/src/workspace_dir.rs` owns the format and the three rules that make writing into someone's project directory safe: **identity never travels** (`id`/`name`/`root`/timestamps stay in the registry, so the same file in a clone is a configuration and not a second workspace), **secrets never travel** (the API key, the providers that carry keys of their own, and the MCP *entries* — whose `args`/`url` can hold a token — are not written; MCP travels as a list of **names**, so a file can switch an existing server and can never introduce a command this installation does not have), and **a file is input, never an instruction** (unparsable files, absent keys and unknown names are ignored, not fatal). Creation writes it before the record is pushed, so a root that will not take a `.bos` is refused with nothing added; editing rewrites it and puts the whole record back if the write fails — the runtime is applied before the write and the backup taken before that, so a restore that ran late would leave a model the folder never took; `apply_preset` writes it only when the preset's root is a folder, because that door has never validated a root, and the Settings dialog's save writes it too — that door copies the globals onto the workspace in use (`push_globals_to_active_workspace`), and leaving the folder behind there would mean the next load folded the older file back over the user's own save. Loading folds each file over the registry copy, and field keys are `Option`s precisely so "absent" (inherit) and "set to empty" stay different answers. Evidence: seven module tests (round trip; no secret and no identity key, checked as TOML keys rather than substrings; missing/unparsable file; unknown MCP name; a plain file where `.bos` goes; second write replaces with no `.tmp` left; an empty root skipped in both directions, with the check owning the path it probes), three `app::tests` through the new `create_workspace_record`/`rewrite_workspace`/`push_globals_to_active_workspace` seams (which exist so a test can drive them without rewriting the user's registry), one `settings::tests` load from a real file on disk, and the dialog's own copy before the folder is picked — the claim it used to make, "Nothing is changed in the folder itself", is deleted because it stopped being true, and the replacement is guarded in `markup_selftest.sh` as a sentence rather than as a key, since a claim nobody checks is how the false one survived. Limits: a pre-existing workspace gets its folder on its next edit rather than on launch (startup must not write into folders nobody asked it to touch); a read-only folder cannot be added as a workspace; an **empty** root is skipped in both directions rather than resolved to the process working directory, because a bundled app launches from `/` and a create there is a permission refusal that would take the whole creation down; the pane does not print the resolved path; sessions and the app's other stores stay in `~/.bos/gui/`. |

| 114 | 2026-10-12 | **The folder-plus opened a form *about* a folder instead of asking for the folder, and a workspace is a folder.** The user pointed at the control and at the system's file window and said "use system folder dialog directly", and the order they asked for is the truthful one: the platform's panel is now the first thing that button does, and the pick *is* the workspace — named for the folder it names, seeded from the installation as it stands, with `<root>/.bos` written before the record exists (rev 113). A folder that is already some workspace's root is **focused rather than recorded twice**, because with a panel one click on a folder you already have is the easy mistake; a dismissal adds nothing, writes no error and opens no form. The Add-a-workspace form is now the **fallback** rather than the door: a machine with no panel gets it with the reason and with the app's own browser already listing folders, and a refused creation gets it with the folder and the fresh reason still in it — so the `WorkspaceDraft` payload (model, the deny-lists, `mcp_enabled`) is still sent, from the one path that has a form to send it from. Nothing that form could settle at creation is lost: the model, the MCP servers, the plugins, the tools and the skills are all on the workspace's own inline Edit form. `+ New workspace` in the Settings Workspaces pane doorways into the same flow instead of a second one. Evidence: the live guard's M9 drives the panel-first path (the pick becomes the workspace with `config: null`; the duplicate pick focuses the other workspace and creates nothing; the dismissal leaves the store and the DOM alone), the Settings doorway reaching the same panel, the no-panel fallback opening the form with its browser and the reason, and the whole form path behind it (browser walk, MCP payload, refusal, Escape) — 138 checks, with the one pre-existing image-bubble red unchanged. |
| 115 | 2026-10-12 | **The last workspace could not be removed, and the registry had no empty state.** The user asked to allow removing it, and the refusal was doing two jobs that only look like one: keeping `active_workspace` pointing at something (which the panel can say out loud without a workspace) and having somewhere to re-home the chats. Those come apart. A registry with none is now a state the sidebar has a face for: the list is a hint that a workspace is a folder plus the folder-plus's own gesture, and nothing creates a chat into the void — `create_session` refuses one that names no workspace, and the composer, the row's `+` and `/new` all say why instead of leaving a record no row could ever show. The last removal's chats have **no successor to keep them in**, so the form stops offering a choice it cannot honour: it states that they go with it — the one case where a delete is what the user asked for, §24.6's rule being that the *choice* is explicit, not that one always exists — and the payload sends `deleteSessions: true`. Keeping them with nowhere to keep them is refused by name, and the refusal writes nothing, which is what tells it apart from a delete that looks like one. The seed rule came out of the same ask: `migrate` adopted a rootless "Default" on **every** load, so the removal had just been undone by the next restart. A file at the current schema is now believed about its workspaces, including the empty answer; only a file from before workspaces existed (schema 0, whose sessions still say `workspace = "default"`) adopts one. Evidence: `the_last_workspace_can_be_removed_and_a_chat_needs_one_to_live_in` (the active removal re-homes its chat, the last one refuses the keep policy and writes nothing, deleting takes the chats, and creating one is refused afterwards) and `an_emptied_registry_is_not_refilled_with_a_default_workspace`, plus the settings and preset fixtures that now say they are legacy files rather than new installs — 361 Rust tests green, with the sixteen that had been reading a fresh install as a migrated one made explicit about which they are. |
| 116 | 2026-10-12 | **The workspace row showed the model and put its gestures on the left; the user drew a box around the two and said "don't show model info here", "move them to where the model is".** A row is about the workspace, not about the model the chat in use happens to run on, and asking the row to answer both made the model a chip fighting the name for the left edge while `+` and `…` sat before it. The chip is gone and the two gestures are where it was: `name  +  …  ●  count`, with the name keeping the flex so the row still reads left to right, the fold (on hover, and only for a group that can fold) still leading the name, and the model left to the status line, which names it for the chat in use. The `.ws-model` stylesheet rule stays because the workspace *form's* model field borrows the class; the header no longer draws one. Evidence: the page guard asserts the row's order directly (`name < + < … < dot < count`, a fold if any ahead of the name, and no `.ws-model` inside the header), which is a check that fails if the chip comes back or the buttons move back. |
| 117 | 2026-10-12 | **Adding a workspace also started a chat in it; the user called it a bug ("添加工作区会缺省添加一个chat session，这是bug，需要修复").** The promise was old and explicit — "a workspace with no chat gets one: the composer is the workspace's front door" — but the door was opened for the reader at the moment the *folder* was added, so every new workspace arrived holding a "New chat" nobody had asked for, and every visit to an empty workspace left another one behind. The create moved to where the question is: `syncTranscriptToWorkspace` now leaves an empty workspace empty (no open chat, the transcript's own "New conversation" hint, the row's `⊕` still aiming at it), and the composer's first message starts the chat — `sendText` creates one and sends into it when nothing is open, exactly as `⊕` / Cmd+N / `/new` do. Nothing else creates a chat for the reader. Evidence: the live guard counts `create_session` across the folder-plus's pick and asserts the transcript is the empty hint (the bridge's `create_workspace` seeds no session, so the pick either creates one or does not); the page guard counts the round trips around the same gesture and then sends the first message, which must create *and* send. Both go red when the create is put back in the snapshot applier. |
| 118 | 2026-10-12 | **The sidebar's icons were font glyphs, so its rows read differently from the reference the user is matching ("根据设计图…图标和显示效果要尽量保真").** The head already spoke the right language — the search, view and folder-plus controls are inline SVG outlines on a 24-grid (`class="ico"`, stroked in `currentColor`) — while the rows below it used `"+"`, `"…"` and no folder at all, so a workspace row led with nothing and its add-session control was a font's plus. The row now leads with the folder (the disclosure takes that slot under the pointer, and a row that cannot fold keeps its folder rather than trading it for a triangle it does not have), and its two controls are the reference's: `⋯` and then the `⊕` of "a chat in this workspace", in that order, with the icon set built by one `ico()` helper over `createElementNS` (no markup strings, so the set cannot disagree with itself). The active row's tint, the hollow/filled session marker and the always-visible pin were already right and are untouched. Evidence: the page guard's anatomy check now names the folder and asserts the reference's order (`[folder, fold], name, ⋯, ⊕, [dot,] count`), which fails if the folder goes, if the two controls swap back, or if a model chip returns; the row's own `+` block stays greppable in `markup_selftest.sh` (its anchor moved to the construction line the icon made), and its click, aria-label and keyshortcut assertions are unchanged. |


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
|---|---|---|
| Left navigation | Brand, New Session, Workspaces, sessions, user profile | Navigation and resource management. The utility surfaces (Tools & skills, MCP servers, Jobs, Files) are **Settings sections**, not sidebar rows — rev 110. |
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

**Column geometry (per workspace, rev 79)**

The three-column shell is resizable, and its geometry is **stored per
workspace root**. Each workspace keeps its own column widths, sidebar rail
state and document-panel state (`bos.layout.v1:<root>`; the unscoped
`bos.layout.v1` remains the "default" scope, so layouts saved before scoping
existed are still honoured). Widths are changed by dragging a divider or
with the keyboard (arrows, shift-arrows for a bigger step, Home resets) and
are clamped — sidebar 264–420 px, document panel at most 70 % of the window
while the centre keeps 400 px — with the compact-window rules (the sidebar rails
and no document panel below 1 024 px; the panel's own toggle hides below 860 px)
overriding the stored width rather
than fighting it. The shell stays Tauri-free: the app names the scope
(`Shell.setLayoutScope`) when the root is known or changes; the geometry on
screen is written back to the scope being **left**, then the new scope's
saved layout is adopted — or the defaults when it has none. UI preferences
(appearance, font size, language, …) stay global: they are about the app,
not the project.

### 12.1 Sidebar

**Layout**

1. Show the product logo and brand.
2. The **Workspaces header** carries the section's three controls, in the order
   the design draws them (rev 112): a magnifier that reveals the search field
   (`#sessions-search`), a sliders glyph that opens the group/order/filter menu
   (`#session-view`), and an icon-only folder-plus (`#workspace-add-primary`)
   that asks the platform's folder panel for a folder and makes the workspace it names
   (rev 114); the Add-workspace form is what that button opens when there is no panel.
   Icon-only, because a sidebar's height
   belongs to the sessions — and none of the three is where a chat is started:
   that is the workspace row's `+`, Cmd/Ctrl+N or the palette's `/new` (rev 111,
   whose brief stay in the brand row rev 112 reverses).
3. ~~Show navigation entries for plugins, automation tasks, and MCP connectors.~~
   **Superseded by rev 110** (the user asked for them out of the sidebar): those
   three, and Files with them, are Settings rail sections. See §12.1 "Files tree".
4. Show a Workspaces section with search, filter/settings, and create actions —
   the filter/settings being the rev-112 menu: group by WorkSpace / Workspace
   Tree / In one list, order by Manual / Last updated, and filter sessions by
   Hide archived / All conversations / Archived only, with the archived counts in
   the labels that decide them.
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

**Files tree (shipped, rev 77; moved to Settings by rev 110)**

The workspace on disk is a **lazy tree** in the Settings **Files** section
(`#files-section`, rail tab `set-tab-files-section`); selecting that section is
what loads it, once.

1. Listing is **one directory per click**: the backend (`list_files`) returns a single level, so expanding a folder never walks the repository.
2. The listing is **workspace-confined**: the requested path is joined to the configured root and canonicalized, and is refused if it lands outside it. Absolute paths, `..` segments and symlinks that leave the root all end in "path escapes the workspace"; an unset root says "no workspace configured"; a root that cannot be canonicalized says "workspace unavailable".
3. `.git`, `target` and `node_modules` are skipped, dot-prefixed entries are not listed, and symlinks are never listed as entries.
4. A level is capped at 1 000 kept rows (4 000 raw reads); the cut is reported and the tree says a folder was truncated instead of pretending to be complete.
5. Rows sort directories first, then names case-insensitively; files show their size.
6. Open folders persist **per workspace root** and are **re-fetched** on the next launch, not replayed from a snapshot; changing the root drops the cache.
7. A folder that refuses to open keeps the reason on its own row, and clicking it retries.
8. Names come from disk and are untrusted: rows are painted with `textContent` only.
9. Clicking a file opens it in the document panel through the same bounded `read_file` the panel already trusts; the document panel stays a plain viewer.
10. `/files` in the command palette opens that section (`navTo("files-section")`).

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

### 12.6 DSH frontend reference (rev 81–85)

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
3. **Space comes from the sidebar first — in width, not in the column (rev 85)**
   — the reference concedes a *manually expanded* sidebar when the right panel
   opens; railed session lists read as broken in use, and the geometry never
   needs the concession (264 + 400 + 280 + 16 = 976 fits under the 1 024px doc
   threshold, and the step-down/deterministic-close rules already protect both
   floors). Opening now narrows an over-wide sidebar just far enough for the
   column's 300px floor — never below 264px — rails only via the explicit ☰
   toggle or the <1 024px rule, and layout schema **v2** heals a rail written by
   the pre-v2 concession once on adopt while keeping a rail the user chose.
   Documented divergence: reference line 30's "collapses a manually expanded
   sidebar" is intentionally not cloned.
4. **Theme presenter** — `color-scheme` on the root, `body[data-ds-dark-theme]`,
   `--content-font-size`, and one owned `<meta name="theme-color">` whose content
   is read back from the rendered body background.
5. **Frame clearances** — `--frame-top-clearance: 48px`,
   `--frame-overlay-top: 68px` and `--frame-leading-clearance` published for the
   window-chrome seats (macOS traffic lights, or a seat mounted beside them).
6. **Keyed panel registry, and a docking host** — the root column is a keyed
   `main` slot with `conversation` reserved; `selectPanel(null)` returns to the
   conversation, a registered panel becomes the active surface, an unknown id
   changes nothing, and the built-in document panel registers itself through the
   same public seam a future surface would use (`registerPanel`/`unregisterPanel`/
   `selectPanel`, with `panels`/`activePanel` readable and the active key mirrored
   onto `#app[data-panel]`). A panel is a **mount point, not just an identity**: a
   `render(host)` function fills a surface created on first activation and used
   exactly once, so re-selecting reuses the DOM instead of rebuilding under the
   reader; unregistering removes the surface. A column that closes releases its
   surface, so a hidden panel is never the active one.
7. **Mod+B** toggles the sidebar, and yields to a control that has its own
   meaning for the chord — an editable field, or the terminal, where `Ctrl+B`
   belongs to the program at the other end of the PTY.
8. **The Ongoing Goal seat** (rev 84) — the goal lives in a **card directly
   above the composer**, where the reference layout puts it, never in a
   right-column panel: objective, state chip, rounds against the cap, the
   blocker, and the inline editor. A `#goal-open` chip in the header and a
   `/goal` command are the always-reachable entries while the card is folded,
   and the card is repainted from `get_goal` on every session switch, restore
   and fork. Its checks read the computed display rather than the class —
   the class alone had never hidden anything (rev 84).

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

**Implemented (rev 88):** the dialog is a header row (title, and a ✕ wired to the
same close path so focus hand-back is unchanged), a **vertical `role=tablist`
rail** of sections, and **one `role=tabpanel` pane at a time** — selecting a
section swaps the pane instead of scrolling a single long form, and the rail is
walked with ArrowUp/ArrowDown/Home/End (`aria-selected`, `aria-controls` and a
roving tabindex keep the tablist pattern honest). Below **720 px** the rail turns
into a wrapped strip above the panes, so the content keeps the full width it needs
for a label beside its control — the 500 px probe is what forced that rule, by
reporting controls outside the viewport when the rail still held a column.
The reference's **Open configuration file** action is deliberately absent: no
command backs it, and a button that opens nothing is worse than no button.

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
- **Flat is a rule of the system, not a taste** (rev 108): every surface is one solid
  fill plus a 1px border. `ui/style.css` carries no `box-shadow`, no gradient, no
  `backdrop-filter`, no brightness `filter` and no `text-shadow`; depth is the border
  itself, and an overlay (menu, popover, modal, tooltip, the mobile drawer) gets its
  edge strengthened with `--border-strong` instead of a shadow, so it stays legible
  on the dark theme where a shadow on a dark background was doing nothing anyway.
  Hover and keyboard focus change the *same* colour token — `--accent-hover`, not a
  `filter` — so pointer and keyboard users see one state.
- **Motion is short and never a surface.** Colour transitions are ≤ 120 ms linear.
  Two things are exempt and are named in the guard so a third cannot be added
  quietly: the streaming caret's `steps(1)` blink (a text caret, one paint per
  second) and the mobile drawer's `transform` slide (a motion, not a depth cue).
- **Every corner comes from the scale** `--radius-xs | --radius-sm | --radius-md |
  --radius-lg` (4 / 6 / 8 / 12 px). `999px` (a pill) and `50%` (a circle) are shapes
  rather than corners and are allowed; a literal px radius is a defect, and before
  rev 108 the chrome held nine different ones (3/4/5/6/7/8/9/10/12/14 px).
- Why the rule is enforced twice: a text scan of the stylesheet cannot see a shadow
  that JavaScript sets on an element, and a rendered-element probe cannot see a rule
  that never applies. `markup_selftest.sh` scans every declaration in `ui/style.css`,
  `layout_selftest.sh` asks every laid-out element in the shipped page what it
  actually paints, and both must report nothing.

### 14.2 Suggested initial design tokens

These are implementation starting points, not pixel measurements extracted from the screenshots.

| Token | Suggested value |
|---|---|
| Base spacing | `--space-1 … --space-5` = 4 / 8 / 12 / 16 / 24 px |
| Sidebar width | Approximately 280–340 px (`--sidebar-w` 280 px) |
| Center workspace width | Approximately 400–520 px |
| Panel radius | `--radius-xs / sm / md / lg` = 4 / 6 / 8 / 12 px (as shipped, rev 108) |
| Overlay edge | `--border-strong` — the border that replaces a shadow on a menu, popover, modal, tooltip or drawer |
| Accent hover | `--accent-hover` — a solid colour, never `filter: brightness()` |
| Shadow | deliberately absent; there is no `--shadow-*` token any more (rev 108) |
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
Workspaces                                    🔍  ⚙  📁+
📁 bos                                             ⋯  ⊕  ●  2
    ○ gui transcript work         ☆              2m        …
    ○ plan for M8                                1h        …
    ⧉ Archived (2)
▸ harness-notes
[ + New workspace ]
──────────────
BOS  ◐  EN          ← the sidebar ends here (rev 110)
```

The four utility rows rev 88 drew under **New session** (Plugins & skills,
Automation tasks, MCP connectors, Files) were removed in rev 110 at the user's
request: the first three duplicated a Settings rail section that already
existed, and Files became one. What a Settings section is reachable from the
palette (`/settings`, `/files`) as well as from the rail.

The section head carries the three gestures the reference sidebar puts there: search,
refresh, and **Add a workspace** (the folder-plus). A chat row's `…` is a menu (pin /
rename / fork / archive / delete, with Delete the only entry drawn as destructive) and
its `☆` is the pin's own control, so the state is one click away instead of two. An
archived chat is not in a group at all — the `⧉ Archived (n)` disclosure is the only
place it is still counted, which is what makes "archived" mean something. **A workspace
with no chats is still drawn**, because a folder that was just added has to be visible
(and be a place for `+` to aim at) before anyone starts a chat in it.

A workspace row is the switcher, and it is drawn the way the reference sidebar draws
one: it leads with the **folder's own icon** (the disclosure `▸`/`▾` takes that slot
while the row is under the pointer, so a row that cannot fold keeps its folder), shows
the root on hover, expands to its sessions, and carries the reference's two gestures on
the right — `⋯` (the menu below) and then `⊕` (a chat in *this* workspace) — where the
bound model used to sit. Every icon here is the header's own language: an outline on a
24-grid stroked in `currentColor` and built with `createElementNS`, not a font's idea of
"+", "…" or a folder (rev 118). The model is **not** on the row: a row is about the
workspace, and the model of the chat in use is the status line's job. The menu still
holds rename / set-root / duplicate / remove (remove asks what happens to its sessions:
keep them in a rescued workspace or delete them). Session search stays global but
its results are grouped by workspace. The nav entries below the list keep opening
the unified settings surface (§ 12.1), where a **Workspaces** section owns the
management table (rename, root, model, remove, "use defaults", presets). **Creation is
not there**: adding a workspace is its own flow on the sidebar (§24.11), because it
points the app at a folder no workspace owns yet — the pick *is* the workspace, and
everything that may run in it is inherited until it is asked otherwise — while Settings
inspects and edits what exists.

The registry can be **emptied**. The last workspace is removable like any other, and with
none left the panel is nothing but an invitation — a hint that a workspace is a folder plus
the folder-plus's own gesture — because with nowhere to run, the rest of the panel has no
subject. Nothing creates a chat into that void: the backend refuses a record that names no
workspace, and the three gestures that would have made one say so instead. The last
removal's chats have no successor workspace to be re-homed to, so *that* one case states
what happens instead of offering the keep/delete choice — they go with it, and the form
says so before the button is pressed — since §24.6's rule is that the choice is explicit,
not that one always exists. A silent delete is the outcome the rule exists to prevent, and
it stays prevented by being said out loud.

### 24.5 Commands

`list_workspaces`, `create_workspace` (taking an optional `WorkspaceDraft`, §24.11),
`update_workspace`, `remove_workspace`
(with a keep/delete policy for its sessions), `set_active_workspace`, `choose_folder` (the platform's folder panel, §24.11) and
`list_folder` (the in-app fallback chooser, §24.11), plus the existing session commands
taking a workspace filter. `get_settings`/`save_settings`
keep their shape for the global half so the Add-workspace form and its tests do not churn.

### 24.6 Acceptance

1. An existing install opens with every chat present and reachable under one
   migrated workspace — no session lost, no title changed.
2. A session's model, tools, MCP servers and instructions follow its workspace;
   switching workspace switches all of them together.
3. Removing a workspace never silently destroys sessions: the keep/delete choice is
   explicit and the keep path leaves them reachable — and where no successor workspace
   exists to keep them in, the form states that they go with it rather than offering a
   choice it cannot honour.
4. Global UI state stays device-local and never travels with a workspace folder.
5. The last workspace can be removed, and an emptied registry is a state with a face:
   the panel invites a folder, and no chat is created into the void (a record that names
   no workspace is refused rather than stored).
6. A workspace-less registry survives a restart: a settings file at the current schema is
   believed about its workspaces, including the empty answer.
7. Adding a workspace starts a workspace and nothing else. An empty workspace stays empty:
   the row's `+`, Cmd/Ctrl+N, `/new` and the first message typed into the composer are the
   ways to begin a chat, and none of them happens as a side effect of looking at a folder.
   A workspace with no chats is drawn (item 13) precisely so that this is visible.

### 24.7 Choosing the root folder

The root is the workspace's identity, so it is chosen, not typed, wherever a panel can
be asked: the folder-plus asks the platform's own panel directly (§24.11, not a dialog
plugin), and the form's **Choose folder** button re-points the field the same way. The
typed path stays beside it for headless use and for people who paste a path — a machine
with no panel is a supported machine, not an error. Whichever way
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
| --- | --- | --- |
| built-in tools (bash, files, search, plan, …) | `disabled_tools: Vec<String>` — an **opt-out deny-list** | a new built-in tool should appear for users who never had an opinion, not silently stay off everywhere |
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
   and switching workspace changes the available set. **Landed**: the built-in
   tool deny-list is enforced by `caps::build_agent`; `disabled_skills` is enforced by
   `register_skills_from_dir_denying` (the seam this note used to call open); the
   per-server MCP switch is enforced (`attach_mcp_tools` skips any handshake whose
   server this workspace switched off or never added, so nothing configured means
   nothing attaches); and `disabled_plugins` is enforced by rebuilding the plugin
   registry without the denied names (§24.11).
7. Applying a preset fills the workspace; editing afterwards diverges and leaves the
   preset untouched; re-applying restores it.
8. Migrated workspaces (§24.3) arrive with every capability in its previous state:
   migration must not silently enable MCP servers that were disabled, nor disable
   tools that were on.

### 24.11 Add a workspace: one flow, one payload

Adding a workspace is a flow of its own, and **the folder is the gesture**: the
folder-plus in the Workspaces head asks the platform's panel *first*, and the answer is
the workspace — named for the folder it names, inheriting today's installation, with its
own `.bos` written before the record exists (§24.15). A folder that is already a
workspace's root is **focused rather than recorded twice** (with a panel, one click on a
folder you already have is the easy mistake), and a dismissal changes nothing and is not
an error. `+ New workspace` in the Settings Workspaces pane doorways into the same flow —
it closes Settings and asks the panel — so there is still only one answer to what adding
a workspace does.

The form is the **fallback**, not the door: a machine with no panel opens it with the
reason and with the app's own browser already listing folders, and a folder that refuses
its configuration opens it with the folder and the reason still in it. Nothing is lost by
creating in one gesture — the model, the MCP servers, the plugins, the tools and the
skills are all on the workspace's own inline Edit form, so a workspace made by picking a
folder inherits until it is asked differently. What the form's draft buys is that a
deliberate configuration can be stated *before* the first chat exists; the one-gesture
path buys the folder being the first question, which is what a workspace is.

**The folder chooser is the platform's, with ours behind it.** The folder-plus opens the
system's own panel directly, and the form's "Choose folder…" opens the same one — on macOS
`osascript`'s `choose folder`, on Linux
`zenity --file-selection --directory` (falling back to `kdialog`), on Windows
PowerShell's `FolderBrowserDialog` — because the root of a workspace is chosen once and
trusted for a long time, and the gesture that picks it should be the one the user already
knows. The Tauri dialog plugin is not available offline here, and a hand-rolled panel
would be a weaker version of something every desktop already ships. `crate::picker`
builds that command line and reads the answer as one of three outcomes, and the two that
are easy to get wrong are the reason the outcome is a type rather than a `String`:
**a dismissal is not a failure** (macOS reports `-128`, the others exit non-zero with no
output; the field must keep what it had and no error may be shown) and **a missing panel
is not a failure either** (a container, a headless session, a Linux box without zenity):
the form opens, and the in-app browser — `list_folder`, kept as the fallback — opens
inside it with the reason. A modal panel cannot be automated, so what is tested is everything decided around
it, plus one test that runs *the same command line* with only the panel call swapped for a
reference to a folder that exists, against the real interpreter: the `-e` statements, the
AppleScript escaping, `POSIX path of` and the `-128` handler are all under test, and only
the human click is not.

`list_folder` is the fallback and stays honest on its own terms: it walks one level at a
time and answers with sub-folders only — never a file's contents, never a file at all —
and every path it returns is resolved, so `~`, `..` and symlinks cannot make one folder
look like two. Two decisions are deliberately the opposite of the file sidebar's: it
**reads outside the current workspace** (`crate::filetree` refuses to leave a root;
choosing a root happens before there is one to leave), and it does **not** apply the
sidebar's skip list — someone pointing the app at a folder inside `target/` or
`node_modules/` is making a choice, not making noise. The read is capped at 500 folders
with the number left out reported, and entries that cannot be read are counted rather
than named. Whether the path arrives from the panel, from the browser or from the
keyboard, `folders::resolve` is the one place it is canonicalized and checked, so all
three doors land on the same rule.

**One payload.** `create_workspace(state, name, root, config: Option<WorkspaceDraft>)`
carries everything the form collected — and `config: null`, which is what the
one-gesture path sends, is "no opinion": the workspace is seeded from the installation as
it stands, which is what picking a folder expressed.

```rust
struct WorkspaceDraft {
    model: String, system_prompt: String,
    bash_enabled: Option<bool>, file_tools_enabled: Option<bool>, require_approval: Option<bool>,
    disabled_tools: Vec<String>, disabled_skills: Vec<String>, disabled_plugins: Vec<String>,
    mcp_enabled: Option<Vec<String>>,   // None = keep each seeded server's own switch
}
```

`apply_draft` runs **before** the workspace is added, so a draft naming an MCP server
this installation does not have (`no MCP server named "ghost"`) or a root that is not a
folder is refused whole: the caller gets either a workspace that matches the form or an
error, never a half-configured one. `mcp_enabled: Some` means *exactly these* servers
are on, so a server the user did not tick stays off even when the global it was seeded
from was on; `None` keeps the seeded switch, which is what an older caller's payload
(none at all) means.

**Plugins.** A workspace carries `disabled_plugins`, enforced by *rebuilding* the
registry: `PluginRegistry` has `register` and `clear` and no per-name switch, so
`caps::apply_plugin_deny` re-registers the survivors and a denied plugin is absent from
the agent rather than merely unwelcome — the same shape as `reattach_cached_agents`, for
the same reason: a registry that is only ever added to cannot forget anything. Honest
scope: nothing in this build registers a plugin, so the dialog's Plugins section reads
"No plugins are registered" and the field is inert in practice until one exists. It is
stored, enforced and tested, and waiting for a plugin to govern.

### 24.12 Flat surfaces, and the cost they avoid

The Add-workspace form — the fallback the folder-plus opens when there is no panel — is
**flat** and stays flat: no shadow, no gradient, no blur, no
transition longer than a 90 ms hover colour, and no depth that would have to be
composited. Every state is one solid fill, a 1px border and a colour change — and pointer
hover and keyboard focus are the *same* colour change, so there is one mechanism to learn
and one to keep working. Sizes come from the shell's scale (`--space-*`, `--radius-*`,
`--control-h`, `--font-ui`) rather than from numbers typed at the call site.

That look is also the cheap one, and the mechanism is explicit:

- `contain: content` on the dialog card and on each of its three scroll regions (the two
  capability lists and the folder list), so ticking a box or descending a folder cannot
  invalidate layout outside the box it happened in;
- the capability inventory is read **once per open** (`list_capabilities`), not per
  repaint, and the MCP list costs **zero** round trips because it is read from the
  workspace the sidebar already holds;
- the folder browser reads no geometry (no `getBoundingClientRect`, no forced reflow) and
  rebuilds only its own list on each level;
- no `innerHTML`: the lists are built with `createElement` and `textContent`, so a folder
  named `<img onerror=…>` is a folder name.

The guards hold it to that: the page guard counts the round trips around the open, and
the wiring guard proves every command the dialog can reach is registered.

### 24.13 Acceptance (add a workspace)

9. The folder-plus opens the platform's panel directly, and the folder it names becomes
   a workspace — named for the folder, seeded from the installation — and the active one,
   without opening Settings. Picking a folder that already has a workspace focuses that
   workspace and records nothing. Dismissing the panel changes nothing, is not reported
   as an error, and opens no form.
10. The panel's other two outcomes: a machine with no panel opens the form with the reason
    and the in-app browser already listing folders, and a creation the backend refuses
    opens the form with the folder in it and the fresh reason. The browser walks the
    filesystem, "Use this folder" writes the resolved path, a folder that will not list
    says why beside the list, and a creation with no folder is refused before any round
    trip. Whichever door was used, the path is resolved before it is trusted.
11. One `create_workspace` call carries the folder, the name and the draft: the unticked
    boxes arrive as the deny-lists and the ticked MCP servers as `mcp_enabled`, so a server
    that started on and was ticked off arrives off. The one-gesture path sends
    `config: null` instead, and the workspace it makes is seeded from the installation.
12. A backend refusal keeps the form open with the fresh reason and the folder still
    filled in; a stale error from anything else can neither keep it open nor be shown as
    this attempt's failure. `+ New workspace` in Settings runs the same panel-first flow.
13. A workspace with no chats is drawn as a group, and the workspace created this way is
    the active one.

## 24.14 Review findings (rev 109)

A three-way review of `crates/gui` was commissioned in rev 109: external parity research
against OpenAI Codex (`main`, rust-v0.162.1) and DeepSeek Harness (`master`), then a
performance/resource-bounds pass and a feature-completeness/dead-end pass over this crate.
The findings live here rather than in a chat log because an unwritten finding is a finding
that will be rediscovered. **Landed in rev 109** are the three that were defects under a
green suite (the token meter that never painted, the scroll burst that broke the `WINDOW`
cap, and the unconditional 30 s schedule save). The rest are queued, not dropped, and each
carries the file it lives in so the next round starts from evidence.

| # | Finding | Where | Why it matters | Cheapest fix |
|---|---|---|---|---|
| P1 | `load_all` reads and JSON-parses **every** session file to answer a question about one id (~4–6 times per turn; ~6 MB per call at 300 sessions) | `src/session.rs:548` ← `app.rs:612, 1156, 1176, 1235, 1425, 3587` | one turn's store traffic grows with the *whole history*, not with the chat on screen | route by-id loads through the existing `cached_sessions()` (`session.rs:492`) |
| P2 | `brokers: Vec<Arc<ApprovalBroker>>` is appended on every agent build and never pruned; every approval scans it | `src/app.rs:54, 160` | an unbounded vector on a long-lived window, and an O(n) scan per approval | keep the broker in `AgentEntry`; rebuild the iteration list from `agent_cache` |
| P3 | `estimateContext()` walks every character of every message once **per frame** during a stream | `ui/app.js:256-263` ← `:2290` | ~1M chars per frame at 5 000 messages, and invisible to the perf guard because it mutates nothing | cache tokens per message, updated on append |
| P4 | `renderTrajectory` scans all messages *before* its `trajKey` early return, and reads `scrollHeight` on the streaming path | `ui/app.js:1894-1911` ← `:1980` | O(messages) per frame while the tab is open, plus a forced layout | derive the key from the tail, compare before scanning |
| P5 | Sidebar search has no debounce: every keystroke (≥2 chars) is a round trip and a full sidebar rebuild | `ui/app.js:5503-5515, 6220-6230` | typing "budgets" costs 6 round trips and ~13 full rebuilds | 130 ms debounce + latest-query guard, as the mention picker does (`:3540`) |
| P6 | `search_files` walks the whole workspace with no cap on entries visited, and accumulates every fuzzy hit before truncating | `src/app.rs:1745-1757`, `src/mentions.rs:91, 99-125` | on the mention input path, under the state lock, on the command thread | cap visited entries and keep a bounded top-N |
| P7 | The forwarder does blocking `load_all`, `save` and spill writes on an async runtime worker while holding a std `Mutex` | `src/app.rs:3453, 3587, 3619` | blocks a runtime worker on file I/O mid-stream | snapshot under the lock, write outside it (or `spawn_blocking`) |
| P8 | `state.cache`, `shell.js`'s `documents`, the spill directory and `persistFileTree`'s stored path list all grow without a cap | `ui/app.js:235, 2914`; `ui/shell.js:653`; `src/context.rs:205`; `ui/app.js:6511` | memory and `read_dir` cost grow with use, not with what is on screen | LRU-bound the transcript cache and open documents; prune spill by age; cap stored paths |
| F1 | `respond_approval` rejection is swallowed in **both** answer paths; the modal closes regardless | `ui/app.js:5271, 5278` | the user believes a tool was approved when the backend never heard it | surface the failure and keep the prompt open |
| F2 | `/export` is defined twice, so the ZIP row is unreachable by typing (only the palette row reaches it) | `ui/app.js:3273, 3279, 3483, 4227` | a shipped feature that the natural gesture cannot reach; also zero test coverage | rename the second to `/export-zip`, add a page check |
| F3 | `delete_session`, `stop_streaming`, `restore_plan`, `load_session`, `list_capabilities`, `list_profiles` failures are all swallowed, and "no plan" / "no profiles" / "no match" copy is shown for an error | `ui/app.js:2643, 2907, 2922, 3095, 3110, 3372, 5155` | an error is reported as a state of the data, which is worse than a spinner | one shared error-row helper; keep the last good render |
| F4 | Removing a workspace resets `active_workspace` to `workspaces[0]` unconditionally | `src/app.rs:2668` | removing an unrelated workspace can switch the workspace the user is working in | keep the active id when it still exists |
| F5 | `prompts/get` is implemented in the GUI (`mcp_get_prompt`, catalogue Fetch button) but has **zero** coverage — no bridge handler, no check | `src/app.rs:3334`, `ui/app.js:3771` | a real command nothing drives; the doc still says it does not exist | bridge handler + one live check |
| F6 | The goal editor (`set_goal` through the pane's save/clear/block form) has a bridge handler but no check ever calls it; the same is true of `purge_trash`, the provider list and `forget_allowed_tool` | `ui/app.js:2804, 2866, 4312, 4832`; `tests/live/bridge.js:1060` | the M3 acceptance list claims these work; nothing has driven them | one live check per surface |
| D1 | §5's parity "today" column and the §6 milestone rows disagree about M7/M10/M11/M12/M13/M14 and the right dock, inline images and schedules | `docs/gui-requirements.md` §5/§6 | the matrix is the document's headline claim, and it contradicts its own detail table | refresh §5 from the guards, not from memory |
| D2 | §24.9 documents the old flat `Preset` struct; the code holds `Preset { id, name, workspace }`, and the promised "customized" marker does not exist | `docs/gui-requirements.md:1959`; `src/settings.rs:657`; `ui/app.js:671, 1183` | a preset that diverged is indistinguishable from one that did not | store `from_preset` and render the marker, or correct the promise |
| D3 | §24.4 promises a workspace menu with rename / set-root / duplicate / remove; the menu has New session / Rename / Delete | `docs/gui-requirements.md` §24.4; `ui/app.js:199-205` | three menu promises, two of them absent | add Duplicate (the preset copy path exists) and set-root, or correct the list |
| D4 | The M8/Q1 milestone row says `prompts/get` does not exist and that per-session skill activation is still open; both are false in this tree | `docs/gui-requirements.md:703, 746` | stale claims in the matrix a reviewer reads first | strike both, then cover F5 |
| D5 | Rev 106's stated preset limit ("cannot carry MCP switches yet") is false — `save_preset` clones the whole `Workspace` | `docs/gui-requirements.md:913`; `src/settings.rs:406-408, 657-666` | a "limit" that discourages a feature that already works | correct the limit |
| D6 | §7's "still open: workspace grouping in the sidebar" and rev 49's "a hit row does not yet show which workspace it came from" are both implemented | `docs/gui-requirements.md:779, 853`; `ui/app.js:274-300, 6015` | stale debt hides real debt | strike both |
| D7 | The revision prose references rev 121/122/123 while the revision table ends at 109 | `docs/gui-requirements.md` §6/M8 | the document's own history is ambiguous about which revision did what | renumber the prose to the table (or vice versa) in one pass |
| D8 | `zip_selftest.sh` exists but is absent from `AGENTS.md`'s guard table and from every workflow; no guard except WDIO runs in CI | `AGENTS.md`; `.github/workflows/gui-e2e.yml` | ten local guards protect a tree that CI never checks | add a CI job running every `tests/*_selftest.sh`, and list `zip` in the table |

Deliberately **not** queued: the finding that fewer than ten of the hundred commands are
`async fn` and that a single `Mutex<Inner>` serializes the rest. That is the architecture
this crate chose on purpose — one lock, one owner, no partial state — and the fix is a
redesign (per-store locks, I/O off the command thread), not a patch. It is recorded as the
reason P1/P6/P7 are worth doing first: they are the *work* that redesign would move, so
making them cheap is useful whichever way the lock goes.

### 24.15 The workspace's own `.bos` folder (like `.vscode`)

A workspace's configuration had exactly one home: `~/.bos/gui/settings.toml`, the
app's user-level registry. That is the right place for "which workspaces exist", and
the wrong *only* place for "how this folder runs": a project's settings belong next
to the project, where they can be read, edited, diffed, committed, and carried to
another machine with the code.

**What happens now.** Adding a workspace **creates** `<root>/.bos/` and writes
`workspace.toml` in it *before the record exists anywhere*, so a folder that cannot
take one is refused with the system's reason and nothing is added — the creation
dialog keeps the refusal path it already had (§24.11), with one more thing to refuse.
Editing a workspace rewrites the same file, and a write that fails puts the record
back, so the app and the folder cannot disagree about what was saved. The Settings
dialog is a third door — it copies the globals onto the workspace in use — and that
save reaches the folder as well, because a registry that moved on while the file kept
the older answer would be silently reverted at the next load. Applying a
preset writes it too, except when the preset's root is not a folder: `apply_preset`
has never validated a root, and a preset saved from a folder that has since moved
must still apply. Loading folds each workspace's file over the registry copy, so a
folder cloned with its `.bos` arrives configured. Removing a workspace leaves the
folder alone, the way removing a project from an editor leaves `.vscode`. The one
root that gets no folder is the **empty** one — the dialog's "empty = current
folder" — because the directory the app was launched in is not one the user picked:
nothing is written into it, and (the same rule read the other way) nothing is read
from it either, so a stray `.bos` there cannot configure the workspace that stands
for "here".

**Field by field: the file wins, and absent means inherit.** Every key is optional,
and the two states are different — a key the file does not have leaves the registry's
value in place, one it has replaces it. The file carries the model, the base URL, the
system prompt, temperature and reasoning effort, the bash and file-tool switches, the
approval gate, the context budget, the instruction-folding switch, the memory switch
and its file, the skills directory, the three deny-lists, and which MCP servers are
on.

**Three rules make that safe inside someone's project directory.**

- *Identity never travels.* `id`, `name`, `root` and the timestamps stay in the
  registry: the file says how a folder runs, not which records point at it, so the
  same file in a clone is a configuration rather than a second workspace.
- *Secrets never travel.* The API key, the fallback providers (which carry keys of
  their own) and the MCP **entries** — whose `args` and `url` can hold a token —
  are not written. What travels about MCP is the list of **names** that are on, so a
  file can switch an existing server on or off and can never introduce a command this
  installation did not already have. The file is therefore safe to commit.
- *A file is input, never an instruction.* One that will not parse, a field it does
  not carry, and an MCP name this installation does not know are each ignored rather
  than fatal: the app must not fail to load because a hand-edited file next to
  someone's code has a typo.

Writing is atomic — a sibling temporary file, then a rename — so a crash mid-write
cannot leave a half-parsed file where the next load reads it.

**Evidence.** `crates/gui/src/workspace_dir.rs` owns the format, and its seven tests
cover the round trip, the absence of every secret *and* of the identity keys (checked
as TOML keys, not as substrings that a field name might contain by accident), a
missing or unparsable file leaving the record alone, an MCP name that matches nothing,
the refusal when a plain file sits where `.bos` has to go, a second write replacing
the first with no `.tmp` left behind, and the empty root being skipped in both
directions — that last check owning the path it asserts about, and failing loudly if
the skip ever regresses, because the write would land in the folder the tests were
started from. `app::tests` drives the three
composed promises — a creation that prepares the folder before it records anything, an
edit that rewrites the file or puts the whole record back (the model included, because
the runtime is applied before the write and the backup is taken before that), and the
globals push a settings save performs, checked by reading the file the folder actually
got — through `create_workspace_record`, `rewrite_workspace` and
`push_globals_to_active_workspace`, which exist precisely so a test can reach them
without rewriting the user's registry. `settings::tests` loads
a real file from disk and asserts that the folder's value wins while a key the file
does not carry stays. The dialog says what will happen before the folder is picked
(`.ws-add-desc` and `ws.add.bos`, both languages), and the sentence it used to
carry — "Nothing is changed in the folder itself" — is gone because it is no longer
true. `markup_selftest.sh` owns the replacement as the sentence a reader gets rather
than as a key: the claim it deleted was false for as long as nobody checked it.

**Limits.** A workspace that existed before this feature gets its folder on the next
edit, not on the next launch: startup must not write into folders nobody asked it to
touch. A folder that cannot take a `.bos` cannot be *added* as a workspace (nor have
a root moved into it), which is the same strictness §24.7 applies to a root that is
missing or is not a directory; a *preset* whose root is gone still applies and simply
skips the write; and an empty root is skipped rather than resolved to the process
working directory, so "add a workspace and leave the folder blank" neither writes to
nor reads from wherever the app happened to start — the older empty-root meaning
survives for the tools, not for the configuration. The launch directory is not a
stable identity for a workspace, and that is the stronger half of the reason: a file
written there would be picked up by the next launch, or not, depending on the
directory the app was started in, so the workspace's configuration would follow the
*invocation* rather than the workspace. The workspace pane does not print the resolved path — the dialog
names the folder, and the pane would have to guess a platform separator. Sessions,
spills, exports and the MCP policy store stay in `~/.bos/gui/`: this section moves
the workspace's *configuration*, not the app's database.

