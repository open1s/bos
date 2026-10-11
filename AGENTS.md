# BOS Agent Instructions

High-signal facts for working in this repo.

---

## Workspace Structure

```
crates/
├── agent/          # Core agent: tools, skills, hooks, plugins, sessions, memory
├── bus/            # Pub/sub, queryable, caller/callable
├── config/         # TOML/YAML config loading
├── jsbos/          # Node.js bindings (NAPI-RS)
├── logging/        # Tracing, instrumentation
├── nbos/           # Python bindings (cdylib, maturin)
├── qserde/         # rkyv archive support used by the bus wire format
├── qserde_derive/  # Derive macros for qserde
├── react/          # ReAct engine, LLM integration
└── resource/       # Standalone resource subsystem + `rex` CLI (no dependents)
```
---
## Version Control
This repo use jj(jujutsu) for version management
Core Concepts
- change_id: Unique identifier for a change (use this, NOT commit hash)
- stack: Ordered list of changes (your working history)
- working copy: Always attached to a change

Key difference from Git:
jj is stack-based, not branch-based. You are expected to edit, reorder, and clean history before pushing.
```bash
# Status
jj status

# Create new change
jj new

# Describe change
jj describe -m "<crate>: <title>"

# View stack
jj log

# Edit change
jj edit <change_id>

# Split change
jj split

# Squash into parent
jj squash

# Reorder changes
jj rebase -r <change_id> -d <destination>
```
---

## Essential Commands

```bash
# Build the workspace
cargo build --workspace

# Test single crate / whole workspace
cargo test -p <crate>
cargo test --workspace

# Lint (warnings are errors) and format
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# API docs (fails on any missing-docs warning)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Standalone resource CLI
cargo run -p rex -- --help

# Python binding (crates/nbos)
cd crates/nbos && maturin develop

# Node.js binding (crates/jsbos)
cd crates/jsbos && npm install && npm run build
```

---

## Crate Dependencies

Each layer depends only on the layers to its right:

```
nbos / jsbos  →  agent  →  react  →  bus  →  logging  →  config
```

- `qserde` / `qserde_derive` provide the rkyv-backed wire format that `bus`
  serializes payloads with.
- `resource` is an independent subsystem; nothing in the agent stack depends on
  it (it pulls `bus`/`config`/`logging` only for its optional `cli` feature).

**Key**: Agent-to-agent messaging and pub/sub flow through `bus`. Direct
request/response work (LLM calls, tool execution) flows through the trait seams
in `react` (`LlmClient`, `Tool`/`AsyncTool`), not the bus.

---

## Python/JS Bindings

| Bindings | Entry | Build |
|----------|-------|-------|
| Python | `crates/nbos/` | `maturin develop` |
| JS | `crates/jsbos/index.js` | `npm run build` |

**User guides**: `docs/python-user-guide.md`, `docs/javascript-user-guide.md`, `docs/rust-user-guide.md`

---

## Unified API (Python ↔ JS)

Python and JavaScript APIs are designed to be consistent:

```python
# Python
from nbos import BrainOS, tool

@tool("Add")
def add(a, b): return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("What is 2+2?")
```

```javascript
// JavaScript
const { BrainOS, ToolDef } = require('brainos');

const addTool = new ToolDef('add', 'Add', (args) => args.a + args.b, ...);
const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').register(addTool);
const result = await agent.ask('What is 2+2?');
```

---

## Testing Notes

- Use `#[tokio::test]` for async tests
- Run with: `cargo test -p <crate> name -- --nocapture`
- Set `RUST_LOG=debug` for tracing output
- Current green baselines:
  - Rust (excluding the binding crates): 443 passed, 0 failed, 8 ignored
  - Python (`cd crates/nbos && pytest -m "not llm"`): 229 passed, 4 deselected
  - JS (`cd crates/jsbos && npx ava`): 94 tests (13 parity, 3 native-binding, 21 API-doc, 9 content, 14 memory)

---

## GUI Testing (crates/gui)

The GUI is guarded in layers. **Pick the lowest layer that can see the behavior** — a
render bug belongs in a fixture guard, not e2e. All guards are shell scripts under
`crates/gui/tests/` and must stay green; each ends with a `failures: N / M` line.

| Guard | Runs against | Sees | Use for |
|---|---|---|---|
| `ui_selftest.sh` / `shell_selftest.sh` | own fixture DOM (`ui_selftest.html`, `shell_selftest.html`) | pure render functions, repaint timings, shell contract | renderer logic, keybindings, panel registration |
| `markup_selftest.sh` | shipped `index.html`/`app.js` as text | structure, a11y attributes, i18n marks, duplicate ids, CSS custom properties, `#` selectors handed to `$` (which is `getElementById`) | structural invariants no fixture can reach |
| `layout_selftest.sh` | shipped page, probe-injected | geometry at widths 1440/900/560/500 | overflow, off-screen, drawer, squeeze |
| `package_selftest.sh` | `tauri.conf.json` + `icons/` as files | identifier/product agreement, `frontendDist` and `index.html` present, no build step, every listed icon exists and carries its format's magic bytes, one icon per platform | the install artifact — a renamed or truncated icon fails the bundle silently otherwise |
| `wiring_selftest.sh` | source text, both directions | `#[tauri::command]` ↔ `invoke()` names, and the plan status vocabulary (`PLAN_ICONS` vs the tool's JSON schema) | every command reachable, no phantom calls, no cross-crate vocabulary drift |
| `page_selftest.sh` | shipped page + **settled-fixture** bridge | handler → invoke → render, one microtask pass | static wiring of a panel/dialog on the real markup |
| `live_selftest.sh` | shipped page + **stateful async** bridge (`tests/live/bridge.js`) | streams that end, queues that flush, approvals, rollbacks, PTY attach/write/kill, compaction splits, job tails, self-inspection, replay, scoped approval rules, durable reminders (add → tick → deliver-late → cancel → clear), knowledge curation (add → refuse a blank title → search → edit in place → delete), a per-tool MCP policy (deny → the row, the server badge and the self-inspection inventory all follow; ask → the default select; clear a rule), adding a workspace (the sidebar opens its own dialog, the system panel's path lands while a dismissal changes nothing and an absent panel opens the in-app fallback with the reason, the fallback walks a real tree and reports a folder it cannot list, the MCP switches travel as one payload, the workspace lands in the store and the sidebar, a refusal keeps the dialog open with the fresh reason), authoring a skill (the pane's listing against the double's directory → Use prefills the composer → a save lands in the pane, the capability list and the double's disk → a second save refuses by path → Replace overwrites in place), reading a server's published resources (the catalog listing → a row read into the pane → a body cut at its cap with both numbers → a URI the server does not hold → a resource with no text) | behaviors that only exist against a live backend |
| `perf_selftest.sh` | shipped page + the same bridge, asked for a heavy workspace via `window.__PERF_SCALE__` | frames requested, repaints run, DOM mutations, rows drawn, round trips, line caps held, per-turn cost across a soak | what a behavior costs — work-per-burst budgets for the streaming hot path and the shell's output flood, plus a 10-turn soak proving the drawn window and the per-turn cost do not grow with the session |
| `e2e/` (WDIO + `tauri-driver`) | **the real app binary** | everything, including the Rust side | Linux/Windows only — tauri-driver has **no macOS backend**; the preflight blocks it |

**Skipping work needs an exact comparison.** The transcript's per-row signature
is deliberately lossy — `length` + the last 24 characters — because a full hash on
every streaming frame would be O(n²), and that is enough to notice a new token.
It is *not* enough to notice a rewrite of the same length and tail, which is
exactly what a backend finalizing a reply does; pairing reloaded messages by that
signature silently kept the draft on screen. Compare content when the decision is
"is this the same message", and reserve the signature for "did the streamed tail
change".

**A test that asserts a count must own its file.** A store test that adds a
record, saves, edits the file by hand and asserts "exactly one entry survives
load" passed when run alone and failed after any earlier run, because it read
whatever an earlier run had left in the temp directory — the failure surfaced
during a falsification batch, in the *restored* state, which is what gave it
away. Delete the file first (and at the end); an assertion about a number is an
assertion about the whole file.

**A re-attach that clones can only add.** A policy change has to reach the
agents a session is already running, and the first version of that code cloned
each cached agent and attached the survivors to the clone — which left a tool the
policy had just denied sitting in the registry, because the clone still held the
old wrapper. It was written, documented as reaching existing agents, and would
have shipped silently: nothing drove it, and the tests that existed only built
*fresh* agents. Build the new agent from the base (`Inner::build_agent_for` is
now the single construction path for a first build, a re-attach and the connect
pass), because only a from-scratch build can remove. The general shape: when a
function's promise is "this reaches what already exists", the test has to look at
what already existed.

**A settled fixture answers the same thing before and after.** Three checks for
the skill pane's cache invalidation were written, a falsification batch removed
the invalidation, and all three stayed green: the page guard's fixtures are
static, so the capability list and the palette answered with the same skill
list whether or not the save had re-read them. Asserting on the render cannot
detect a cache drop — the *round trips* are the observable, so the check counts
`list_capabilities` calls around the save and around the palette's next open
(`2→3 then 3→3` with the invalidation removed). One of the three had a second
flaw of the same family in the other direction: the palette caches on its first
open since the last close, and the check had never opened it before the save, so
there was no stale cache to keep and the check could not have failed for the
right reason. A falsification that stays green is a finding about the check, not
about the code — write the precondition down too. And a batch mutates the
machine, not only the source: removing the containment rule during this batch
left a real `SKILL.md` at `../evil` and one in `crates/gui/nowhere`, which made
the two tests asserting those paths are empty fail in the next *workspace* run
while passing in a crate-only run. Name a path you own (or delete it first) when
you assert about one.

**A falsification batch mutates the machine, and a killed batch keeps mutating
it.** This round's first batch was started with `nohup … &` inside an ordinary
tool call; the harness reaped that process group when the call returned, and the
last injections it had applied — a `take(MAX_ROWS + 1)` in `mcp_view.rs` and a
row click that had lost its server in `app.js` — stayed in the tree. The *next*
batch then measured a mixture of its own injection and that residue (the Rust
results were nonsense: a test about `empty` failing under an injection about
argument counting), and its own "restored clean: yes" line was vacuous because it
searched `jj status` for a phrase that never appears there. Two habits fix it:
long-running work goes in a **managed background job** (`run_in_background`, no
`nohup`, no `&`), and the residue check **snapshots the bytes of every file the
batch touches and compares them when it ends**, printing the names that differ.
A check that cannot fail is worse than no check: `"restored clean" in status`
was green while three defects sat in the source.

**A check whose precondition makes both answers the same cannot fail.** Two of
this round's checks passed their injections and had to be rewritten. Injecting a
name-ignoring `read_target` left the Rust suite **green**, because the only test
for that refusal built an `Inner` with *no* clients — where "the one that was
named" and "the first one there" are identical answers; it now seeds two live
clients and asserts the second is returned for the second name. On the page side,
a row that read from the shared picker instead of from its own server was
invisible while the fixture had a single connected server (the picker's default
*was* the row's server); the fixture grew a fourth server and the check clicks
*its* row while the picker names another. Write the precondition down next to the
assertion: a fixture that cannot distinguish the defect is a fixture, not a test.
The Add-workspace batch produced a third instance from the other direction:
**redundant guards make a check indistinguishable from itself.** A missing root is
refused twice — once by the `metadata` lookup, once by `canonicalize` — so weakening
the lookup left "a missing root is refused" green. The test now also hands the
function a path that *exists and is a file*, which only the directory rule refuses.
When two code paths produce one outcome, no test on that outcome covers either one.

**A mutation count must be drained in both places.** `takeRecords()` returns only
what is *still queued*; records delivered to the observer's callback are gone by
then, and an empty callback throws them away. Count in the callback **and** with
`takeRecords()` — one path alone silently drops everything that spans an `await`,
which is every turn, flood and post-frame repaint.

**A settled fixture cannot answer per argument.** `page_selftest.sh`'s fixtures
are static, so a command whose answer *depends on what it was asked* — the folder
chooser, which returns a different level for each path — cannot be checked with one
frozen value. The dispatcher now calls a fixture that is a function, and the folder
fixture is one; before that, the "descend, then Use this folder" check asserted the
path of the level the fixture always returned, which is a check that would have
passed for the wrong reason. When a fixture has to lie to make a gesture checkable,
prefer teaching the fixture the argument.

**A stale global status makes a fresh verdict unknowable.** The Add-workspace
dialog decided whether it had succeeded by reading `state.error` *after* the command
— and any earlier failure in the run left that non-empty, so a **successful**
creation kept the dialog open and blamed the folder for it. The live guard caught it
(a `waitFor` for the dialog to close timed out). Clear the status before the attempt
and let the verdict be this attempt's; a global that outlives its operation is not a
result, it is a rumour.

**A group derived from its members cannot show an empty one.** `groupSessions`
built its groups by walking sessions, so a workspace with no chats was not a group at
all — which meant a folder just added was invisible until someone happened to start a
chat in it, and `+` had nothing to aim at. Groups are now seeded from the workspace
list as well (the ones sessions did not mention come after them, in configured
order), and the ui fixture asserts the empty group directly. The general shape: a
list that exists to show a *set* must not be derived from the subset that happens to
have members yet.

**Scope a fixture query to the thing being asserted.** A check that "the remove form
has no name field" searched the whole document for `.ws-name` and passed only
accidentally, because no workspace header happened to be drawn behind the form; once
empty workspaces gained a group, the header's own `.ws-name` span made it fail. The
selector is now `.ws-form .ws-name`. An unscoped query in a fixture is an assertion
about everything else on the page.

**A modal cannot be driven — swap the modal call, do not skip it.** The platform's
folder panel (`osascript`/`zenity`/PowerShell, `crates/gui/src/picker.rs`) is a dialog
on the user's screen: no guard can click it, and no unit test can either. Leaving it
untested is how a typo in the AppleScript wrapper ships, so the test runs **the same
argv with only the panel statement replaced** by a reference to a folder that exists.
That puts the `-e` list, the escaping, `POSIX path of` and the error-handler syntax
under the real interpreter. The one statement the swap removes is still checked, by
handing the **real** argv to `osacompile`, which validates the AppleScript — the
`choose folder` verb, its parameter names, the handler — and writes a compiled script
**without opening a panel**; a typo in the verb fails that check and nothing else.
Exactly one thing is left unverified: the human click and what AppKit does with it,
which is reported as such rather than claimed as automated. Two
consequences worth copying: the swap is only honest if you assert it happened (a
statement that silently fails to match would leave the panel call in place and hang
the suite), and the shape test must also assert the parts a *successful* swap cannot
exercise (the `-128` cancellation handler is why the panel can be dismissed without a
red error — nothing else in that test would notice if its number were wrong).

**Cancelling is not failing, and a modal's answer deserves a type.** `ChooseOutcome`
is `Picked` / `Cancelled` / `Unavailable` rather than `Option<String>` plus an error,
because the three answers lead to three different UIs: a path lands, a dismissal
changes nothing and writes no error, and an absent panel opens the fallback **with the
reason**. Collapsing them is how "changed my mind" becomes a red message. Building it
found a small defect of the same family the guards exist for: the fallback's reason was
written to the hint and then `showInAppBrowser()` — which sets the same hint — was
called, so the user was told the fallback had opened and never why. One element written
by two functions in sequence is a lost update; pass the reason into the reveal.

**Flat is a rule, not a taste, and it is also the cheap option.** The Add-workspace
dialog carries no shadow, gradient, blur or transition longer than a 90 ms hover
colour; every state is one solid fill plus a 1px border; pointer hover and keyboard
focus are the *same* colour change; sizes come from the shell's scale (`--space-*`,
`--radius-*`, `--control-h`, `--font-ui`). The mechanism behind it: `contain: content`
on the dialog card and on each of its scroll regions, one capability read **per open**
(never per repaint), the MCP list read from state at **zero** round trips, no geometry
reads in the folder browser, and no `innerHTML` anywhere in it.

**Flat covers the whole chrome, and it is checked in two places because each is blind
to the other.** Rev 108 removed the last seven shadows, the one `filter: brightness()`
and 53 literal corner radii from `ui/style.css`, and both layers now own the rule.
`markup_selftest.sh` scans the stylesheet as text — no `box-shadow`/`text-shadow`/
`backdrop-filter`/`filter` other than `none`, no `gradient(`, no literal px radius, no
non-`transform` transition over 120 ms — with two exemptions named in the guard (the
streaming caret's `steps(1)` blink; the mobile drawer's `transform` slide), because a
rule whose exceptions are implicit is a rule that quietly grows one. `layout_selftest.sh`
asks every laid-out element in the shipped page what it *paints* (`painted=[]`), since
the scan cannot see a shadow set from JS and the probe cannot see a rule that never
applies; injecting one `.modal-card` shadow reddens both. Radii come from
`--radius-xs|sm|md|lg` (4/6/8/12), and overlays take `--border-strong` instead of a
shadow — on a dark background a shadow was buying almost nothing and the edge was
already the depth cue.

**A check that calls the inner function cannot see the pass that owns the element.**
The token meter passed its ui check for months while it never painted in the app: the
check called `renderUsage()` directly (the meter's text and near-limit class were right),
and the defect lived one level out — `renderStatus()` cleared `#status-right` before
filling it, so the meter was built and thrown away on every streaming frame. Fixing it
took three attempts, each caught by a check written to enter through `renderStatus()`:
the first still lost the meter to `renderArchived`, which clears the same host, so a value
written between two clears is a lost update even when each writer looks correct on its
own. Enter the door the user's gesture uses, and when several functions write one element,
find out which of them clears it.

**A bound proven on one path is not proven on another.** The perf soak showed the drawn
transcript stays at `WINDOW` rows across a ten-turn stream, and the ui fixture showed one
scroll-up adds one window — both green, while the scroll *handler* grew the window per
scroll **event**. A gesture fires dozens per frame, so one flick measured 50 events →
`windowExtra` 10 000 → the whole 1 000-row chat drawn: the documented cap held only until
the first scroll-up. The check that sees it drives the burst the user's hand produces (50
dispatched events, one render) instead of the single event the code was written for. When
a limit is stated over a gesture, count the events one gesture can deliver.

**Why `perf_selftest.sh` counts work instead of timing it:** it needs
`--virtual-time-budget` for the bridge's pushed events to land, and under virtual
time the clock advances only on timers — a synchronous 201-row repaint measures
`0.00 ms`, which is a lie about cost. So it counts frames, repaints, DOM
mutations, rows drawn and round trips. Wall-clock milliseconds live in
`ui_selftest.sh`, which runs a static fixture **without** virtual time and
therefore reports real ones.

### Writing a live e2e check (`live_selftest.sh`)

This is the macOS-reachable end-to-end loop: the shipped page runs with a backend
double that holds state across commands, delivers events on timers, and can fail a
command on demand.

1. **Backend** (`tests/live/bridge.js`):
   - Add the command to `handlers` (return plain JSON in the Rust command's shape).
     A command the UI calls with no handler is recorded as an error — the harness's
     hygiene check fails, on purpose.
   - Streamed turns are scripted in `scriptTurn`, keyed by the message text
     (`/\bwrite\b/` → approval scenario, `/slow/` → long turn, else plain). Emit
     `agent-event` / `approval-request` / `stream-finished` payloads in the exact
     shapes app.js consumes (`args` is a JSON **string**), and write the final
     assistant turn into `store.cache` at finish, because the UI reloads the
     transcript from the backend when a stream ends.
2. **Checks** (`tests/live/live_checks.js`):
   - Assert on what a user would see (`#messages` text, `#status-right` spans,
     `.session-row`), not on internals. Use `waitFor(fn, "what")` — it fails with
     a readable name. Bridge assertions come from `LIVE.called(cmd)` /
     `LIVE.lastArgs(cmd)`.
   - Interact the way a user does: `type(text)` sets the composer value **and
     dispatches the `input` event** (the value alone leaves the Send button
     disabled). Queue a follow-up with **Enter** on the composer — clicking the
     send button while streaming means *Stop*. Tool output lives behind the
     collapsed `.tool-head`; click it, then `repaint()`, before asserting.
   - Error paths: `LIVE.failNext(cmd, "message")` makes the next invoke reject;
     assert the rollback (pair gone, prompt restored, `.err` span).
   - The file must end by appending `<pre id="result">` with
     `{checks: {desc: "PASS"|"FAIL"}, failures}` — `read_result.py` parses that
     convention, and the `catch` writes it too (with page state in the message),
     so a red run says what the transcript showed.
3. **Runner** (`tests/live_selftest.sh`): assembles the temp page — `<base>` as
   the **first** child of `<head>` (URL resolution happens per parsed element),
   bridge before `app.js` (it reads `__TAURI__.event` at module load), checks
   before `</body>` — then drives headless Chrome with
   `--virtual-time-budget=15000`. Timers fast-forward; **rAF starves after the
   first frames** (a nested rAF never fires — measured), so after any interaction
   that schedules a repaint, call `repaint()` (it runs `window.renderMessages()`,
   the same work the frame would do).

Conventions: check names read like the behavior they prove; never add comments to
`bridge.js`/`live_checks.js` that restate the code; run `node --check` on both
files; new checks must fail first if you want them trusted (drive the page wrong
once and watch it go red).

### Dev run on macOS (dock icon)

A bare `target/debug/bsh` gets a second-class dock icon: the dock conforms
runtime-set icons on its own (content is scaled to fill the standard
squircle, canvas margins are cropped away), so resizing the artwork's tile
changes nothing — and the icon cache (`/var/folders/…/C/com.apple.dock.iconcache`)
keeps a stale render alive across rebuilds. The fix is a real bundle:
`bash crates/gui/make-dev-app.sh && open target/debug/BOS.app` — it wraps the
binary with Info.plist + icns so the icon goes through the same code path as
every bundled app (verified pixel-identical to Docker/Chrome: 824/1024 hard
extent, standard corner mask). Re-run the script after every `cargo build`.

### The real-app suite (`e2e/`)

`cd crates/gui && cargo build -p bsh && cd e2e && npm install`, then
`xvfb-run --auto-servernum npm run e2e` (Linux) or `npm run e2e` (Windows,
msedgedriver). `BOS_APP_BINARY` picks the build, `TAURI_DRIVER_URL` attaches to a
running driver. Do not try to make it run on macOS — that is a documented
platform limit, and `live_selftest.sh` covers the same intent here.

---

## Key Patterns

- **Tools**: Wrap a closure with `FunctionTool` (sync) or `AsyncFunctionTool`
  (async, may await I/O), or implement `Tool`/`AsyncTool` directly; register
  with `ToolRegistry::register`/`register_async`; the agent
  invokes them through the `react` seam (`LlmClient`, `Tool`), never the bus.
- **Bus**: Build a primitive and attach a session.
  - Publisher: `Publisher::new(topic).with_session(session)?.publish(&payload)`
  - Subscriber: `Subscriber::new(topic).with_session(session)` then
    `recv()` / `recv_with_timeout(d)` / `run(handler)`
  - Query: `Query::new(topic).with_session(session)?.query(&req)` against a
    `Queryable::new(topic).with_handler(..)` or `with_stream_handler(..)`
  - RPC: `Caller::new(name, session).call(&req)` against a
    `Callable::new(uri, session).with_handler(..)` then `start()`
  - Raw topic access: `session.publish(topic, &payload)` /
    `session.subscribe(topic, handler)`
- **Memory**: `agent::memory::MemoryStore` stores and recalls text across
  turns; `InMemoryMemory` ranks items by keyword overlap and breaks ties
  toward recency. The trait is object-safe, so a vector or file backend
  implements the same seam. `FileMemory` persists the same ranking to a
  JSON-lines file so memories survive a restart. Attach one with
  `Agent::with_memory` and
  matching items are appended to the system prompt on each run;
  `Agent::recalled_context` exposes the block on its own, and
  `Agent::remember`/`forget`/`recall` are the ergonomic path to the store.
  `MetadataFilter` scopes recall to a metadata key/value and
  `FileMemory::with_max_items` caps a file store. Python and JS
  expose the same store as a `Memory` class, pinned to the Rust ranking
  by the shared `memory_ranking.json` fixture; `AgentBuilder.with_memory`
  (Python) / `withMemory` (JS) prepends recalled matches to each text run,
  bounded by a recall limit. `Memory.search` accepts a metadata filter and
  `with_max_items`/`withMaxItems` caps the store. `Memory.save`/`Memory.load`
  read and write the same JSON-lines format as the Rust `FileMemory`.
- **Templates**: `react::template` renders `{{name}}` prompts with
  `PromptTemplate`/`render_template` and places recall with `with_memory`;
  the agent substitutes a `{{memory}}` placeholder in the system prompt and
  appends the block when there is none.
- **Bindings**: Python and JS share one canonical vocabulary — `publish_text`/
  `publish_json`, `ask`/`ask_json`, `call`/`call_json`, `handle`/`start`/`run`/
  `run_json`, `recv`/`recv_json`. Python keeps the legacy `create_*`/`publish_*`
  names as deprecated aliases for backward compatibility; never remove them.
  Three static guards in `crates/jsbos/test/` keep the surfaces honest:
  `parity.test.js` compares the wrapper classes across languages,
  `native-bindings.test.js` checks that every wrapper call on a native object
  resolves to a real member of the generated interface, and `api-docs.test.js`
  checks that every public binding member is documented in `docs/api-reference/`.
- **Config**: Use `ConfigLoader.discover()` for auto-loading `~/.bos/conf/config.toml`
- **Documentation**: Every crate carries `#![warn(missing_docs)]` at its root, so
  the workspace must stay at zero missing docs. Document new public items in the
  same change. The sole exception is `resource/src/action.rs`, which suppresses
  the lint for the rkyv-generated resolver enum.
- **Solutions**: `docs/solutions/` — documented solutions (bugs, best practices, patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`)

---

## Last Updated: 2026-10-11
