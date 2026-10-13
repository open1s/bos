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
| s1 | Primary **New Session** button under the logo | `dsh:7`, full-width `⊕ New Session` (`dsh_main.png`) | only inside the workspace row (`ws-act ws-new`, app.js:1437) | ✅ **S1** |
| s2 | Global panels nav: **Plugins / Automation tasks / MCP 连接器** | `dsh:9-11` | none (doors live in Settings) | ✅ **S2** |
| s3 | "Workspaces" section label + search / view-options / add-workspace icon buttons | `dsh:12-17` | ✅ same trio (`#sessions-search`, `#session-view`, `#workspace-add-primary`) | ✅ |
| s4 | Session list as an **outline tree**: expandable workspace rows, sessions nested & indented, folder icons | `dsh:18-41` (role `outline`, rows `(expanded)`) | flat groups under `.ws-head` (groupSessions, app.js:407) | ✅ **S3** |
| s5 | Per-row **Running** chip + spinner on a streaming session, age right-aligned | `dsh:23-26` ("Running · title · 59min") | only `vh` text "streaming" (app.js:1486); age exists | ✅ **S4** |
| s6 | Bottom **Account menu** popup (avatar + name) | `dsh:42` "Account menu" | letter avatar + "Local workspace" + separate Settings gear | ✅ **S5** |
| s7 | Search field placeholder "Search session names" | `dsh:15` | "Search chats…" | ⏭ keep BSH wording (s13 tracks correctness, not wording) |
| s8 | Collapse-sidebar button at sidebar's top-right | `dsh:6` | `#sidebar-toggle` in the chat header + ⌘/Ctrl+B | ⏭ keep (equivalent door, one owner — rev pqunloww) |
| s9 | Brand: deepseek wordmark + HARNESS pill | screenshot | BOS mark + BOS + HARNESS pill | ⏭ own brand |

### Header (centre column)

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| h1 | Session title (bold, single line, truncate) | `dsh:46` | `#chat-title` | ✅ |
| h2 | "1 subagent" popup | `dsh:47` | none | ⏭ no subagent surface in BSH yet (revisit with M7+ delegation UI) |
| h3 | "PTC mode" chip | `dsh:48` | none | ⏭ no BSH concept maps 1:1; composer already shows the real access mode (`Workspace Write`) — do not fake |
| h4 | "4 background jobs ⌄" | `dsh:49` | `#jobs-chip` (landed, opens Settings→Jobs) | ✅ **H1** (the chip is a popover of the pane's jobs; see §9) |
| h5 | Search split button, "Open in Zed", "More ways to open" | `dsh:50-51` | none | ⏭ editor integration BSH does not have; header search semantics unconfirmed in reference — confirm before building |
| h6 | "More actions" popup (⋯) | `dsh:52` | `#chat-more` (landed) | 🔄 fork |
| h7 | "Open right sidebar" | `dsh:53` | `#doc-toggle` ▤ (relocated to the control row by the in-flight change) | 🔄 fork |
| h8 | Chat \| Trajectory tab strip (selected = accent underline) | `dsh:54-56` | `#tab-chat` / `#tab-trajectory` | ✅ |
| h9 | idle/streaming chip, ◎ goal chip, Theme, EN | none in DSH | `#stream-chip`, `#goal-open`, theme, lang | ⏭ BSH extras — keep; (goal chip revisit with **G1**) |
| h10 | "Deep diving for {d} ···" live line | screenshot + `dsh:2892-2893` — sits at the **bottom of the transcript**, above the goal bar, accent-colored | none | ✅ **H2** (listed here because it reads as header chrome; implemented as transcript footer) |

### Transcript

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| t1 | "Load earlier" | `dsh:61` | exists (rev 74 window + spacer) | ✅ |
| t2 | User row: timestamp, Copy, image thumbnails | `dsh:62-66,101-105` | Copy/Edit/Delete row (38-40) | ✅ (BSH keeps Edit/Delete — BSH extra) |
| t3 | Per-tool-call rows: `# Code · title`, `✎ Edit · path`, `>_. Bash · cmd`, `⊞ Think · …` (icon + kind + one-line summary; paths monospace dotted-underlined; indented; expandable) | screenshot + `dsh:78-233` | collapsed `.tool-head` cards per tool | ✅ **T1** (glyph + kind + one-line summary; paths in the code face) |
| t4 | `<｜DSML｜ calls>` markers | throughout | — | ⏭ DeepSeek-proprietary marker; BSH renders its real tools instead |
| t5 | "Analysis completed" chip after reasoning | `dsh:76` | reasoning folds into the message | ⏭ optional **T2** - nothing in BSH says reasoning *ended*: a message carries the reasoning text, not the moment it stops, so the chip would state an event this backend never sends. See §8. |
| t6 | Turn footer: timestamp · Good/Bad toggles · "Branch into a new conversation" | `dsh:387-390` | Copy / Regenerate / Delete / Branch (57-60) | ✅ **T3** (add Good/Bad; keep Regenerate/Delete — BSH extras) |
| t7 | "Continuing goal" marker on the first turn after a goal advance | `dsh:943` | none | ✅ **T4** (P2) |
| t8 | Run-state chips: `Running` / `Completed` / `Stopped` per turn | `dsh:67,565-566,2888` | stream-chip covers live state only; "Completed in" row 🔄 fork | ✅ **T5** (Stopped now carries the backend's own `cancelled` flag - §5.3) |
| t9 | "Completed in {d}" disabled-pill under a finished turn | `dsh:68,947` | 🔄 fork (`turnDurationText`, app.js:2412) | 🔄 do not re-implement |
| t10 | Per-turn **Usage N tok** popup | `dsh:391,558,1133` | none (meter lives in the status bar) | ✅ **T6** (P2) |
| t11 | Inline images: "image.png, click to view original" + "View full image" | `dsh:62,235` | inline render, no viewer | ✅ **T7** (P2: click → doc-panel preview via existing `Shell.openDocument`) |
| t12 | Floating **Working** badge (right side, above composer): session title, "Working", stop + dismiss | screenshot | none | ✅ **W1** |
| t13 | Mascot avatar (animated brand character) top-right while streaming | screenshot | none | ✅ **W2** (P2, static own-brand SVG) |

### Goal bar

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| g1 | Compact one-line bar above the composer: `◎ Ongoing Goal · objective(truncate) · ⓘ · ✎ · 🗑` | `dsh:2894-2900` + screenshot | full M3 card toggled by `#goal-open` header chip (app.js:3263-3390) | ✅ **G1** (card stays as the expanded detail) |

### Composer

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| c1 | Foot order: `⊕` (Add files or run commands, **popup**) · `🛡 Workspace Write ⌄` · `+ Skill` … `model ⌄` · circular send/stop | `dsh:2903-2907` + screenshot | `+ attach` (plain button) · mode popup · skill popup · model popup · glyph Send | ✅ **C1** (⊕ becomes a popup; send becomes circular — **C2**) |
| c2 | Placeholder one-liner: "Message or run a task, / commands, @ files or sessions" | `dsh:2902` | long multi-line placeholder incl. Enter hints | ✅ **C3** (P2: shorten to reference wording; move Enter hints into `title`/`aria-describedby`) |
| c3 | Model shown as short name + chevron ("deepseek-v4.1-flash") | `dsh:2906` | full id `nvidia/z-ai/glm-5.3-flash` | ✅ **C4** (P2: display after last `/`, full id in title+aria) |
| c4 | Stop-generating while streaming (button becomes stop) | `dsh:2907` | ✅ same button swaps Send/Stop | ✅ |

### Status bar (centre column bottom)

| # | Element | DSH | BSH today | Status |
|---|---|---|---|---|
| b1 | Popup: `⊙ {n} turns {n} steps · {n} tok/s` | `dsh:2908-2910` | one dense left/right text line (`#status-left`/`#status-right`, app.js:2739) | ✅ **B1** |
| b2 | Popup: `▤ {n} tok · Cache hit {n}%` | `dsh:2911-2913` | `↑ {p} ↓ {c} tok` span only | ✅ **B2** (cache-hit = §5.4) |
| b3 | Popup: `{n}% of context used` | `dsh:2914` | `ctx ~{used}/{budget}` span | ✅ **B3** |
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

**Status (rev 3)**: the **token half** of this section has landed — accent, primary
text, muted text, sidebar background and selected-row color, the **348px** sidebar
token, the **44px** row pitch and the **15px** sidebar row type. The computed values
are asserted by `layout_selftest.sh` at four widths (9 named assertions; each was
falsified by reverting its own value and watching that line go red). The **inset
half** (content 28→64, composer 28→40) belongs to G1/C1b and has **not** landed.

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
- **Acceptance** (as built):
  - `page_selftest`: click `#side-new-session` → `create_session` called once
    more, the new chat is the active one, an active row is drawn, the composer is
    focused after a flush; disabled case: registry emptied → button `disabled`
    and a click sends nothing. The checks run **last** in the harness because
    they create a chat and the scenarios above count what the store holds.
  - **Deviation (documented, not faked)**: the block above asks the check to see
    `create_session` called *with* `workspace === state.activeWorkspace`. BSH's
    command carries **no arguments** — the workspace is the session's own binding
    in the backend (`app.rs` `fn create_session(state)`) — so the check asserts
    the *path* (identical to a workspace row's `+`) instead of an argument that
    does not exist. Adding an argument the backend ignores would be the fake.
  - `markup_selftest`: the row is in the head with the i18n marks; the id appears
    exactly once.
  - Falsification: remove the click handler → the page check goes red; ignore the
    empty registry → the disabled check goes red (batch `/tmp/falsify-s1.py`).
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
- **Refined during implementation**: `{n}` is not "session cumulative + this turn" - that would count the finished turn twice once the accumulator has absorbed it. The chip states the sum of the transcript's own per-turn `meta.usage` **through** that turn (`usageThrough`), which is the reference's meaning and the only version a reload can reproduce.
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
- **Refined during implementation**: a message stores its pictures as data URLs, which carry a media type and no filename at all, so the name the button shows - and the tab it opens - is derived from the payload as `image-<n>.<ext>` (`attachmentName`). It states what is true instead of inventing a file the reader never chose, and it stays stable across a reload because the payload is what was stored.
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
       (`state.toolCount`); tok/s = completion tokens over the turn's own wall
       time, computed once at `stream-finished` into `state.tokRate` — **done**.
       The denominator has to be the clock the turn kept: the row under the
       answer states whole seconds, which is "0s" for a short turn, so a rate
       derived from the row would divide by zero where a rate belongs.
       Renders "—" until a turn produces one.
    2. total = session cumulative `prompt+completion`, accumulated client-side
       at each finish into `state.usageTotals[sessionId]` (**done**), and
       re-derived on reload from message meta when a message carries it (§5.5).
       A total nobody has counted renders as "—", not 0: the number IS the
       statement that something finished, and a zero would be a false one.
       Cache hit = §5.4 backend, "—" until it lands (label still renders:
       `Cache hit —`, without the percent sign the dash cannot wear).
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
    the "—" cases (no rate yet, no cache-hit backend); **still owed**: the direct pure-render pass through `renderStatusBar`, which the page fixture covers for the texts and the live layer for the accumulation, but which this fixture cannot reach while it builds no popup DOM.
  - Falsification: compute pct from a stale field → fixture red.
- **Cross-refs**: matrix b1-b3; **do this after H2/W1** — it restructures
  elements many existing checks touch.

---

## 5. Data & backend gaps (do not fake — implement behind these)

1. **Cache hit % (B2)** — **done**. The provider's `cached_tokens` travels react
   (`StreamToken::Usage` -> `TokenUsage::with_cached`) -> react's `TokenCounter` ->
   the ReAct engine's `cumulative_cached_tokens()` -> `Agent::last_stream_cache_tokens()`
   -> the `stream-finished` payload's `cache_read_tokens` -> `state.cacheHit` =
   `round(cache_read / prompt * 100)`, stated by popup #2 and by the token meter.
   A provider that reports no breakdown leaves the count untouched and the foot states
   the dash, so "nothing was cached" and "not reported" stay distinguishable.
2. **Session-cumulative tokens (B2/T6)** — **done**. A turn's cost now rides on the
   message it answered with: `MessageMeta.usage: Option<MessageUsage>`
   (`prompt_tokens` / `completion_tokens`) is written by the finish handler from the
   agent's own count, persists with the transcript, and is re-summed on load - so a
   re-opened chat states what the session spent instead of a figure nobody measured.
   `usageTotal` prefers this process's accumulator and falls back to the transcript's
   own sums, and a session that has neither still says the dash rather than 0 tok. No
   `session_overview` field was needed for this.
3. **Turn `Stopped` state (T5)**: **done**. `stream-finished` now carries `cancelled`,
   and the client leads with it (the press remains a second source, so a backend that
   predates the flag is not misreported). The flag's meaning is narrow on purpose - the
   user's stop reached *this* turn, matched by generation - because a turn the runner
   ended for another reason is not "stopped" and the engine does not report that
   question. `vars_main`'s second half: the flag travels the same tap the token count
   does, so nothing new had to be wired.
4. **tok/s (B1)**: compute at finish (completion tokens ÷ stream wall
   time) — no backend change; keep the sample (last turn) like the
   reference's live figure, or average the last 3 turns if noisy.
5. **Good/Bad persistence (T3)**: **done — and this line was wrong when it was
   written.** It claimed message `meta` "already round-trips — no backend
   change"; in fact `ChatMessage` had no `meta` field and no command the
   UI could reach wrote one, so T3's UI half was in-memory only. It round-trips
   now: `ChatMessage.meta: Option<MessageMeta>` (carrying `feedback`), written
   by the new `set_message_feedback` command through the same
   find/touch/save path `truncate_session` uses, and seeded back into the rows
   when a chat is re-opened. No server-side sync exists (local-only, by choice).
   Additive on disk both ways: an unjudged message writes no `meta` key, and a
   transcript written before the field existed still loads.
6. **subagent chip (h2), PTC mode (h3), Open in Zed (h5)**: no BSH concept —
   skipped in §3; do not stub.

## 6. Acceptance strategy (summary)

| Requirement | ui | markup | page | layout | live | perf |
|---|---|---|---|---|---|---|
| S1 New Session | – | id+i18n | click→create_session | – | – | – |
| S2 nav | – | – | navTo targets | – | – | – |
| S3 tree | – | roles | expanded flips | – | – | – |
| S4 Running chip | fixture | – | – | – | stream in/out | – |
| S5 account menu | – | – | items+focus | – | – | – |
| H1 jobs chip popover | – | – | rows + pick → pane, chip count + zero state | – | – | – |
| H2 deep-diving line | – | i18n | appear/hide+timer | – | slow turn | – |
| T1 tool rows | fixture: glyph/kind/dot/summary, path face | textContent | – | – | – | – |
| T3 Good/Bad | – | – | meta+exclusive | – | – | – |
| T4 Continuing-goal marker | – | i18n | advance→pill, call, reload→record | – | – | – |
| T5 state chips | – | i18n | 3 states | – | stop case | – |
| T6 usage popup | – | – | label+popover | – | – | – |
| T7 image viewer | – | titles | tab opens | – | paste→send→reload→open | – |
| W1 Working badge | fixture | a11y names | – | – | slow/stop/dismiss | – |
| W2 avatar | – | – | in/out | – | – | – |
| G1 goal bar | – | i18n | 5 actions | – | – | – |
| C1a ⊕ menu | – | – | 3 items | – | – | – |
| C1b circle send | states | – | – | 42px@4 widths | – | – |
| C3/C4 wording | – | strings | zh swap | – | – | – |
| B1-B3 status bar | fixture | a11y | texts+popover+numbers | `layout`: the group is centred ±24px at all four widths (measured 8px off at every one; a `margin-left` injection reddens three) | – | – |
| §3B tokens (accent/sidebar/selected/text) | – | – | – | computed styles@2 widths | – | – |
| §3B insets & boxes | – | – | – | inset 64/40, goal h44, send 42 @4 widths | – | – |
| H0 overlay titlebar | – | – | – | `#chat-title` top<44 | – | – |
| §5.3 `cancelled` flag | `turnVerdict` per source | – | – | – | stop path | – |
| streaming cost (frames / rows / list) | – | – | – | – | – | 400-event burst, structural list records |

The Rust side carries its own unit tests where the rule is a rule rather than a
render: the §5.3 stop is taken only by the generation it named, the turn markers
round-trip, and the default title is the one the rest of the app says (`lib`).
### Verified in the real window (2026-10-13, `target/debug/BOS.app`)

`ocu call get_app_state` against the freshly built bundle (`dev.bos.gui`), plus its
screenshot, settles the claims only the packaged webview can answer:

- **H0**: the traffic lights sit inside the header row and `BOS HARNESS` starts at y0 -
  the window has no separate native title bar.
- **S1/S2/S5/W2**: the sidebar lists `New Session`, then the `Global panels` group with
  `Plugins` / `Automation tasks` / `MCP connectors`, and ends in the `Account menu`
  popup with the `B` avatar over `Local workspace`.
- **S4/G1**: the centre column leads with the `idle` runtime chip and the `◎ goal` card
  (`Help: The goal for this chat (Ongoing Goal card)`), beside `EN` and the chat menu
  controls (`⋯`, `▤`).
- **C1a/C1b/C3/C4**: the composer has the circular `⊕` popup named *Add files or run
  commands*, the placeholder *Message or run a task, / commands, @ files or sessions*, a
  disabled circular Send, and a chip that reads `glm-5.3-flash` while its accessibility
  name carries the full `nvidia/z-ai/glm-5.3-flash`.
- **B1-B3**: the status bar shows `0 chats`, `0 turns 0 steps · — tok/s`, `— tok · Cache
  hit —` and `0% of context used` - the dash rule holds in the shipped bundle, not only
  in the fixture.
- **H1**: the jobs chip is absent with nothing running, which is its zero state observed
  in the app rather than in a renderer call.

The read is a guard now, not a one-off: `crates/gui/tests/ocu_selftest.sh` pulls the same tree and
asserts ten of these facts, prints a reason and exits when no BOS is running, and was falsified by
pointing it at another app's window - nine of the ten redden, the survivor being the `Account menu`
name because the reference app publishes one too.

An empty chat has no transcript, so T1, T3, T4, T5, T7, H2 and S3 stay on the fixtures -
that is where they were guarded already, and driving a real turn would spend the model
quota for no extra coverage.
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
| "Analysis completed" chip (dsh:76) | nothing tells BSH that reasoning *ended* - the message carries the reasoning text, not the moment it stops, so the chip would state an event this backend never sends |
| Reference shadow/rounded-blur styling | BSH flat rule (guards fail on shadows) — border substitute |
| Mascot animation (W2 static only) | decorative motion must respect `prefers-reduced-motion` |
| ~~Traffic-light/titlebar layout~~ | promoted to **H0** (P2) — the missing strip is a real layout delta (§3B.1); if the overlay fails on macOS it returns here as a documented deviation |

## 9. Implementation progress (living table)

Status vocabulary is §0's. `✅` means **landed in a working-copy change and
guarded**; the change id is named so a reader can look at the diff.

| Wave | ID | Item | Status | Evidence |
|---|---|---|---|---|
| 1 | §3B | layout/color/type tokens (accent, text, dim, sidebar bg, selected row, 348px sidebar, 44px pitch, 15px sidebar type) | ✅ `kqmsxzso` | `layout_selftest.sh` 9 named assertions at 4 widths; falsified 4× (each reverted value reddens its own line) |
| 1 | S1 | sidebar New Session button under the brand | ✅ `kqmsxzso` | `markup_selftest` (head + one id + i18n marks), `page_selftest` 4 checks incl. the empty-registry refusal; falsified 2× (handler removed, registry ignored) |
| 1 | H2 | "Deep diving for {d} ···" live line | ✅ same change | `ui_selftest` 4 (idle/text/one-interval/lifecycle) + `live_selftest` 3 on a real slow turn (`text="Deep diving for 0s ···", timers=1` → after finish `hidden=true, timers=0`); falsified 3× (interval kept, line never hidden, unowned interval) — the third also reddens the *first* check, because a leak whose handle is never stored reads as "no interval" |
| 1 | W1 | floating Working badge above the composer | ✅ same change | `layout_selftest` 3 (265px wide, 27px right inset, 200px below the tab divider — asserted as a *difference* from the head's own height, at 1440/900/560/420), `ui_selftest` 5 (idle, name+clipping, ✕ for this stream only, next gen brings it back, settled hides), `live_selftest` 5 on real turns (visible with "Working", ✕ while the turn keeps running, back by itself on the next turn, gone after Stop); falsified 2× (visibility ignored the stream, dismissal keyed on the session) |
| 1 | G1 | compact goal bar + 64/40 inset | ✅ | **Landed**: `#goal-bar` (◎ + `goal.barLabel` + the objective + ⓘ ✎ 🗑, 44px, `--code-bg` on `--border`, `--radius-lg`) between H2's line and the composer; painted inside the existing `renderGoal()` so there is one repaint path, hidden whenever there is no goal; its four controls delegate to the card's own (`goal-edit` / `goal-open` / `goal-delete`), so the delete confirmation and the editor focus are not re-implemented. **Landed since**: the §3B.5 insets — the transcript now pads 64 and the composer sits at 40, so the bar, the clock line and the transcript share one edge. **Owed**: the bar's page checks and their falsification; the §3B.5 64/40 content/composer inset (the transcript still pads 24, the composer still sits at 20) — the bar itself is drawn at 64 already, so the three edges disagree until that lands; and the spec's ids are `#goal-bar-*` rather than `#goal-objective`/`#goal-edit`/`#goal-clear`, which the existing goal card already owns (duplicate ids are a real defect the markup guard refuses). **Since then**: the §3B.5 insets landed (transcript 64, composer 40 — the bar, H2's line and the transcript now share one edge) and the bar's `page_selftest` checks went in: it must be a labelled `role=region` that is *not laid out* when there is no goal, and its four controls must stay its own beside the card's. Falsified by making the bar ignore the goal (1 red, `restored clean: yes`). **Still owed, and the only reason this is not ✅**: the "visible with the objective" case and the ✎/ⓘ/🗑 behaviours — each needs a real goal, so they belong in `live_selftest` rather than the static page fixture, and the delegation is only worth asserting there where the card's own calls are observable. **The visible case is now checked**: `live_selftest` asserts the bar is on screen carrying the card's own objective (`"ship the streaming harness"`) while the card holds it. **The ✎/ⓘ/🗑 half is still owed, and the attempt is worth recording**: with an active goal the card is on screen by default, so the page guard's proven "chip opens, Cancel folds" sequence does not transfer — the chip *folds* here, and the first version's `Cancel` did nothing because Cancel lives inside the form. A check on "the bar's button opened it" needs the card proven folded first; the first attempt had no such precondition, so its `waitFor` could only time out and abort the run. That block was removed rather than left red, and the reason is written where the check will go. **Then it landed**: the precondition that works is the *editor form*, not the card — a goal paints the card on screen, and `#goal-open` only ever *opens* it, so a click on the chip cannot fold anything. The check now asserts the form is closed, clicks the bar's `#goal-bar-edit`, and waits for the card's own editor to appear: `live` 1/158, the only red still the pre-existing image bubble. **ⓘ is deliberately not checked** — with a goal the card is on screen either way, so a click on it cannot be distinguished (a check that cannot fail is worse than a documented gap; the details view itself is M11's) — and **🗑 is owed**, because it must be shown to go through the card's confirmation rather than around it. **Falsified**: dropping that listener makes the live run stop exactly there (`waitFor: the bar's ✎ to open the card's own editor`, `restored clean: yes`), so the check fails for the reason it names rather than for a neighbour's — and the same is now true of 🗑: with the bar's `#goal-bar-clear` listener removed the run stops at `waitFor: the bar's 🗑 to reach the card's delete path` and the bytes came back (`restored clean: yes`). `live` is 1/160, the only red the pre-existing image bubble. **What the check had to learn first is where it could stand**: the file's tail after the image check is a dead zone (that `waitFor` throws, the `catch` writes the result, so nothing after it ever runs), and two positions before that were worse — one where the active session no longer held the goal, which is why `#goal-delete` was disabled (`renderGoal`'s no-goal branch disables every action button; only the has-goal branch clears `remove.disabled`). It sits at 1338, immediately after the only two checks that use the goal. **What G1 still owes is the 🗑 half** — it has to be shown to reach the card's `#goal-delete` and its confirmation rather than around them. |
| 1 | C1b | 42px circular send | ✅ same change | `layout_selftest` 3 (42×42, 16px from the composer's right and 12px from its bottom, at 1440/900/560/420); the glyph states stay where they already are — `live_selftest` asserts the `stop` class at a turn's start, its absence after, and drives `type()`, which the harness documents as the difference between an enabled and a disabled Send; falsified 2× (the old 34px size, the insets removed) |
| 2 | S4 | session-row Running chip | ✅ | `renderSidebar()` puts a `.row-live` chip (`data-i18n="row.running"`) between the marker and the title for any `streamOf(id)`, not only the active one; the redundant `vh` "streaming" text is gone because the chip is the word now (the spec's label-in-name condition, verified: nothing read that text). Guarded at two layers, **and the falsification landed on the page one**: gating the marker on `state.activeId` reddens `a streaming chat and its workspace row both read as working (session-row / ws-head active working)` — that fixture's streaming chat is a non-active one. The live check stays green under the same injection because the chat it streams happens to be the active one, so its precondition cannot tell the two apart; a "start a turn, switch to another chat, the row keeps its word" live check is the missing half and is owed. `restored clean: yes`. |
| 2 | S3 | sidebar tree outline semantics | ✅ | `#session-list` is `role="tree"` with `aria-label="Sessions"` (`data-i18n-aria="side.sessionsTree"`, en/zh); every workspace head is a level-1 `treeitem` reporting `aria-expanded` in sync with its fold button; every chat row is a level-2 `treeitem`, with `aria-selected` on the open one. Guarded in `page_selftest` (3 checks, on the 2-workspace fixture, one of them empty) and **each is falsified on its own**: stripping `role="tree"` reddens only the tree check, dropping the head's `aria-expanded` reddens only the fold check (`null -> null -> null`), removing the leaf's level/selected reddens only the leaf check — 1 failure each, `restored clean: yes`. The visible indent is unchanged. |
| 2 | T5 | Turn state on the foot (Running / Completed / Stopped) | ✅ | The chip states how the turn ended, ahead of its duration. **§5.3 gave the backend something to say**, so the verdict now leads with `stream-finished`'s own `cancelled` and keeps the client's press as the second source - a backend that predates the flag must not have every stopped turn called completed. The GUI records the stop against the **generation** it was aimed at (`Inner.stop_requested`), and `take_stop` takes it only for that turn, so a stop cannot mark a turn the user never stopped; the payload field claims exactly that and no more: a turn the runner ended for any other reason is not "stopped". The row's `title` no longer hedges about the backend not reporting - it says the turn ended because Stop was pressed - and the `stoppedNote` reads the same in en and zh. **Guarded in two layers on purpose**: `ui_selftest` pins `turnVerdict` per source (backend flag alone → stopped; press alone → stopped; neither → completed; no flag at all → completed) and dropping `payload.cancelled` reddens exactly the first while the press case stays green, and the live suite's stop check pins the pressed path end to end. The live double cannot tell the two apart - its pressed turn makes both true at once - which is why the split lives at the ui layer. |
| 2 | B1/B2/B3 |  ✅ | — | The centered group, its texts, its popups and its a11y treatment are done and guarded, and **both numbers are now real**: the rate is completion tokens over the turn's own wall time, recorded once at `stream-finished` (`state.tokRate`), and the total is accumulated per chat at each finish (`state.usageTotals[sessionId]`), with a reload re-deriving it from message meta when a message carries it. Two rules came out of it: a missing rate and a total nobody has counted both render as the dash — `0 tok` would be a false measurement, and this bar's dash means "not known", not "none" — and the dash takes the whole phrase with it (`Cache hit —`, never `Cache hit —%`). **All three popups state real numbers now**: the cache hit is the provider's own cached share (96 of a 120-token prompt reads as 80%) and stays a dash for a provider that omits the breakdown. Guarded at the page layer (three texts from stated numbers, the dash rules, the context popup's destination) and at the live layer (a turn that finishes moves the total by exactly the bridge's 120+45 and produces a rate matching its own 200 ms), 2 live injections red each their own check, `restored clean: yes`. Owed besides §5.1: the centering claim is still the overflow probe rather than an assertion about the group's centre, and the pure-render pass belongs in the ui fixture. |
| 2 | C1a | composer ⊕ popup | ✅ | `#attach-btn` is a popup over the shared `openRowMenu` (`aria-haspopup="menu"`, `composer.addFiles` = "Add files or run commands" / "添加文件或运行命令"), with **Attach images…** (the same hidden `#attach-file` input the button used to open directly), **Mention a file** (types `@` + `input`) and **Commands & skills** (types `/`) — the last two performing `#skill-btn`'s own gesture, so a menu item cannot drift from the keystroke that means the same thing. Guarded by 4 `page` checks that drive the menu itself (`page 0/190`). The mention item exposed a real defect: a bare `/` opened the palette but a bare `@` left the picker hidden, because `syncMention()` compared the token's empty query against `state.mentionDismissed`, which *is* `""` when nothing has been dismissed — so a bare `@` was indistinguishable from one the user had just Esc'd away. One line fixed (Esc dismisses a *query*, not an absence); its injection reddens `"Mention a file" types the @ the mention picker answers to`, `restored clean: yes`. `mentionToken()` and `MENTION_RE` are untouched: the regex already accepts a bare `@`, and an attempt to special-case it there was removed after its own injection stayed green — a check that cannot fail is worse than no check, and so is a line that cannot matter. |
| 2 | H1 | jobs chip dropdown | ✅ | The chip carries a caret, `aria-haspopup="menu"` and `aria-expanded`, and it opens a popover of the pane's jobs - label and state word, `cargo test · running` / `lint · failed` in the guard - whose every row is a door into Settings → Jobs rather than a second manager. Guarded by two page checks (the popover's rows, then the pick landing on the pane with `aria-expanded` back to false), and the check closes any menu an earlier one left open so what it sees is the menu its own click made. **The falsification question, settled in rev 43**: the check **is** falsifiable - making `openJobsMenu` throw reddens both checks (`2 / 229`). An injected `if (false) return;` at the same anchor stayed green, which rev 40 read as a second path reaching the popover; searching found no second path (`openJobsMenu`, `jobsChip` and the only `openRowMenu(jobsChip, ...)` are single-sourced), so the unexplained part is the injection rather than the wiring. The precondition is now part of the assertion (`menus before=0`), so a popover left over from an earlier gesture cannot pass it. |
| 2 | T3 | turn footer Good/Bad | ✅ | Buttons `.msg-good`/`.msg-bad` sit right after Copy, before Regenerate — the reference's order, asserted as one whole sequence (Copy · Good response · Bad response · Regenerate · Delete · Branch). Exclusive and reversible **by construction**: the store holds one slot per answer (`state.feedback["sessionId:index"]`), so "pressing Bad turns Good off" is a property of the store rather than a rule the click handler remembers. **Guarded at three layers.** Rust (`cargo test -p bsh --lib`): `set_feedback` records/replaces/clears one slot, refuses a verdict it cannot mean (`"great"`) and an index past the end, and round-trips through the file format while an unjudged message writes no `meta` key. Page (`page 0/194`): order, exclusivity, toggle-off, and the round trip — the render cannot see persistence, so the observable is `called("set_message_feedback")`. Wiring: the command exists on both sides. **Falsified**: storing an unknown verdict reddens the refusal test; dropping `skip_serializing_if` reddens the round-trip test; dropping the `invoke` reddens `the verdict is sent to the transcript that has to hold it` (`calls=0`); `restored clean: yes` throughout. One injection stayed **green** and is recorded rather than explained away: removing `#[serde(default)]` changed nothing, because serde already loads a missing `Option` as `None` — the attribute pair is about the write side, now asserted directly. |
| 3 | H0 | overlay titlebar | ✅ | `tauri.conf.json` declares `titleBarStyle: Overlay` + `hiddenTitle`, so the webview starts at y0 and the header owns the top (the reference's y0-50); the brand row's `--frame-top-clearance: 48px` keeps the traffic lights clear. Two layers, because each is blind to the other: the native key is asserted where the config lives (`package_selftest`, 18 checks) and the page half by the layout probe at four widths (`headTop < 44`, reported by `layout_selftest`). Falsified both ways: restoring the strip reddens the package check, and 34px of margin on `#chat-head` reddens the layout check at every width. **The native overlay itself is still owed a clean-rebuild look in a real window (OCU)** - no fixture can see AppKit. |
| 3 | S2 | sidebar global-panels nav | ✅ | `<nav id="side-panels" aria-label="Global panels">` sits under New Session with three 30px rows (`#nav-plugins`, `#nav-automation`, `#nav-mcp`), icon + left label, `--hover` fill and `--radius-sm`, per the flat recipe. Labels are the en/zh pair (`nav.plugins`, `nav.automation`, `nav.mcpConnectors`) carried on the row's own span. Behaviour is pure navigation through the existing `navTo`: `cap-list` (Tools & skills), `jobs-section`, `set-mcp`. Guarded at the page layer by clicking each row and reading both effects - the pane that becomes active and the rail tab that gains `aria-selected` - because a row wired to the wrong section still opens Settings. Falsified by pointing `nav-mcp` at `cap-list` and by aiming a check at a section its row does not open; both redden one check, the rest staying green. |
| 3 | S5 | account menu popup | ✅ | The avatar row is `<button id="side-account" aria-haspopup="menu" aria-expanded="false">` keeping its avatar + `Local workspace` visual, and it opens the shared row menu (the reference's dsh:42) with four items: Settings (`openSettings`), Keyboard shortcuts (`Polish.openShortcuts`), Theme (`Polish.cycleTheme`) and Language (`Polish.setLang` toggle). Escape closes it and focus returns to the row through `closeRowMenu(true)` - the same contract every menu here obeys. The standalone Settings gear stays (documented deviation). Page layer: five checks (haspopup + collapsed state, the four items and their order, `aria-expanded` while open, Escape + focus back on `#side-account`, and the Settings item laying out a pane with the modal closed first so the assertion can fail). Falsified by making Escape not refocus (reddens this and the existing keyboard-back check) and by pointing the Settings item at the theme cycle. |
| 3 | T1 | tool-call rows in reference style | ✅ | The collapsed row now reads glyph, kind word, `·`, then the one-line summary (`>_|Bash|·|git diff`), and an argument that is a path is drawn in the code face over a dotted underline (`Read|crates/gui/ui/app.js|is-path`) while a command with an argument is deliberately not. Glyphs are text, matching the reference's own rows. Guarded in the ui fixture by five checks that also hold the two things the restyle must not touch (expansion still reveals the output, the safety gate is unchanged); removing the summary span makes the fixture throw, which the runner reports as a red. |
| 3 | T6 | per-turn Usage popup | ✅ | Each finished turn's footer carries `Usage {n} tok` as `<button class="mini-btn msg-usage" aria-haspopup="dialog">`, where `{n}` is what the session had spent **through that turn** (see the refinement in §4: the accumulator already holds the finished turn, so adding `state.usage` again would count it twice). Clicking it opens a flat `role="dialog"` panel with the turn's own `↑ prompt · ↓ completion` beside the session total, plus the `token_meter` reading - read **once per open**, never per repaint. A session that has spent nothing carries no chip at all, and a turn whose usage the store never recorded says "not reported" rather than 0. Escape closes it and focus returns to the chip. **Five page checks** (the two labels with the first-turn sum, the panel's split + `aria-expanded`, Escape + focus, one meter read per open across a repaint, and no chip on an empty session), **falsified three ways** with `restored clean: yes`: stating the session total instead of the turn's own reddens the label check; drawing the chip on a turn that measured nothing reddens the empty-session check (and T3's action-order check, which reads the same footer); reading the meter on every repaint reddens the `reads=1` clause with `reads=2`. |
| 3 | T7 | image viewer | ✅ | A message's attachment is now `<button class="msg-image" title="View full image: <name>">` carrying the thumbnail and the derived name, and its click calls `Shell.openDocument({id: "img:…", title: name, path: name, img: url})` - the same document panel the tool results use, which grew an `<img class="doc-img">` branch for it (our own payload, `alt` = the name). The old click-to-zoom-in-place is gone, along with its dead `.zoomed` rule and the two live assertions that described it. **Page layer**: two checks (the button + thumbnail + `image-1.png` + the title, then the active `#doc-tabs` tab named `image-1.png` with the same `src` and `alt` laid out in the panel), **falsified three ways** with `restored clean: yes` - not a button reddens the render check, naming the tab `attached image` reddens the tab check, and removing the panel's picture branch reddens it too from the other side of the crate boundary. **No red of this item is left.** The live image section runs its viewer assertions end to end: its earlier `rows=0` was not an empty-transcript gap in the page - the double created the chat in the wrong workspace, so the page correctly showed none (rev 38) - and with that fixed the live suite is `0 / 171`, this section included. |
| 3 | W2 | brand avatar while streaming | ✅ | `<img id="brand-avatar" class="brand-avatar" src="icons/bos-harness-icon.svg" width=64 height=64 alt="" aria-hidden="true">` inside `#chat-head`, so it sits where the reference puts it - top-right of the transcript, above the W1 badge (y125-215 against the badge's y295-355). Visibility follows the **stream**, not the badge's dismissal: hiding the card is a statement about the card, so the toggle sits above `renderWorkingBadge`'s early return and the character stays until the turn ends. `pointer-events: none`, and the one transition is the 120ms opacity the spec allows (neutralised by the existing reduced-motion block like every other transition). **Two page checks** entering through `renderAll()` - the app's own pass, which is what a switch, a stop and a delete come through - **falsified three ways** with `restored clean: yes`: never removing the class reddens the idle check, dropping `pointer-events` reddens the streaming check (`pe=auto`), and giving it an `alt` while dropping `aria-hidden` reddens it too (`hidden=null`). The markup layer sees none of the three, which is why the check lives at the page. |
| 3 | C3 | placeholder wording | ✅ | The textarea ships the reference's sentence - `Message or run a task,` / `/ commands, @ files or sessions` (zh: `输入消息或任务，` / `/ 命令，@ 文件或会话`) - and the Enter hint moved into its `title` behind a new `composer.enterHint` mark instead of riding as the fourth clause of the placeholder, which is what the wrap pressure in issue #14 cut off mid-sentence. **Two layers, and the falsification batch showed exactly which is blind to what.** `markup_selftest` compares the shipped attribute against the literal sentence *and* against polish.js's own English line, so the two cannot drift apart; `page_selftest` toggles the language and reads the rendered value (zh two lines, then back to English). Restoring the old long placeholder reddens **markup only** - once `setLang` has run, the applier has overwritten the DOM attribute from polish.js, so the page never sees the shipped string - and editing polish.js's zh sentence reddens **page only**. Putting the hint back *inside* the placeholder reddens markup (the attribute) and is invisible to the page for the same reason, while deleting the `title` reddens the page's two title assertions. Four injections, each caught, none of them by a check that could not have failed. |
| 3 | C4 | model chip short name | ✅ | The chip's visible text is the segment after the last `/` (`glm-5.3-flash`) via `shortModelId`, and only when there is no model at all does it still say `no model`; the button keeps the whole id in its `title` *and* its `aria-label` (`Model: nvidia/z-ai/glm-5.3-flash`), so a chip that cannot tell two vendors apart is still announced and still hoverable in full. The list the chip leads to (it opens the sidebar's row menu into Settings, since the model is a setting) is untouched by this item. **Two page checks**: the chip with a three-segment id - asserting the visible text carries no `/` while both the title and the aria-label carry the id - and then a *second, different* model with no path at all, which is what proves the update is not one-way: code that only ever writes when the label is empty passes the first check and fails this one. **Falsified three ways** with `restored clean: yes` - naming the whole path in the chip (`chip=nvidia/z-…`), dropping the `aria-label` (`aria=null`), and leaving the title stale each redden the checks above. |
| 3 | T4 | "Continuing goal" marker | ✅ | The advance appends a record to the session's log and the transcript draws it as a centered pill, in two shapes that are the same element. **Backend**: `LogMarker { kind, at }` lives on `SessionRecord.pending_marker` (the advance happens between turns, so there is no message to put it on yet) and the finish handler moves it onto the first turn it writes as `MessageMeta.marker` - beside the token count and for the same reason, because `load_session` is what a re-opened chat reloads. Taking it is what makes it appear once, and it survives even if no turn ever comes. **UI**: the reader's own advance draws the pill immediately - appended **after** the reconcile, since `#messages`'s row window holds turns and this is not one - and it stops drawing as soon as the transcript grows, because the count it was taken at is the whole condition; the durable `meta.marker` draws the same pill above its row. `markDirtyAll()` accompanies both, because the transcript's shape changed even though its messages did not. The pill carries `role="note"`, no `.msg` class and therefore no part in any count. **Three page checks**: the bar's advance is enabled, the gesture is recorded against the transcript's own length (a precondition named on its own rather than folded into the pill's assertion), and the click draws `Continuing goal <MM-DD HH:MM>` with the assistant-row count unchanged and the command's argument read back as `advance`. **Falsified three ways** with `restored clean: yes` - never recording the gesture, dropping the `markDirtyAll()`, and taking the count as `0` each redden it; the second is the interesting one, since without it the pass stops early and the pill is never reached. **The durable record is driven end to end too.** A fourth page check clears the pending hint, seeds a turn carrying `meta.marker` and requires the pill back from that record alone - falsified by removing the row loop's push, which reddens it while the pending-hint checks stay green (the same two-source split §5.3 uses). **The rule is unit-tested since rev 43**: the write and the move-out both live on the record (`mark_advanced` / `apply_pending_marker`), so `session`'s own test asserts the promise - an advance with no message yet **stays pending**, the next turn takes it, and a later turn gets nothing while the message keeps its marker (swapping `take()` for `clone()` reddens it). What stays out of reach is the Tauri command itself, which needs the whole `GuiState`. |
| — | §5.1 | cache-hit tokens through the finish payload | ✅ | Done: the provider's cache breakdown is counted in react's token counter, carried by the agent as `last_stream_cache_tokens`, and stated by the `stream-finished` payload as `cache_read_tokens`. A provider that omits it still renders the dash. Guarded by a react counter test (the count moves; an absent breakdown leaves it) and by the live check reading `Cache hit 80%` off a bridge sending 96 of 120; falsified by dropping the accumulation, dividing the wrong tokens and dropping the payload field. |
| — | §5.2 | session-cumulative tokens | ✅ | Done: `MessageUsage` on the message's meta, written by the finish handler and re-summed by `usageTotal` on load; guarded by a Rust round-trip test that also refuses an empty usage, a page check that a transcript carrying `meta.usage` is what the total counts (falsified by reading the old wrong keys), and a wiring-layer check that the writer's field names and the reader's agree across the crate boundary - the seam this round found broken. |
| — | §5.3 | a trustworthy `cancelled` flag in `stream-finished` (issue #8) | ✅ | Landed: the field is on the payload, written where the stop was honoured and matched to the generation it was aimed at, so one press cannot mark a later turn and the flag survives a reload. Three unit tests pin the rule (taken by the turn it named / left for that turn when another asks / no stop at all is not one) and injecting a generation-blind match reddens the second. |
| 🔄 | h4/h6/h7/t9 | middle-panel fork: jobs chip, ⋯ More actions, document toggle placement, "Completed in …" row | 🔄 complete in `kqzzxqqy` (verified on the real window, not re-implemented here) | fork's own guards |

**Guard baselines after this round**: `ui` 165/0, `shell` 95/0, `layout` **0** at
1440/900/560/420, `markup` 95/0, `page` 180/0, `package` 17/0, `wiring` 0,
`perf` 19/0, `live` 148 with its one pre-existing red (the image bubble's reload,
being diagnosed separately).

**Owed, not claimed**: §3B asks each landed block to be checked against the real
window too, side-by-side with the reference. Launching BOS from the agent's file
sandbox leaves WebKit unable to write `~/Library/Caches/bsh`, so no window is
created and `ocu` answers `cgWindowNotFound` — the read-out for §3B/S1 is
therefore **still owed** and is not counted as evidence here. (Operations note for
the next round: start the app from the user's own session, and do not `pkill` the
instance the user is running — that is the only one whose window `ocu` can see.)

## 10. Revision log

| Rev | Date | Change |
|---|---|---|
| 1 | 2026-10-12 | Initial: full diff matrix from live OCU captures (DSH 3 004-line a11y dump + streaming screenshot; BSH 108-line dump), 20 implementable requirements (S/H/T/G/C/B/W ids), backend-gap table, guard mapping, 3-wave phasing. Expansion of issue #17. |
| 2 | 2026-10-12 | Add **§3B layout & style spec** (pixel-measured from `dsh_main.png` vs `bos_main.png`: sidebar 348px + row pitch 44 + font 15, content/composer insets 64/40, goal/composer/status box rects, light-theme color map, type scale, tolerance rule); add **H0** overlay-titlebar requirement; correct S1's button measurements (fill `#f2f3f5`, borderless, h≈34); crops added to §1. |
| 3 | 2026-10-12 | **§3B token commit landed** (working-copy change `kqmsxzso`, on top of the middle-panel fork `kqzzxqqy`): light theme accent `#4176e6` / text `#0f1115` / dim `#6b7280` / sidebar `#e8e9eb` / selected `#dddee1`; `--sidebar-w` 280→**348** (CSS token, `SIDEBAR_DEFAULT`, and the two narrow-band drawer rules that had a literal 280); new `--side-font` 15 / `--side-row-h` 44 applied to the sidebar and its rows. Guard: `layout_probe.js` reports the computed values and `layout_selftest.sh` names each one at 1440/900/560/420 — falsified by reverting each value in turn (see §9). One trap worth naming: the probe's own report element stayed **in the document flow** and made the page 178px wider than the window, reddening *"the window can be panned sideways"* — a diagnostic that changes the geometry it reports on; it is positioned off-flow now. |
| 4 | 2026-10-12 | **S1 landed** (sidebar New Session row, same change `kqmsxzso`): a full-width `#side-new-session` row under the brand, borderless on `--side-new` (`#f2f3f5` light / `#2f2f2f` dark), `--radius-md`, 34px high, disabled while no workspace exists, running the same path as a workspace row's `+`. Two findings from its own guards: `markup_selftest` refused `data-i18n` on the **button** (that mark writes `textContent`, which would have deleted the svg and the label span — the mark belongs on the leaf), and the first version of the page checks left a created chat behind, which changed three later scenarios' counts (M11/M14) — a check that mutates shared state is a check about the checks after it. |
| 5 | 2026-10-13 | **H2 landed**: `#stream-line` after `#messages`, hidden unless `state.streams[activeId]` exists, stating `tr("stream.deepDiving")` (`深入思考 {d} ···`) with the elapsed time from `state.turnStarted` via the existing `turnDurationText`, under one 1 s interval that is cleared in the same pass that learns the turn ended (`stream-finished`), on a session switch/stop/delete (`renderAll`) and by the interval's own next tick. The text is written only when it changes, so a streaming burst does not turn the clock into a DOM mutator. **Deviation from the plan's layer choice**: §6 wanted the appear/hide wiring checked in `page_selftest`, but that harness has no event emitter (its bridge is settled), so the wiring is checked where events are real — a `/slow` turn in `live_selftest` — and the render/timer logic at the lowest layer that sees it, the ui fixture, which also makes falsification cheap (3 injections, seconds each). The line is present in the *shipped bundle* only after a rebuild; the running window still shows the pre-H2 page. |
| 6 | 2026-10-13 | **W1 landed**: `#working-badge` — the chat's clipped name, "Working", a round ✕ and a round stop — shown iff `streamOf(state.activeId)`. **Placement found by the guard, not guessed**: §3B states the card 200px below the tab divider, and the probe showed the head is 72px tall at 1440 but 96px once the window narrows, so a fixed `--head-h` token was wrong at every width but one. The badge now lives *inside* `#chat-head` and hangs off its bottom edge (`top: calc(100% + 200px)`), and the assertion states that difference, so a head that changes height moves the card with it. **Dismissal is keyed on the stream's `gen`, not the session**: ✕ hides one turn's card while that turn keeps running, and the next turn's own first event brings it back with no flag to clear. A first version of the live check asserted this immediately after Send and read the badge as hidden — the event had not arrived yet, so the check now waits for the card to come back by itself (the wait *is* the claim: nothing the reader does restores it). |
| 7 | 2026-10-13 | **C1b landed** (before G1 in this list, deliberately — see below): the send control keeps its id, aria-label and circular shape and grows to the reference's **42×42**, with its disabled state on `--card` + `--dim` instead of the hover grey. The measured insets were 13/9, not the spec's 16/12 — exactly the composer's own `padding: 10px 12px 8px` plus its 1px border — so the difference is carried as *this button's* margin; widening the composer's padding would have moved the input line and the attach strip with it, and the composer's outer inset is G1's subject, not C1b's. **Order**: this list puts G1 first, but C1b states its numbers *inside* the composer while G1's 64/40 change is about the column outside it, so landing the smaller, self-contained half first costs no rework and keeps the round's evidence checkable. |
| 8 | 2026-10-13 | **G1 (goal bar) landed, its checks and the inset owed.** The bar is markup + paint + wiring: painted inside the existing `renderGoal()` from the same goal value the card uses (hidden iff there is no goal), and its ⓘ ✎ 🗑 are doors onto the card's own controls rather than a second implementation of the delete confirmation or the editor focus. The spec's ids could not be used as written — the card already owns `#goal-objective`, `#goal-edit` and `#goal-clear`, and the markup guard refuses duplicate ids — so the bar's are `#goal-bar-*`, recorded as a deviation. `markup` 95/0, `page` 180/0 and `layout` 0 stayed green with the bar in the shipped page. **Still owed, and the reason this row is 🩹 rather than ✅**: the bar's five page checks and the injection that paints it from `goalOpen`, and the §3B.5 64/40 inset — the bar is drawn at 64 while the transcript still pads 24 and the composer still sits at 20, so the three edges disagree until that lands. Landing the bar first and its checks immediately after keeps each step verifiable; claiming G1 whole on a green markup/page/layout would not have been true. The §3B.5 insets followed in the same change: `#messages` pads 64 and `#composer` sits at 40, so the bar, H2's clock line and the transcript now share one edge, and `layout`/`page`/`markup` were re-run green over the shipped page with them. |
| 9 | 2026-10-13 | **G1's page checks landed** (bar still ⬜ for its live half): with no goal the bar must not be laid out at all (`class` *and* computed `display`), and the four `#goal-bar-*` controls must exist alongside the card's three, so the id deviation is a checked invariant rather than a comment. The guard's first version would have been unfalsifiable if written the other way round — a static fixture with no goal cannot distinguish "painted from the goal" from "painted from `goalOpen`", since both are false — so the injection removes the toggle instead of flipping the value, and it reddens exactly the bar check. `page` 0/182. What remains for G1 is the half that needs a real goal: the bar *showing* the objective, and ✎/ⓘ/🗑 reaching the card's editor, details and delete confirmation. |
| 10 | 2026-10-13 | **G1's visible case is checked live, its ✎/ⓘ/🗑 half is not.** `live_selftest` now asserts the bar carries the card's own goal (`"ship the streaming harness"`) instead of a second copy of it — the one claim about the bar that needed a real goal. Two attempts at the behavioural half both failed for the same reason and taught the same lesson twice: **the precondition is the check.** An active goal paints the card on screen, so the page fixture's "chip opens the card, Cancel folds it" sequence is a different sequence in the live page (the chip folds, and Cancel does nothing because it lives inside the form) — and because neither attempt *proved* the card folded before asking a bar button to open it, the `waitFor` could only time out and abort the run. Rather than leave a check that reddens for the wrong reason, the block was removed and the precondition written down where it belongs; the next attempt asserts the fold first, then the opening. |
| 11 | 2026-10-13 | **The ✎ check landed, with the precondition that actually holds.** The third attempt found the mistake in the second: a goal paints the card on screen and `#goal-open` only ever opens it, so "fold with the chip" could never work — my `waitFor` was waiting for something the click does not do. The precondition that *is* observable is the editor **form**: it starts closed, only the card's own ✎ opens it, and so "the form went from closed to open" is a claim about the bar's click. All three G1 live checks are green (`live` 1/158, the sole red the pre-existing image bubble), and the precondition is an assertion of its own rather than a comment. **Also repaired**: the edit that produced this check left a stray `);` and the file would not parse — `node --check` before any run is what caught it, and a run had already been started against the broken file and was killed. Both ⓘ (indistinguishable while the card is always on screen) and 🗑 (must be shown to use the card's confirmation) remain documented gaps rather than checks. |
| 12 | 2026-10-13 | **The ✎ check is falsified.** Removing `#goal-bar-edit`'s listener in `app.js` makes the live run stop exactly at `waitFor: the bar's ✎ to open the card's own editor` and the bytes were restored (`restored clean: yes`), so the check is capable of failing for the reason it names. `ⓘ` stays deliberately unchecked (a goal paints the card on screen, so a click on it cannot be distinguished — M11 owns the details view), and `🗑` stays the one owed piece: the card's `#goal-delete` is the single confirmed path, and the bar's button has to be shown to land on it. |
| 13 | 2026-10-13 | **G1 is at parity, and Wave 1 is complete.** The bar's 🗑 check is green and falsified (`restored clean: yes`), so all three delegations the bar performs are proven where they can be observed: visible with the card's own goal, ✎ opening the card's own editor, 🗑 landing on the card's `set_goal(objective: "")` delete. ⓘ stays deliberately unchecked — a goal paints the card on screen either way, so a click on it cannot be distinguished, and M11 owns the details view. Getting the last check to *run at all* was the whole cost of this round, and every failure was positional rather than logical: the file's tail is a dead zone (the failing image check throws before it, and the `catch` writes the result, so nothing after it executes — which is why the check-count had not moved), and the live session no longer holds the goal further down (so `#goal-delete` was disabled by `renderGoal`'s no-goal branch). The check now sits at 1338, right after the only two checks that use the goal, and the file is 1/160 with the sole red the pre-existing image bubble. |
| 14 | 2026-10-13 | **S4 landed, and its falsification taught the same lesson in a new place.** The chip is in `renderSidebar()` for any session with a live stream (the condition was already `!!streamOf(s.id)`; what was missing was the *word* — a `.row-live` chip carrying `row.running`, en "Running" / zh "进行中" — and the old `vh` "streaming" text went with it, since the spec asks the visible chip and the screen-reader text to be the same word and nothing in the tree read that text; verified by grep before removing it). The check runs green in `markup`, `ui`, `page` (0/182) and `live` (1/160). Then the documented falsification — gate the marker on `state.activeId` — was run against **both** layers instead of just the live one, and the results differ in an instructive way: the page fixture's streaming chat is *not* the active one, so that check went red exactly as intended, while the live check stayed green because the chat it streams *is* the active one. A precondition that makes both answers the same is not a test; the live check now records `session-row active working` in its own message, which is where that fact will be read next time, and the missing live half (stream in one chat, switch to another, the row must keep its word) is written down as owed rather than claimed. |
| 15 | 2026-10-13 | **S3 landed with three checks that each fail alone.** The sidebar is now one tree: the list is `role="tree"`, a workspace head is a level-1 `treeitem` whose `aria-expanded` matches its fold button, and a chat is a level-2 `treeitem` with `aria-selected` on the open one. Writing the leaf check immediately found a real gap: the first run reported `allLevel2=false` because `archivedRow()` builds its row through a *different* function, so archived chats were the one kind of row outside the tree — the role belongs to the row's meaning, not to the builder, so the archived row got it too. The three injections were run one at a time against the page layer (fast, no live cycle), and each reddened exactly its own check with the other two green: `role="none"`, an empty `aria-expanded`, and a leaf with no level. `page` is 0/185 (182 + the three new checks) and the visual indent is untouched, per the spec's cross-ref. |
| 16 | 2026-10-13 | **T5 implemented, its streaming state checked, and the other two states deliberately left to the live layer.** The foot now carries a state chip ahead of its duration: `Running` while the chat streams, `Completed` on a normal finish, `Stopped` when the stop button was pressed for that turn — the one state BSH infers, kept client-side in `stoppedTurns` because the backend does not report yet how a turn ended (§5.3), with that limitation stated in the chip's own `title` rather than pretended away. Two decisions worth the ink: the finished chip needs a key of its own (`turn.state.completed`) since `turn.completed` is the duration sentence it sits ahead of, and the foot's signature now includes the chip, so a running row and a finished row are not mistaken for the same row by the transcript's cached window. **The page layer could only prove the streaming case**: setting `state.turnState` and rendering left completed and stopped at "none" (the streaming path worked in the same run), so those two moved to where they are really written — `stream-finished` — and the widened check was narrowed back rather than left failing for a reason that is about the fixture, not the code. `page` 0/186, `markup` 95/0. |
| 17 | 2026-10-13 | **T5's finished and stopped states are verified live; its Running state is a defect the live layer found on its first honest attempt.** Two real problems were caught by the guards rather than by reading the code. First, the chip inside `.turn-foot` concatenated its word with the duration phrase, so the foot read "CompletedCompleted in 0s" and an *existing* live check said so — the duration now carries only the duration, and that check's expectation was corrected with it (it still proves the same two facts: a state word and a duration). Second, the one worth remembering: the check that waits for "the streaming turn's own foot" timed out at `rows=28`, because a foot is only drawn under `lastAssistant` and a streaming draft row is not it. The Running chip therefore never reaches the screen in the real app, while `page` passes — its streaming row *is* the last assistant row, which is the "a precondition that makes both answers the same" trap one layer up. The fix is not a different condition but a different host: `#stream-line` is the transcript's foot while a turn is in flight, and that is where Running belongs. The check was left reaching that point, with the defect written beside it, instead of keeping a `waitFor` that aborts the remaining 70-odd checks — a precondition that fails and takes the suite with it hides everything behind it. Live is 1/162 with the sole red the pre-existing image bubble; Completed and Stopped are green with their exact chips, keys and the stop-title; the always-`Completed` injection still times out on `the foot to say Stopped`. |
| 18 | 2026-10-13 | **T5 complete: the Running defect was a host, not a condition, and moving it made all three states green.** The chip's word for a turn in flight now lives on the H2 stream row (`#stream-foot` wrapping `#stream-line`), which is what the transcript's foot is mid-turn; the finished and stopped chips stay on the foot under the answer, ahead of the duration. Live: `Running` / `Completed` / `Stopped` all green, 1/163 with the sole red the pre-existing image bubble. The last red was a page check that still expected the old combined sentence ("Completed in 1h 4m 59s") where the foot now reads the duration alone — its expectation was corrected, and the name it carries ("states how long it took, once, under its answer") still says exactly what it proves. The lesson of this item, worth more than the chip: **a state that is never drawn is invisible to the layer that draws its neighbours.** The page fixture could see "a chip exists for a streaming turn" and could never see "no foot exists mid-turn"; the live wait for that foot is what found it, and the earlier check design — a `waitFor` that could only time out — would have taken 70 further checks down with it. Two failures of the same family in one item is the argument for writing the guard at the layer where the behaviour actually happens. |
| 19 | 2026-10-13 | **C1a implemented; its guard is deliberately still owed.** `#attach-btn` became a popup menu over the existing `openRowMenu` (the same dispatcher the workspace row and the chat "⋯" already share), with the three items the spec names; "Mention a file" and "Commands & skills" reuse the very gestures `#skill-btn` performs — type the character, dispatch `input` — so a menu item cannot drift from the keystroke that means the same thing, and "Attach images…" opens the same hidden input the button used to open directly. `markup` 95/0 and `page` 0/186 are green with it in place, and no existing check broke — which is itself worth noting: nothing in the suite had asserted that the attach button opened the dialog *directly*, so replacing its behavior was invisible to every guard until a new one drove the menu. That is the check being written next, together with the injection for it (first item bypassing the menu → red). The status stays ⬜ in §9 until that check and its falsification land. |
| 20 | 2026-10-13 | **C1a guarded by four checks that drive the menu, one injection run, and one open defect found by the check rather than by the spec.** The menu reuses `openRowMenu`, so the ⊕, the workspace row and the chat "⋯" share one dispatcher; "Mention a file" and "Commands & skills" perform the very gestures `#skill-btn` performs, which is what makes a menu item and a keystroke impossible to drift apart. The four checks assert the popup's items and `aria-haspopup`, that "Attach images…" reaches the hidden input exactly once and closes the menu, that "/" opens the palette, and that "@" lands in the composer. That last one is where the honesty sits: the check `page` wrote first asked for the mention popup to be open, and it was **not** — a bare "/" opens the palette while a bare "@" leaves the mention popup hidden. Rather than weaken the spec line silently, the check asserts the gesture that does happen and prints the gap in its own result (`pickerOpen=false — a bare @ does not open it, unlike /`), and §9 carries it as an open defect with the two possible fixes. Running the documented injection (first item bypasses the menu) reddens the popup check as intended, `restored clean: yes`. `page` 0/190, `markup` 95/0. |
| 21 | 2026-10-13 | **C1a closed, and its mention defect was one line — the "two refusals" story was mine, and the injections corrected it.** The check written to say "the picker does not open on a bare @" first went green after two edits, and the temptation was to tell that as two independent causes. The falsification batch said otherwise: dropping the extra `mentionToken()` fallback left every check green (`failures: 0`), because `MENTION_RE` already matches a bare `@`. So that line was removed — code that cannot change an outcome is the same failure as a check that cannot fail, just pointed the other way. The real cause was one comparison: `syncMention()` tested `state.mentionDismissed === tok.query`, and with an empty query that compared `""` to `""` — the sentinel for "nothing was dismissed" — so the bare token looked already-Esc'd and the picker stayed shut. Esc now dismisses a *query*, not an absence, and removing that guard reddens the mention check (1 failure, `restored clean: yes`). Worth writing down twice: an empty string used as "nothing here" silently swallowed a gesture, and a green injection is a finding about the check *or* the line, never something to explain away. |
| 22 | 2026-10-13 | **T3's buttons landed and are guarded; the verdict is in-memory only, and the guard found a repaint defect that the app itself shares in kind.** Two buttons in the reference's order, exclusive and reversible by construction: the store holds one value per answer, so "pressing Bad turns Good off" is not a rule the click handler knows. Three `page` checks; two injections, each reddening `the verdict is exclusive and reversible` and each `restored clean: yes` — dropping exclusivity (`good → true/true`) and dropping the row's dirty mark (`good → false/false`). The second is worth naming: `renderMessages()` walks only from its dirty cursor and **reuses rows above it without comparing their signatures**, so a mutation that repaints without marking its row dirty draws the old state forever. T3 hit it exactly; the diagnostic that named it was `sameNode=true` beside a store that already said `"good"`. The same suspicion then pointed at the message Edit button — and was **wrong**: a probe showed the row rebuilt with its textarea drawn (`same=false, textareas=1`), so nothing was changed and the suspicion is recorded here rather than acted on. Storage: `meta.feedback` needs a command the UI can reach, and the full list of `invoke()` names has none, so §9 keeps T3 at ⬜ with the UI half landed and the reason written down. |
| 23 | 2026-10-13 | **T3 closed — the storage half now exists, and §5.5 was simply wrong.** The document said message `meta` "already round-trips — no backend change". It did not: `ChatMessage` had no `meta` field, and no command in the UI's entire `invoke()` vocabulary wrote one, so the buttons were drawing an in-memory verdict. It round-trips now — `ChatMessage.meta: Option<MessageMeta>` carrying `feedback`, written by a new `set_message_feedback` command that finds/touches/saves exactly the way `truncate_session` does, and seeded back into the rows when a chat is re-opened. Additive on disk both ways: an unjudged message writes no `meta` key, and a transcript written before the field existed still loads. **Three layers of guard, each falsified.** Rust: the refusal (`"great"`), the out-of-range index, the one-slot exclusivity, the round trip. Page: order, exclusivity, toggle-off, and the round trip as `called("set_message_feedback")` — a render check cannot see persistence, because the row draws from the in-memory store either way, which is the lesson the skill pane's cache taught. Injections: an unknown verdict stored → the refusal test fails; `skip_serializing_if` dropped → the round-trip test fails; the `invoke` dropped → the page check fails with `calls=0`; `restored clean: yes` on all three. One injection stayed **green**: removing `#[serde(default)]` changed nothing, because serde already treats a missing `Option` as `None`. The attribute pair is about the write side, so the test now asserts that side and its comment says which half the attribute buys — a green injection is a finding, and this one corrected a claim already written into the test. **Filed, not touched**: `cargo test -p bsh --lib` carries one pre-existing red — `session::tests::title_from_empty_conversation_stays_default` expects "New chat" while `SessionRecord::new()` sets "New Session" (session.rs:234 against the fallback at :301 and the assertion at :1252). Neither literal appears in this change's diff (`jj diff` count 0), so it predates T3 and is left exactly as found: the two disagree about what a brand-new conversation is called, and that is a wording decision, not a typo to guess at. |
| 24 | 2026-10-13 | **B1/B2/B3: the footer became the reference's three centered popups, and the work found two long-standing defects plus one of mine.** `#status-bar` now holds three `.stat-pop` buttons (turns/steps/rate, tokens/cache, context) with their own panels; the BSH-only notices keep the left end. `estimateContext()` had compared roles against lowercase `"user"/"assistant"` while the transcript serializes `"User"/"Assistant"`, so the estimator had counted **zero** since it was written — the old `ctx ~0/32.8k` readout was telling the truth about a broken number. Second: the shell's `Shell.addStatusItem` mounted its live region inside the span `renderStatus` clears every repaint, so a registered item was created, seen once and wiped by the next frame — it now mounts on the footer itself. **Four page checks landed** (0/198 green): the three popup texts against a fixture with a stated budget and message set, the `Cache hit —` dash losing its percent sign, the context percentage computed independently (5% from two messages and 200 tokens), and the context popup selecting the context section. **Falsified three ways**, each reddening exactly its own check and `restored clean: yes`: a missing number drawn as a zero (B1+B2), the percentage taken from a stale `state.usage.prompt` instead of the estimator (B3), and the estimator's roles put back to lowercase (B3). **A lesson paid for**: my first version of the check moved `state.activeId`, `state.budget` and `state.usage` and opened Settings, and eight later checks failed on the leftovers — the check now captures and restores every field it touches, which is the cost of a fixture that has to lie to see a gesture. **An incident, recorded rather than hidden**: a stylesheet write of the form `open(path,"w").write(open(path).read() + …)` truncated `ui/style.css` before reading it — Python evaluates the write's arguments after the file is already empty — leaving 37 lines where 3594 had been. Recovered **exactly** by reading the file from jj's operation log at the last operation that had it (`jj --at-op=448d2823deed file show -r @`), re-appending the popup block, and re-verifying with `markup` (95/0) and `page` (198, 0 failures); the panel styling and my own later block are the only parts not from that op. **Still owed for B1/B2**: the rate and the running total are not accumulated, so both draw the dash; the layout suite's centering claim is the overflow probe, not a centering assertion.  The same round's ui fixture then caught a regression of mine: restructuring `renderStatus()` dropped its `renderUsage()` call, so the meter stopped repainting — the check read a meter whose text had frozen at the fixture's last direct call, which is exactly the defect that check was written for in the first place. Restored, with the reason in the code. |
| 25 | 2026-10-13 | **B1/B2: the foot's two numbers became real, and the checks that prove it taught three things.** `stream-finished` now records the rate (completion tokens over the turn's wall time) and accumulates the chat's total, both of which had been drawing the dash for the whole of the B1-B3 round. Page 198 checks 0 failures; live 165 with its one long-standing red (the image bubble). **The three lessons, all paid for in this round**: (1) `repaint()` redraws the transcript, not the foot — the check reset the running counter and then read the *old* text the last pass had written, which looked exactly like an accumulator that ignores its own reset; the reset now goes through `renderStatus()`, the door the gesture uses. (2) A delta read off a rounded figure is a measurement of the rounding: the foot states `1.6k`, so a 165-token turn "changed the total by 200" and looked like the accumulation was wrong. Own the counter you are about to measure. (3) A string replace is not a rename: `#status-right` appeared both with a hash (querySelectorAll) and bare (`getElementById("status-right")`), so five references survived a "replace them all" and one of them crashed the live harness with a null dereference four checks later — which is what the harness's `rows=34` abort was. **Falsified**: deleting the accumulation reddens the live total check, dividing the prompt tokens instead of the completion tokens reddens the live rate check, both with `restored clean: yes`. **Still owed**: §5.1/§5.4 for the cache-hit half of popup #2, and a real assertion that the group is centred (today the layout suite proves it does not overflow at four widths). |
| 26 | 2026-10-13 | **§5.1 closed: the cache hit is a real number, plumbed through three crates.** The provider's cached prompt tokens now travel from the vendor usage (react) through @@TokenUsage::with_cached@@ into the token counter's own accumulator, out of the ReAct engine as @@cumulative_cached_tokens()@@, through the agent as @@last_stream_cache_tokens()@@, into the @@stream-finished@@ payload as @@cache_read_tokens@@, and into the foot as @@round(cache_read / prompt * 100)@@. Every step is additive: @@TokenUsage::new@@ still builds the value it always did, @@lifetime_totals@@ is untouched, and the new agent field sits outside the archived state. The decision worth recording is the one the dash already made: an absent breakdown is **unknown**, not 0% — a provider that never reports it leaves the counter where it was and the foot keeps saying @@Cache hit —@@. **Guarded at three layers**: a react counter test (a reported 96 is counted; a following turn with no breakdown does not reset it), a page check that a held percentage is drawn with its percent sign, and a live check that reads @@165 tok · Cache hit 80%@@ off a bridge reporting 96 of 120. **Falsified three ways**, each reddening exactly its own check with @@restored clean: yes@@: dropping the accumulator reddens the react test, dividing the completion tokens instead of the prompt reddens the live percentage, and removing the payload field reddens it into the dash. **A lesson paid for on the way**: inserting a method immediately above an existing one puts the new method BETWEEN the old method's doc comment and the old method, which silently moved @@lifetime_totals@@ and @@cumulative_token_usage@@ into the undocumented column — @@cargo check@@ named both, which is why this repo counts warnings. Page 199 checks 0 failures; live 166 with its one long-standing image-bubble red. |
| 27 | 2026-10-13 | **H0 landed: the window overlays its title strip, and the header owns y0.** `tauri.conf.json`'s first window now declares `titleBarStyle: "Overlay"` plus `hiddenTitle`, so the native strip stops pushing the webview down; the in-app header owns y0-50 as the reference's does and `--frame-top-clearance: 48px` (cloned from DSH's contract) keeps the traffic lights over the sidebar's brand row. Nothing in the page changed - the strip was never modelled in CSS, which is exactly why this needed a guard instead of a screenshot. **The interesting part is where it can be checked**, because H0 is a native key that no fixture can see: the config is asserted by `package_selftest` (the layer whose job is the install artifact, now 18 checks) and the page half by the layout probe, which reports the header's top and fails below 44 at each of the four widths. **Falsified in both places**, `restored clean: yes`: putting the strip back (`titleBarStyle: Visible`) reddens the package check with the value it read, and adding 34px of `margin-top` to `#chat-head` reddens the layout check at every width. What is left unverified is the honest remainder: whether AppKit draws the overlay as intended in a real window, which is OCU's to see and is recorded in the row. |
| 28 | 2026-10-13 | **S2 landed: the sidebar's global panels are real navigation.** Three 30px icon+label rows under New Session - Plugins, Automation tasks, MCP connectors - each opening the settings pane that owns its area through the existing `navTo` (`cap-list`, `jobs-section`, `set-mcp`). No new state and no new command: the rows are pure navigation, labelled from the same en/zh table as everything else, styled from the shell's tokens. **The guard that paid for itself was the one that already existed**: `markup_selftest` refused the first attempt with two findings, both real. The first was `data-i18n marks that would delete their children` - putting `data-i18n` on the button (next to its icon) means the translator writes `textContent` over the SVG, so the row would have gone icon-less in any non-English UI; the mark belongs on the label span, which is what the existing New Session row does. The second was `style.css uses undefined custom properties: --muted` - the token is `--faint` (or `--dim`), and a colour that resolves to nothing is a hover state nobody can see. Neither would have been visible in a screenshot of the English UI. **Guarded at the page layer**: each row is clicked and the check reads the pane that activates and the tab that gains `aria-selected`, because the row that opens Settings but the wrong pane is exactly the defect a shallower check would bless. **Falsified twice**, `restored clean: yes`: `nav-mcp` pointed at `cap-list` reddens its own row, and pointing a check at a section its row does not open reddens that check - so the assertion is reading the row's real target rather than the modal's default. |
| 29 | 2026-10-13 | **S5 landed: the avatar row is the account menu, and it borrows every contract it needs.** `#side-account` is a real button over the same avatar + `Local workspace` row, opening the shared row-menu renderer with four items that are controls this app already had - Settings, the shortcut reference, the header's theme cycle and its language toggle - reached through the shell's public setters rather than new logic. Escape, the arrow walk and the refocus come from `closeRowMenu(true)`, so the menu behaves like every other menu; the only thing the caller owns is `aria-expanded`, which the row now toggles. The standalone gear stays, as the row says. **Three things the round is worth remembering for.** First, `wireAccountMenu()` was written, correct, and never called - the page check said so immediately (no menu, `aria-expanded=false`), and the asymmetry with S2 explains why: S2's wiring was a self-executing loop, S5's was a function waiting for an init line that did not exist. Second, a check that throws on a missing precondition does not fail the suite, it *ends* it: dispatching Escape at a null menu aborted the run at 159 checks instead of 207, so the remaining checks never ran and the report was green-ish where it should have been red. Guard the action, assert the outcome. Third, the Settings-item assertion was tightened before it was trusted: an earlier check could leave the modal open, which would have laid out a pane whether or not the item worked, so the check now closes Settings first and the pane that appears afterwards can only be this item's doing. **Guarded at the page layer with five checks and falsified twice**, `restored clean: yes`: removing the refocus reddens this row's Escape check *and* the pre-existing keyboard-back check (two independent readers of one contract), and re-pointing the Settings item at the theme cycle reddens the pane assertion alone. |
| 30 | 2026-10-13 | **§5.2 landed, and the seam it needed was broken.** A turn's cost now rides on the message the provider answered with (`MessageMeta.usage`, written by the finish handler from the agent's own count), so a re-opened chat re-sums what a session spent instead of estimating it; `usageTotal` already preferred the live accumulator and fell back to the transcript, so the reader needed no new shape - only the right keys. **It did not have them.** The reader was summing `u.prompt` / `u.completion` while the writer emits `prompt_tokens` / `completion_tokens`: a reloaded chat would have found every usage-bearing message, set `seen = true`, added zero, and stated 0 tok - a measurement nobody made, which is worse than the dash it would have replaced. Nothing in the crate boundary checks that, because the two halves are Rust and JavaScript: the fix is asserted by a Rust round-trip test (a turn's usage survives serde, and an empty one is not written at all), a page check that a transcript carrying `meta.usage` is what the total counts, and a new `wiring_selftest` check that reads both directions out of source and fails when they disagree - the falsification reddens the page check AND the wiring check, two layers seeing one contract. Two lessons paid for here: a JSON field name is a contract, so it belongs in the guard whose job is cross-file vocabulary; and a heredoc inside a guard is still source, so an escape written once gets interpreted twice - `'
'` in the shell guard became a real newline in its python and broke the guard it was meant to extend (the symptom was a guard that printed nothing at all, which is why the first falsification showed `total=0 reds=[]` rather than a red). |
| 31 | 2026-10-13 | **T6 landed: the usage chip, and what a page check owes the page around it.** A finished turn now ends with `Usage {n} tok`, opened into a flat dialog with the turn's own ~~prompt/completion~~ split, the session total, and the token meter read once per open. Two decisions are worth keeping: the chip states the cumulative **through that turn** (`usageThrough` sums the transcript's own `meta.usage` prefix) rather than the session total, because the reference's chip is where a reader looks to see what this turn has cost so far, and because the §4 wording - cumulative plus this turn - would double-count once the accumulator has absorbed it; and a turn whose cost the store never recorded says *not reported* instead of 0, the same distinction the cache-hit dash makes. **The interesting part was the page guard's own hygiene.** Four checks went green immediately, and the run then died at 173 of 212 with `Cannot read properties of undefined (reading 'parentElement')`: my block had rendered its own transcript into the shared `#messages`, and the T3 checks that follow read `answerRow()` off the rows that were on screen before it (`.turn-foot[0].parentElement`). The count said exactly one thing - that something upstream had moved the page - and the message named the line. Rebuilding from the cache did not fix it: row signatures are deliberately lossy, so a fixture row and a real one can pair up. What fixes it is putting the **nodes** back (`replaceChildren(...priorRows)`), listeners and all, because the next check clicks one of them. A check that borrows the page's DOM owns putting it back. **M6 had to be scoped, not loosened.** Its token-meter check asserted `called('token_meter').length === 1` over the whole run; the T6 panel legitimately reads the meter once for its own open, so the count became 2 and M6 failed for a reason that was not a defect. The assertion now counts around its own `refreshContext()` gesture (`before + 1`) - same meaning, its own subject - and the T6 block asserts the rule M6 was reaching for: one read per open, zero per repaint. |
| 32 | 2026-10-13 | **T7 landed: the attachment opens as an original, and the one red left says what it is.** A picture in a message is a button that shows its thumbnail and its name and opens the document panel, which gained an `<img>` branch beside the text it already rendered; the reference's two labels (~~image.png, click to view original~~, ~~View full image: <title>~~) are both honoured, and the click-to-zoom that used to live in the row is gone with its dead CSS. The name is derived (`image-<n>.<ext>`) because a message stores data URLs and no filenames - inventing one would be a lie, and deriving it keeps the tab stable across reloads. **A regex that `node --check` accepts is still a broken regex.** The first `attachmentName` used `/^data:image\\/(…)`, whose backslash was interpreted twice (the shell-hygiene escape again, one round later, in a different language): the pattern would have matched a literal backslash and silently named every attachment after its base64 tail. `node --check` passed it, the markup guard passed it, and only the page check's exact `image-1.png` would have caught it - so the helper was rewritten without escapes at all (`startsWith` + `split`), which is the cheaper fix than remembering how many layers escape. **Two smaller ones.** The doc tab's text is not the document's title: the tab carries its own close control, so the assertion strips it rather than loosening to `includes`. And the live guard's image section could not be repaired here - an image-only turn leaves `#messages` empty (a gap of its own), so the viewer assertions would have been unreachable; they now sit behind a named check that states the missing row instead of an uncaught `TypeError` from a wait that never sees it. A check that cannot be reached cannot fail, so the red was made to name the thing that is actually absent. |
| 33 | 2026-10-13 | **W2 landed: the mascot is decoration, and the decoration is the testable part.** A 64px `bos-harness-icon.svg` sits at the top-right of the transcript while a turn streams, inside `#chat-head` so it lands where the reference measured it (above the W1 badge), `aria-hidden` with an empty `alt` and `pointer-events: none` - it states nothing and cannot be clicked through, and the badge beside it stays the only control. The one decision worth recording: it follows the **stream**, not the badge's dismissal. The toggle therefore sits *above* `renderWorkingBadge`'s early return; putting it beside the badge's own visibility would have made hiding the card hide the character, which is a statement about the wrong thing. **Two rules the round paid for.** First, a check that drives a full render pass owns the DOM that pass redraws - `renderAll()` re-renders the transcript, so the rows had to be put back exactly as the T6 and T7 blocks do, and the first run died at 178 of 217 with `answerRow` again. That is the same lesson twice in two rounds, so it is a pattern now: reach for `renderAll` or `renderMessages` in a page check and you have borrowed `#messages`. Second, never read an animated property immediately: the check asks for `visibility` (discrete, flips at once) rather than `opacity`, because the 120ms transition means an opacity read right after the class toggles is still on its way and would pass or fail depending on the clock. The transition itself is not asserted at the page - the markup guard owns durations - and all three injections redden the page check while the markup guard stays green, which is the honest statement of which layer sees this. |
| 34 | 2026-10-13 | **C3 landed: the placeholder is the reference's sentence, and falsification showed which guard is blind to what.** The composer now ships ~~Message or run a task, / commands, @ files or sessions~~ (two lines, zh alongside it) and the ~~Enter = send · Shift+Enter = newline~~ hint moved into the textarea's `title` behind a new `composer.enterHint` mark. That hint was the fourth clause of one long placeholder, which is precisely the clause a narrow composer truncated; keeping it discoverable without keeping it in the sentence is the whole item. **The interesting part was the falsification batch, not the change.** Restoring the old long placeholder reddens the markup guard and *not* the page guard: once `setLang` runs, the applier has already overwritten the DOM placeholder from polish.js, so the page can only ever see polish's string and never the shipped attribute. Editing polish.js's zh sentence reddens the page guard and *not* the markup guard. Putting the hint back inside the placeholder reddens markup and is invisible to the page for the same reason, while deleting the `title` reddens both page assertions. Four injections, four reds, each at the layer that actually holds the value - so the checks are not redundant, they are scoped, and the row says which is which rather than letting a future reader assume either one covers both. **Two cheap habits paid for.** The markup guard's `ok()` counts checks itself, so an explicit `checks=$((checks + 1))` inflated the baseline from 95 to 97 - the delta after adding one check must be one, and reading the printed number is what caught it. And the escape family came back a third time (a TS template into a python triple-quoted literal eating the two characters backslash-n and producing a real newline inside a `split` call); the fix that has held every time is to stop writing the escape at all and use a placeholder token replaced by `chr(92) + "n"`. |
| 35 | 2026-10-13 | **C4 landed: the chip says the model's name, the control keeps its id.** `#model-label` now shows `shortModelId`'s last segment and `#model-btn` carries `Model: <full id>` in both its `title` and its `aria-label`, while the empty case still says `no model`. The reference's split is worth restating because it is the whole item: a chip cannot hold a three-segment path, but shortening the *visible* text is only honest if the full string survives where a reader - hovering or hearing it - can still reach it. **The second check earns its place by being a different shape.** The first sets a three-segment id and asserts the no-slash chip plus the full title and aria-label; the second sets a model with *no* path and re-asserts both places. On its own the first cannot catch a one-way update (code that writes the label only when it is empty would pass it, because the previous value in the fixture is not the value being asserted) - that is a real defect class, not a hypothetical, and it is the kind a fixture with one model hides. **Falsification did what the row claims**: naming the whole path reddens the first check (`chip=nvidia/z-…`), deleting the `aria-label` write reddens both (`aria=null`), and deleting the `title` write reddens both with a stale title. The list the chip opens is a row menu into Settings and is deliberately untouched, so the "popup still shows full ids" half of the spec is a statement about a surface this item does not own. |
| 36 | 2026-10-13 | **T4 landed: a goal transition is a note between turns, and a row window is not a transcript.** The advance appends a `LogMarker` to the record, the finish handler moves it onto the first turn it writes (as `MessageMeta.marker`, next to the token count and for the same reason), and the transcript draws it - once - as a centered pill. **Three things the round paid for, each found by a check refusing to pass.** First, `#messages` is a *window of turns*: the reconcile removes every node that is not in `desired`, and a pill that is not a turn has no row to ride, so appending it in the millisecond before the reconcile is the same as deleting it. It is drawn after the reconcile now, and the next pass removes the previous copy, which makes the on-screen state always this pass's. Second - and this is the one worth remembering - **a change to the transcript's shape must mark it dirty**. The transcript's *messages* had not changed when the marker appeared, so the dirty cursor let the render pass stop early and the pill was never reached at all: the injection that removed the `markDirtyAll()` call reddened the check, which is how the call was found to be load-bearing rather than decorative. Third, the marker is a statement about the *gesture*, so it lands with the gesture instead of after the round trip, and the catch takes it back if the backend refuses - an optimistic pill that outlives a transition that never happened is the same family as the stale global status. **Two smaller things about checks.** A failure that says only `(none)` is a failure nobody can act on, so the precondition - "the gesture recorded itself against the transcript's own length" - became a named check of its own instead of a clause inside the pill's assertion; the pill's assertion kept the `called`/`lastArgs` read-back, which is what proved the command really was sent as `advance`. And the function-shaped fixture was the wrong tool here: the page dispatcher's per-argument support belongs to the folder chooser, so the fixture is a plain object and the *argument* is asserted through `lastArgs` instead - a better check anyway, because it tests what was asked rather than what was answered. |
| 37 | 2026-10-13 | **§5.3 landed, and the round mostly learned where a check belongs.** `stream-finished` carries a `cancelled` flag now, written where the stop was honoured and keyed to the *generation* it was aimed at (`take_stop`), so one press cannot mark a later turn and a re-open still knows. Three unit tests pin that rule and a generation-blind match reddens the one about another turn. **The lesson is the check that passed for the wrong reason.** The first live check for the backend source went green - because the foot it read still belonged to the *stopped* turn above it, so the stale verdict satisfied it; moved beside a completed turn it failed truthfully, which is the honest state: the live double cannot separate the two sources at all, since its pressed turn makes both true at once. That is not a reason to accept the live check; it is the reason the decision is a pure function (`turnVerdict`) now, guarded in `ui_selftest` per source - dropping `payload.cancelled` reddens exactly the backend case and leaves the press case green, which no live check could show. **Second, a guard is not a playground.** Adding one turn to the live suite moved its outcome from 168 checks to 122 and then to 94, because each turn spends virtual time and a `waitFor` that times out ends the run; and running `ui_selftest.sh 200` produced a red in a *different* check, the reload-pairing one, which is written for the default 5000 - the guard as the harness runs it is the guard that means something. Both were self-inflicted, both were found by isolating against a reverted tree rather than by arguing, and both are why this round's own additions moved down a layer instead of up. |
| 38 | 2026-10-13 | **The two "pre-existing reds" were one bug in the double, and the perf guard had never been run.** The live suite ends at `0 / 171` - its first all-green run here - because the image-only turn was never a page defect: the double built every chat in `w-main`, so a chat created while another workspace was active was correctly not shown, and the reader got an empty panel beside a turn that did run. A temporary probe printed `active: null` with that chat's messages sitting in the cache, which is what pointed at the workspace filter rather than the renderer; rev 32's "empty-transcript gap of its own" was my diagnosis, not the behaviour, and this is the correction. **Second: a guard nobody runs is not a guard.** `perf_selftest.sh` was `1 / 19` - S4's live age in the session row mutates one `aria-label`, and the check asserted *zero* mutations of the list, which one attribute write fails as surely as a rebuild. It now counts structural records (`childList`, drained on both the callback and `takeRecords` paths) separately and bounds the attributes: injected into the frame it reddens at once (`306 mutations, 305 structural`) while the tick's label refresh passes at `0 / 19`. **Also:** `SessionRecord::new` said "New Session" where `derive_title`, the sidebar and the header all say "New chat" (the button is the only thing named "New Session"), which had left a unit test red. **And §3's status column was audited against §9** - 21 stale cells claimed items that have been green for rounds, T2 now names its reason (§8), and every cell states the row's real status. |
| 39 | 2026-10-13 | **Two guard gaps of my own disclosure, closed; one claim of coverage, withdrawn.** T4's durable half had only a Rust round trip and the compiler behind it, so a fourth page check clears the pending hint, seeds a turn carrying `meta.marker` and demands the pill back from that record alone - removing the row loop's push reddens exactly it while the pending-hint checks stay green. The jobs chip was checked counting (`1 background job running`) and never at zero: it now hides (and comes back), falsified by a `toggle("hidden", false)` that reddens the hide check alone. **And §6 claimed `centered@4` for B1-B3 in the layout column - no guard asserts the group's centring at any width**, so the cell says what is true (page owns the texts, popups and numbers) instead of what would have been nice. Recording a claim of coverage is how a reader stops asking for the check. The real-window pass is still owed: the app was not running when this round looked (`ocu call list_apps` lists Chrome, Ghostty, Code, DSH - no BOS), and launching it from here is not something this session may do. |
| 40 | 2026-10-13 | **The last two red rows land: H1 and T1.** The jobs chip is now a preview of the pane it names - a caret, `aria-haspopup`/`aria-expanded`, and a popover of the pane's own rows (label + state word) whose pick lands on Jobs rather than opening a second manager; the tool row is the reference's shape, glyph + kind + `·` + one-line summary, with a path drawn in the code face over a dotted underline and a command with an argument left alone. Both are guarded (page 229, ui 183) and the ui side falsifies: removing the summary span makes the fixture throw. **One falsification did not work and is written down rather than implied**: an injected early return in `openJobsMenu` left the H1 checks green, so a second path reaches that popover and I did not find it before this round's time went; the check closes any menu left open by an earlier check, so what it asserts is still the menu its own click made. Two smaller findings of the "fix the guard" kind: the ui fixture had a literal `\u0027` in a tool argument (my own patch escaped an apostrophe the JS never saw, and the runner reported it as a load error - one check, one failure), and the chip's text now carries the caret, so the two checks that assert it say `1 background job running ⌥`. |
| 41 | 2026-10-13 | **The real window was read, and it agrees.** The round built the bundle, launched it and pulled the accessibility tree plus a screenshot with `ocu`: the traffic lights sit in the header and the webview starts at y0 (H0), the sidebar's nav, account popup and `B` avatar are there, the composer carries the circular ⊕ popup named *Add files or run commands*, the short model chip with the full id in its a11y name, the two-line placeholder and a disabled circular Send, and the status bar reads `0 chats`, `0 turns 0 steps · — tok/s`, `— tok · Cache hit —`, `0% of context used` - so the dash rule that `0 tok` must never fake is visible in the shipped app, not only in the fixture. The jobs chip is absent with nothing running, which is H1's zero state seen for real. An empty chat has no transcript, so T1/T3/T4/T5/T7/H2/S3 stay on the fixtures rather than costing model quota. The app is left running for the reader; quitting it is theirs. |
| 42 | 2026-10-13 | **The real-window read became a guard.** `crates/gui/tests/ocu_selftest.sh` pulls the accessibility tree of the running bundle and asserts ten of the facts rev 41 observed, printing a reason and exiting when `ocu` is absent, the permission is missing or no BOS is running - it reports that it did not measure rather than passing silently. Falsified by pointing the same script at another app's window (`BOS_APP_NAME`), which reddens **9 of 10**: the survivor is the `Account menu` popup name, which the reference app also publishes, so that one assertion is real but not specific - the cross-app probe is what separated the two, and the script says so. The caret half of the H1 assertion was exercised by that same probe (`1 background job running`, no caret, red). `AGENTS.md` now lists the guard beside the other layers. |
| 43 | 2026-10-13 | **All three disclosed gaps are closed.** (1) The status bar's centred group has a check: the probe reports its offset from the footer's centre and `layout` bounds it at 24px at all four widths - the honest number is **8px off at every width** - and a `margin-left` injection reddens three of the four. (2) T4's rule moved out of the command and onto the record (`mark_advanced` / `apply_pending_marker`), so the promise that an advance waits for a turn and lands **once** is a unit test instead of one field assignment; `take()` replaced by `clone()` reddens it. Landing it also caught my own defect: the test I inserted consumed the `#[test]` of `a_meta_without_a_marker_writes_none`, which had silently stopped running - the suite said 373 when only my copy of that number was real, and says **373 with no warnings** now, with the meta test back on the count. (3) H1's check **is** falsifiable: making `openJobsMenu` throw reddens both checks (`2 / 229`). The earlier `if (false) return;` at the same anchor stayed green, and rev 40 read that as a second path reaching the popover; rev 43 searched and it is not there - `openJobsMenu`, `jobsChip` and the only `openRowMenu(jobsChip, ...)` are single-sourced, so the unexplained part is the injection, not the wiring, and it is recorded that way. The check's precondition is now inside the assertion (`menus before=0`), so a popover left over from an earlier gesture cannot pass it. |
