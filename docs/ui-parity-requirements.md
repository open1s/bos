# UI parity requirements: BSH (bos gui) ↔ DeepSeek Harness (DSH)

Goal: mirror DSH's UI in BSH wherever BSH has a real concept behind it — at a
granularity an agent can implement directly. Every requirement below names the
element, its order, its label (en/zh), its data source, its interaction, and
the guard that proves it. Nothing here may be faked: a control whose data does
not exist yet is listed in §5 with its backend gap, not stubbed.

## 0. How to read this document

- **Status values**: ⬜ missing in BSH → implement per the block below;
  🔄 in flight in the shared working copy (another agent's middle-panel fork —
  do NOT re-implement); ✅ already at parity; ⏭ deliberate skip (reason given);
  🩹 bug filed separately (issue id).
- **Each requirement block** is self-contained: `ID · P0/P1/P2 · surface`,
  Reference (DSH evidence), Spec (markup / visual / behavior / i18n / data),
  Acceptance (guard layer + assertion + falsification), Cross-refs.
- **Implementers must obey §2** (repo conventions) — the guards will fail
  otherwise.

## 1. Evidence

| Source | Path / command | What it gives |
|---|---|---|
| DSH live a11y dump | `ocu call get_app_state --args '{"app":"DeepSeek Harness","text_limit":"max"}'` → saved `/var/folders/xv/k4hxm0x92djb628tw580jvw00000gn/T/opencode/dsh_full.txt` (3 004 lines) | every control, its role, name and order (element numbers cited as `dsh:NNN`) |
| DSH streaming screenshot | `/var/folders/xv/k4hxm0x92djb628tw580jvw00000gn/T/opencode/dsh_main.png` | visual layer: geometry, colors, Working badge, Deep-diving line, goal bar, status bar, composer foot |
| Region crops + PIL measurements | same dir: `crop_header.png`, `crop_sidebar_top.png`, `crop_sidebar_rows.png`, `crop_transcript.png`, `crop_bottom.png`; method = patch-medians (colors), text-band profiles (row pitch), border-run scans (box rects) | §3B's numbers |
| BSH screenshot + dump | `ocu call get_app_state --args '{"app":"BOS"}'` → `bos_shot.json` / `bos_main.png`, `bos_full.txt` (108 lines) | BSH's current control inventory + geometry |
| Issue #17 | https://github.com/open1s/bos/issues/17 | short-form diff + priorities; this document is its implementable expansion |

Capture both dumps fresh before implementing — BSH is under active development.

## 2. Repo conventions every implementer must obey

From `AGENTS.md`, enforced by guards:

1. **Flat design**: no `box-shadow` / `text-shadow` / `backdrop-filter` /
   `filter` (except `none`), no `gradient(`, no literal px border-radius — use
   `--radius-xs|sm|md|lg` (4/6/8/12) or `50%` for circles, no non-`transform`
   transition >120 ms. The reference's shadows are replaced by a 1px
   `--border-strong` edge (documented deviation, checked by
   `markup_selftest.sh` + `layout_selftest.sh`).
2. **Tokens only**: spacing `--space-1..5`, height `--control-h` (30px), font
   `--font-ui` (13px), colors `--bg/--sidebar/--card/--hover/--active/
   --border/--border-strong/--accent/--text/--dim/--faint/--error/--success/
   --warn` (`ui/style.css`).
3. **i18n**: every user-visible string goes through `tr()` / `data-i18n*`
   marks with en+zh (`ui/polish.js`); never hard-code text in JS or HTML.
4. **DOM**: `$` = `getElementById` — ids must be unique and present in
   `index.html`; user data via `textContent` only; no `innerHTML` with user
   content.
5. **Guards**: put each check in the lowest layer that can see it —
   `ui_selftest` (pure render), `markup_selftest` (static structure/i18n),
   `page_selftest` (shipped page + settled bridge), `layout_selftest`
   (geometry), `live_selftest` (stateful async). Every new check must be
   **falsified by injection** (remove the feature → check goes red → restore,
   verify byte-identical).
6. **Keyboard/a11y**: every new control is a real `<button>` with an
   accessible name; menus are `aria-haspopup="menu"`; popovers close on
   Escape and hand focus back.

## 3. Complete difference matrix

### Sidebar

| # | Element | DSH (evidence) | BSH today | Status |
|---|---|---|---|---|
| s1 | Primary **New Session** button under the logo | `dsh:7`, full-width `⊕ New Session` (`dsh_main.png`) | only inside the workspace row (`ws-act ws-new`, app.js:1437) | ⬜ **S1** |
| s2 | Global panels nav: **Plugins / Automation tasks / MCP 连接器** | `dsh:9-11` | none (doors live in Settings) | ⬜ **S2** |
| s3 | "Workspaces" section label + search / view-options / add-workspace icon buttons | `dsh:12-17` | ✅ same trio (`#sessions-search`, `#session-view`, `#workspace-add-primary`) | ✅ |
| s4 | Session list as an **outline tree**: expandable workspace rows, sessions nested & indented, folder icons | `dsh:18-41` (role `outline`, rows `(expanded)`) | flat groups under `.ws-head` (groupSessions, app.js:407) | ⬜ **S3** |
| s5 | Per-row **Running** chip + spinner on a streaming session, age right-aligned | `dsh:23-26` ("Running · title · 59min") | only `vh` text "streaming" (app.js:1486); age exists | ⬜ **S4** |
| s6 | Bottom **Account menu** popup (avatar + name) | `dsh:42` "Account menu" | letter avatar + "Local workspace" + separate Settings gear | ⬜ **S5** |
| s7 | Search field placeholder "Search session names" | `dsh:15` | "Search chats…" | ⏭ keep BSH wording (s13 tracks correctness, not wording) |
| s8 | Collapse-sidebar button at sidebar's top-right | `dsh:6` | `#sidebar-toggle` in the chat header + ⌘/Ctrl+B | ⏭ keep (equivalent door, one owner — rev pqunloww) |
| s9 | Brand: deepseek wordmark + HARNESS pill | screenshot | BOS mark + BOS + HARNESS pill | ⏭ own brand |

### Header (centre column)

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| h1 | Session title (bold, single line, truncate) | `dsh:46` | `#chat-title` | ✅ |
| h2 | "1 subagent" popup | `dsh:47` | none | ⏭ no subagent surface in BSH yet (revisit with M7+ delegation UI) |
| h3 | "PTC mode" chip | `dsh:48` | none | ⏭ no BSH concept maps 1:1; composer already shows the real access mode (`Workspace Write`) — do not fake |
| h4 | "4 background jobs ⌄" | `dsh:49` | `#jobs-chip` (landed, opens Settings→Jobs) | 🔄 fork: dropdown listing jobs instead of navigating — see **H1** |
| h5 | Search split button, "Open in Zed", "More ways to open" | `dsh:50-51` | none | ⏭ editor integration BSH does not have; header search semantics unconfirmed in reference — confirm before building |
| h6 | "More actions" popup (⋯) | `dsh:52` | `#chat-more` (landed) | 🔄 fork |
| h7 | "Open right sidebar" | `dsh:53` | `#doc-toggle` ▤ (relocated to the control row by the in-flight change) | 🔄 fork |
| h8 | Chat \| Trajectory tab strip (selected = accent underline) | `dsh:54-56` | `#tab-chat` / `#tab-trajectory` | ✅ |
| h9 | idle/streaming chip, ◎ goal chip, Theme, EN | none in DSH | `#stream-chip`, `#goal-open`, theme, lang | ⏭ BSH extras — keep; (goal chip revisit with **G1**) |
| h10 | "Deep diving for {d} ···" live line | screenshot + `dsh:2892-2893` — sits at the **bottom of the transcript**, above the goal bar, accent-colored | none | ⬜ **H2** (listed here because it reads as header chrome; implemented as transcript footer) |

### Transcript

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| t1 | "Load earlier" | `dsh:61` | exists (rev 74 window + spacer) | ✅ |
| t2 | User row: timestamp, Copy, image thumbnails | `dsh:62-66,101-105` | Copy/Edit/Delete row (38-40) | ✅ (BSH keeps Edit/Delete — BSH extra) |
| t3 | Per-tool-call rows: `# Code · title`, `✎ Edit · path`, `>_. Bash · cmd`, `⊞ Think · …` (icon + kind + one-line summary; paths monospace dotted-underlined; indented; expandable) | screenshot + `dsh:78-233` | collapsed `.tool-head` cards per tool | ⬜ **T1** (restyle, keep behavior) |
| t4 | `<｜DSML｜ calls>` markers | throughout | — | ⏭ DeepSeek-proprietary marker; BSH renders its real tools instead |
| t5 | "Analysis completed" chip after reasoning | `dsh:76` | reasoning folds into the message | ⬜ optional **T2** (only if BSH message model carries a reasoning-complete event) |
| t6 | Turn footer: timestamp · Good/Bad toggles · "Branch into a new conversation" | `dsh:387-390` | Copy / Regenerate / Delete / Branch (57-60) | ⬜ **T3** (add Good/Bad; keep Regenerate/Delete — BSH extras) |
| t7 | "Continuing goal" marker on the first turn after a goal advance | `dsh:943` | none | ⬜ **T4** (P2) |
| t8 | Run-state chips: `Running` / `Completed` / `Stopped` per turn | `dsh:67,565-566,2888` | stream-chip covers live state only; "Completed in" row 🔄 fork | ⬜ **T5** (Stopped needs backend gap §5.3) |
| t9 | "Completed in {d}" disabled-pill under a finished turn | `dsh:68,947` | 🔄 fork (`turnDurationText`, app.js:2412) | 🔄 do not re-implement |
| t10 | Per-turn **Usage N tok** popup | `dsh:391,558,1133` | none (meter lives in the status bar) | ⬜ **T6** (P2) |
| t11 | Inline images: "image.png, click to view original" + "View full image" | `dsh:62,235` | inline render, no viewer | ⬜ **T7** (P2: click → doc-panel preview via existing `Shell.openDocument`) |
| t12 | Floating **Working** badge (right side, above composer): session title, "Working", stop + dismiss | screenshot | none | ⬜ **W1** |
| t13 | Mascot avatar (animated brand character) top-right while streaming | screenshot | none | ⬜ **W2** (P2, static own-brand SVG) |

### Goal bar

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| g1 | Compact one-line bar above the composer: `◎ Ongoing Goal · objective(truncate) · ⓘ · ✎ · 🗑` | `dsh:2894-2900` + screenshot | full M3 card toggled by `#goal-open` header chip (app.js:3263-3390) | ⬜ **G1** (card stays as the expanded detail) |

### Composer

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| c1 | Foot order: `⊕` (Add files or run commands, **popup**) · `🛡 Workspace Write ⌄` · `+ Skill` … `model ⌄` · circular send/stop | `dsh:2903-2907` + screenshot | `+ attach` (plain button) · mode popup · skill popup · model popup · glyph Send | ⬜ **C1** (⊕ becomes a popup; send becomes circular — **C2**) |
| c2 | Placeholder one-liner: "Message or run a task, / commands, @ files or sessions" | `dsh:2902` | long multi-line placeholder incl. Enter hints | ⬜ **C3** (P2: shorten to reference wording; move Enter hints into `title`/`aria-describedby`) |
| c3 | Model shown as short name + chevron ("deepseek-v4.1-flash") | `dsh:2906` | full id `nvidia/z-ai/glm-5.3-flash` | ⬜ **C4** (P2: display after last `/`, full id in title+aria) |
| c4 | Stop-generating while streaming (button becomes stop) | `dsh:2907` | ✅ same button swaps Send/Stop | ✅ |

### Status bar (centre column bottom)

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| b1 | Popup: `⊙ {n} turns {n} steps · {n} tok/s` | `dsh:2908-2910` | one dense left/right text line (`#status-left`/`#status-right`, app.js:2739) | ⬜ **B1** |
| b2 | Popup: `▤ {n} tok · Cache hit {n}%` | `dsh:2911-2913` | `↑ {p} ↓ {c} tok` span only | ⬜ **B2** (cache-hit = §5.4) |
| b3 | Popup: `{n}% of context used` | `dsh:2914` | `ctx ~{used}/{budget}` span | ⬜ **B3** |
| b4 | BSH-only notices: compacting…, mcp n/n, errors, archived, token meter | — | present | ⏭ keep — parked at the bar's left end (documented deviation) |

## 3B. Layout & style spec (pixel-measured)

The parity bar for **geometry, colors and type** — 布局和风格尽量一致 means
matching these numbers, not just having the same widgets. Measured 2026-10-12
with PIL from `dsh_main.png` (1280×918, light) vs `bos_main.png` (1280×846,
light, doc panel docked): patch-medians for colors, row-darkness profiles for
text bands, border-run scans for boxes (crops in §1). All DSH values are
**window px**; re-measure before implementing — captures go stale.

Both apps have the same centre-panel width when the doc panel is closed
(1280 − sidebar), so **absolute px are directly comparable**; targets below
are absolute. BSH's native title strip (≈34px, removed by **H0**) offsets
every DOM y relative to these until then — `layout_probe` rects are
webview-relative.

### 3B.1 Window & global geometry

| Metric | DSH measured | BSH today | Target |
|---|---|---|---|
| Sidebar width | **348** | 304 in the running build, token `--sidebar-w: 280px` (reconcile source vs build) | **348px** |
| Title strip | none — traffic lights live in the sidebar top row (y22–40); in-app header starts at y0 | native strip y0–34 carrying "BOS" | **H0** overlay titlebar |
| Content-column inset (title, tabs, transcript text, goal bar, deep-dive line) | **≈64–66** (x=414 with panel at 348) | 28 (x=332 with panel at 304) | **64px** |
| Composer inset | **≈40** (x=388–1231, + scrollbar allowance right) | 28 | **40px** |
| Status bar | one centred line (text x521–1097, centre 809 ≈ panel centre 814) | 3-part split at the insets | centred (B1–B3) |

### 3B.2 Sidebar

| Metric | DSH measured | BSH today | Target |
|---|---|---|---|
| Sidebar bg (light) | vertical gradient `#dee3eb` (top) → `#e8e9eb` (middle) → `#e1e3e7` (bottom) — translucent material | `#f6f7f9` | solid **`#e8e9eb`** (flat rule forbids the gradient — documented deviation) |
| Brand row | logo + wordmark + HARNESS pill, y76–105 | equivalent | ✅ keep |
| New Session button | x18–328 (**w≈311, h≈34**), fill `#f2f3f5`, **no border**, radius ≈8, icon+label centred, top y≈134 (≈30 below the brand) | absent | S1 with these numbers |
| Global-panels nav | 3 rows, pitch **49**, icon x≈36, label x≈66, y205–320, font ≈15 | absent | S2 |
| "Workspaces" header | y355–385; label x36 muted; icons right-aligned (x≈252/296/338, last ≈12 from edge) | same pattern, repositions | ✅ pattern |
| Tree rows | single-line, pitch **44** (trinno 416 → fix 460 → pwe 504 → bos 548 → selected 592 → llm 636 → 升级 680); folder icon x≈22, label x≈64; session text aligned to the label column; age right-aligned muted 12px | one-line rows, pitch ≈30, font 13–14 | pitch **44**, font **15**, DSH indents |
| Selected row | neutral pill `#dddee1`, x16–330, radius ≈8; spinner in the icon slot while running | tinted pill + accent edge | `#dddee1`, no accent stripe |
| Account row | avatar + name, y864–893 (popup `dsh:42`) | letter avatar + gear | S5 |

### 3B.3 Header

| Metric | DSH measured | BSH today | Target |
|---|---|---|---|
| Title row | y0–50; title **≈16px bold** at the 64 inset (x414); secondary chips on the **same row** (x≈565+, 13px muted: `1 subagent ⌄ · PTC mode · 4 background jobs ⌄`); right controls end ≈12 from the edge | y34–78 below the strip; chips right = idle/goal/mic/EN/⋯/panel (BSH extras ⏭ h9) | one row at the top (post-H0), inset 64, right pad 12 |
| Tab strip | y55–95: active tab accent text + **2px** accent underline (y93–95); full-width hairline divider below (y94, `#fafafa`) | y81–105, blue underline | ✅ verify 2px + hairline divider |

### 3B.4 Transcript

| Metric | DSH measured | BSH today | Target |
|---|---|---|---|
| Text column | x414–1200 (≈786 wrap width) | x332–945 | inset 64 |
| Body type | ≈14–15px, line pitch ≈24 | 14 / 24 | keep (parity) |
| Tool rows | flat one-liners `icon · kind · summary`, indent **+64** (x478), pitch **30**, muted `≈#81858d`, paths monospace + dotted underline | `.tool-head` cards | T1 with this geometry |
| Deep-diving line | y667–683 at the 64 inset, accent `#4176e6`, rocket icon + trailing `⋯`; ≈27 above the goal bar | absent | H2 |
| Working badge | floating card ≈x988–1253, y295–355 (**w≈265, h≈60**), white, radius → `--radius-lg`, bold title + muted "Working", ⟲/■ round buttons ≈36, ✕ at the top-left corner | absent | W1 |
| Mascot | ≈x1185–1275, y125–215 while streaming | absent | W2 |

### 3B.5 Boxes: goal bar / composer / status

| Box | DSH measured | Target |
|---|---|---|
| Goal bar | x413–1206, y710–754 → **h≈44**; fill `#f8f9f9`, 1px `--border`, radius ≈12; ≈27 below the deep-diving line | G1: inset 64, h 44 |
| Composer | x388–1231 (**w≈843**), y766–882 → **h≈116**; white, 1px ≈`#ebebeb`, radius ≈16; placeholder zone ≈59, foot row y829–871 (h≈42), bottom pad ≈11 | C1: inset 40, h≈116, foot order per C1 |
| Goal → composer gap | 754 → 766 | **12** |
| Composer → status | 882 → status text 892; status text h15; bottom margin ≈11 | ≈10 / ≈11 |
| Send button | **42×42 circle**, fill `#4176e6`, white glyph; inset 16 right, 12 bottom | C1 (today: 32px grey ↑) |
| Foot chips | ⊕ circle h≈32; `Workspace Write` / `+ Skill` pills h≈30–34 | `--control-h` 30 (±3 ok) |

### 3B.6 Color map (light theme)

| Role | DSH measured | BSH token now | Target |
|---|---|---|---|
| Accent (tab / links / send / deep-dive) | **`#4176e6`** | `--accent: #3558d6` (`--focus-ring` is already `#4176e6`) | `#4176e6` |
| Primary text | `#0f1115` | `--text: #1b1d21` | `#0f1115` |
| Muted text (tool rows / meta) | `≈#81858d` | `--dim #5c6270`, `--faint #8a90a0` | `--dim ≈ #6b7280`, keep `--faint` |
| Sidebar bg | `#e8e9eb` (gradient mid) | `#f6f7f9` | `#e8e9eb` |
| Selected row | `#dddee1` | `--active #e2e6ee` | `#dddee1` |
| New Session fill | `#f2f3f5` | — | `#f2f3f5` (S1) |
| Goal fill | `#f8f9f9` | — | `#f8f9f9` (G1) |
| Page / card bg | `#ffffff` | `#ffffff` | ✅ |
| Borders | composer ≈`#ebebeb`, goal `#ededed`, divider `#fafafa` | `--border #d8dbe2`, `--border-strong #b3b8c4` | keep tokens for interactive chrome; the two boxes above take a lighter edge ≈`#ebebeb` (add `--border-soft` if needed) |

**Dark theme**: DSH's dark variant was not captured (read-only rule — no
clicks in the user's session). Keep BSH's dark tokens; capture DSH dark first.

### 3B.7 Type scale

| Element | DSH | BSH now | Target |
|---|---|---|---|
| Header title | ≈16 bold (band h17) | ≈16 bold | ✅ |
| Sidebar rows / nav | ≈15 (band h15) | 13–14 | **15** |
| Body / transcript | 14–15, pitch 24 | 14 / 24 | keep 14/24 |
| Chips / status / meta | 12–13 | 13 (`--font-ui`) | ✅ |

**Tolerance**: ±2px and ±1 shade are acceptable (and unguardable). The deltas
that **must** close: 28→64 inset, 304→348 sidebar, 30→44 row pitch, and the
accent / sidebar-bg / selected-row colors. Verify each landed block with an
`ocu` screenshot side-by-side of both windows plus its guard.

## 4. Requirements

Priorities: **P0** = first iteration (visible every session, no backend gap);
**P1** = second; **P2** = polish. Each block is one implementation unit with
its own guard work. **Geometry, color and type targets for every block live in
§3B** — a layout block's guard asserts those numbers, and token-level color /
type changes ship as one commit guarded by the layout probe's computed styles.

---

### S1 · P0 · Sidebar New Session button

- **Reference**: `dsh:7` + §3B.2 — full-width button directly under the brand:
  x18–328 (**w≈311, h≈34**), fill `#f2f3f5`, **no border**, radius ≈8
  (`--radius-md`), `⊕` icon + label centred, top ≈30px below the brand,
  hover `--hover`.
- **Spec**:
  - Markup: `<button id="side-new-session" class="side-new" …>` inserted in
    `index.html` inside `.sidebar-head` after `.brand`; unique id; icon is an
    inline `<svg class="ico">` (plus-in-circle), `aria-hidden="true"`.
  - Label: `data-i18n="side.newSession"` → en **"New Session"**, zh
    **"新建会话"**; `aria-label`/`title` same text.
  - Behavior: click → the same path as the workspace row's `ws-act ws-new`
    (`newChat()`, app.js:3566) — creates a chat in the active workspace and
    focuses the composer. Shortcut stays ⌘/Ctrl+N.
  - States: disabled when no workspace exists (mirror the empty-registry
    state); focus ring = accent outline (matches existing buttons).
- **Acceptance**:
  - `page_selftest`: click `#side-new-session` → `create_session` called with
    `workspace === state.activeWorkspace`; composer focused after flush;
    disabled case: registry emptied → button `disabled` and click sends
    nothing.
  - `markup_selftest`: id appears exactly once; `data-i18n` mark present.
  - Falsification: remove the click handler → the page check goes red.
- **Cross-refs**: matrix s1.

---

### S2 · P2 · Sidebar global-panels nav

- **Reference**: `dsh:9-11` — three icon+label rows under New Session:
  Plugins (plug icon), Automation tasks (clock icon), MCP 连接器 (connector
  icon). Reference label is literally "MCP连接器" even in English UI; BSH
  ships the English/Chinese pair instead.
- **Spec**:
  - Markup: `<nav id="side-panels" aria-label="Global panels">` with three
    buttons `#nav-plugins`, `#nav-automation`, `#nav-mcp`, rows 30px,
    icon + label left-aligned, hover `--hover`, radius `--radius-sm`.
  - Labels: `nav.plugins` = Plugins / 插件; `nav.automation` = Automation
    tasks / 自动化任务; `nav.mcpConnectors` = MCP connectors / MCP 连接器.
  - Behavior: `navTo("cap-list")` (Tools & skills — owns plugins/skills),
    `navTo("jobs-section")`, `navTo("set-mcp")` (openSettings + focusSection,
    app.js:7164).
- **Acceptance**:
  - `page_selftest`: each button → `openSettings` fired and the target
    section has `aria-selected="true"` after flush (settle: one microtask
    pass as the harness does).
  - Falsification: point one button at the wrong section → red.
- **Cross-refs**: matrix s2.

---

### S3 · P1 · Sidebar tree outline semantics

- **Reference**: `dsh:18-41` — one `outline` ("Sessions") containing
  `(expanded)` workspace rows whose children are the session rows; keyboard
  model: ArrowRight/Left expand/collapse, Enter activates.
- **Spec** (structure only — grouping already exists):
  - `#session-list` gains `role="tree"`, `aria-label="Sessions"`
    (`side.sessionsTree` / 会话).
  - Workspace header (`.ws-head`) becomes `role="treeitem"`
    `aria-expanded="{!collapsed}"` `aria-level="1"`; the collapse control is
    the existing hide-toggle button (`Hide {name}'s chats`) with
    `aria-expanded` kept in sync.
  - Session rows: `role="treeitem"`, `aria-level="2"`, selected row keeps
    `aria-selected="true"`.
  - Empty workspace still seeds its group (already true — rev groupSessions)
    so the tree can show an empty branch.
- **Acceptance**:
  - `page_selftest`: computed ARIA roles/levels/expanded on a 2-workspace
    fixture (one empty); toggle → `aria-expanded` flips.
  - Falsification: strip `role="tree"` → red.
- **Cross-refs**: matrix s4; do not change the visual indent (keep current
  grouping look).

---

### S4 · P1 · Session row Running chip

- **Reference**: `dsh:23-26` — a streaming session's row shows a spinner
  glyph + the word **Running**, then title, then age right-aligned.
- **Spec**:
  - In `renderSidebar()` (app.js:1464+): for every session with
    `streamOf(id)` (any session, not just active), prepend
    `<span class="row-live" data-i18n="row.running">` — en "Running", zh
    "进行中" — plus the existing spinner treatment; keep `vh` text for
    screen readers only if the visible chip carries the same word (label in
    name satisfied).
  - Age span stays right-aligned (exists).
- **Acceptance**:
  - `ui_selftest` (pure render): fixture with one streaming session in a
    non-active workspace → row renders the chip; idle row does not.
  - `live_selftest`: `scriptTurn` streaming (bridge pushes events) → sidebar
    row of that chat shows "Running"; after `stream-finished` → chip gone.
  - Falsification: gate on `state.activeId` only → the live check (stream
    started from another row) goes red.
- **Cross-refs**: matrix s5; depends on `state.streams` (exists).

---

### S5 · P2 · Sidebar account menu

- **Reference**: `dsh:42` — avatar row at the bottom-left is itself a popup
  button ("Account menu").
- **Spec**:
  - Keep the avatar + "Local workspace" visual, but wrap in
    `<button id="side-account" aria-haspopup="menu" aria-expanded="false">`.
  - Menu items (reuse the shared row-menu renderer): **Settings**
    (openSettings), **Keyboard shortcuts** (`ui-shortcuts` dialog opener —
    the same control as `View shortcuts` in `shell.js:1186-1202`), **Theme**
    (cycle — mirrors the header control), **Language** (toggle en/zh).
  - Escape closes and returns focus to `#side-account` (same contract as
    every menu in this app).
  - The standalone Settings gear button remains (documented deviation —
    reference has no gear).
- **Acceptance**:
  - `page_selftest`: open → 4 items; Settings item → settings visible;
    Escape → menu gone, focus restored.
  - Falsification: omit focus restore → red.
- **Cross-refs**: matrix s6.

---

### H0 · P2 · Window chrome: overlay titlebar

- **Reference**: §3B.1 — DSH has **no title strip**: the traffic lights sit in
  the sidebar's top row (y22–40) and the in-app header starts at y0. BSH shows
  a native strip (y0–34, "BOS") that pushes the whole layout down ~34px, so
  none of §3B's y-targets line up until this lands.
- **Spec**: `tauri.conf.json` → `app.windows[0]`: `titleBarStyle: "Overlay"`
  (macOS, `decorations` stays `true`) + transparent top so the traffic lights
  overlay the sidebar; the brand row then owns `--frame-top-clearance`
  (already 48px, cloned from DSH's contract). Drop the visible native title
  text (`hiddenTitle`) — `#chat-title` is the title; the app name stays
  reachable via the menu/dock.
- **Acceptance**: `layout_selftest` probe — `#chat-title` rect top < 44px in
  webview coords and the header row occupies y0–50. Falsification: restore
  default decorations → the probe goes red.
- **Risks**: native-only; verify on a clean rebuild (`make-dev-app.sh`) that
  dialogs/menus still clear the traffic lights (`--frame-overlay-top: 68px`
  already models this). If macOS fights it, keep the strip and move this row
  back to §8 as a documented deviation with the probe asserting the strip
  height instead.
- **Cross-refs**: §3B.1; replaces the old titlebar non-goal in §8.

---

### H1 · P1 · Jobs chip opens a dropdown

- **Reference**: `dsh:49` "4 background jobs ⌄".
- **Spec**: `#jobs-chip` (landed) gains a chevron and opens a popover
  listing `jobs_list` rows: title, state word, and a click-through to
  Settings → Jobs (`navTo("jobs-section")`) — the popover is a preview, the
  pane remains the manager. Reuse the Jobs pane's row renderer if it can be
  extracted pure; otherwise the popover shows name+state only.
- **Acceptance**: `page_selftest` — chip click → popover with the fixture's
  two jobs (`running, failed`), one row click → jobs section selected;
  `aria-expanded` toggles. Falsification: chip still calls navTo directly
  with no popover → red.
- **Cross-refs**: matrix h4; 🔄 coordinate with the in-flight fork first —
  if the fork already converts the chip, mark ✅ and skip.

---

### H2 · P0 · "Deep diving for {d} ···" live line

- **Reference**: screenshot — bottom-left of the transcript, directly above
  the goal bar, accent color (`--accent`), leading status glyph, then
  **"Deep diving for 7m 48s ···"**; `dsh:2892-2893` (a11y name repeats the
  same phrase). Visible only while a turn streams; disappears the moment it
  ends.
- **Spec**:
  - Markup: `<div id="stream-line" class="stream-line hidden" role="status"
    aria-live="polite">` inside the centre column, after `#messages`, before
    the goal bar.
  - Text: `tr("stream.deepDiving", "Deep diving for {d} ···")` with `{d}` from
    the existing `turnDurationText()`-style formatter (reuse
    `turnDurationText`, app.js:2412 — same 7m 48s shape); zh:
    **"深入思考 {d} ···"**.
  - Data: `state.streams[activeId]` exists → visible; timestamp = first
    stream event for this turn (store `streamStartedAt` when the stream
    begins); a 1 s `setInterval` repaints `{d}` while visible; the interval
    is cleared on `stream-finished` / stop (never leak — the perf soak
    counts work).
  - Idle: element `hidden`, interval cleared, `#stream-chip` in the header
    remains the idle/streaming indicator (BSH extra, kept).
- **Acceptance**:
  - `page_selftest`: start stream via bridge event → line visible with
    "Deep diving for"; `stream-finished` → hidden and timer cleared (assert
    by driving two streams and confirming no interval leak — expose
    `window.__streamLineTimers` or assert repaint count).
  - `live_selftest`: `/slow` turn → line present mid-stream, absent after.
  - Falsification: forget to hide on finish → live check red.
- **Cross-refs**: matrix h10.

---

### T1 · P2 · Tool-call rows in reference style

- **Reference**: screenshot — under an assistant turn, one indented row per
  tool call: kind glyph + kind word + `·` + one-line summary
  (`# Code · Fix the stale jobs_list counts…`, `✎ Edit · crates/…/live_checks.js`,
  `>_. Bash · Run the live guard…`, `⊞ Think · …`); file paths monospace with
  dotted underline; rows expand on click to the full args/output BSH already
  renders.
- **Spec**:
  - Restyle the existing `.tool-head` collapsed state to this row shape (do
    not change the expand/collapse logic, output rendering, or approvals):
    glyph = existing inline SVG per kind; summary = first line of the
    tool's display text, `clip()` ~100 chars; `path`-looking arguments get
    `code` font + `border-bottom: 1px dotted var(--faint)`.
  - Keep one row per tool call (BSH granularity already matches).
  - `<｜DSML｜ calls>` is skipped (t4).
- **Acceptance**:
  - `ui_selftest`: fixture tool cards render row shape; expand still reveals
    output; approval-gated card unchanged.
  - `markup_selftest`: no `innerHTML` with tool args (textContent only —
    existing check covers).
  - Falsification: remove the summary → ui fixture red.
- **Cross-refs**: matrix t3; largest visual item — do after P0 wave.

---

### T3 · P1 · Turn footer Good/Bad + keep BSH extras

- **Reference**: `dsh:388-389` — two toggle buttons after Copy,
  `Good response` / `Bad response`, exclusive toggle (one on → other off).
- **Spec**:
  - Add `#msg-good` / `#msg-bad` (scoped per row: `.msg-good`/`.msg-bad`)
    buttons to the assistant row footer next to the existing
    Copy/Regenerate/Delete/Branch; `aria-pressed` reflects state; exclusive.
  - Storage: message `meta.feedback = "good"|"bad"` via the existing message
    save path (transcript files already store `meta`); remove on third
    click (toggle off). No server sync (local, stated in the doc).
  - Order to match reference: Copy · Good · Bad · (BSH: Regenerate,
    Delete) · Branch.
- **Acceptance**:
  - `page_selftest`: click Good → `aria-pressed=true`, Bad → Good flips off
    and the saved message meta reads `"bad"` (read back through the
    harness's session fixture); toggle-off clears.
  - Falsification: drop the exclusivity → red.
- **Cross-refs**: matrix t6.

---

### T4 · P2 · "Continuing goal" marker

- **Reference**: `dsh:943` — pill "Continuing goal {time}" above the first
  turn after the goal advances.
- **Spec**: when `goal_action: advance` succeeds, append a marker record to
  the session's log (kind `goal_advanced`, timestamp) that the renderer
  draws as a centered pill (`tr("goal.continuing", "Continuing goal {t}")` /
  "继续目标 {t}") and skips in Trajectory counts. Depends on the M3 goal
  machine (`goal_action` exists).
- **Acceptance**: `page_selftest` — call the advance handler → marker row
  appears between turns; `live_selftest` optional (F6 already tracks goal
  bridge coverage).
- **Cross-refs**: matrix t7.

---

### T5 · P1 · Per-turn run-state chips

- **Reference**: `dsh:67` `Completed`, `dsh:566` `Stopped`, `dsh:2888`
  `Running` — a chip at the turn's footer-left stating how the turn ended.
- **Spec**:
  - States: `Running` while streaming this turn; `Completed` on normal
    `stream-finished`; `Stopped` when the finish carried a stop/cancel
    (backend gap §5.3 — until it lands, render `Stopped` only when the stop
    button was pressed for that turn, tracked client-side, and state the
    limitation in the row's `title`).
  - Labels: `turn.running` = Running / 进行中; `turn.stopped` = Stopped /
    已停止; `turn.completed` already exists ("Completed in …" — place the
    state chip **before** the duration pill, reference order).
- **Acceptance**:
  - `page_selftest`: normal finish → `Completed` chip; `failNext`-style stop
    → `Stopped`; streaming → `Running`.
  - Falsification: chip always "Completed" → stop case red.
- **Cross-refs**: matrix t8; backend gap §5.3 (issue #8); 🔄 coordinate with
  the fork's "Completed in" row (same footer).

---

### T6 · P2 · Per-turn Usage popup

- **Reference**: `dsh:391` `Usage 19.7M tok` — popup at the end of an
  assistant turn showing cumulative usage.
- **Spec**:
  - Row footer gains `<button class="msg-usage" aria-haspopup="dialog">`
    `tr("usage.total", "Usage {n} tok")` where `{n}` = session cumulative
    (same accumulator as B2) **plus** this turn's `prompt+completion` from
    `state.usage` (set at finish).
  - Click → popover: this turn (↑/↓ from last usage), session total, and the
    existing `token_meter` numbers if present (read once per open, never per
    repaint — flat/perf rule).
  - Hidden when the session has no usage yet.
- **Acceptance**: `page_selftest` — turn with usage → label matches
  `/\d+(\.\d+)?[KM]? tok/`; click → popover with both numbers; empty
  session → button absent.
- **Cross-refs**: matrix t10.

---

### T7 · P2 · Image attachments open a viewer

- **Reference**: `dsh:62` `image.png, click to view original` and
  `dsh:235` `View full image: <title>`.
- **Spec**: a message image renders as a button (thumbnail + filename);
  click → `Shell.openDocument({id: "img:"+…, title, path/dataUrl})` — the
  doc panel already displays text; for data-URLs render an `<img>` inside
  the document view (safe: `src` from our own payload, `alt` = filename).
- **Acceptance**: `page_selftest` — fixture message with one image → button
  present → click → document tab titled with the filename.
- **Cross-refs**: matrix t11.

---

### W1 · P0 · Floating Working badge

- **Reference**: screenshot + §3B.4 — floating card at the right edge of the
  centre column, measured ≈x988–1253, y295–355 (**w≈265, h≈60**, right inset
  ≈27 from the panel edge, ≈200 below the tab divider): ✕ (top-left corner),
  session title (clipped, bold), **Working** subtitle, and two round icon
  buttons (≈36px): stop (`▢`) — reference also shows a ↺ (retry) which BSH
  skips (no safe retry-of-a-running-turn concept) — dismiss semantics below.
  BSH pins it top-right (the reference may float with scroll — noted
  deviation, pinning is the stable choice).
- **Spec**:
  - Markup: `<div id="working-badge" class="working-badge hidden"
    role="status" aria-live="polite">` — children: `#working-close`,
    `#working-title`, `#working-state`, `#working-stop`.
  - Visual: `--card` fill, 1px `--border-strong` (replaces the reference's
    shadow — flat rule), radius `--radius-lg`, width 265px, absolutely
    positioned top-right inside the centre column (right ≈27px, top ≈200px
    below the tab divider; `z-index` below dialogs, above transcript).
  - Copy: title = active session title (`textContent`, clipped ~30 chars);
    state = `tr("working.badge", "Working")` / "工作中".
  - Behavior: visible iff `streamOf(state.activeId)`; ✕ hides the badge for
    the current stream only (stream continues; reappears on the next turn);
    stop button → `stopStream()` (same as composer stop).
  - Accessibility: buttons are real `<button>`s with names
    (`working.stopAria` = "Stop generating" / 停止生成;
    `working.closeAria` = "Hide the working badge" / 隐藏工作提示).
- **Acceptance**:
  - `ui_selftest`: render badge from a streaming fixture vs idle (hidden);
    dismiss flag suppresses it; stop button invokes the handed callback.
  - `live_selftest`: `/slow` turn → badge visible with "Working"; Stop →
    badge gone + turn stopped; dismiss mid-stream → badge hidden while the
    stream continues; next turn → visible again.
  - Falsification: tie visibility to `state.error` or forget the dismiss
    reset → live check red.
- **Cross-refs**: matrix t12.

---

### W2 · P2 · Brand avatar while streaming

- **Reference**: screenshot — the DSH mascot (animated cloud character) sits
  top-right of the transcript during a stream.
- **Spec**: static `icons/bos-harness-icon.svg` (already shipped) at 64px,
  top-right of the transcript, visible iff streaming, `aria-hidden="true"`
  (decorative), `pointer-events: none`. No animation beyond
  `prefers-reduced-motion`-safe opacity transition (≤120 ms).
- **Acceptance**: `page_selftest` — streaming → img present; idle → absent.
- **Cross-refs**: matrix t13.

---

### G1 · P0 · Compact Ongoing Goal bar

- **Reference**: `dsh:2894-2900` + screenshot + §3B.5 — a single-row bar
  directly above the composer (below the Deep-diving line, ≈27px under it,
  ≈12px above the composer): `◎ Ongoing Goal` (bold, fixed width) ·
  objective (one-line truncate) · right cluster `ⓘ` `✎` `🗑`. Measured:
  x413–1206 (content-column inset 64), **h≈44**, fill `#f8f9f9`.
- **Spec**:
  - Markup: `<div id="goal-bar" class="goal-bar hidden" role="region"
    aria-label="Ongoing Goal">` between `#messages` and the composer;
    children: label span (`data-i18n="goal.barLabel"` = "Ongoing Goal" /
    "进行中的目标" — matches the existing card's label), objective button
    `#goal-objective`, then `#goal-info` (ⓘ), `#goal-edit` (✎),
    `#goal-clear` (🗑).
  - Visual: goal fill `#f8f9f9` (token `--goal-bg` or reuse `--code-bg` if
    it matches), 1px `--border`, radius `--radius-lg`, height **44px**,
    inset 64 (same column as the transcript), objective
    `text-overflow: ellipsis`, icon buttons 24px `--control-h`-aligned,
    hover `--hover`. Flat (no shadow).
  - Data: `get_goal` (exists, M3) — bar `hidden` iff no goal. Reuse
    `refreshGoal()`'s existing paint pass (one repaint path, not two).
  - Behavior: objective click or `#goal-edit` → opens the existing editor
    card (focus its objective field); `#goal-info` → opens the M3 card
    expanded (state chip, rounds, blocker); `#goal-clear` → the card's
    existing delete confirm (do not bypass the confirmation); `#goal-open`
    header chip **stays for now** (deviation from the reference — revisit
    after the middle-panel fork lands).
- **Acceptance**:
  - `page_selftest` (extend the existing 6 goal-card checks): goal present →
    bar visible with objective text; no goal → hidden; ✎ → editor card
    visible + focused; 🗑 → `goal_action` delete called with the same
    confirmation flow as the card; ⓘ → card visible.
  - Falsification: paint the bar from `goalOpen` instead of the goal value →
    the "hidden when no goal" check goes red.
- **Cross-refs**: matrix g1; fixes the seat of issue #16's surface (card
  remains, bar becomes the persistent entry).

---

### C1 · P1 · Composer foot: ⊕ popup + circular send

Two sub-requirements; ship together (one foot row).

**C1a · ⊕ "Add files or run commands" popup**

- **Reference**: `dsh:2903` — the leading control is a *popup button*, not
  plain attach.
- **Spec**: replace `#attach-btn`'s direct file-dialog opening with a menu
  (`aria-haspopup="menu"`): items **Attach images…** (opens the existing
  hidden file input), **Mention a file** (inserts `@` into the composer and
  closes — reuses the mention picker), **Commands & skills** (inserts `/`
  — reuses the palette). Label: `composer.addFiles` = "Add files or run
  commands" / "添加文件或运行命令". The native file dialog itself remains
  untestable (documented skip) — the menu item is testable.
- **Acceptance**: `page_selftest` — open menu → 3 items; Mention → input
  value ends with `@` and mention picker open; Commands → palette open
  (the +Skill precedent at page_checks:2270); Attach → hidden file input
  `.click()` invoked (spyable). Falsification: first item bypasses the menu →
  red.

**C1b · Circular send/stop button**

- **Reference**: screenshot + §3B.5 — **42×42** circle, accent fill
  (`#4176e6`), white glyph, right inset 16 / bottom 12 inside the composer;
  while streaming it is the stop square (already).
- **Spec**: `.send-btn` becomes `width/height: 42px; border-radius: 50%;
  background: var(--accent); color: #fff;` disabled → `--card` bg +
  `--dim` glyph; keep `#send` id and `aria-label` ("Send (Enter)" /
  "停止生成"); transition ≤120 ms background only.
- **Acceptance**: `layout_selftest` — width == height == 42 at every probe
  width (1440/900/560/500), never overflows the foot row; `ui_selftest` —
  disabled/enabled/stop glyph states.
- **Cross-refs**: matrix c1.

---

### C3 · P2 · Placeholder wording

- **Reference**: `dsh:2902` "Message or run a task, / commands, @ files or
  sessions".
- **Spec**: placeholder → `tr("composer.placeholder", …)` with the
  reference sentence; zh: "输入消息或任务，/ 命令，@ 文件或会话"; move
  "Enter = send · Shift+Enter = newline" into the textarea's `title` (kept
  discoverable, not truncated mid-sentence on narrow columns — issue #14's
  wrap pressure).
- **Acceptance**: `markup_selftest` — placeholder equals the en string and
  the i18n mark exists; `page_selftest` — zh toggle swaps it.
- **Cross-refs**: matrix c2.

---

### C4 · P2 · Model chip shows the short name

- **Reference**: `dsh:2906` "Select model, current deepseek-v4.1-flash".
- **Spec**: visible text = segment after the last `/`
  (`glm-5.3-flash`); `title` + `aria-label` keep the full id
  ("Model: nvidia/z-ai/glm-5.3-flash"); popup list still shows full ids.
- **Acceptance**: `page_selftest` — chip text has no `/`, aria-label does;
  switching model updates both.
- **Cross-refs**: matrix c3.

---

### B1/B2/B3 · P1 · Status bar as three centered popups

- **Reference**: `dsh:2908-2914` — bottom of the centre column, horizontally
  centered group of three popup buttons separated by spacing (not dividers):
  1. `⊙ {turns} turns {steps} steps · {rate} tok/s`
  2. `▤ {total} tok · Cache hit {pct}%`
  3. `{pct}% of context used`
  Screenshot shows leading glyphs per popup (turn-circle, db, context-ring).
- **Spec**:
  - Markup: `<div id="status-bar" class="status-bar">` replacing the
    current left/right spans **inside the centre column**; three
    `<button class="stat-pop" aria-haspopup="dialog">` ids
    `#stat-turns`, `#stat-tokens`, `#stat-ctx`, each with an inline SVG
    glyph, `aria-label` mirroring its text.
  - Data:
    1. turns = user asks in this session; steps = tool calls this session
       (`state.toolCount`); tok/s = `lastCompletionTokens /
       lastStreamSeconds`, computed once at `stream-finished` (state field
       `state.tokRate`), shown as "—" before the first turn.
    2. total = session cumulative `prompt+completion` accumulated client-side
       at each finish (`state.usageTotals[sessionId]`, persisted in the
       session cache so a reload re-derives it from message meta if present —
       see §5.5); Cache hit = §5.4 backend, "—" until it lands
       (label still renders: `Cache hit —`).
    3. pct = `Math.round(estimateContext() / (state.budget||1) * 100)` —
       existing estimator, no new backend.
  - Popover on click (shared popover component): #1 → turn/tool breakdown
    (Trajectory hint), #2 → the existing `renderUsage()` meter + token_meter
    numbers (read once per open), #3 → `navTo("context-section")`.
  - Deviations kept at the bar's **left end** (outside the centered group):
    archived, compacting…, mcp n/n, errors — BSH-only notices (matrix b4).
  - i18n: `status.turns` "{t} turns {s} steps" / "{t} 轮 {s} 步";
    `status.rate` "{r} tok/s"; `status.tokTotal` "{n} tok";
    `status.cacheHit` "Cache hit {p}%" / "缓存命中 {p}%";
    `status.ctxPct` "{p}% of context used" / "已用上下文 {p}%".
- **Acceptance**:
  - `page_selftest` — **existing** status-bar checks must be updated to the
    new structure (they assert `#status-right` spans: usage meter, tools,
    ctx — move their assertions onto the popups; do not delete coverage).
    New: fixture with known numbers → popup texts equal the expected
    strings; click #3 → context section selected.
  - `layout_selftest` — the group stays centered and never wraps at
    1440/900/560; at 500 the left notices may wrap to the second line, the
    three popups must not overflow.
  - `ui_selftest` — pure render of all three from a stats fixture, including
    the "—" cases (no rate yet, no cache-hit backend).
  - Falsification: compute pct from a stale field → fixture red.
- **Cross-refs**: matrix b1-b3; **do this after H2/W1** — it restructures
  elements many existing checks touch.

---

## 5. Data & backend gaps (do not fake — implement behind these)

1. **Cache hit % (B2)**: the provider response's `cache_read_tokens` (and/or
   `cache_creation_tokens`) must be surfaced through the stream-finish
   payload into `state.usage`. Frontend renders "—" until then.
2. **Session-cumulative tokens (B2/T6)**: no all-time counter exists
   (`token_meter` estimates the live context). Approach: accumulate
   `prompt+completion` per session in message `meta.usage` at each finish so
   a reload re-sums; expose through `session_overview` later if needed.
3. **Turn `Stopped` state (T5)**: `stream-finished` lacks a reliable
   cancelled flag (issue #8 — file/extend there). Until fixed, the client's
   stop-button press is the only signal; the row's `title` states this.
4. **tok/s (B1)**: compute at finish (completion tokens ÷ stream wall
   time) — no backend change; keep the sample (last turn) like the
   reference's live figure, or average the last 3 turns if noisy.
5. **Good/Bad persistence (T3)**: message `meta` already round-trips — no
   backend change; no server-side sync exists (local-only, documented).
6. **subagent chip (h2), PTC mode (h3), Open in Zed (h5)**: no BSH concept —
   skipped in §3; do not stub.

## 6. Acceptance strategy (summary)

| Requirement | ui | markup | page | layout | live |
|---|---|---|---|---|---|
| S1 New Session | – | id+i18n | click→create_session | – | – |
| S2 nav | – | – | navTo targets | – | – |
| S3 tree | – | roles | expanded flips | – | – |
| S4 Running chip | fixture | – | – | – | stream in/out |
| S5 account menu | – | – | items+focus | – | – |
| H1 jobs dropdown | – | – | popover+nav | – | – |
| H2 deep-diving line | – | i18n | appear/hide+timer | – | slow turn |
| T1 tool rows | fixture | textContent | – | – | – |
| T3 Good/Bad | – | – | meta+exclusive | – | – |
| T5 state chips | – | – | 3 states | – | stop case |
| T6 usage popup | – | – | label+popover | – | – |
| T7 image viewer | – | – | tab opens | – | – |
| W1 Working badge | fixture | a11y names | – | – | slow/stop/dismiss |
| W2 avatar | – | – | in/out | – | – |
| G1 goal bar | – | i18n | 5 actions | – | – |
| C1a ⊕ menu | – | – | 3 items | – | – |
| C1b circle send | states | – | – | 42px@4 widths | – |
| C3/C4 wording | – | strings | zh swap | – | – |
| B1-B3 status bar | fixture | a11y | texts+popover | centered@4 | – |
| §3B tokens (accent/sidebar/selected/text) | – | – | – | computed styles@2 widths | – |
| §3B insets & boxes | – | – | – | inset 64/40, goal h44, send 42 @4 widths | – |
| H0 overlay titlebar | – | – | – | `#chat-title` top<44 | – |

Every new check: **write it red first** (remove the feature → confirm FAIL →
restore), per `AGENTS.md`. Long falsification batches: managed background
jobs, byte-snapshot every touched file, print the diff names at the end.

## 7. Phasing (implementation order)

1. **Wave 1 — P0, frontend-only, low collision** (each is one PR-sized
   change): §3B token commit (accent `#4176e6`, sidebar `#e8e9eb`/348,
   selected `#dddee1`, text `#0f1115`, sidebar type 15 / pitch 44 — the
   sidebar is free) → S1 → H2 → W1 → G1 → C1b (the 64/40 inset + box rects
   land in the same centre-column commit as G1/C1b — check the fork first).
2. **Wave 2 — P1**: S4, S3, T5 (coordinate with fork's footer), B1–B3
   (restructure + update existing checks), C1a, H2 interval hardening,
   H1 (if the fork hasn't taken it), T3.
3. **Wave 3 — P2**: H0 (native titlebar — verify on a clean rebuild), S2,
   S5, T1, T6, T7, W2, C3, C4, T4.
4. **Backend follow-ups** (§5): cache-hit surfacing, issue #8's cancelled
   flag, meta.usage accumulation.

Coordination: the middle-panel fork (working-copy change `nlymmoxm`)
owns the centre column's in-flight items (🔄 rows: h4/h6/h7, t9, and the
tool-row/t1 footer area) — check its landed state before starting any Wave-2
centre-column item; sidebar (S1–S5) and status bar (B1–B3) are free.

## 8. Deliberate non-goals (with reason)

| Reference element | Why skipped |
|---|---|
| `Open in Zed` / `More ways to open` (dsh:50-51) | no editor integration in BSH — faking a menu with dead items fails the "no phantom controls" rule |
| `PTC mode` chip (dsh:48) | no BSH concept maps; composer already exposes the real access mode |
| `1 subagent` popup (dsh:47) | no subagent surface yet — revisit with delegation UI |
| `<｜DSML｜ calls>` markers | DeepSeek-proprietary protocol artifact |
| Reference shadow/rounded-blur styling | BSH flat rule (guards fail on shadows) — border substitute |
| Mascot animation (W2 static only) | decorative motion must respect `prefers-reduced-motion` |
| ~~Traffic-light/titlebar layout~~ | promoted to **H0** (P2) — the missing strip is a real layout delta (§3B.1); if the overlay fails on macOS it returns here as a documented deviation |

## 9. Revision log

| Rev | Date | Change |
|---|---|---|
| 1 | 2026-10-12 | Initial: full diff matrix from live OCU captures (DSH 3 004-line a11y dump + streaming screenshot; BSH 108-line dump), 20 implementable requirements (S/H/T/G/C/B/W ids), backend-gap table, guard mapping, 3-wave phasing. Expansion of issue #17. |
| 2 | 2026-10-12 | Add **§3B layout & style spec** (pixel-measured from `dsh_main.png` vs `bos_main.png`: sidebar 348px + row pitch 44 + font 15, content/composer insets 64/40, goal/composer/status box rects, light-theme color map, type scale, tolerance rule); add **H0** overlay-titlebar requirement; correct S1's button measurements (fill `#f2f3f5`, borderless, h≈34); crops added to §1. |
