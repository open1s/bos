# bsh crate instructions

## Gold requirement: win on performance and elegant usability

**BSH wins by being extremely performant and elegantly usable, against the
competition** (user requirement). Every change is judged against that, not only
against feature parity. Three consequences:

- **Performance is a requirement, not a polish pass.** The settings file is read
  once per launch and per workspace switch, so parsing stays allocation-light and
  the view never re-reads state it already holds. A re-parse per render, a full
  walk of the session store per frame, or a synchronous disk read on the input
  path is a defect, not a tradeoff to revisit later.
- **Elegance is fewer concepts, not more decoration.** One source of truth per
  fact, no control that only looks wired, no state a reader has to infer. A
  refusal that says why beats a silent fallback; a pane that needs prose to
  explain its own state has already lost.
- **Parity is the floor, not the goal.** The reference sets the shape (the
  standing rule below); beating it on speed and clarity is the point of the
  crate.

When a change trades one against the other, say which won and why.

## Standing rule: every UI increment ends with a gap check

When a UI feature is finished in `bsh`, compare it against the real DeepSeek
Harness UI before calling it done, and close every gap that has backing in the
BOS framework. Never leave a control that only looks like the reference. If a
reference feature has no backing here (no data behind it, no framework seam),
say so in the final report instead of faking it.

**The mirror is functional, not only visual** (user requirement, round 110). Every
increment reports the four axes too, and this file's own round notes keep the running
status:
1. **Backend in Rust** - the behaviour behind the control (a setting written, a turn
   opened, a tool gated) lives in the BOS crates, not in the view.
2. **Interaction** - what a click, double click, key press and mouse move or hover do,
   against the reference's own handlers.
3. **Tips, help and prompts** - tooltips, placeholders, hints, empty states and the
   accessibility labels the reference carries, in its wording where a wording exists.
4. **Errors** - the UI side (a disabled or refused state, inline error copy) and the
   backend side (a failed call surfaces where the reference surfaces it), and what the
   round actually proved about both.

How to look at the reference (in order of preference):

Capture recipe (works; captures intermittently fail):

```
pkill -f "BSH.app"; pkill -f target/debug/bsh; sleep 2
nohup ./target/debug/bsh --settings=SECTION > /tmp/bsh_ui.log 2>&1 & disown
sleep 6; pgrep -fl target/debug/bsh     # confirm before capturing
ocu call get_app_state --args '{"app":"bsh","text_limit":200}'   # write the
                                       # image payload to a PNG, then read it
```

It sometimes returns `Apple event error -10005: cgWindowNotFound` or
`appNotFound` while the process is healthy — retry once or twice.
`ocu snapshot bsh` prints the accessibility tree, but GPUI exposes almost
nothing (window, one text field, the window buttons), so element-index clicks
are not available and a pane must be reached with `--settings=SECTION`.
`open target/debug/BSH.app` registers the bundle as "BOS Harness"
(com.bos.bsh) for a Dock icon, but LaunchServices drops `--args`, so the raw
binary is the one to launch when a section has to be deep-linked.

1. `ocu` against the running app: `ocu call get_app_state --args
   '{"app":"DeepSeek Harness","window_placement":"agent_display"}'` parks the
   window on the agent display and returns both the accessibility tree and a
   screenshot. Never print the base64 screenshot into the conversation; write it
   to a file and open it with the image reader.
2. The reference's own bundles, parsed from
   `/Applications/DeepSeek Harness.app/Contents/Resources/app.asar` (an asar:
   `[4][pickle size][json size][json]`, file data at `8 + pickle size`). The
   settings packages are `dsh-client-ui-settings-*`; `/tmp/bsh_settings_labels.py`
   dumps their label vocabulary:
   - one EN: line and one ZH: line per package, strings separated by a
     pipe, so slicing between a package name and the next marker, or grepping
     for one word, recovers the exact wording of a pane in both languages. It
     is a tmp file, so recreate it from the recipe above if it is gone; it is
     the cheapest way to pin reference wording.
3. The reference's settings structure is recorded in `docs/agents-shell-reference.md`:
   structure; `docs/gui-requirements.md` records the wider shell.

Sidebar entries are PLUGINS with their own dictionaries (round 73), which is where a row name actually lives: each one does `ctx.locale.register(NS, { zh, en })` and then `const t = ctx.locale.bind(NS)`, so recovering a sidebar wording means grepping the NAMESPACE and reading its en/zh objects, not the sentence. Measured: the panel id constant is `const PANEL_ID = "plugins"` for the plugins page and `const PANEL_ID = "schedules"` for the one the Automation tasks row opens, and the plugin-manager namespace is `NS = "pluginManager"`. The MCP row is not one of those two ids, so it comes from a third plugin namespace that is still unidentified - that is why sentence-grepping failed for it twice. Keys tried and refuted (zero hits) so they are not retried: `automationTasks`, `mcpConnector`, `connectorNav`, `nav.plugin`.

THE cheap way to read reference wording is the packed locale files (round 75). The asar header JSON is a file index; walk it and every plugin that carries UI strings ships `node_modules/@deepseek-ai/<pkg>/locale/en.json` and `zh.json`, with the plain, unescaped string as the value - no grep over 121 MB needed. Exactly 10 packages ship `locale/en.json`, and `dsh-experimental-schedule-bundle` pins the reference English for the Automation tasks row: `meta.title = "Automation tasks"`. The per-plugin files carry manifest metadata and panel strings, NOT the core sidebar dictionary, so the MCP row is still unfound - but the route is now a file read, not a scan.

Grepping the asar is slow: 121 MB of mostly packed JS, so a broad pattern takes
minutes (one round spent 13 minutes on two patterns that matched nothing). Keep the
pattern narrow and the hit count small, and prefer a literal marker - a CSS
attribute, an i18n key - over a word every vendored library also contains.

The default session title is localized now (round 71). It was hardcoded in two
places - `Session::new(&workspace, "New chat")` in state.rs and a fallback inside
`Session::title_from` in model.rs - while the `row.newChat` key sat unused with a
translated value (新会话), so a session created in the Chinese locale was titled in
English. Both sites now go through `i18n::tr(lang, "row.newChat")`, `title_from` takes
the lang, and a unit test pins the fallback. The new-session title is stored at
creation, so sessions that already exist keep the title they were given.

## i18n: rust-i18n (done 2026-10-14)

Strings live in `crates/bsh/locales/{en,zh}.yml` (201 keys each, flat dotted keys).
`rust_i18n::i18n!("locales", fallback = "en")` is invoked once at the **crate
root** (`lib.rs`) — invoking it inside a module fails to compile, because the
macro forwards to `crate::_rust_i18n_t`. `i18n::tr(lang, key)` is still the
single accessor and now returns `String` from
`rust_i18n::t!(key, locale = lang.locale())`; helpers that take a translated
label therefore take `impl Into<SharedString>` or `&str`, not `&'static str`.
Add a string by editing both YAML files; a missing key renders as the key.

The reverse check (rounds 71-72) is cheap and worth repeating when UI code moves: scan for user-facing text that never reaches a key. Round 71 went keys -> source and found three dead keys, one hiding a hardcoded title; round 72 went source -> text and found none left, over these literal shapes: `txt(` + ", `Button::new(` + ", `placeholder(` + ", `label(` + ", a struct field such as `title: ...`, and the title argument of `Session::new`. Residual, stated rather than assumed: a label built through a helper with another name (`chip(`, `row_label(`, `text_button(`, ...) sits outside that list, so the scan proves those shapes clean, not the whole crate. The scan also skips lines containing `tr(`, which is why the fixed title no longer appears.

Refinement (round 76): that "zero" is pattern-dependent. Re-run with a looser shape and 9 hits remain, all of them legitimately unkeyed and none of them prose - five are control IDS (`hdr-more`, `hdr-jobs`, `hdr-open-more`, `side-sort`, `account-menu`) that must stay stable for the click tests, and four are marks: the `"✓"` glyph, the brand `"BOS"`, the `"HARNESS"` badge and the avatar initial `"B"`. Brand identity is deliberately untranslated - the reference does the same with its own brand row - so the honest claim is "no unkeyed user-facing PROSE", not "no hardcoded string anywhere". Do not turn an id or a brand mark into a locale key.

## Theming: the gpui-kit Theme (done 2026-10-14)

BSH's colours come from the active gpui-kit theme. `theme.rs` holds the 16
colour tokens (page, sidebar_bg, selected_row, new_session_fill, hover, text,
muted, faint, border_soft, border, border_strong, accent, error, success, warn,
user_bubble) in one per-thread `Palette` cell, exposed as accessors. Per-thread
rather than per-process because a GPUI App paints on one thread: a process
global let one test case's `theme::apply` repaint a frame another case was
asserting on. `sync(cx)` copies the kit's
`ThemeColor` into them (`background`, `sidebar`, `sidebar_accent`, `secondary`,
`secondary_hover`, `foreground`, `muted_foreground`, `border`, `primary`,
`danger`, `success`, `warning`); `mix()` derives `faint` and the two mid greys,
because the kit has a single border token and BSH keeps a three-step border
hierarchy. `apply(name, window, cx)` runs in `Shell::new`;
`toggle(window, cx)` is the account-row Sun/Moon button, and the mode persists
as `settings.theme_mode` (serde default "system", the reference's measured default). 192 call sites moved from
`theme::CONST` to accessor calls; the size and layout consts are unchanged.

The palette itself rides in `crates/bsh/src/assets/bsh-theme.json`, a theme set
generated from the kit's own `default-theme.json`: both built-in palettes are
copied wholesale, so no key the kit reads is missing, and 20 light + 7 dark keys
are overridden with BSH's reference values (background #FFFFFF, sidebar #E8E9EB,
border #D8DBE2, foreground #0F1115, muted #6B7280, secondary #F2F3F5, primary
#4176E6, plus the primary/ring/sidebar-primary families; dark keeps the kit's dark
surfaces and takes the same blue accent). `install()` registers the set with
`ThemeRegistry::load_themes_from_str` and points `light_theme`/`dark_theme` at
"BSH Light"/"BSH Dark"; a following `Theme::change` re-applies the current mode.
Verified by probe: `light=BSH Light dark=BSH Dark page=FFFFFF accent=4176E6` — the
reference blue accent is back while the palette still comes from the kit theme.
Without the set the kit's Default Light primary is near-black (#171717), which is
what made the accent look wrong for a round.

Traps worth knowing (each cost real time):

- `Theme::change(mode)` installs the registry's default light/dark configs
  only when the `Theme` global is absent, and `Theme::default()` gives both
  configs a plain light `ThemeConfig`. `gpui_kit::application()` runs
  `gpui_component::init`, which registers "Default Light" and "Default Dark",
  so in practice `change` finds a proper global and dark mode works. Verified
  from the resolved tokens: `mode=dark name=Default Dark page=0A0A0A
  text=FAFAFA`.
- Writes from the agent shell to paths outside the workspace are silently
  discarded (the sandbox reports success and the mtime never moves), so
  hand-editing `~/.bos/bsh/settings.toml` never reaches the app and a light
  window will look like a theme bug. To exercise a non-default setting, give
  the app a sandboxed home: write `.tmp-home/.bos/bsh/settings.toml`, launch
  with `HOME="$PWD/.tmp-home"`, and delete `.tmp-home` afterwards.
- Screenshots are intermittent (`cgWindowNotFound` can hit every attempt of a
  run). When they do, put a temporary `eprintln!` in `sync()` printing the
  mode, the theme name and a few resolved tokens: that proved the dark palette
  while the screenshot tool failed.
- The dark palette is now visually captured too (2026-10-15): the window renders
  near-black with light text, so BSH Dark is confirmed by screenshot and not only
  by the token probe. When a UI path only exists behind an untriggerable click
  (the session-search field, for example), flip its default for one build, launch,
  capture, and flip it back: that is how the search field was shown rendering with
  its placeholder.

- `render_transcript` returns early for two states - no active session
  (`empty.none`) and an empty transcript (the welcome state). bsh used to select no
  session at startup, so a probe past those returns never ran: two launches printed
  nothing while looking healthy. Since round 22 the shell restores a session, so
  the transcript path is reachable on launch; the trap only bites when the restored
  session is itself empty, in which case anchor the probe above the returns.
- A nested `#[cfg(test)] mod tests` in a file that does `use gpui_kit::*` must
  not `use super::*`: gpui re-exports a `test` attribute macro that shadows the
  builtin one, and the expansion recurses until `recursion limit reached while
  expanding #[test]`. Import the items you need by name.
- Keep `#[cfg(test)] mod tests` last in the file (`items_after_test_module` is a
  warning that `-D warnings` turns into an error, and a module inserted next to
  the function it covers lands mid-file).

- A hand-written session file must match `Session` field for field, and the
  store drops the ones that do not **silently**: `Store::open` reads every
  `sessions/*.json` and keeps `if let Ok(session) = serde_json::from_str(..)` with no
  log, so a bad file just disappears (and `restore_session` quietly falls back to
  another session). My mistake was `archived`: it is a
  `Vec<serde_json::Value>` (`model.rs`), not a bool - sending `false` fails as
  `"invalid type: boolean, expected a sequence"`, which cost a round to find
  because the error went nowhere. Temporarily printing the `Err` in the loader
  found it in one run. To seed, copy a real session file and override only
  `goal`/`id`/`title`/`updated_at`, then point `active_session` at it.

- The rail did **not** need a virtual list, and my earlier note that the transcript
  cannot be virtualized was aimed at the wrong crate. bsh renders with `gpui-pre` 0.3.9
  (aliased as `gpui` by gpui-kit), whose `ScrollHandle` records every child's
  **painted bounds** (`bounds_for_item(ix)`), and whose `scroll_to_top_of_item(ix)`
  derives the offset from those measured bounds, so variable-height rows jump with
  no size precomputation. `gpui-base` 0.7.3's fixed `item_sizes` `virtual_list`
  is a different crate the kit uses internally, not what bsh renders with; if the
  transcript ever wants real virtualization, `gpui-pre`'s `list` and `ListState`
  (`ListAlignment::Bottom`, `scroll_to(ListOffset)`) are the auto-measuring seam.


The reference's Font size setting is real now (round 92), and the package that owns
it is the cheapest place to read reference copy: unlike the settings packages,
`dsh-client-ui-theme/lib/client.js` carries its whole dictionary inline (`appearance.*`
and `fontSize.*` in both languages), and its README pins the behaviour - both rows sit
in **General**, the stepper takes 10..22 px and defaults to 14, and a step moves
conversation headings and base text together while flow-row titles, summaries and
tables stay one step under the body (small text and code keep fixed sizes). The same
file measures the namespace and fields (`ui-theme` / `preference` default **"system"**,
`fontSize`), the Appearance cube metrics (`flex:180px`, `padding:20px 32px`, `.5px`
border at `--dsw-alias-border-l4`, `border-radius:var(--dsw-radius-xl)`, `gap:4px`,
14px/22px, hover `--dsw-alias-interactive-bg-hover`) and the group's separator
(`padding:16px 0`, `gap:8px`, `.5px` bottom border at `--dsw-alias-border-l2`). bsh
derives conversation sizes from the setting at each call site (`theme::body_size_at` /
`small_size_at`) rather than a global, because a process-global mutable size moves row
heights under layout tests: `rail_tick_click_scrolls_the_transcript` failed exactly
that way while the size lived in a `static AtomicU32`. Round 94 measured the
reference's own type scale instead of trusting the kit's: `--dsh-content-font-delta` is
`content - 14px`, so every conversation size is additive, `--dsh-content-font-size-secondary`
is `min(size-1, max(13, size-2))`, and `--dsw-font-markdown-h1..h4` are 21/19/18/body
plus that delta. `theme.rs` carries those formulas (`delta_at`, `small_size_at`,
`heading_size_at`, `body_line_at`, `table_line_at`) and `Shell::markdown_style` feeds
them to a `gpui-component` `TextViewStyle`, whose `heading_font_size` closure overrides
the kit's proportional `body * 1.8 / 1.33 / 1.13 / 1 / 0.93 / 0.87` scale. Three honest
limits: the component style has no per-heading line-height hook, so heading line heights
stay the kit's while sizes are the reference's; inline code cannot be sized
(`HighlightStyle` carries no size field); and the kit text input exposes no size hook, so
the composer draft does not follow the step. The full measured table, with the source
and the extraction recipe, is `docs/ui-mirror/reference/dsh-type-scale.md`.
Round 95 measured the controls those sizes sit in
(`AppearanceRow`/`FontSizeRow.module.css`, inline in `lib/client.js` too); their
metrics are recorded in this file's round notes, and the click test proves the
hover-revealed stepper chevrons stay hit-testable at `opacity: 0`. Seams that cost
compile cycles: the kit's
border macros are whole-pixel only, so a `.5px` hairline is written into
`element.style().border_widths` directly; that field is
`EdgesRefinement<AbsoluteLength>` (an `Edges` literal does NOT fit), assigning the
sides one at a time also dodges clippy's `field_reassign_with_default`, and the
helper must be generic over `E: Styled` because `.id()` returns `Stateful<Div>`, not
`Div`. The full sheets are `docs/ui-mirror/reference/dsh-general-rows.md`.

Round 96 measured the dialog those rows sit in, and the route that makes it cheap:
the reference's CSS is extractable. The asar data base is `8 + pickle size` (the u32
at offset 4), NOT `16 + json size` - the two-byte error is invisible in prose and
obvious in files, because every extracted file then begins with the previous file's
closing `}`; all 57 stylesheets are now in `docs/ui-mirror/reference/dsh-css/` with an
INDEX, so a measurement is a file read. The settings shell's own sheet is inline in
`dsh-client-ui-settings-general/lib/client.js` behind a `//#region dsh-css:` marker,
and `docs/ui-mirror/reference/dsh-settings-chrome.md` holds it verbatim with the token
map. Measured and drawn: the panel is 800 x min(800, viewport - 48) on radius-panel 28
with the prominent elevation and NO border (its own JSX comment's "760x500" is stale),
the rail is 188 wide with no background or divider over 40px cells (radius 12, padding
9/16/9/12, 14px) under a 16px/500 title, the header is 54 with a 28x28 close, the
options box is 0/24/24 and scrolls, and the save bar holds ONE 13px Save on the
label-primary fill - the modal-wide Cancel is gone, because the reference's save row
has none and the header's close discards the draft (`settings.cancel` stays: the
provider-delete confirmation still uses it). Field rows take the measured 12/0 padding,
`.5px` border-l2 rule, 13px/500 label, 12px label-secondary reset link and 12px
label-tertiary hint, over a 34px control box (radius 12, `.5px` border-l4, layer-3
fill); the chrome uses min-height 34 rather than a fixed height because the kit Input
owns its own. Still owed: the 24x24 per-field help button and its disclosure (no
per-field help copy exists here), `.invalid`, a `:last-child` rule that would drop the
last row's hairline, and the layer/elevation/nav-token mappings above.

Round 97 measured the shell the dialog opens over, and where its CSS lives: NOT in
the web frontend bundle (76 KB of fonts and a boot splash, zero shell rules) but
inline in each `dsh-client-ui-*` package's `client.js` behind a `//#region dsh-css:`
marker - and the literal is `const css = "..."` for a one-module file, `const css$1 =
"..."` for a multi-module one, so a search that insists on the `$N` suffix silently
finds almost nothing. `docs/ui-mirror/reference/dsh-shell-chrome.md` carries the
AppFrame and SidebarRoot sheets verbatim. Measured and drawn: the rail's store default
is **280** (the old ~308 was a screenshot guess) and it is a *default*, not a fixed
width - an 8px `col-resize` handle at the seam drags it; macOS draws NO sidebar
border, hanging the shell's only rule on the centre column instead (`.5px` at
border-l3, dropped when the rail folds) over a translucent sidebar wash; the sidebar
root owns a `6px 12px` padding and a `font-size: 14px`, which is why the 15px bsh
carried for rounds was simply wrong; the traffic-light strip is 52, the brand row is
60 and overlaps it by 12, the brand name is 18/600, `newSession` is 38 tall over a
`.5px` border-l3 at radius 12 with `margin: 0 2px 12px` and a 14/500 label, `panelRow`
is `min-height: 36px` with `line-height: 22px` at radius 12 and `padding: 7px 8px`,
`panelList` is a 4px column, and `iconButton` is 28x28 at radius 8. Two seams worth
keeping: the `.5px` hairline helpers now live in `ui/mod.rs` (`hairline_all`,
`hairline_bottom`, `hairline_left`) because two modules need them; and the measured
`line-height: 22px` is load bearing - gpui's own line box for 14px is 22.5, so the row
came out 36.5 tall until the measured value was set, i.e. `min-height` alone does not
reproduce a reference row height. Still owed: the opaque fill under a translucent
reference, the macOS gradient, the resize handle, `letter-spacing` (no kit hook
found), and the tree's own inset where the reference bleeds a region to the window
edge.

Round 98 measured the centre column, which one package owns: `dsh-client-ui-conversation`
(712 KB) holds ConversationRoot, InputBar, HeroShell, ContextMeter, QueueDock, TodoPanel,
EnterBehaviorRow, ReferenceChip and composer-editor as inline CSS modules, and
`docs/ui-mirror/reference/dsh-conversation-chrome.md` carries both sheets verbatim with the
extraction script. Measured and drawn: the header is a 76 min-height over `10px 28px 0 20px`
with a `.5px` border-l3 rule, its title row is 30 tall with a 14px/20px weight-500 crumb (the
16px bold title bsh carried for rounds was wrong, and TITLE_SIZE left with it), and the tabs
are 13px/16px at weight 500 over a 9px pad with a 2px active underline in business-primary, 36
apart behind an 8px lead-in. The composer card is radius-panel 28, the input-major fill, the
`elevation-soft` shadow and NO border (in light it is the same white as `bg-base`, so
the stroke and two 3%-black blurs separate it) - over a 12px column gap, an 8px top pad, an
input padded
4/8/0/14 and a bottom row padded 2/8/6 at gap 12; every chip is the reference's `.select`
(28 tall, radius 8, 13px/20px at weight 500, label-secondary, no border); and its width is
the chat content width **plus 32**, which is why the composer now sits in its own centred box
outside the transcript column. The chat column itself is
`clamp(680px, conversation-column * 0.64, 920px)`, so CHAT_W moved 670 -> 680, the clamp floor
at the reference's window (the responsive formula is owed, as is the 10px `col-resize` handle
that drags it). Two seams worth keeping: gpui's `min_h` rendered **2px above** the CSS floor on
the title row (32 where the reference renders 30 with the same 20px text child), so the row
uses the measured 30 as a fixed height; and a GPUI shadow carries no ring, so the reference's
`--dsw-elevation-stroke` is drawn as a `.5px` hairline.
Round 99 measured the transcript's own rows, which `dsh-client-ui-chat` owns: `ChatView`,
`MessageItem`, `TurnNavigator` and fifteen more CSS modules ship inline in its `lib/client.js`,
and `docs/ui-mirror/reference/dsh-transcript-chrome.md` holds the sheets that matter verbatim
with the module inventory. Measured and drawn: the turn rail is a **28px frame parked 12px off
the transcript's right edge**, capped at `min(band - 64, 420)` and scrolled when the marks
overflow, where a mark is a **10px slot holding a 2px x 20px bar, right-aligned**, drawn at
`scaleX(.6)` while idle in `border-l4`, `.9` on hover in `label-tertiary` and `1` in
`label-primary` for the turn at the top (bsh's 14x3 accent pill was wrong on all three axes);
and the user bubble is radius-xl 20 over `10px 16px` with the measured `line-height: 22px +
delta` and an 82% cap. Three seams worth keeping: the rail is an **absolute overlay** (as a
flex sibling it ate 40px of the row, so "12px from the right edge" measured the wrong edge);
a flex item's default `min-width: auto` blew the transcript to **2958px** inside a 680px
column until the scroller carried `min_w_0()`; and marks **shrink** in a flex column, so each
10px slot needs `flex_shrink_0()`, while a bubble's own `line-height` rule is 22 + delta
against the body's 24 + delta (`theme::bubble_line_at`). Owed: the unloaded and busy
mark states, the fade masks, and `MessageItem`'s compaction, context and retry rows.
Round 100 measured the reasoning disclosure, which is not a chat-local row but the shared
one: `dsh-client-ui-primitives/lib/DisclosureRow.module.css` pins the header every flow
row uses - a `24px + delta` row whose title line is the same 24, a `16px + delta`
leading box holding a 14px glyph with a 6px right margin, the glyph fading to a chevron on
hover, the title at the secondary size in label-tertiary and label-secondary under the
pointer - while `ReasoningRow.js` adds the 2px dot at 8px margins, the `20px +
secondary delta` nowrap ellipsised summary and `IconThinkOutlineRegular` at 14. bsh
draws that row, with the summary rule as a pure `think_summary` function and
`Shell::think_open` (cleared on a session change) toggling the indented body on a row
click.
## Interaction tests: clicks are verifiable now (round 32)

`crates/bsh/src/ui/shell_tests.rs` is the first test that presses a real control: it
boots the shell into a GPUI test window, looks the element up by debug selector,
synthesises a click and asserts the state moved. Verified:
`header_panel_toggle_click_flips_the_sidebar ... ok` (71 tests total, eighteen of them click tests). Every trap it
cost, so the next test is cheap:

- `[dev-dependencies]` takes `gpui = { package = "gpui-pre", version = "=0.3.9",`
  `features = ["test-support"] }`. The package lib name is `gpui` (which is why the
  kit aliases it), and `TestAppContext`/`VisualTestContext`/`simulate_click`/`debug_bounds`
  are `cfg(any(test, feature = "test-support"))` inside gpui-pre, where that `test`
  means gpui-pre own tests, not ours - so the feature has to be switched on by
  name. It drags 11 crates (proptest and friends) into the shared registry, which
  the file sandbox denies, so the first download needs one
  `sandbox_permissions: danger-full-access` run.
- `#[gpui::test]` needs `#![recursion_limit = "256"]` at the crate root, otherwise the
  expansion aborts with "consider increasing the recursion limit".
- The test module must **not** glob-import: gpui re-exports the `test` attribute macro,
  the macro expansion emits an unqualified `#[test]`, and under a glob that resolves
  back to the macro itself - the expansion recurses until the parser blows the
  stack (the same shape as the `use super::*` trap above, reached from the other side).
- `add_window_view` wants a `V: Render` value, not an `Entity`, so the file holds a
  one-field `Harness { shell: Entity<Shell> }` whose `render` returns the shell clone.
- `Entity::read` takes `&App` and a `VisualTestContext` is not one: wrap reads in
  `cx.update(|_, cx| ...)`.
- `debug_bounds(selector)` reads **debug selectors**, not `.id(..)`: `header_button`
  now sets `.debug_selector(|| id.to_string())`, which compiles to a no-op without
  debug assertions, so release pays nothing.
- The store is swapped, not `HOME`: `Store::open_at(dir)`, `AppState::with_store(store)`
  and the test-only `Shell::build_for_test(window, open_settings, state, cx)` let a test
  point the store at `target/bsh-scratch-<label>-<pid>`, so a click that saves a session
  never writes under the user home. Other tests assert the `settings::dir()` string, so
  overriding `HOME` or a `BSH_HOME` process-wide would make them race. The pid is part
  of the root because `scratch_store` removes the directory on the way in: without it
  two runs sharing `target/` delete each other's stores mid-test.
- **A test may not depend on a single draw after a state change.** A click schedules
  work (focus lands on the next dispatch, a text run lays out on the next frame, an
  editor records its box during PAINT), so `click; draw once; assert` is green on an
  idle machine and loses the frame under the suite's real load. Use
  `draw_until(cx, what, max_frames, predicate)`: it pumps bounded frames and panics
  naming the predicate. The tree's double-click is wall-clock bounded
  (`SELF_DOUBLE_MS`), so synthesise it with `press_twice` - a draw between the two
  presses can outlast the window.
- Interaction tests are discrimination-checked, not just run: the pin test pins an
  inactive row, then clicks that same row body and asserts it *did* open, so the
  negative assertion cannot pass by accident; deleting one `stop_propagation` from
  sidebar.rs fails it with the expected message. That is the evidence for the
  propagation fix.
- Row and workspace controls carry debug selectors now: `session-..`, `pin-..`,
  `archive-..`, `stop-..`, `more-..`, `ws-..`, `archived-..`, `restore-..`.
- Pressed for real so far (round 34): the header panel toggle; the sidebar search
  control (`search_control_click_opens_the_filter`); both tablist entries, in both
  directions (`tab_click_switches_the_main_column`); the settings gear
  (`settings_gear_click_opens_the_dialog`); and the pin control of an inactive row.
  Each test reads the state back after the click, so a control that only looks wired
  fails the suite.
- The rail is click-verified too (round 35): `rail_tick_click_scrolls_the_transcript` seeds 30
  turns into the scratch store, clicks the last tick and asserts the handle reached
  the end, then clicks the first tick and asserts it came back up. Two traps it cost:
  the observable is `ScrollHandle::offset()`, NOT `bounds_for_item` - the latter reports the
  laid out position, which a scroll request never rewrites, so the first version of
  this test failed while the wiring was already correct (`offset 0 -> -6751.5px`);
  and turn 1 lands at the timestamp row height (child 0), so the first tick ends near
  the top, not at exactly 0.
- The tail is closed, with evidence from the reference own bundle (round 36).
  Its chat CSS carries a `[data-chat-following-tail]` rule - the conversation scroller
  turns `overflow-anchor` off while the reader is following the tail - so DSH tracks
  that state and manages the scroll itself while it holds. bsh had
  no such state and opened every transcript at offset 0, i.e. on the oldest turn.
  `Shell` now carries `following_tail`, `tail_max` and `tail_session`:
  the newest message stays in view, a rail jump clears the flag, and content that
  grows while the reader is still at the end (or a session switch) sets it again - so
  a reader who scrolled up is not yanked to the bottom by the next stream.
  `transcript_opens_at_the_newest_message` pins the opening position (`offset == -max`).
  Two traps: the naive rule - derive following purely from the handle - pulled the
  reader back down right after a rail jump, and the rail test caught it (the flag has
  to be cleared by the jump itself); and on the very first frame the content height is
  still unknown (`max == 0`), so following needs one extra frame, which `cx.notify()`
  requests only while the height is unmeasured.


### Round history (rounds 37-77)

The round-by-round record - the seam each round closed, its evidence and its
traps - lives in the round notes in this file, condensed when this file passed the
workspace instruction budget. What stays here is what changes how the next
increment is written:

- `debug_bounds` reads debug selectors, and kit widgets carry none: a `Button` records
  no bounds even when a wrapper `div` does. Make the observable a returned
  value (a label, a resolved setting) rather than a control.
- Pin `settings.active_session` (or the seeded workspace) in any pane test whose
  premise depends on which session the shell restores: `Shell::assemble` restores
  it, and a wrong premise looks exactly like a broken feature.
- A row added above a list can push the list past a scrolling pane viewport, and
  a click on a clipped row is a silent no-op even though the harness reports its
  bounds happily. Place rows where the section is meant to hold them.
- Pane tests read `state.draft()`, never `settings`; a pane that saves writes
  the draft.
- A `Settings` field needs its `Effective` twin (E0609 otherwise), and an
  `Option`/count field needs its own serde default when zero parses as valid.
- The kit menu builder is `Fn`, so captured values are iterated or cloned, and
  `PopupMenuItem` needs its explicit import.
- Keep `#[cfg(test)] mod tests` last, do not glob-import gpui inside it, and
  remember `i18n::tr` wants `key: &'static str`.
- A batch patch that dies on an anchor check writes nothing, which is a feature: an
  aborted move leaves the file untouched rather than half-moved.
- Recover reference wording from the label dump (see above) before inventing any
  label: measure first, then wire.
- Inserting a test by anchoring on its `fn` signature can land it
  between the preceding `#[test]` attribute and that `fn`, which both
  breaks the attribute and silently stops the older test running. Anchor on the
  attribute line, or on the line above it. Plain `cargo test` does not catch this;
  `cargo clippy --all-targets -- -D warnings` does, through
  `duplicated_attributes` and `dead_code`.
- `Settings::default()` ships a complete endpoint (base URL plus model), so any
  refusal test must clear the field it refuses on; starting from the default
  silently exercises the success path instead.
- Seeding a scratch state before `Shell::build_for_test` is wasted:
  `assemble` reloads the draft. A pane test that needs a draft provider must
  seed it after the shell exists, then draw again.
- The harness cannot press a control the pane lays out past the window:
  `debug_bounds` reports the position happily and `simulate_click` there is a
  silent no-op, so drive the handler own function and assert the control is on
  screen instead. Measured in the models pane: the row lays out at y=759 and its
  right edge sits at x=1306.
- A string replace whose anchor has gone writes nothing and says nothing, so a
  restore that silently did not restore can hide for several test runs. Verify a
  re-insertion by grepping the line back, or replace a line range instead.
- Deleting a span you located by searching for the NEXT marker, rather than by
  counting the span you inserted, removes everything in between: a 6 KB over-deletion
  took the provider form with it and cost a history restore. Recover it with
  `jj op log` + `jj --at-op=<op> file show <path>` (jj snapshots the
  working copy per operation), and filter candidates cheaply with
  `rustfmt --edition 2021 --emit stdout <file>`, which fails on the same
  unbalanced delimiter the compiler reports. Do not reach for git `HEAD`: this
  repo keeps rounds of work in the jj working copy, so HEAD trails it.

- A new protocol arm is only half a change: a transport that branches on the mode
  without handling it silently speaks the wrong wire format. Adding
  `ApiMode::Anthropic` made `OpenAiVendor` (which checks only for
  `Responses`) fall through to Chat, so both of its entry points now refuse the
  Anthropic protocol outright, offline-tested. Close the refusal before drawing the
  control that offers the protocol.
- A new header line or row above a list can push a real control out of reach at the
  reference own window size (1280x820): the models pane provider row sat at y=790 with
  about 30px of headroom, so an intro drawn as a first row made the row click a silent
  no-op while `debug_bounds` still reported bounds. Prefer the header slot (the
  section tuple hint) over an extra row, and treat a broken click test as evidence about
  reachability, not as a test to relax.

PTC mode is explained now, and it stays unwritten (round 37). The header chip is
an agent-preset concept, not a chat mode: the bundle own documentation says a
preset selecting a PTC mode mounts against `ctx.ptcRuntime`, a host-plane service,
that `dsh-tools` ships renderers for, and that a deployment composing no such
runtime refuses to mount that preset at all. bsh composes no PTC runtime, so the
chip has nothing to report; drawing a label would be invention.

Two locale keys are parked on purpose (round 71), both measured and both with a
Chinese value, but not rendered: `row.running` ("Running" / 进行中) names the running
session row in the reference, which reaches the a11y tree only - GPUI exposes none -
so drawing it as a visible label would deviate from the reference look; and
`stream.deepDiving` ("Deep diving" / 深入思考) is a streaming PHASE label the reference
can show mid-turn, which bsh does not model (bsh renders the measured phase it does
have, "Think", from the dynamically built msg.* family). Neither is dead code to
delete: each is the vocabulary for a feature that is still open. A third one,
`row.newChat`, WAS dead and is now wired - see the session-title note below.

## Measured from the running reference

The window measurements, the reference-facts table and the still-open gap list
moved to `docs/agents-shell-reference.md` when this file passed the workspace
instruction budget (65,536 bytes): they are reference material read on demand, and
keeping them here truncated the newest traps below. Read that file before
measuring a shell surface again.

Round 101 measured the turn rail's hover preview (`TurnNavigator`): its `@container
(width <= 900px)` gate counts the CONVERSATION column, so `Shell::conversation_w` is the
viewport less the sidebar; a GPUI scroller clips absolute children, so the card sits in a
`relative` frame beside the marks'; `ScrollHandle::scroll_to_item` counts the scroller's
CHILDREN; `simulate_mouse_move` takes THREE arguments where `simulate_click` takes two;
and `-webkit-line-clamp` clips in Rust at the measured budget with a `max_h`.

Round 102 measured the stat pills and the usage dialog (`StatsPills.module.css` and
`stat-dialog.module.css`): a centred 12px gap over
22px pills (20px line + 1px padding) at a 999px radius, each a 14px glyph beside a
`secondary - 1px` label with a 6px-margin `·`, and the usage pill triggers the token
panel 8px above it. `human_tokens` matches `formatTokens` (a scaled 100+ rounds to a
whole number, a smaller one keeps one decimal) and `cache_hit_percent` never rounds a
partial hit to full (199/200 is `99.5`). Three seams: `.when` is `ui::DivExt` (import it or the method is
missing); an absolute panel needs `.occlude()` or clicks fall through it, and a
`relative` anchor or a scroller clips it; and a glyph leaf inside a fixed-height row
needs its own `line_height` or it sets the row from its own line box.

Round 103 measured the context meter (`ContextMeter.module.css`): a 14px ring (5.5px
radius, 2px stroke) beside the reading in a 1/8 padded radius-sm trigger, opening a 264px
panel 8px above it with a three-tint legend. bsh prices its own figures under the
reference's heuristic (model.rs: 4 chars a token, 4 a block, 4 a role) from the prompt,
the tool schemas and the transcript, printing `~` because that projection is composition,
not billed size. The tool array exists only once an agent is built, so that bucket is
seeded in `assemble` (per-turn only, it reads zero at launch); no GPUI arc, so the ring is
a canvas path.

Round 104 measured the hero (`HeroShell`): a blank session centres a stack 32 off the
bottom - a 32-tall headline over a 28-tall radius-sm workspace chip, with its placeholder.
Traps: `set_placeholder` ALWAYS notifies, so refresh it from `Shell::render` behind a cached
phase flag; its getter is `state.presentation().placeholder()`; a canvas-path parser that
drops the number pending before a command makes `build()` fail, so the mark silently draws
NOTHING - pin it with a tessellated-bounds test.

Round 105 measured the to-do panel (`TodoPanel` + the shared `StateDot`) and closed a DATA
gap: `Session.plan` was never written, because `Runner::spawn` drops the agent's
`PlanStore`. `AppState` now keeps `plans: HashMap<String, PlanStore>`, `Shell::send`
attaches `agent.plan()` before spawning, and the pump saves it on the terminal event (and
on a stop). Traps: a plan seeded on a turn-less session STAYS IN THE HERO PHASE, where the
composer stack is never reached, so seed a message; a `.5px` hairline grows a GPUI box by
1px (37/93 where CSS says 36/92); and the rotating spinner has no animation seam, so bsh
paints the reference's reduced-motion arc.

- Round 106 measured the queue dock (`QueueDock`) and closed the concurrent-turn bug it
exposed: `Shell::send` used to open a second turn over the same transcript while one
streamed, so `send` now queues when the session `is_streaming`, and `begin_turn_in`
plus the new `spawn_turn` are the one turn-opening path the composer, the scheduler and
`drain_inbox` share, with `pump` draining on the terminal event and on a stop. The dock
is 648 wide (the 712 card less 2 x 32), rounded on its top corners, tucked 3px under the
card by `margin-bottom: calc(0 - gap - 3px)`, with a 36 header drawn only when more than
one message waits. Traps: the kit hands `on_click` to a `Stateful<Div>` and not to a
plain `Div`, so every action box needs its own `.id(..)`; `margin-bottom: calc(0 - gap
- 3px)` is `-(3 + 6)`, and a sign slip puts the dock above the card instead of under it;
a single `.5px` edge adds 0.5px (40.5/72.5/36.5 measured). Honest gap: no steer action -
the runner exposes only `spawn`/`stop`, and neither `crates/agent/src` nor
`crates/react/src` has a mid-turn injection seam.


- Round 107 measured the composer's own editor and made it the reference's growing field:
`TextareaState`/`Textarea` with `auto_grow(1, 20)` + `submit_on_enter(true)` (Enter submits,
Shift+Enter opens a line), inside a `.scroll` wrapper carrying the measured `margin-right:4px`,
the 36px floor and the measured 336px cap; the queue row's editor does the same at its measured
127. Traps: `auto_grow`/`soft_wrap` live on `TextareaMode`, NOT on the single-line `InputMode`,
so a growing field has to be a `Textarea`; the kit's multi-line field draws 8px of interior
padding a side (measured: 36 tall over a 20px line box), so the wrapper carries the reference's
padding less that; `overflow_y_scroll` needs a `Stateful<Div>`; and `debug_bounds` reports a
*clipped* rect, so a row taller than the dock's 180px list plateaus at the visible height -
assert the ceiling as "six and twelve lines are the same height" rather than pinning the clipped
number. The draft now follows the Font size step, closing the round-94 input-size gap. Reported,
not drawn: the reference's `EnterBehaviorRow` is a General settings row (busy-state Enter =
queue | steer) and bsh has no steer seam.

- Round 108 drew the composer's `.add` control, which is the reference's own round `+` and
not a chip: `.add{background:var(--dsw-specific-selector);width:28px;height:28px;
border-radius:999px;display:grid;place-items:center}` holding `IconPlusOutlineMedium` at 14
in label-primary, hovering to `interactive-bg-hover-solid` and wrapped in
`Tooltip{label: t("input.commands"), side:"top", delayMs:500}` (the same string is its
`aria-label`). bsh renders a `Stateful<Div>` 28x28 `rounded_full` on
`theme::new_session_fill()` - the kit's `secondary`; the reference's `specific-selector`
is the *static* #f5f6f7 (dark #353638), 3/255 off. The tooltip rides gpui-pre's
`.tooltip(...)` + `.tooltip_show_delay(500ms)`, because the kit's own `ManagedTooltipExt`
is `pub(crate)`: `.tooltip(|window, cx| Tooltip::new(label.clone()).build(window, cx))`
works on any `Stateful<Div>` since `gpui_kit::component::tooltip::Tooltip::build` returns
the `AnyView` gpui-pre wants. The label moved onto the reference's own key and wording
(`composer.attach` -> `input.commands`, zh corrected to 添加文件或调用指令), so it is live
again. Honest gap: the reference's `+` opens a command LISTBOX; bsh's opens the file
picker, so the menu stays owed. Measured for next: the `.select` chevron is a 12x12
data-URI (`M3 4.5L6 7.5L9 4.5`, #81858C, 1.5) at `right 4px center`.


- Round 109 drew the composer's primary control and found the architecture the chips come
from. Measured: `.primary` is **34x34** (bsh drew 30) at radius 999 on
`--dsw-alias-button-info-fill` = `--dsw-static-deepseek-500` **#4176e6** (dark `-400`
#7aaaff - exactly bsh's accent in light), `color:#fff`, `transform:translateY(-2px)`
(`.relative().top(px(-2.0))`) and `:disabled{opacity:.4}`; the glyphs are inline 16px
svgs (stop = `rect x=3 y=3 width=10 height=10 rx=3`, send = a filled arrow path), and the
tooltip is `input.send`/`input.stop` on the top side after 500ms. The label rule is the
behaviour: `primaryStops = running && subagent === null && (empty || blocked)` - so a
running turn with an EMPTY draft stops, while a draft typed during a run SENDS, which is
bsh's queue path. It is the pure `primary_stops(streaming, can_send)` now, with a truth
table test. A 34px control is 4 taller than the old 30px one, so the composer card moved
95 -> 99 / 395 -> 399 and the round-107 height test pins the new numbers.
- The composer's permission, plan, model, right, left and activity controls are PLUGIN
SLOTS: the InputBar JSX holds `div.modes{renderSlot("conversation.input.permission"),
renderSlot("conversation.input.plan")}` and `div.standardControls{renderSlot(
"conversation.input.right"), renderSlot("conversation.input.model")}`. That is why
`yhfFVG_select` - the class carrying the 12x12 chevron - is referenced NOWHERE in the
conversation bundle: the chips ship in `dsh-client-ui-permission-presets` /
`-model-selection` (plus `-plan`, `-input-trigger`/`-commands` for the command
listbox, `-attachment`, `-reference` for ReferenceChip, `-skill`, `-approval`,
`-agent-preset`). Do not draw that chevron on either chip before reading those packages.
The asar's file list is cheap (`obj['files']` is a list of `{path,bytes,sha256,
executable}`; 12,417 entries, 71 `dsh-*` client bundles), but extracting one bundle is
not solved yet: the entries' `bytes` sum to **331 MB against a 121 MB archive** because
the header carries a `sharedPackages` table, so the round-96 data-base recipe
(`data base = 8 + pickle`) only holds for inline-packed files.
- clippy trap that `cargo test` cannot see: a doc-comment line that STARTS with `- `
makes clippy read a list item and then reject the unindented continuation as
`doc_lazy_continuation`, which `-D warnings` turns into a build failure. Rephrase
the line instead of indenting it, and run clippy before claiming green.


- Round 111 drew the model chip's own menu (`dsh-client-ui-model-selection`), and the
  measured trigger says more than its CSS: `[triggerIcon?] [triggerLabel] [triggerEffort?]
  [dot|chevron]` with the icon behind `display:var(--dsh-composer-model-icon-display,
  none)` - so drawing none is measured - the effort span at `flex-shrink:1000` in
  label-caption, and the mark `busy ? StateDot{state:"ongoing"} : chevron` while only
  `locked` (another DSH instance) disables the trigger, i.e. busy disables the option ROWS
  and not the chip. The kit seam that makes a two-pane menu possible:
  `dropdown_menu_with_anchor`'s builder is `Fn(PopupMenu, &mut Window, &mut
  Context<PopupMenu>)` - hero.rs's `move |menu, _, _|` hid that - so
  `menu.submenu(label, window, cx, f)` builds a real submenu and wires its parent link.
  `PopupMenuItem::element` is how a measured non-item line (the 12px/18px label-tertiary
  `.empty`) gets drawn, and `PopupMenuItem::submenu` cannot show a row's trailing VALUE.
  This package's wording is inline in its own dictionary (`menu.model`, `empty.models`,
  `trigger.fallback`), so no asar scan was needed. Two traps: locale YAML lines carry NO
  trailing comma, so an edit anchor that includes one silently fails; and `streams` is
  private to state.rs, so a test that needs a running session calls
  `state.begin_turn_in(&id, text)`.


- Round 112 extracted the WHOLE shipped bundle corpus, so a reference question is
  now a grep over files: `python3 docs/ui-mirror/reference/extract-asar.py --list |
  tail -n +2 | awk '{print $2}' | xargs python3 docs/ui-mirror/reference/extract-asar.py
  docs/ui-mirror/reference/pkgs` gives 72 `dsh-*.js` files, 12 MB, each
  sha256-verified. `docs/ui-mirror/reference/dsh-slot-map.md` records the composer
  footer's measured child order and all 60 `conversation.*` registrations with the
  package behind each; read it before drawing anything in the composer or the header.
  The finding that changed code: NOTHING registers `conversation.input.left` or
  `.right`, so a control drawn there is invention - bsh's `Skill` chip was, and it
  is gone with its action, its builder and its two locale lines. The map also pins
  that `conversation.input.activity` is the EXPERIMENTAL voice-input package,
  `conversation.hero.workspace` is real, and `conversation.hero.agentPreset` renders
  only while `developerTools` is on (`if (!main || !developerTools || !ready) return
  null`), which bsh has no switch for. One grep answers "does the reference have this
  control?": `grep -o 'name: "conversation\.[a-zA-Z.]*"' reference/pkgs/*.js`.


- Round 113 measured the background-jobs trigger (dsh-client-ui-jobs,
  conversation.session.header.actions). It is NOT a pill: .trigger{min-height:28px;
  border-radius:radius-sm;background:0 0;border:0;gap:3px;padding:3px 2px;font-size:12px;
  line-height:18px;color:label-tertiary} over [StateDot ongoing] [count{margin:0 5px}]
  [chevron 12], and the panel it opens is 500 wide (max min(560px,100vw - 32px), tall
  min(480px,100vh - 140px)) at radius-lg 16 on specific-menu, led by the 11px/16px
  .sectionHeader "Running". Its dictionary owns a real plural rule (count.live.one|other),
  so a count is never hardcoded. Trap: a kit Button owns its own height, so a measured
  min-height must be set as .h(px(28.0)) - .min_h left it at the Button's own 32.
- Trap: the SHARED working copy's parent commit can change under a round (the concurrent
  writer's catalog/input-types refactor landed at @-). Before blaming your own change for a
  red test, run `jj file show -r @- crates/bsh/src/ui/shell_tests.rs` and grep for the test
  name; a test that exists at @- and touches code your round never edited is inherited.


- Round 114 made the chat column follow the reference's own clamp, because bsh drew the
  floor everywhere: `.body` carries `--dsh-chat-content-width: var(--dsh-chat-user-width,
  clamp(680px, calc(var(--dsh-conversation-column-width) * .64), 920px))` and the composer
  card is `+ 32`. `theme::chat_width_at(conversation_w)` (CHAT_MAX_W 920, CHAT_SHARE .64) and
  `Shell::chat_w()` now feed all twelve sites, and a frame with no measurement (zero) keeps
  the 680 floor. Trap: gpui-pre's `Window::resize` does NOT change the harness viewport (a
  test asking for 1000x900 still measured a 1640 column after two draws), so prove a
  responsive rule by asserting the relation to the live `conversation_w` plus one absolute
  value that discriminates against the old constant.

- Round 115 drew the content column's width handles and made the width they set real.
  Measured: TWO strips, `min(10px, calc((100% - content) / 2 - 24px - 24px))`, parked
  `right:/left: calc(50% + content / 2 + 24px)` off the body, each with a 2px bar 4px
  in from the content edge (`:after{right:4px}` / `{left:4px}`, opacity 0 until hover
  or `[data-dragging]`), and the drag is `outwardWidth() = base + outward * 2` with the
  pointer committed only when it moved, over `resolveContentWidth(column, preference)`
  = `clamp(preference, 640, max(640, column - 176))` and the clamp when there is none.
  `settings.content_width` is the store (the reference uses localStorage), `assemble`
  reads it back, and `Shell::chat_w()` resolves through it. Traps: GPUI snaps painted
  children to a device pixel, so a computed-edge geometry assertion needs a half-pixel
  tolerance; and a `cx.listener` mouse handler can sit in the render chain, but the
  handle that takes `on_mouse_down` wants its own `.id(..)`. Owed: the hover reveal and
  its pointer-Y gradient (gpui-pre has no hover trait and GPUI no CSS gradient), and
  the `:has([data-conversation-composer-overlay])` hide.

- Round 116 drew the width handle's bar and its reveal. Measured: the bar is a 2px
  column, 72 tall, whose four CSS stops (transparent y-36, scrollbar-bg-l1 y-8,
  scrollbar-bg-l1 y+8, transparent y+36) are a solid 16px band between two 28px
  fades, centred on a pointer-y whose DEFAULT is 50% - so a 2px column centred in a
  10px strip is the measured 4px inset. The colour is `--dsw-alias-scrollbar-bg-l1`
  = `--dsw-static-neutral-200` #e5e5e5 light / `-700` #3c3c3d dark. Traps:
  gpui-pre's `Background` is fixed at two stops (`colors: [LinearColorStop; 2]`,
  private fields, the only public helper `linear_gradient(angle, from, to)`), so a
  four-stop CSS gradient has to be stacked layers, and the angle is 180 for CSS's
  `to bottom`; and a child's hover state needs `cx.stop_propagation()` in its own
  move handler, or the parent's move clears it in the same dispatch. Owed:
  pointer-y tracking and the composer-overlay hide.

- Round 117 drew the sidebar's own resize handle. Measured in
  `dsh-client-ui-layout-client.js`: `setSidebar` is
  `clampWidth(px, 264, 420)` with `clampWidth = Math.min(max, Math.max(min,
  Math.round(px)))`; `onSidebarStart` records the column width and the drag sets
  `base + dx` (the pointer's own delta - this column is NOT centred, so unlike the
  content width there is no doubling); the shared `DragHandle` is an 8px
  `col-resize` strip at `left: cols.sidebar` with `margin-left:-4px` and
  `z-index:11`, rendered only while `!sidebarCollapsed`, and `data-dragging`
  disables its `left` transition; `sidebarPreference` falls back to 280 when the
  store holds 0, and darwin's collapsed width is 0 (56 elsewhere). Trap worth
  keeping: gpui dispatches a mouse MOVE to the element under the pointer but a
  mouse UP back to the element pressed, so a widget that must follow a drag needs
  its press and release on itself and its moves on an ancestor - a probe showed
  the down and move arriving while a frame-level up never fired. Owed: the
  reference's 1024 `SIDEBAR_AUTO_COLLAPSE` with its `narrowExpanded` flag, the
  `left` transition, and the rightbar handle.

- Round 118 closed the sidebar's auto-collapse. Measured in
  `dsh-client-ui-layout-client.js`: `SIDEBAR_AUTO_COLLAPSE = 1024`, and
  `sidebarCollapsed = narrow ? !layoutInfo.narrowExpanded : layoutInfo.sidebar === 0`
  with `narrow = viewport < SIDEBAR_AUTO_COLLAPSE` (strict, so exactly 1024 is not
  narrow); `toggleSidebar` flips the runtime `narrowExpanded` below the breakpoint
  and the stored `sidebar = 0` collision above it; `collapsedWidth` is 0 on darwin
  (56 elsewhere). bsh mirrors it as `theme::sidebar_folded` +
  `AppState.narrow_expanded` + `Shell.viewport_w` + `Shell::toggle_sidebar_at`, and
  the old `AppState::toggle_sidebar` went with it (clippy's dead_code found it).
  Trap: a harness `cx.notify()` schedules a frame, and `render` re-reads the real
  window width into the recorded viewport, so a test that measures a narrow
  viewport must set the field WITHOUT notifying or its next read sees the old width.

- Round 119 drew the jobs panel's own row. The reference's live line is
  `.rowLineLive` (fill-l2, radius-md, `padding:4px 8px 4px 2px`) holding a
  transparent `.row` (min-h 28, radius-sm, `padding:4px 7px`, 12px/17px
  label-primary), the ongoing dot, the label and `.duration` (11px/18px
  label-tertiary); the stop control is `.stop` (20x20, `.5px` border-l2, a
  10px mark in label-secondary), `.stopArmed` grows to `width:auto` with a 4px
  gap, `0 7px` padding and an 11px/18px `Confirm stop`, `KILL_ARM_MS = 3e3` and
  `KILL_FAILED_MS = 4e3` are its two windows, and `formatDuration` is hours >
  0 / minutes > 0 / seconds. bsh renders it through `job_chip`'s
  `PopupMenuItem::element` rows, `JobArm`/`JobPhase` and a 1-second `tick_jobs`
  clock. Owed with the registrant named: the row's `kind` chip and 10px
  `status` word (no producer names a kind; bsh keeps no progress/detail), the
  mono label family, the `chevronBox` with the whole output panel (`jobs`
  exposes no retained output), and the settled roster with
  `section.settledCount`/`section.clear` (bsh writes no settled job record).
  Trap worth keeping: a python `replace(..., 1)` whose anchor appears in several
  functions edits the FIRST one, so scope a splice with
  `text.index("fn job_chip", ...)` before cutting, and a corrupted file is
  recoverable from `jj --ignore-working-copy --at-op=<op> file show -r @ <path>`
  by scanning the op log for the last snapshot that still holds the deleted
  marker.

- The settings rail is the reference's, not ours (round 121): DSH draws one cell
  per `settings.section` registration and ships five - `account` (-10), `general`
  (0), `models` (10), `plugins` (15, "Built-in plugins"), `agent-presets` (20) -
  so `NAV` in `ui/settings.rs` is exactly those five and everything else is a card,
  a tab or a sidebar panel. Panes the rail does not carry live in the Built-in
  plugins pane's `VIEWS` list (inventory tab first, then Shell 10 / Agent loop 20,
  then the sidebar panes); About is General's own `current-version` row group. A test
  asserts `NAV` + `VIEWS` partition `SECTIONS`, so a new pane goes into one of them.

- The composer's command card is `crates/bsh/src/ui/command_menu.rs`: `entries(
  streaming)` is the reference's option list (attach a file, new chat, settings,
  toggle the sidebar, plus stop turn while a turn streams), `slash_query` is the
  trigger (the draft's last whitespace token when it starts with `/`),
  `filter_indices` is its `filterOptions` substring test over label and detail, and
  `run_command` closes the card, consumes the token, then runs the row. The `+` and
  an open card's Enter both land there. Escape/arrows/Tab are owed to the kit: its
  `InputEvent` is `Change | PressEnter | Focus | Blur` and bsh has no `bind_keys`.
  Trap: the test harness's `set_value` does not fire the kit's `InputEvent::Change`,
  so a draft the menu must see is set through `set_command_draft` (both the field
  and `composer_value`); the tests prove it by asserting the draft.
- The composer card has two sources (round 123): `Trigger::Command` (the `/` token
  or the `+` control) and `Trigger::Reference` (the `@` token, the reference's
  `inputTriggers` source in `dsh-client-ui-reference-client.js`). Both build
  `CardRow`s through `card_rows`, so the renderer and the measured numbers are
  shared; `at_query` is the trigger (a quoted mention keeps its spaces), `mention`
  writes `value.mention` (quoted when the name has a space, `/`-suffixed for a
  directory), `reference_files` is the bounded workspace walk, and a directory row
  drills (`CardAction::Drill`) instead of inserting.

Round 124 rebuilt the Models pane onto the tree-backed settings, and the work was
mostly finding why six of the seven things it should do were silently not doing them.
The single highest-value seam: the shipped presets name **bundles** (`shell`, `files`,
`plan`) while `build_agent` compared a **tool** name (`bash`, `read_file`,
`update_plan`) against them, so `off("bash")` read `!allowed.contains("bash")` ->
`true` and every preset switched every tool off - `bundles` was empty on every build
while every operand of the gate read green. The gate answers by bundle, the tool switch
by tool (`off_bundle` / `off_tool`), and the two cannot lean on each other. Two traps
that made the green probe misleading: the anchor string carried an apostrophe the
comments did not, so an instrumentation splice silently matched nothing and reported
a healthy file; and a probe `eprintln!` landed outside the `mod tests` it needed,
which is the same mistake as a standalone `#[test]` at file end - append a probe
INSIDE the module, with `crate::` absolute paths.

Second seam, same shape: `Session::new(workspace)` stores whatever string it is handed,
`new_session` passed the workspace's **root**, and the sidebar filters `in_workspace`
by the registry's **id** - so every new session was invisible to the row it lived
under and to `restore_session`. A session is tagged with the stable id, which is
what `WorkspaceRef`'s own doc already said. Third: `ensure_workspace` set the active id
to the first *registered* workspace but never *registered* one, so a fresh install
had no root to confine its tools to and no session it could create; it now names the
directory the launch came from, which is a fact about the launch rather than a seeded
preference.

Fourth: a test that changed a preference wrote the **user's real** configuration,
because `save()` recomputed the path from a static. The file carried a workspace
pointing at a test's temp directory, which is how the shell ended up confined to
one. `Settings` now carries `save_root` (serde-skipped, so a location is never a
setting) and `assemble` points it at the scratch store, which closes it for every
test rather than for the one that was audited.

The switch-over is complete: the three scoped `#![allow(dead_code)]` in the settings
model are gone and clippy proves every definition in the tree is wired. What that
exposed first was the **refusal seam** - `Refusal`, `ModelChoice::refusal` and
`Resolution::refusal` were documented and unused, so a stale name was quietly
substituted. `Settings::refusal()` now maps the three variants onto the composer's
own keys (`composer.noModels` / `composer.modelUnavailable` /
`composer.presetUnavailable`), `Shell::send` shows one instead of opening a turn
against a model nobody chose, and the provider editor draws the refusal it already
computed rather than making the reader guess why Apply did nothing. The pane's own
head is identity - name, `Custom` tag, credential dot, Edit/Delete - with no way to
select the endpoint by pressing it, because the reference has no such affordance.

Switching-over also meant the input-type fold had to become a fact about the **id
shape** (`contains('/'` folds) rather than about what a leaf declares: once no leaf
auto-declared `text`, the old rule `case_insensitive = text_only_model(provider).is_none()`
became tautological and every provider folded, which is the `ModelIdDuplicate` the
user reported as the wrong model. The green suite is 254 passing, fmt/clippy/doc
clean.


Round 125 finished the add card the two screenshots disagreed on. The live pane
drew the catalogue as a stack of clickable provider rows — a green-marked list of
endpoints — while the reference draws one select.input.selectInput whose value is
a provider **id** (an option element per addable row, value: row.entry.provider).
The instruction was "Green marked should show as dropdown selections which is the
provider id", and the two are different controls, not different ink: a list claims
the screen the reference gives to the editor under it, and it reads as a set of
choices to make rather than the one value a select holds.

So catalogue_pick is now the measured control: a field column carrying the
Provider label in the shared editor_field style, a 32px closed box at the
selectInput metrics (.5px border-l4, radius-md, bg-layer-1, padding 0 10px with
padding-right 32px, max-width 240px, a ChevronDown at 12) that toggles
Shell::catalogue_open, and, only while open, one option row per
addable_providers() id styled as candidateLabel (radius-md, 28px, padding 0 10px)
that sets catalogue_choice and closes again. The chooser no longer prints the
family name, because the id is the key the tree is written under and the prefix
the engine's router registers — a reader who picks by family cannot tell which of
two same-family providers they are editing. catalogue_open resets wherever
catalogue_choice does, so a cancelled card never reopens the list.

The same pass found three chrome gaps and closed two. The catalogue heading was a
single modelsCustomized/modelsInherited line; the reference draws
modelCatalogHeading as a column (gap 2px) with modelCatalogTitle (the Models
title, 12px/500 label-secondary) ABOVE modelCatalogMeta (label-tertiary), so the
title is back. The modelListHead toolbar was fetch-then-restore; the reference's
own order is restore-then-fetch, and modelListHead is align-items flex-start with
gap 12px, which both now hold. The addModelButton takes its measured metrics
(28px, gap 4px not 6, .5px border-l3, align-self flex-start), and the fetch
trigger now names settings.fetching ("Asking the provider" / 正在询问提供商) while a
request is in flight, so a second press cannot start a second request — the busy
flag is cleared on the answer, on a refusal, and when the provider it was asking
about is deleted.

Root cause of the raw keys in the earlier screenshot, since it was the bug the
pane actually showed: **six live keys were used in crates/bsh/src but absent from
both locale files**, and four more existed with drifted values. A missing
rust-i18n key renders as the key itself, which is why settings.create and
settings.modelsEmpty appeared as literal text. All six added and four corrected
against the reference's own inline dictionary; the key-resolution test now covers
eleven keys, and a python call-site audit counts the use of each.

Honest gaps, not drawn: GPUI has no dashed border, so modelEmpty's
"border:1px dashed border-l3" is a solid .5px l3 rule; no select primitive exists
in gpui-kit, so the dropdown is built from the same Stateful<Div> rows the pane
already uses rather than a host control (which costs the chevron's exact 12x12
data-URI, replaced by the kit's IconName::ChevronDown at 12px); and the
settings.models title string is free copy rather than a measured one, because the
reference's title for that heading is not in the Models package dictionary.
The green suite is 255 passing, fmt/clippy/doc clean, and
the_catalogue_provider_chooser_is_a_select_of_ids presses the closed box, proves
the list is closed, presses it again, proves all five ids are offered, picks one
and proves both that the choice moved and that the list closed.

Round 126 drove the add-model flow in the running app with `ocu` and found five
defects that the test suite could not see, because each one only shows as a
window that renders the wrong thing.

**The capture recipe that works** (the one in this file needed three fixes).
`get_app_state` captures the WINDOW through ScreenCaptureKit and downscales it
so the width is at most 1280, so a 1280x820-point window gives 1 screenshot pixel
per point while a fullscreen 1800x1130-point window gives 0.711. `click` with
`click_method: "app_post"` takes those raw screenshot coordinates — NOT global
screen coordinates, and NOT the window's backing pixels, and adding the window
origin (measured at 160,160) lands every press 160px up and left, on the title
bar. `app_post` on a GPUI view only ever paints the hover/press state: the pane
lights the row under the pointer and no `on_click` fires. `sky_click` at the same
coordinates does fire it, so that is the method to use for a GPUI window. An
Accessibility action (`click` with `element_index`) also fires, but the dialog's
controls are not in the tree — only the shell's chrome and the window buttons are
— so the pane is still reached with `--settings=SECTION` and pressed by
coordinate. Typing needs the field focused first; `ocu js` with
`click` then `typeText` in one process is far steadier than two `ocu call`
invocations, which intermittently typed into nothing. Launch the dev binary
(`cargo build -p bsh` then `./target/debug/bsh --settings=models`), not the
`.app`: LaunchServices drops `--args`, and a bundle-id launch also lets
`ocu` resurrect the app through `open` with an inherited environment.

The five defects, in the order the flow exposed them:

1. **The add card's field labels were raw keys.** `editor_field` took a
   `label: &'static str` and drew it verbatim while every caller passed a locale
   key, so the card read `settings.provider` and `settings.apiKey`. It
   translates now, which also fixed the custom route's four fields
   (`settings.customRoute`, `settings.displayName`, `settings.customBaseUrl`,
   `settings.apiMode`). A key argument has to be translated where it is drawn,
   or the missing-`tr` is invisible in review: the call sites all look correct.
2. **`settings.workspace.agent.permission` was not a key.** The General pane's
   permission row named a dotted path that exists in neither locale, so the row
   was titled with its own key while `settings.permission` ("Permission" / 权限)
   sat unused in both files — the same shape as the earlier
   `settings.modelOptions` bug. Grep the key, not the call site.
3. **The editor's Add model button read fields the pane never drew.** It read
   `new_row_id`/`new_row_name`, which only the add card's Custom route renders,
   so in the provider editor it could only ever answer `settings.modelIdRequired`
   about text nobody could type. The editor now owns `new_model_id`/
   `new_model_name` (a model id typed for the provider already open is a
   different fact from the custom route's, so sharing one engine carried text
   between two forms), draws them in the measured 28px row above the control, and
   clears them only on a successful add so a refusal keeps what was typed.
4. **`reload_model_rows` read `state.settings` while every pane write lands in
   `state.draft()`**, so rebuilding the rows echoed the copy the reader had not
   applied and the model that was just added never appeared. This is the rule the
   round history already states — a pane reads the draft — broken inside a
   helper rather than at a call site.
5. **A locale edit did not rebuild the crate.** `rust_i18n::i18n!` embeds
   `locales/*.yml` at compile time and nothing declared them as inputs, so
   `cargo build` reported "Finished" in under a second with the old strings
   still baked in, and a correct translation kept rendering as its key.
   `crates/bsh/build.rs` now emits `cargo:rerun-if-changed` for both files.
   This is the one that made bugs 1 and 2 look like a stale binary.

Evidence: the flow was driven end to end in the window — add card, select
`nvidia` from the provider select, type a key, Create, Edit, type
`z-ai/glm-5.3-flash`, Add model — and the pane then showed an editable model row
with that id, the heading flipped from "Using the adapter defaults" to
"Customized model catalog", the empty-state note withdrew, and the add fields
cleared. `the_editor_add_model_row_draws_the_id_it_adds` pins it headlessly and
was discrimination-checked twice: pointing the selector at a name that is not
drawn fails it, and reverting the draft read fails its row count. Gates: fmt
clean, clippy 0, `cargo test -p bsh` 256 passed, doc 0.

Honest gaps: `app_post` producing hover without a click is unexplained (GPUI may
drop a synthetic click whose window number the event lacks), and it is why the
recipe insists on `sky_click`; the model row's own name field and chevron are
drawn but this round did not drive them; and the measured 24x24 per-field help
button from round 96 is still owed.

Round 127 made settings actually reach the files that own them. The audit found
five places where a value was written to the wrong copy, and the shape of all
five is the same: a pane edits the DRAFT, but the write went to `settings` (the
committed tree) or the save persisted `settings` while the value lived in the
draft.

Measured live, with a probe workspace so both files are readable: `HOME` a
scratch home whose `settings.toml` names a workspace rooted at
`/tmp/bsh-ws-probe`. Pick Read Only in General and the screen shows Read Only;
the workspace file still said `workspace-write`, because the LANGUAGE pick beside
it called `settings.save()` and stamped the committed copy of every other field.
That is the bug the four axis check calls a silent loss: the reader is told
"saved" and the file disagrees with the screen.

1. **The preview actions persisted the committed tree.** `language_action`,
   `theme_action` and `font_size_action` wrote `state.settings` and saved it;
   `developer_tools_action` wrote the draft and then saved `settings`, so its own
   toggle was the one field the write did not carry. All four edit the draft now
   and let Save persist. `Shell::cancel_settings` puts the two previews back
   (interface language, palette) from the committed tree, so a cancelled dialog
   no longer keeps the language it was merely trying on.
2. **Every render read the committed tree.** Twenty-two sites in composer, header,
   meter, settings, shell, statusbar and transcript read
   `state.settings.global.general.*`, so NO preview followed the draft - the font
   size, the transcript view, the developer-tools gate. They read `state.draft()`
   now, which is also what makes Cancel revert them correctly. Two of the
   twenty-two were WRITES (the drag-committed content and sidebar widths) and take
   `draft_mut()`.
3. **The drag commits saved the committed tree.** `commit_chat_width` and
   `commit_sidebar_width` wrote the draft then called `settings.save()`, so the
   width a reader dragged never reached the file. Both go through
   `commit_settings()`, which is the one path that moves a draft into the
   document.
4. **Creating a provider made the document uncommittable.** `select_provider`
   wrote the provider half and left the model half empty, and the workspace file
   refuses a half-made selection - so adding a provider failed the NEXT save with
   "the selection \"deepseek/\" names only half a model", about a model the reader
   had never been asked for. A provider with no models now leaves the selection
   untouched (the pair is both or neither, and the previous working model is not
   dropped); one with models moves both halves.
5. **A refused save closed the dialog.** `commit_settings` cleared the overlay
   before writing, so the refusal arrived in a notice with the pane already gone -
   the one place the fix can be made. It closes only on success and hands the
   draft back on failure.

A sixth change is the guard that makes the rest legible: `save()` no longer
silently skips the workspace document when the active workspace has no root. A
blank document is still nothing to report, but a document with content and no
root is an error the Save bar shows, because the only copy was the window.

Evidence, live: with the probe layout, changing the Context window and pressing
Save took the workspace file from `context_budget = 32768` to `64000` while the
global file carried no budget at all; creating a provider and pressing Save put
`[providers.deepseek]` (base URL, key, protocol) in the GLOBAL file while the
workspace file kept `provider = ""` and no provider block. Both directions of the
split are proven on disk, not only in a unit test.

Tests: `a_save_writes_each_fact_only_to_its_own_file` (both paths and both
directions of the split, read back from disk), `a_draft_edit_writes_nothing_until_
it_is_committed`, `a_save_with_no_workspace_root_reports_instead_of_dropping`,
`selecting_a_provider_never_half_makes_the_selection`,
`a_refused_commit_keeps_the_dialog_open` and
`cancelling_settings_restores_the_previewed_language`. Two of them were
discrimination-checked by re-injecting the bug (dropping the workspace write; and
removing the preview restore). Five older tests asserted against `settings` after
driving a control and now read `draft()`, which is the rule this crate already
states for pane tests. Gates: fmt clean, clippy 0, `cargo test -p bsh` 262
passed, doc 0.

Honest gaps: the save bar sits inside the scrolling options box, so in the
General pane it is below the fold at the reference window size and neither
synthetic wheels nor keyboard paging reach it through `ocu` - the live Save press
was proven on the Context and Models panes instead. `developer_tools` is drafted
rather than written through the controller as the reference does (round 124's
measurement), chosen so one rule covers every preference; its inline error now
belongs to the commit. And `~/.bos/bsh/settings.toml` is still the old flat
format, so the app runs on defaults until it is rewritten - unchanged from the
last round, and never written by these tests because every one redirects the
save.

Round 128 closed the two gaps round 127 reported, and the second one uncovered a
third defect that the live check had been hiding.

**The save bar is pinned.** It was the last child of the scrolling options box, so
the dialog's ONE committing control scrolled away with the section. Measured in
the harness: in General it laid out at **y=1214 in a 900-tall window** - off
screen - while Models put it at 885. `the_save_bar_does_not_scroll_with_the_section`
pins it by asserting the models and general panes place the bar within 40px of each
other, which is true only when it is a sibling of the scroller; re-injecting the
old layout fails it with exactly those two numbers (885 / 1214.5). That also
retires the round-127 gap: the General pane's own Save is now reachable, and the
live proof it could not give before was run - Permission -> Read Only and
Language -> 中文, Save pressed at the pinned (985,757), after which the GLOBAL file
held `lang = "zh"` and the WORKSPACE file held `permission = "read-only"`, with
both files still on their old values before the press.

**A provider is named by its name, never its endpoint.** `built_in` answers
`(display name, base URL, protocol)`, and two sites read the SECOND element as
the label: the Models card printed `https://integrate.api.nvidia.com/v1` where the
provider's name belongs, and the provider editor opened its display-name field
pre-filled with the endpoint. Both go through `provider_label(id, name)` now -
the tree's name, else the shipped family's, else the id -
and `a_provider_is_named_by_its_name_not_its_endpoint` asserts every shipped id
produces a name with no `://` in it. The wrong element was visible in a round-126
screenshot and read as a design deviation; it was an off-by-one in a tuple
pattern, which is why the test checks the INVARIANT (a name is not a URL) rather
than the expected strings alone.

**The user's own configuration is in the new format.** `~/.bos/bsh/settings.toml`
was the old flat file, which the tree refuses under `deny_unknown_fields`, so the
app had been running on defaults and would have moved the file aside on the first
Save. It was rewritten by GENERATING it from the real types (a temporary test that
built the documents and asserted they reparse, then was removed) rather than
hand-typed, so the schema cannot drift; the legacy file is kept at
`settings.toml.legacy-2026-10-18`. Carried over: the local relay as
`[providers.nvidia]` (base_url `http://127.0.0.1:11436/v1`, api_key, `api_mode =
"chat"`) with its `z-ai/glm-5.3-flash` leaf, the workspace rooted at this repo
(id `bos`) selecting that model, and the agent section (system prompt,
temperature 0.7, permission workspace-write, budget 32768, steps 10, shell 300,
cap 16384, memory on), plus `skills_dir`. Dropped because the redesigned schema
has no such field: `require_approval` (the permission mode expresses it),
`web_tools_enabled`, `active_provider`, `catalogs`, and the empty
`memory_path`. The workspace half was written to `.bsh/settings/settings.toml`
inside the workspace (which `.gitignore` already covers), the global half needed
one `danger-full-access` write because it lives outside the session workspace.
Verified live: the shell now starts on workspace "bos", the status bar reads
`z-ai/glm-5.3-flash`, the Models card reads NVIDIA with its credential dot, and
the editor opens on that model under "Customized model catalog".

Gates: fmt clean, clippy 0, `cargo test -p bsh` 264 passed, doc 0.

Honest gaps: `web_tools_enabled` had no home in the redesigned schema, so a
preference the flat file carried is gone by design rather than by oversight, and
nothing was written for `catalogs`; the Base URL is not shown on the collapsed
provider card (it sits under "Customized settings"), so the endpoint was confirmed
from the file and the editor's model row rather than by reading it off the card.

Round 129 gave the two preferences round 128 had to report as homeless a real
home, and made the workspace document travel with the checkout.

**Web search is a setting now, not a dropped field.** The stack already carried
the capability and nothing offered it: `react::llm::LlmTool::web_search` builds a
HOSTED tool (the provider runs the search and returns results in its own stream),
and the only place it was attached was a test. The chain is now four links, and
each link exists because of what the previous one cannot know:

- `AgentSection.web_tools_enabled` (workspace tier, default true) is the reader's
  half, beside `bash_enabled` and `file_tools_enabled` - the same three switches
  the flat file carried.
- `Resolution` carries it and `Effective` exposes it. Two structs, not one: the
  first attempt put the field on `Resolution` only, and the compiler said
  `Effective` had no such field; `Effective` is built FROM a `Resolution`, so both
  need it and the constructor reads `resolution.web_tools_enabled`.
- `AgentConfig.web_search` is the agent's half, and `offers_web_search(enabled,
  api_mode) == enabled && api_mode == "responses"` is the rule. The protocol is
  part of it because a hosted tool is an object only the Responses API
  understands: offering it to a chat-completions or anthropic endpoint would fail
  every request rather than add a capability. The rule is a pure function with a
  truth table test, so the decision is not a caller's to remember.
- The Tools pane draws the row. It is NOT in the capability list below it,
  because nothing local registers it - the provider runs it - so a switchless
  tool would have been invisible. `the_tools_pane_switches_the_hosted_web_search`
  presses it and asserts both the draft and the built agent follow.

The bindings do not expose the knob, matching `parallel_tool_calls`: nbos, jsbos
and the resilience example take `web_search: false`, which is what
`AgentConfig::default()` says. Extending the Python/JS option structs is a
separate change with its own docs and parity entries.

**The workspace document travels with the checkout.** `.gitignore` ignored all of
`.bsh/`, which contradicted the file's own documented purpose ("the file travels
with the checkout, so a second machine that clones it inherits the same agent
configuration without carrying anyone's credentials"). It is now `.bsh/*` with
`!.bsh/settings/` re-included, so `git check-ignore` answers ignored for
`sessions/` and `log/` and NOT for `settings/settings.toml`, which is the split
that sentence claims.

Also fixed while in the pane that gained the row: the Tools pane printed its hint
TWICE - the pane header draws the section hint and the first row repeated the same
key. The row is gone, so the sentence appears once. The same duplication shape
exists in a few other sections (mcp, workspaces, archived) and is left for a
round that touches them.

Evidence, live: with a redirected home so the write is permitted, toggling Web
search off and pressing Save put `web_tools_enabled = false` on line 13 of the
workspace file and nothing in the global file. With the real home the same press
was REFUSED - "cannot read the settings file: Operation not permitted" - which is
this agent's sandbox rather than the product, and it incidentally re-proved the
round-127 refusal path: the dialog stayed open with the draft intact.
`~/.bos/bsh/settings.toml` still cannot be written from here for the same reason,
so the installed workspace document gained the line by hand and was re-read by the
app (the switch renders on, which is the round trip).

Gates: fmt clean, clippy 0 across the workspace, `cargo test -p bsh` 266 passed,
`cargo test -p agent` 190 passed, doc 0.

Honest gaps: `require_approval`, `active_provider` and `catalogs` remain
unmigrated rather than wired - the first two are expressed by the permission mode
and the workspace selection, and `catalogs` was absorbed into the provider tree,
so nothing was invented for them; and the hosted tool is only reachable on a
Responses-protocol provider, which is why the row's description says so instead of
silently doing nothing on a chat endpoint.

Round 130 verified the three caveats round 129 reported, with the session's file
sandbox lifted so the one that could only be reasoned about became measurable.

1. **The four constructors.** `AgentConfig.web_search` broke every literal that
   names all fields: the TOML bridge (`agent/config.rs`), both bindings
   (`nbos`, `jsbos`) and `examples/with_resilience.rs`. All four now carry it, and
   `cargo build --workspace` is the proof that no literal was missed - the other
   sites use `..Default::default()` and need nothing. A brace-matching scan is NOT
   a usable check here: `format!("{}/{}", ...)` inside a literal makes a naive
   depth count report fields missing that are present, which is how the first
   attempt "found" seven missing that did not exist.
2. **The real-home save.** With `danger-full-access` the same Save that reported
   "Operation not permitted" advanced BOTH real files (13:49:15 -> 13:51:46),
   closed the dialog, and showed "Settings saved", and the composer's permission
   chip followed the file (`read-only`, then `workspace-write` after restoring).
   So the refusal was the agent's sandbox and never the product. One artefact
   worth recording: a synthetic multi-click sequence (open the menu, pick, press
   Save) can leave the dialog up while the write still lands, because a press that
   arrives as a menu is closing is not the press the next step assumes. The clean
   sequence (one Save press) behaves correctly, and
   `a_successful_commit_closes_the_dialog` now pins the success half of
   `commit_settings` headlessly, beside `a_refused_commit_keeps_the_dialog_open`.
3. **The three homeless fields.** `require_approval` and `catalogs` appear nowhere
   in the crate; `active_provider` exists only as `Settings::active_provider()`, a
   getter derived from the workspace selection rather than a stored copy. The
   substitute is real rather than a rename: `is_confined()` keeps the shell
   validator and the files' root for every mode but full access, and
   `allows_writes()` keeps the shell and the writer OUT OF THE REQUEST under Read
   Only, so "no approval needed" is expressed as "the tool is not offered".

Gates: fmt clean, clippy 0, `cargo test -p bsh` 267 passed, `cargo test -p agent`
190 passed, doc 0. The live configuration was left as it was found: global file
untouched after the round's writes (byte-identical to its backup), workspace
document with `permission = "workspace-write"` and `web_tools_enabled = true`, and
the legacy flat file still at `settings.toml.legacy-2026-10-18`.

Round 131 made the whole on-disk configuration the current format, not only the
two documents that had been rewritten. The audit found three things the earlier
rounds had left behind.

**Two superseded flat documents were sitting in the live settings directory.**
`settings.toml.bak` and `settings.toml.legacy-2026-10-18` were both
`schema_version = 1` flat files - the format the tree refuses. Nothing reads
`*.bak`, but a directory whose contents disagree about their own format is a
trap for the next reader and for the next round. Both moved to
`~/.bos/bsh/archive/legacy-flat/` with `.toml.old` names, so the live directory
holds exactly the documents the app reads: `settings.toml`, `sessions/`, and
`log/`. Nothing was deleted.

**All sixteen sessions were orphaned.** Every session document carried a
`workspace` id that the registry no longer names - four different dead ids, and
the old flat files' own registries pointed at `root = "/"` (that was the old
format's bug). The sidebar therefore listed none of the history: the shell
filters `in_workspace` by the registry id, so a session tagged with an id nobody
registers is invisible while the file parses perfectly. All sixteen are re-tagged
to the one live workspace (`bos`), which is what made them appear, with a copy of
each file kept under `~/.bos/bsh/archive/sessions-before-retag/`. This is a
migration the user asked for by name ("ensure all settings in new format"); the
session document itself needed no schema change, only its workspace reference.

**The `api_key` found in one session is not a schema leak.** A grep for the field
matched three places, all inside tool OUTPUT - a `cat` of the old settings file, a
source grep, a test's printed config - not a structured field of the session
document. The session schema stores the provider and model as a reference and
never a credential, which held. Redacting a verbatim quote inside conversation
history would corrupt the transcript, so it was left alone and reported instead.

Two surfaces deliberately untouched: `~/.bos/conf/config.toml` and
`~/.bos/config/app.toml` belong to the BOS framework, not to bsh ("BSH does not
read the ambient BOS config: the settings files are the whole truth for this
shell"), and their own backup clutter is that system's business.

Verified live: the app starts on the one workspace, the composer chip reads
`Workspace Write` from the workspace document, the status bar names
`z-ai/glm-5.3-flash` from the global one, and the sidebar lists the recovered
history - which is the proof that every session file parses under the current
schema, because the store drops the ones that do not, silently.

No source changed this round: it is a data migration plus an audit.

Round 132 fixed the three problems round 131 could only report, and the work
turned up a fourth that has been quietly destroying the reader's configuration.

**The superseded document no longer sits beside the live one.** `save()` used to
rename a file it could not parse to `settings.toml.bak` IN PLACE, so an
old-format document stayed in the live directory for good and every later reader
had to work out which was which. It now goes to `archive/legacy-flat/` with the
epoch in its name (`settings-flat-<unix>.toml.old`), which is both a record and
impossible to mistake for a live file. `an_unreadable_document_is_archived_beside_the_tree`
pins the live file, the absence of a `.bak`, and the archived name and content.

**A session whose workspace nobody registers is re-homed, not hidden.** It loaded
fine and then `in_workspace` filtered it out of every row: history on disk and
unreachable, which is a worse failure than not loading, because nothing looks
wrong. `Store::rehome_orphans(active)` attaches those sessions to the workspace in
use - there is no other workspace to attribute them to - and deletes the
quarantined copy so the id is not on disk twice. That last part needed a second
fix: `read` did not dedupe by id, so the same session on disk twice was shown
twice; it now keeps one per id, newest wins.

**A configured credential never reaches a session file.** A transcript can quote
the configuration - a tool that printed the settings file puts the key in the
conversation - and the session document is documented to carry provider
REFERENCES, never a credential. `Store` now takes the live keys
(`Settings::secrets()`) and scrubs the exact configured values out of the
serialized text at the one place a session is written, replacing them with `***`.
Only the exact values, never a pattern, so ordinary prose is not mangled; keys
shorter than six characters are skipped because they are not credentials and
replacing them would corrupt text. The sixteen sessions already on disk were
scrubbed with the same rule and a copy of each kept under
`archive/sessions-before-scrub/`.

**The fourth problem: a test build could write the REAL configuration.** While
repairing the file it was overwritten three times with fixture values - a
`providers.work` entry pointing at `https://work.example` and a workspace rooted
at a DIFFERENT checkout, `~/.local/share/opencode/worktree/97f97e/feature-work`.
Every test redirects its writes by setting `save_root` to its scratch store, and
that redirect works here (`cargo test -p bsh` leaves the file byte-identical), so
the writer was a test run in that second worktree: one `$HOME` is shared by every
checkout on the machine. A forgotten redirect anywhere is therefore not a failed
assertion, it is the reader's settings replaced by a fixture. `save()` now
refuses outright when it is a test build with no `save_root`, which makes the
class impossible in this build, and
`a_test_build_never_writes_the_real_configuration` pins it. The other worktree
still needs the same line before its test runs stop overwriting the file.

Evidence: each of the three store/settings behaviours was re-injected and the test
failed with its own message (a no-op re-home, an identity redact, a plain rename);
`cargo test -p bsh` ran with the real file's md5 unchanged before and after.

Gates: fmt clean, clippy 0, `cargo test -p bsh` 271 passed, `cargo test -p agent`
190 passed, doc 0. The reader's configuration was restored from the backup taken
in round 130 and verified intact afterwards (provider, relay base URL, key, model
leaf and workspace all present).

Two follow-ups on the same round, both learned by watching the file: the
clobbering happened again DURING the gate run, with the same signature (a
\`providers.work\` fixture and the feature-work worktree's root), which is the
confirmation that it is that checkout and not this one - \`cargo test -p bsh\` here
leaves the file byte-identical. And the sixteen re-tagged sessions were moved from
the quarantine (\`~/.bos/bsh/sessions\`) into the workspace they now belong to
(\`.bsh/sessions\`), because a session tagged \`bos\` is SAVED beside \`bos\` while its
old copy sat in the quarantine: the id would have been on disk twice, which
\`dedupe_by_id\` would mask rather than fix. \`.bsh/sessions\` is gitignored, so the
history stays local while the settings document travels.

Round 133 removed the legacy documents for good. The reader asked for the old
format to be GONE, not archived, so the two \`settings-flat-*.toml.old\` records and
the \`settings.toml.bak\` that a test run in the second checkout had left behind are
deleted, and \`~/.bos/bsh/\` now holds exactly one settings document - the new format
- beside \`sessions/\`, \`log/\` and the pre-migration session copies. \`archive_unreadable\`
stays in the code, because it is not legacy SUPPORT: it is the one guard that stops a
file this build cannot parse from being overwritten unseen, and it fires only when a
load has already failed. Nothing in the crate reads or writes the flat format; a flat
file simply fails to parse, which is what makes it the right thing to refuse.

The repeated overwrites were confirmed as the second checkout's problem, with a
timestamp: \`~/.bos/bsh/settings.toml\` became a default-value fixture (workspace rooted
at \`.../worktree/97f97e/feature-work/crates/bsh\`, no providers) at 14:05, 14:07, 14:14
and 14:15, each time while that tree's tests ran. This checkout's suite cannot do it -
\`cargo test -p bsh\` leaves the file byte-identical - because of the round-132 guard.

Round 134 reviewed the merged right panel and fixed seven defects in it, all
found by reading the pane rather than by a failing test - the suite was green
before the round and the compiler had nothing to say.

**A rename could not be made.** `files_rename_row` was a CHILD of
`files_new_row`, which only renders after a double-click on the tree's blank
space, so pressing the pencil armed `renaming` and drew nothing: no field, no
confirm control, no way to type. The row renders on its own condition now, and
`begin_rename` focuses the field the press asked for.

**The header's maximise control closed the panel.** `right_panel_control`'s
handler folded the column for BOTH controls, so the glyph promised a width the
press threw away, and the bar carried two controls doing one thing. Maximise is
real now (`RightPanel::maximized`, the glyph flips to Minimize, the width becomes
the window less the sidebar), and a hand drag gives the maximised column up.

**A click on the seam discarded the dragged width.** `commit_right_panel_width`
cleared `user_w` whenever the drag had not moved, so touching the handle reset a
size the reader had chosen. The sidebar's seam has always kept it; this one does
too, and `drag_right_panel_width` only stores a width once the drag is real.

**Row and tab element ids were keyed by file NAME.** Two branches can hold the
same name - the state fields already use a rel for exactly that reason - so the
rows shared an element id, a hover group and their action ids. Ids and the group
key on the path now; the DEBUG selector stays name-based because that is the
address the click tests use.

**Two refusals were hardcoded English.** "That path leaves the workspace." and the
terminal's "refused by policy" came from string literals; they are
`right.files.outside` and `right.terminal.refused` in both dictionaries now.

**Notes orphaned onto a path nothing pointed at.** Notes are keyed by the
workspace-relative path, so a rename left them under the old key and a delete left
them forever. `Notes::rename` and `Notes::forget` move and drop them, called from
`rename_file` and `delete_file`.

**The centre column never learned the panel existed.** `render` computed
`conversation_w = viewport - sidebar`, so with a 480px panel open on a 1280
window the chat width, the width handles and the rail's own container query all
believed in 1640px of space that the layout had given away. It subtracts the
panel now.

That measurement exposed the layout bug the capture showed: the chat width's
680px FLOOR then overflowed the 520px column, hanging the composer and the hero
over the panel's edge with their text clipped. `theme::chat_width_at` caps the
result at the column less the composer's own widen - the floor exists so the chat
is readable, not so it overflows - while a column of zero still means "not
measured yet" and keeps the floor. A maximised panel leaves a zero column, which
is what maximising a side panel means.

**And switching workspaces left the old workspace's files on screen.** Every path
in the pane is relative, so after a switch an open tab's rel resolved into the NEW
workspace's file and a save would have written there. `refile_workspace` drops
the tabs, the tree and the expanded branches, then re-lists the workspace in use;
the hero's workspace picker calls it.

Verified by five new tests, each re-injected and watched to fail with its own
message (rename row hidden, maximise closing, seam clearing, `chat_width_at`
uncapped, refile a no-op), and by two captures of the running app: the panel open
on Files with the rename row drawn over the tree and the composer wrapping inside
the narrow column, then the same panel maximised across the whole centre.

Trap worth keeping: `shutil.copy2` preserves the source's mtime, so restoring a
file that way leaves cargo believing its binary is newer and the next `cargo test`
runs the OLD build. Five tests "failed" against a fix that was already correct
until the files were touched. When a restore is involved, touch the files (or
`cargo clean -p`) before believing a red suite.
Round 135 closed the three gaps round 134 reported rather than fixed.

**The two dead locale keys are gone.** `right.files.up` and `right.files.save`
named a browser's up-one-level and a save button that the tree and cmd-s
replaced; nothing read either, so they are deleted from both dictionaries
instead of being parked. A key is vocabulary for a control that exists.

**The panel's seam carries the reveal bar.** The 2x72 bar the content handles
draw is now ONE definition (`ui::handle_bar`), used by the chat handles and by
the panel's strip: 2px wide, 72 tall, a solid 16px band between two 28px fades
of `scrollbar_bar`, at opacity 0 until the pointer is on the seam (or a drag
holds it). `Shell::right_hover` is set by the strip's own move, which stops
propagation, and cleared by the column's move - the same pair the chat handles
use, and the reason the clear cannot win the same dispatch.

**The terminal runs a real shell.** The module documented a `portable_pty` child
it never had; it now opens one per command and reads the child's output on a
thread into a shared scrollback, so what the pane shows is the shell's own
output rather than an echo. Two things had to be measured rather than assumed:
the reader's `$SHELL` cannot be silenced (fish reprints its prompt and echoes
the command regardless), and an INTERACTIVE zsh cannot either - zsh's ZLE
re-enables echo and redraws its line editor, which filled the screen with `%`
fragments and a second copy of every command. So a command runs non-interactively
under `zsh -f` (or `bash --norc`), the pane writes its own `cwd $ command` line,
and the trade-off is stated where it belongs: a `cd` does not outlive its
command, because there is no session to outlive. The gate still decides: a
refused command is never written to a pty at all.

Wiring traps worth keeping: the pane polls a line VERSION and copies the
scrollback only when it moved (a quiet terminal costs one integer compare per
120ms tick, not a lock per frame); `Terminal::send` kills the previous child
before starting the next, so one command cannot outlive its successor; and the
probe that opened the panel for a capture must anchor on a line INSIDE
`Shell::new` - `rfind` found a later `shell` in an unrelated `this.update`
closure, the build succeeded, and the capture showed no panel at all.

Verified by two captures of the running app (the terminal showing `$ echo
from-the-pty; ls | head -3` followed by `from-the-pty`, `AGENTS.md`,
`ARCHITECTURE.md`, `CHANGELOG.md` - the shell's output, not an echo) and by
tests that were re-injected and watched to fail: `send` not writing,
`strip_ansi` as the identity, and the seam move setting hover false.
Round 136 pinned the sidebar's account row. The column was brand, new session,
panels, tree, a `flex_1` spacer and then the row - but the TREE had no scroll
and no `min_h`, so a long session list made the column taller than the window:
the spacer collapsed to nothing and the row was pushed below the viewport. The
row was also missing `flex_none`, so under pressure it SHRANK (44 measured as
33).

The tree is now the column's only flexible region (`flex_1`, `min_h(0)`,
`overflow_y_scroll`, with its own `id` because a scroller must be stateful), the
spacer is gone, and the account row is `flex_none` with a debug selector. The
measured evidence is the test's: with forty sessions seeded, the row's bounds
were `y=2129, 33 tall` in a 1080-tall window before the fix and sit at the
bottom, 44 tall, after it - `the_account_row_stays_pinned_under_a_long_session_list`,
which was re-injected and watched to fail with exactly that geometry.
Round 137 completed the session menu. bsh offered two operations (Pin, Archive)
while the reference offers five: Pin session, Rename, Fork session, Archive
session, Copy Session ID. The three missing ones are real now, with the
reference's wording, icons and accelerators.

**Rename** edits the title in place, on the row, the way the reference's ⌥⌘G
does - a shared `InputState` moved from row to row, one field at a time, Enter
commits. `AppState::rename_session` retitles without touching `updated_at`, so a
rename does not reorder the list under the reader's hand.

**Fork session** copies the transcript into a new session (fresh id, its own
file, pin/archive reset) and opens it. It forks the whole transcript, which is
what bsh can honestly do: no per-message branch point is kept yet.

**Copy Session ID** puts the id on the clipboard, which is what a bug report
needs.

**The accelerators are bound, not decoration.** `actions!(bsh_session, ...)`
with `bind_global_actions` binding alt-cmd-g / alt-cmd-f / shift-cmd-a, handled
by `on_action` on the shell frame so they work from wherever focus sits inside
it. The kit's `PopupMenuItem` has no shortcut slot, so each row is an
`element` with [icon] [label] [spacer] [keys]; `simulate_keystrokes("alt-cmd-g")`
in a test proves the hint is true.

Two live-only traps, both found by looking at the running app rather than at the
green suite: (1) the field opened from the menu item and vanished in the same
frame, because the menu's close handed focus back and our `InputEvent::Blur`
handler cancelled the rename - the fix is to commit on Enter and NOT cancel on
blur (a rename is cleared by committing it or by opening another session), and to
focus the field through `window.defer` so the menu's own close lands first;
(2) the row's action overlay is only hit-testable while the row is hovered, so a
scripted click has to hover the row in the same `ocu call --calls` sequence
before pressing the `...`, and the menu's y follows the row it opened on.

Verified: the five rows captured in the running app (icons, labels, ⌥⌘G / ⌥⌘F /
⇧⌘A) and the inline field captured open on the active row; five new tests, four
re-injected and watched to fail (rename a no-op, fork returning the same id, the
clipboard write dropped, the alt-cmd-g binding removed, a menu row removed).
Round 138 added the hero workspace menu's "+ Add workspace..." row, the
reference's own door to a new root. bsh had an add path already - the sidebar's
small plus - but the hero chip's menu listed workspaces and stopped.

`Shell::add_workspace` was split so the dialog half and the registration half are
separable: `adopt_workspace(path)` registers the directory (or selects it when it
is already registered), makes it active, saves, and refiles the files pane - the
last part matters because every path in that pane is relative, so a new root
rebases the open tabs onto another workspace's files. The hero's row and the
sidebar's button now land in the same function, and a test can drive the half
that needs no native dialog.

Measured against the reference: the list ends with a separator and then
"+ Add workspace..." (ellipsis included, because it opens a picker), matching the
row drawn in the reference's own menu; the wording is shared with the sidebar
(`side.addWorkspace`).

Traps: `debug_bounds` takes `&'static str`, so a test that needs a row keyed by a
generated id has to seed a fixed one (the session menu's test does); a scroller
or menu item that is only hit-testable while its row is hovered needs the hover
press in the same `ocu call --calls` sequence as the press that follows it; and
the focus field of an inline rename must not be cancelled on blur, or the menu
that opened it takes the field down with it.

Honest note on my own automation: a stray click landed in the composer and a
keystroke typed into an inline rename field that was still open, retitling one
session "New chata". The title was restored from the session file before the
round ended; the lesson is that a rename field which survives blur keeps taking
keys, so a scripted pass should close it before pressing anywhere else.
Round 139 fixed model switching and added the Effort row the reference carries.

**Switching did not switch.** `AppState::select_model` wrote the settings
DIALOG's draft and nothing else, while `caps::build_agent` - the one function
the turn is built from - reads the committed tree. So the chip's label moved and
the next request still used the old model, and nothing was saved either. The
choice lands in the committed workspace document now (the model is a workspace
fact), keeps any open draft in step, and is saved: the chip, the status bar and
the built agent all read it, and a relaunch keeps it.

**The Effort row exists.** The reference's model menu is two root cells - Model
and Effort - and bsh drew only Model, although `reasoning_effort` already reached
`AgentConfig` from the workspace tree. The row is a submenu of five levels
(Default, Off, Low, Medium, High) with the current one marked, and its label
carries the value (`Effort · High`) because a kit submenu item can only draw its
label. `AppState::set_reasoning_effort` writes the workspace tree and saves;
`caps::build_agent` maps the literal `off` to an ABSENT effort field rather than
sending the string, which is what the reference means by "Off turns reasoning off
and needs no value". `None` is not Off: it hands the decision back to the model's
own declared effort.

Measured from the reference's own package (`dsh-client-ui-settings-models`): effort
is a per-MODEL capability (`reasoningEfforts`, level plus API value) and there is
deliberately no provider-scoped control. bsh's tree declares one effort per model,
so this row is workspace-scoped - the honest limit, reported rather than hidden.

Verified live: the menu draws `Model >` and `Effort · Default >`; picking
stepfun-ai/step-5-preview moved the chip AND the status bar and wrote
`model = "stepfun-ai/step-5-preview"`; picking High drew the `high` suffix on the
trigger and wrote `reasoning_effort = "high"`. Both tests were re-injected and
watched to fail (draft-only selection, the `off` filter removed).
Round 140 replaced the product's artwork and its line. The hero and the sidebar
brand row now draw the mantis-shrimp emblem, and the headline reads
"See Beyond. Stay True." / "洞察未见，忠于所托" where the reference's own
"Into the Unknown" stood.

The source sheet is 2048px RGB with a near-white background, so it cannot be
embedded as-is: 3 MB in the binary, and a white tile behind the emblem on every
dark theme. `assets/bsh-mark.png` is the cut-out - the background flood-filled
from the border with a 22/255 tolerance, alpha feathered 0.8px, trimmed to the
subject, squared and scaled to 128 (30 KB). The asset test pins all of it: PNG
signature, IHDR 128x128, colour type 6 (RGBA, which is what makes the white
paper disappear), and a 64 KB ceiling so the sheet cannot creep back in.

Two measured-chrome traps. The reference's mark is 34 WIDE over a 32-tall
headline row; the emblem is square, so 34 set a 34-tall row and
`the_hero_takes_the_measured_chrome` failed on the line box (42px against the
measured 32). The row keeps its 32 as a BOX and the emblem is drawn at 40 inside
it, centred: a child that overflows a fixed box adds no layout height, so the
artwork takes its optical size without moving anything. And the sidebar's brand
box went 20 -> 24 for the same padding reason.

The old canvas path (`ui/fish.rs`) went with the mark - the hero was its only
caller and clippy's dead_code would have found it anyway - as did the unused
`assets::logo()`; the `.icns`/`.ico`/`.svg` files stay as bundle material,
because the Dock icon is a tile design and swapping it for a bare transparent
emblem would look worse, not better. That remains the one place the old glyph
still lives.

Round 141 gave the workspace row the two controls the reference puts on it: a
circled plus that opens a chat IN that workspace, and an ellipsis whose menu is
Rename (pencil) and Delete workspace (trash, error colour). The bookkeeping
behind them exposed a real bug in the same family as round 138's: the Store
keeps its own `roots` map because it needs one to know where a session file
lives, and that map was built once at open. A workspace added later had no root
to name, so a chat created in it would be SAVED INTO THE QUARANTINE DIRECTORY,
and a deleted workspace's chats kept reading as owned, so the orphan sweep moved
nothing and the rows vanished while their files sat in a directory no row
pointed at. `Store::set_roots` rebuilds the map from the registry, and
`AppState::adopt_workspace`/`sync_store_roots` keep it in step on add and
delete - the registry is the truth and the map is a cache of it.

Delete REHOMES rather than destroys: the registry row goes and its chats move to
the workspace that becomes active, because a list row leaving must not take a
transcript with it. The rename is the display name alone - id and root stay,
which the test asserts field by field - and a blank name is refused with the
field left open rather than silently discarded.
Round 142 gave the session menu the destructive cell it was missing. Archive was
the only way out of the list, and archive KEEPS the transcript - there was no
action that removes a chat, which is what "delete" means to a reader.

`Store::remove(id)` drops the row and its file in one call: removing the row
alone would let the chat reappear on the next launch, because the store loads
whatever is on disk. The file is computed from `dir_for` BEFORE the row is
removed, and a file that cannot be unlinked still drops the row - the reader
asked for it to be gone. `AppState::delete_session` then clears the plan and,
when the deleted chat was the selected one, moves the selection to the next row
in the list (or to none), because deleting the chat the shell is showing must
not leave it pointing at nothing. `Shell::delete_session` also puts away a
rename field on that row first, so it cannot commit into a chat that is gone.

The menu cell follows the workspace menu's design: `PopupMenuItem::element` with
the label and glyph in `theme::error()` - the kit's own item paints its label
from the theme and takes no tint - behind a separator, so archive and delete are
not one muscle-memory apart. Selector `session-menu-delete`.

Verified live: the menu draws 置顶会话 / 重命名 / 分叉会话 / 归档会话 / 复制会话 ID,
then a rule, then 删除会话 in red. The test pins all three halves of the delete
(row gone, FILE gone - searched recursively under the scratch root, since an
unregistered workspace's sessions live in quarantine - and the shell moved to the
sibling), and it was re-injected twice to fail: dropping the unlink fails "and so
is its file", dropping the reselection fails the selection assert.
Round 143 made the workspace tree a real scroller. It already had
`overflow_y_scroll()`, so the WHEEL worked (the kit's overflow scroller takes it),
but nothing could address the offset: no handle, no focus, no keys, so a long list
was reachable only by the wheel and no test could prove any of it.

`Shell` now carries `side_scroll` (a `ScrollHandle` the tree tracks) and
`side_focus`. The tree takes focus on mouse-down and maps Up/Down to one row,
PageUp/PageDown to a page LESS one row of context, and Home/End to the ends
through the pure `tree_key_delta(key, page)`. `scroll_tree_by` clamps between
`-max_offset` and zero - GPUI stores a scrolled-down offset as a NEGATIVE y, and
getting that sign wrong jumps the list to its end on the first arrow press.

The header had to move. `工作区` and its search/sort/add controls were the tree's
FIRST CHILD, so scrolling pushed the section title off the top and left the list
looking like it had no name. They are now a sibling above the scroller
(`workspace_tree_header`), which is what the reference's own sidebar does.

Evidence: the harness dispatches a real `ScrollWheelEvent` (the app-level
`simulate_scroll` is not on a window view, so the test goes through
`window.dispatch_event`) and the offset moves; the key path is pinned through
`scroll_tree_by` (row, page, End clamp); and live, Down x3 and End scroll the real
30-row list while 工作区 stays put. Both halves were re-injected: dropping
`track_scroll` leaves `max_offset` at 0 (the test fails on its premise), and a
no-op `set_offset` fails the Home/End asserts. The ocu `scroll` tool is not a
wheel - it drives an AX element and does nothing here - so the wheel evidence is
the dispatched event, not a screenshot.
Round 144 closed the last thing the scroll exposed: at max offset the tree's
content ended exactly on the account row's divider, so the last row sat FLUSH on
that hairline and read as sliced by the pinned element (`the last one still hide
by pinned element`). Measured live with a temporary probe: tree viewport 332..776,
max 225, deepest content row 1001 - i.e. at offset -225 the last row's bottom was
the tree's bottom exactly. Nothing was overlapping; the LIST simply had no air
under it.

The fix is a floor gap: `theme::TREE_BOTTOM_INSET` (20) is appended as the tree's
last child, so the content is 20px taller than its rows and the final row stops
20 above the divider. 8 was tried first and still read as crowded once the row
hovered; the user asked for 20, which is what ships.

The regression test pins both halves of the geometry: the scroller's bottom never
passes the account row's top, and the last row's bottom keeps at least
`TREE_BOTTOM_INSET` of air above the tree's bottom. It uses a fixed id on the
last row (`debug_bounds` takes `&'static str` only). Re-injected by deleting the
spacer: `row ends at 1036px, floor at 1036px (gap 0)`.

Trap worth keeping: the live `End` can clamp against a max from the frame before
the content settled (a probe showed off=-267 while max read 225), so a screenshot
taken right after the key shows less gap than the steady state - press it again,
or read the geometry, before believing the picture.
Round 145 made deletion undoable: a bin for chats and workspaces.

Before this, `delete_session` unlinked the session file and `delete_workspace`
dropped the registry row - a misclick was final. Both now PARK what they remove
first, as one JSON envelope per item under the settings root:

```text
~/.bos/bsh/trash/session-<unix>-<id>.json
~/.bos/bsh/trash/workspace-<unix>-<id>.json
```

The envelope carries the payload ITSELF (the whole `Session`, or the
`WorkspaceRef`), not a pointer to a file, so restoring cannot depend on a
workspace that moved or a file somebody edited in between. The parking write
comes BEFORE the removal: if it fails the delete does not happen, because losing
the copy is worse than refusing the delete. A chat whose workspace no longer
exists comes back into the workspace in use.

The panel is one overlay from the account row's bin control: sessions and
workspaces as sections, newest first, each row carrying the workspace it came from,
how long ago, Restore, and Delete permanently. An unreadable envelope is skipped
rather than fatal, so one bad file cannot hide the bin.

`Settings::trash_dir` derives from `save_root`, so a scratch configuration gets a
scratch bin - and a TEST build that reaches it without a root PANICS rather than
writing the reader's bin. That guard was added after a stray envelope appeared in
the real bin during a sweep and then vanished (a restore consumed it); with the
guard in place the whole suite passes, so no test can be the source now, and the
one live deletion I performed for evidence was restored.

Evidence: four tests (park+restore a chat; purge it; park+restore a workspace; the
bin follows the settings root), each re-injection-checked - dropping the parking
call fails `and one item is in the bin`. Live: deleting a chat removed the row,
the bin listed it (`新会话 · bos · 6s`), Restore put it back with its id, title and
workspace on disk, and the bin emptied.
Round 146 gave the archived rows the same menu the live ones have.

The archived list offered only the restore arrow, so a chat in the archive could
not be renamed, forked, copied or deleted until it came back. Its row now reveals
the same hover ellipsis, wired to the same `session_menu::session_items`.

One cell had to change. `session_items` takes `archived` and, on an archived row,
swaps the archive cell for `icon Undo` + "Unarchive session" in the SAME slot - a
chat already in the archive cannot be archived again, and a menu that offers it
would be a control that does nothing. The cell carries no accelerator: none is
bound to unarchiving, and a hint is only drawn when pressing those keys does the
same thing.

Both call sites (the sidebar row and the header's More actions) pass the flag from
`Session.archived_at`, through the new `AppState::is_archived`.

Evidence: a click test seeds an archived chat, hovers its row, opens the ellipsis
and asserts the menu carries pin / rename / fork / copy / delete AND unarchive,
and does NOT carry archive. Re-injected by ignoring the flag (`if false`): it
fails on "and the archive cell offers the way back". Live: the archived row shows
↩, the time and the ellipsis; its menu reads Pin session / Rename ⌥⌘G / Fork
session ⌥⌘F / **Unarchive session** / Copy Session ID / (rule) / Delete session.
Round 147 indented the archived rows like the live ones.

The archived list skipped the status-dot column the live rows lead with, so its
titles started 14px to the left of every other session title (measured: 24 against
38). An archived chat is the SAME session put away, not a different rank of thing,
so it now leads with an empty column of the same width -
`theme::TREE_ROW_LEAD` (6) plus the row's own 8px gap, shared by both kinds - and
the two cannot drift apart again.

The regression test renders one live and one archived session (fixed ids, so the
title selectors are literals), reads both titles with `debug_bounds` and asserts
the same left edge. Re-injected by deleting the spacer: 24 against 38, which is
exactly the 6 + 8 the archived row was missing. The titles carry
`session-title-{id}` / `archived-title-{id}` selectors for this.
Round 148 made the centre column scroll with the wheel, smoothly, on both tabs.

Two defects sat behind the report. The first was the real blocker: while
`following_tail` held, `render` re-pinned the transcript to the newest message
EVERY frame, and only a rail jump cleared the flag - so a wheel could not scroll
the chat up at all. The wheel now releases the pin (that is the whole first half,
and the test fails on it when removed).

The second is the feel: the kit's scroller applies a wheel delta in one step, so a
mouse notch cuts instead of scrolling. `ui/smooth.rs` holds the mechanism - a
TARGET offset and a fraction-per-frame walk towards it (EASE 0.28, ~8 frames,
~130ms, SETTLED 0.5) - with the amount of motion and the clamp as pure functions a
test can pin.

The kit's own jump had to be taken back. Its listener runs before this shell's, so
by the time `wheel_chat` sees the event the offset has already moved by the whole
delta; the handler reads how far it jumped, sets the offset back to the settled
one, and aims the easing at `settled + jumped`. Without that take-back the motion
doubled (400px for a 160px notch, measured).

Trajectory gained the same treatment, which also gave it a tracked handle
(`traj_scroll`) it did not have: its wheel worked but nothing could address it.

Traps this round cost: a `#[cfg(test)] mod tests` with `use super::*` in a file
that imports gpui anywhere puts gpui's `test` attribute macro in scope, and rustc
dies with `recursion limit reached while expanding #[test]` (SIGBUS on this
toolchain, which looks like anything but a name clash) - import the items by name.
And `state.tab = Tab::Trajectory` set BEFORE `build_for_test` is lost: `assemble`
settles the tab, so a trajectory test has to switch it after the shell exists.
Round 149 made the centre column's scroll FAST, on the two axes the feedback named.

Time: `smooth::EASE` 0.28 -> **0.6** (eight frames to three, ~50ms) and `SETTLED`
0.5 -> 0.25. 0.28 was chosen for a readable ramp and read as sluggish; the ramp is
now short enough that the wheel answers the hand while still being motion rather
than a cut.

Distance: a mouse wheel sends whole LINES - one coarse detent - where a trackpad
sends precise pixels. At 1:1 a long transcript took dozens of notches to cross, so
`MOUSE_GAIN` (2.0) is applied to the line delta only; a trackpad's pixel delta still
goes through untouched, because its hardware already carries momentum and gaining
it would fight that.

`wheel_delta` is now a thin wrapper over the pure `wheel_pixels(delta, line_height)`
so both halves are pinned: a notch must travel further than its raw pixels, and a
precise delta must come through at exactly its own value. Re-injected by dropping
the gain - it fails with `and further than its raw pixels: -60`.
Round 150 rebuilt the Built-in plugins pane in the reference's own shape.

Measured from `dsh-client-ui-plugins`: a tab strip (Plugin list / MCP / Plugin
Market), a full-width search field with a leading magnifier, then two collapsible
groups - SESSION plugins (what the active agent preset composes, with the preset
as the header's trailing control) and GLOBAL plugins (the installed inventory,
"Shared by the system and every session") - drawn as cards TWO PER ROW, each with
a switch, a name, a description and a chevron. The pane header already carried the
reference's "Open configuration file" action (`settings-open-config`).

Wording comes from the reference's own dictionary: "Session plugins", "Composed
per session by agent presets", "Global plugins", "Shared by the system and every
session", "Search plugins", and the three tab names.

Honest mapping: bsh has no plugin DESCRIPTIONS and no market. A card's description
is the tools the plugin contributes (`bash`, `list_dir, read_file, write_file`),
which is the truth this deployment has rather than invented prose; and the
"Plugin Market" tab is drawn DISABLED, because shipping no market means a tab that
would only look pressable. The "MCP" tab is real and moves the dialog to that
pane (verified live). The session group reads the preset from `GlobalSettings::
preset`, so a user-declared preset replaces the shipped `standard` list exactly as
the agent build does.

`plugin_enabled` reads the DRAFT, so a plugin switched off anywhere reads off in
the card's switch with no second state - the test pins that by toggling "shell"
and watching the switch follow.

Traps: a settings helper that takes `cx` cannot build a menu with `cx.listener`
inside the `'static` menu builder (borrowed data escapes) - clone the entity and
`update` it in the handler; and `Div` vs `Stateful<Div>` is decided by `.id()`, so
a header helper that sets one returns the latter.
Round 152 removed the Built-in plugins pane's view list, and with it the sidebar
rows that had already gone: the pane is now exactly the reference's Plugin list -
tabs, search, Session plugins, Global plugins.

`VIEWS`, `builtin_view_row` and the `settings.pluginViews` key went with the block,
so nothing but the five rail cells names a pane in the settings chrome. Consequence
worth knowing: nine panes in `SECTIONS` (shell, agent-loop, tools, mcp,
archived-sessions, global.workspaces, context, automation, plugin-inventory) now have
no in-app entry - only the command line's `--settings=SECTION` deep link. MCP is the
one exception, because the Plugin list's own tab reaches it (`plugins-tab-mcp`,
click-tested).

The screenshot that prompted the round also showed `settings.global.workspaces`
rendering as a raw key. That was REAL: the key was missing from both locale files,
so the Workspaces pane title and its row printed the key. Added as
"Workspaces" / 工作区 beside the other settings section titles.

Traps: cutting a span that ends at `}` after a searched marker swallowed the old
test's tail and left a stray `"` / `);` / `}` in the file - fix by deleting the
residual lines by INDEX after asserting each one's content, not by another search;
and two tests named the removed list (`the_builtin_plugins_pane_hosts_the_reference_views`
pressed `builtin-view-shell`), so the pane's shape tests and the key list had to move
with the code.
Round 154 answered "左右两边的空白太多了" twice: `CHAT_SHARE` .64 -> .8 -> **.9** and
the fixed 920 cap became a **32px gutter per side**, so the transcript takes the
column rather than a measured band the reference never questioned.

Measured at the harness window (1280 wide, 280 sidebar, 1000 column): the band was
160 each side at the reference clamp, 100 at .8, and is 50 at .9 - half of the
previous round, which is what the feedback asked for. At 1640 the band is 82.

The two width rules had to agree. A .9 share with the reference's 176 drag budget
meant the DEFAULT (1476 at 1640) exceeded the widest a drag could store (1464), so
the first pointer move pulled the column IN and the drag test caught it as
`left: 1464, right: 1556`. `resolve_content_width` now bounds a stored preference by
the same gutter the default keeps, `column - 2 * CHAT_GUTTER`, with the reference's
640 floor still underneath; `CONTENT_EDGE_BUDGET` went with the old rule and clippy
`dead_code` found it.

The handle strip survives: it is `min(10, band / 2 - 48)`, which is 10 while the
band is 164 and 2 at 100 - a drag to the gutter eventually leaves no strip, which is
the reference's own behaviour at its bound.
Round 155 answered "左边的再减少为现在的40%" with an ASYMMETRIC column: the right
band is unchanged (the turn rail and the composer are laid out against it) and the
left one is `CHAT_LEFT_SHARE` 0.4 of it, rounded to a whole pixel.

Measured at the harness window: the transcript's left inset went 100 -> 40, so the
text starts 60px further left while the composer keeps the width and centre it had -
which is what "不要影响下面的输入框" asked for. The cost, stated rather than hidden:
the composer is now the widest thing in the column, hanging 46px left of the message
text, because chat + 32 centred no longer matches the transcript's left edge.

Two things had to move with it. The LEFT handle derives from the inset now
(`left: inset - 24 - strip`) instead of the symmetric `right: 50% + content / 2 + 24`,
and the strip's clearance is asserted within one pixel because the painted padding
snaps to a device pixel after the handle's own exact origin. And `chat_left_inset`
rounds: a fractional inset put the 24px clearance 1.2px off, which the handle test
caught as `measured 22.799988`.
Round 156 moved the token readout off centre: the context meter and the stat
pills now take `meter_shift(chat) = chat / 2 * METER_SHIFT_SHARE` to the left, so
the ring lands at roughly 30% of the composer card - the point the reader drew an
arrow to - instead of the card's middle.

Both boxes are laid out centred as before and then offset with
`.relative().left(px(-shift))`, because a `relative` offset paints without moving
the flex box: the composer keeps its measured width and the pills keep their own
row, and the offset simply carries both. The status row uses the chat width and the
meter the composer width, so the two offsets are the same number and the pair reads
as one block.

0.7 of the half-width was tried first, exactly as asked, and it put the readout's
LEFT EDGE 20px from the column's edge - clipping risk on a narrower window and past
the point the arrow named. 0.4 is the share that lands the centre on that point; the
share is one constant and the report says which number is which.
Round 157 gave the chat the trace shape: each tool call is a STEP row - a
chevron, the KIND the step is (Bash, Read, Write, List, Plan, Skill, Step), a `·`
and a one-line summary - nested one rail in from the message it belongs to, with
its arguments and output in labelled IN / OUT blocks when opened.

The kind is a locale key per tool family (`tool.kind.*`), not the registry name,
so two providers' differently-named file writers read as one kind. The summary is
`tool_summary(args)`: the first well-known field's value (command, path,
file_path, query, code, instruction, prompt), else the first quoted VALUE (a span
followed by `:` is a key and is skipped), else the raw arguments - whitespace
collapsed and clipped at 96 characters, because the body holds the whole text.

The fold direction was inverted and this round fixed it: `liveProcessDetail`
governed SETTLED turns, so every completed turn opened every body - a wall of
arguments where the reference shows one summary line per step. It now governs the
RUNNING turn, and a settled turn folds its process (`folded` true -> closed), which
is what makes a long trace scannable. The screenshot that prompted the round is
exactly that state.

Traps: `debug_bounds` takes `&'static str`, so a dynamically built selector cannot
be passed - use a literal and seed a known message count ([user, assistant] means
the tool is on message 1); and `state_with_turns(label, 1)` is the cheapest way to
get that fixed shape.
Round 158 closed the four gaps the step rows had. Each is real code, not a
stretch of the same row:

1. CODE HIGHLIGHTING. `code_lang(name, args)` names the language a body is -
   bash for a command step, and for a file step the extension of the path it
   wrote or patched - and the body renders through the kit's markdown fence, so
   it is highlighted and carries the fence's own language chip. A step with no
   code claims none and stays plain text.
2. COPY. Every IN/OUT block has a copy control on its header row,
   `cx.write_to_clipboard(ClipboardItem::new_string(..))`.
3. SIZE BADGE. `format_bytes(tool_bytes(tool))` - the payload the step moved
   (its output once finished, its arguments while it runs), 812 B / 7.2 KB, the
   reference's own shape. It is a size OF THE EVENT, not of a file on disk: bsh
   keeps no handle, and a disk read would be a second source of truth.
4. PATCH. A write or an edit renders as `-` / `+` lines with the success/error
   wash (`theme::diff_add` / `diff_del`), from the arguments the model sent -
   the SHAPE of the change, not a disk diff, because no before-image is stored.

Two follow-ups fell out of looking at it: the OUT body unwraps the shell
tool's `{stdout, stderr}` envelope (`output_body`), so a reader sees the text
rather than `\n` escapes; and a command step's IN body shows the command itself
rather than the JSON around it.

Traps: `json_field` flattens escapes for summaries, which is wrong for a patch -
`json_field_exact` keeps `\n` as a newline, and the two cannot share a reader;
and an empty object is not a summary, so `meaningful` suppresses the `{}` that
was sitting beside every row whose arguments carried nothing (`{}` and `[]`
both).
Round 159 put the transcript on disk as a paged Parquet log (`history.rs`) and
made the session document metadata.

SHAPE: one append-only segment per save - msg-000.parquet, traj-000.parquet -
written in row groups of PAGE (128) rows. A save costs O(new rows), not
O(history); a page read decodes ONE row group (`with_row_groups`), not the file;
decoded pages sit in an LRU capped at CACHE_PAGES (8), so a scroll holds a few
pages of text rather than the transcript.

WIRING: `Session` keeps a WINDOW (2 pages = 256) of its transcript plus
`loaded_from`, the global index of messages[0]. `Store::save` writes the rows the
log is missing and clears the document's messages; `Store::read` hydrates the
tail window from the log; `Store::load_older` prepends the window before it, and
`Shell::page_older_at_the_top` calls it when the transcript's offset comes within
PAGE_TRIGGER (400px) of the top, anchoring the reader on the row they were
looking at so the view does not jump.

SECURITY, caught by an existing test: the document used to be the only door the
secret scrubber guarded, and moving the transcript to Parquet would have written
a quoted credential into the log unredacted. `save_history` now maps every
message through `redact_message` (text, reasoning, error, tool args and outputs)
and the redaction test asserts against the LOG rather than the JSON.

Compatibility: a JSON document that still embeds its transcript keeps working -
on its next save the log is rebuilt from it (`History::rewrite`) and the document
drops the messages.

Evidence: `a_session_pages_its_transcript_from_the_parquet_log` (600 rows -> 256
in memory, loaded_from 344, one load_older -> 512/88, page to the start, then
zero), plus the module's own six: round trip, a page read decoding exactly PAGE
rows, the cache ceiling, append costing one segment, a window crossing a page
boundary, and the trajectory written beside the messages.
Round 160 closed the gaps the paged backend opened - three of them were DATA LOSS
paths, which is what the wiring round could not have known until the log became
the only transcript.

1. TRUNCATION (retry / edit / a cut). `truncate_from` mutates the in-memory
   window, so `save_history` saw a log that did not line up and rewrote it FROM
   THE WINDOW - silently deleting every page above it. The rewrite now reads the
   prefix back (`History::prefix`) and rewrites prefix + window.
2. FORK. A fork cloned the session, which holds only a window, so the copy was a
   shortened chat. `Store::copy_history` copies the source's whole segment set
   into the fork's directory before the fork's first save.
3. RESTORE FROM THE BIN. The log never moves while a chat sits in the bin (the
   two-digit id keys both), so a restore has to re-attach it: `Store::rehydrate`
   loads the tail window after the document comes back. `purge_trashed` now
   removes the log too, so a purged chat leaves nothing behind.
   A save also has a safety net: an EMPTY in-memory transcript over a NON-empty
   log is not a truncation and never rewrites - that is the shape a list-only
   read or a restore has, and treating it as a cut is how a transcript vanishes.

4. TRAJECTORY PAGING. The trace tab now reads tool rows from `traj-*.parquet`
   (`Store::trajectory_page`), one page at a time: the first page loads when the
   tab is opened, another when the trace is scrolled to its top, and the rows are
   per session (a switch starts the paging over). A session with no log yet still
   draws the rows it holds in memory, which is how the JSON-era chats render.

Evidence: `a_truncation_keeps_the_pages_above_the_window` (400 rows -> a window of
256 -> cut in half -> the log keeps 360 pages and still starts at row 0) and
`a_fork_copies_the_log` (300 rows -> the copy reads every page).
Round 161 made the turn rail's last mark clickable, and fixed a paging design
error the earlier rounds hid.

THE RAIL. Measured before changing anything: with sixty turns the last tick laid
out at y=913-923 while the transcript area ended at 977 and the frame was 420
tall - i.e. the marks were painted BELOW the frame that hit-tests them, so the
bottom of the column was a dead strip with a sliver of a mark in it. `.max_h`
never capped a flex child, so the track grew past the frame. The track now takes
an explicit `.h(frame_h)` with `flex_none()`, where frame_h is the
min(marks + inset, TURN_RAIL_MAX_H) the preview maths already assumed, and the
rail is tail-biased: while the transcript follows the tail, an overflowing rail
scrolls to its bottom so the NEWEST mark - the one a reader presses to come back
- is the one in view.

THE SIGN TRAP: `ScrollHandle::max_offset()` is a MAGNITUDE (positive) while
`offset()` runs from zero DOWN to -max. The first guard compared them with one
sign and never scrolled; the test caught it as `max 192` where the assertion
expected `max < 0`.

THE PAGING FIX. `loaded_from` was answering two different questions - where the
window in memory starts, and how far back the log has been read. Trimming moved
it forward while paging moved it back, so a caller doing both oscillated forever
(a test binary hung 26 minutes and had to be killed), and once trimming pushed it
to zero paging stopped although the log still held every page above. `Session`
now carries `paged_from` - the paging floor, monotonic toward zero - and
`load_older` reads from THAT, while `trim_front` moves only `loaded_from`.

THE BIN. A session's Parquet pages now move with its envelope (`Envelope.log`,
`trash::park_log`), so a parked chat is whole: restore moves the pages back
beside the document and re-hydrates the window; purge removes them with the
envelope. The log is addressed through `Store::dir_for_id`, the session's own
directory, not a bare root.

Evidence: `the_last_turn_tick_is_reachable` (sixty turns overflow, the rail rests
at its bottom, and the last tick's laid-out position shifted by the scroll falls
inside the frame), `trimming_bounds_the_window` (an exhaustive page-up holds four
windows, the floor reaches Some(0), the log keeps every row), and
`the_bin_carries_the_transcript_pages` (pages leave the workspace, the bin reads
all 400, a purge leaves nothing).
Round 162 finished the rail's last mark. Round 161 made it visible; this one
makes pressing it DO something.

The symptom survived because the newest mark had no action: the click handler
only ever RELEASED the tail pin (`following_tail = false` then jump). Pressed at
the live end - which is where a reader is when they reach for the last mark - the
jump lands where they already are, so the control looked dead. The newest mark now
re-engages the tail: `following_tail = true` plus `scroll_to_bottom`, i.e. it is
the way back to the live end from anywhere, and the way to start following again
while already there. Every other mark still leaves the pin off.

The handler became `Shell::jump_to_turn(child, newest)`, called by the mark's
click, because a 28px overlay does not take a synthetic click reliably - the
crate's own rule for a hard-to-hit control is to drive its method and assert it is
on screen, and the test does both.

Diagnosed, not guessed: a test proved clicks DO reach the rail (pressing the first
mark moves the transcript), so the mark was never dead - it was invisible in
round 161 and inert in this one.

Evidence: `the_newest_mark_reengages_the_tail` (leave the tail, press the newest
mark's own method, assert the pin is back and the offset is at -max, with the mark's
box asserted inside the frame), `the_rail_marks_receive_the_click` (a press on the
first mark moves the transcript), `the_last_turn_tick_is_reachable` (sixty turns,
the rail rests at its bottom, the last mark is inside the frame).
Round 163: the rail's ACTIVE mark, and an honest report on the hover preview.

FIXED AND PROVEN. `visible_turn` picked the last turn whose top sat at or above
the viewport's, which at the tail is always the turn ABOVE the newest one - the
newest turn begins below the top because nothing follows it. The newest mark was
therefore permanently dim while the chat had already ended, which is the
screenshot the user sent (highlight one above the end). It now returns the newest
turn whenever the reader is at the live end (tail pin on, or offset at -max), and
the old rule everywhere else.

Also proven: hovering the NEWEST mark opens the preview card exactly like any
other mark, and the card's top clamp keeps it inside the frame -
`the_rail_preview_opens_on_hover` now hovers turn 2 of 3 and asserts the card's
width and its containment.

NOT FIXED, AND LEFT AS A REPRODUCTION. With an OVERFLOWING rail (more marks than
the 420px frame) the card does not draw at all - not for the newest mark and not
for a middle one - while `rail_hover` is set, so the state is right and the card
is what is missing from the drawn frame. That is the user's case (a long chat).
`an_overflowing_rail_draws_the_hover_preview` is `#[ignore]`d and documents it, so
the next round starts from the reproduction rather than a guess. Candidate causes
to check first: the tail-bias scroll runs every frame while following the tail and
may be moving the mark out from under the pointer between the hover and the card's
build; and the card is a child of a 28px-wide frame inside an absolute rail, which
is worth measuring rather than assuming.
Round 164 found and closed the rail's real bug: an ORDINAL was compared with a
MESSAGE INDEX. That single mismatch caused three symptoms the last four rounds
chased separately.

The mark loop is `turns.iter().enumerate().map(|(turn, index)| ...)`: `turn` is the
mark's ORDINAL (0..n-1, the selector id and the hover payload), `index` is the
turn's MESSAGE index (0, 2, 4, ... 118, the scroll child). `rail_hover` stores the
ordinal, and three readers treated it as a message index:

1. The preview searched `turns.iter().position(|item| *item == turn)`, which only
   matches when ordinal == message index - i.e. mark 0. Every odd ordinal, and
   therefore the NEWEST mark (n-1 is odd for even n), drew no card at all. That is
   'hover the last one, nothing; the others have a card'.
2. `is_last = turns.last() == Some(turn)` compared a message index with an ordinal,
   so the newest mark never took the tail-re-engaging branch - clicking it ran the
   ordinary jump, which at the tail is a no-op.
3. The card's centre used the ordinal anyway, so the cards that did appear were
   correctly placed, which hid the mismatch.

Everything is ordinal now: `preview` reads `turns.get(ordinal)` for the message
index the texts need, the centre uses the ordinal, and `is_last` is
`turn == turns.len() - 1`.

Two probes made this cheap and are the reusable lesson: printing
`turns.first()/last()/len()` beside `rail_hover` showed `last=Some(118) n=60`
against `hover=Some(59)`, which named the bug in one run; and the earlier
'overflow' reproduction was itself wrong - 30 turns is 312px, which FITS the 420px
frame, so it tested nothing. Overflow needs 42+ turns.

Evidence: `an_overflowing_rail_draws_the_hover_preview` (60 turns, hover a middle
mark 50 AND the newest 59, both must draw the card), `the_rail_preview_opens_on_hover`
(3 turns, the newest mark previews at 300px and stays inside the frame),
`the_newest_mark_reengages_the_tail`, `the_rail_highlights_the_newest_mark_at_the_tail`.
Round 165 built the Trajectory tab the reference actually draws. The old pane
rendered the goal card, the plan list and the chat's tool rows - the transcript
again, one tab over - while the reference has its own view: a measure toolbar, a
three-lane strip, a typed event log, and a detail panel.

New module crates/bsh/src/ui/trajectory.rs. `events(shell)` flattens the session
into typed lines (USER / ASSISTANT / TOOL, each with title, outcome, raw payload,
source and ms), `matches` is the search filter, and the view draws the measured
chrome: a segmented [Duration | Turns | Calls] over a search field, three lanes
(Input / Model / Tools), rows with a kind dot, a chip, a one-line title and the
outcome line, and a 470px detail panel with Summary / Preview / Raw / Source tabs
over Source / Status / Duration fields.

THE MEASURE IS LIVE. The segmented control re-weights the lanes rather than only
highlighting itself: duration sums each turn's tool ms, calls counts its calls,
turns gives equal shares. `the_trajectory_measure_redraws_the_lanes` proves both
directions on a session whose turn 1 has one 9s call and turn 2 has three 10ms
calls - by duration turn 1 is widest, by call count turn 2 is.

TRAPS. `txt` WRAPS, so a 28px row became a tower of text until the title and the
outcome carried `.text_ellipsis().whitespace_nowrap()` (the seam queue_dock and
composer already use); a shell heredoc containing `\u{2192}` and backticks in doc
comments silently did not apply - tools.edit on an exact anchor is the reliable
path, and grep must confirm the edit landed; and `overflow_y_scroll` needs an
`.id(..)`, so the detail body carries one.

Wired into the existing machinery rather than beside it: the log rides
`Shell::traj_scroll` with the same eased `traj_wheel`, so
`the_trajectory_scrolls_with_the_wheel` still passes, and the search field is a
real InputState whose Change event sets `traj_query`.

HONEST GAPS, stated in the module doc: the SUBTOOL and CONTEXT chips the reference
draws have no producer here (nested calls and injected time-context messages are
their harness's), the lanes are step densities because no event carries a start
time, and the reference's per-row indent tree is absent because this model has no
nesting.

Evidence: `the_trajectory_draws_its_log_and_opens_the_detail_panel` (the tab
renders log rows, no panel until a line is pressed, pressing opens it, the Raw tab
selects, close drops it) and the measure test above. 367 passing, fmt/clippy
clean.
Round 166 closed two of the trajectory's three stated deviations, and left one
because the data does not exist.

TIME AXIS. The timeline could only be a step density because no event had a
start time - only a duration. `ToolEvent` now carries `at` (epoch ms, serde
default so old records parse) and `AgentEvent::ToolCall` stamps it from
`state::now_ms()`; a message takes the instant of the first call it made, carried
across turns. With timestamps the lanes are a real gantt: a shared domain from the
earliest start to the latest end, a transparent spacer for each wait and a bar
sized by the call, both in ms. A log recorded before the field existed has no
positions, so the lanes fall back to one segment per turn rather than inventing
an axis - which is what the current sessions show.

TURN RAILS. Measured from the table sheet
(`tr[data-turn-start=true] td:before`: 2px, border-l1, translateY(-50%)) the
first line of each turn now carries a 2px top rule, so the log reads in turns
the way the reference's does. The flag is computed before the rows are built - a
closure capturing a running `previous` does not outlive `.children`.

STILL OPEN, with the reason: SUBTOOL and CONTEXT chips have no producer (nested
calls and injected time-context messages are the reference harness's), the
earlierHistory affordance at the track's left (28px gradient) is not drawn, and
the kind slot does not yet take the reference's 44px zh width because bsh sizes
the tag to its own text.

Trap: adding a field to ToolEvent means every fixture literal in shell_tests.rs,
history.rs and model.rs needs it; a regex over `ToolEvent {(...)}` misses bodies
holding braces inside strings, so a brace-matching scan that skips string
literals is the reliable edit - and it must not double-insert on literals that
already carry the field.
Round 167 supported SUBTOOL, and CONTEXT and ERROR with it - the trajectory now
draws every kind the reference draws that this model can produce.

SUBTOOL has NO stored parent, and it does not need one: a call is nested when its
start falls INSIDE another call's interval. `at` (added in round 166) and `ms` are
the two facts every record already carries, so `events()` derives the depth while
it reads and old logs need no migration. A subtool row sits one 18px rail in from
its parent, drawn as a MARGIN rather than padding - padding moves the content
inside the row's box, so the row still starts on the same edge and a bounds test
cannot see the indent.

CONTEXT got its producer too: the session's `goal` is context the agent is handed,
and it was being dropped from the log entirely. ERROR likewise - `ChatMessage.error`
was invisible in Trajectory, so a failed turn now has its own row in state-error.
Both use the same measured 19px / 10px / 650 tag in the 76px (44px zh) slot.

Evidence: `a_call_inside_another_becomes_a_subtool` seeds an outer call running
1_000..6_000 with an inner call starting at 2_000 and a later call at 7_000, then
asserts the kinds are [Tool, Subtool, Tool] AND that the subtool row's origin x is
more than 10px right of its parent's. Trap in that test: the tool rows are not the
first three events - the turn's messages come first - so the indices have to be read
from the log rather than assumed.
Round 168 put the trajectory on the reference's own type: families as well as
sizes.

FAMILY. A tool or subtool row is written in CODE, not in the UI face: measured
`tr[data-kind=tool] .contentText` (and `[data-kind=subtool] .resultPreview`) take
`--ds-font-family-code` at 12px, and `toolCallNameTypeface` names the stack -
Menlo, Consolas, Liberation Mono, PingFang SC, Microsoft YaHei. `theme::CODE_FONT`
records Menlo and the row's text and outcome both use it; every other kind stays in
the UI face, which is the distinction the reference draws.

SIZES, as measured: table base `--dsw-font-xxs-12` (rows 12, already), detail tabs
and the panel header `--dsw-font-xs-13` (were 12), kind tag 10 (unchanged). The
token definitions live outside the extracted corpus, so `xxs-12` = 12 and `xs-13` =
13 come from the class names alone - worth confirming against a live capture if the
reference ever changes them.

Not drawn: the reference's column header row (th, 12 at weight 500) because this
list has no columns, and 8px/10px mono tabular turn labels on the rail, which the
kind-slot design does not have a place for.

Round 169 made the workspace egress allowlist DISCOVERABLE through one
constant, `caps::EGRESS_ALLOWLIST_TEMPLATE`, the single source of truth the
Policy pane's Create action writes and its test compares against, so no copy can
drift. EVERY line of the template is a `#` comment, so the file it writes parses
to ZERO rules (`EgressRuleSet::parse`).

Round 170 reversed the SEEDING half: the file is TRI-STATE and only its own
presence constrains. No file at all -> `PolicyData::egress_unrestricted` ->
`Egress::Unrestricted` (the network works out of the box, which is what
Standard/PTC inherit); a PRESENT file that parses to no rule -> `Egress::DenyAll`
(the reader opted into a lockdown by creating it); rules -> the allowlist. The
automatic seeder is GONE (`seed_egress_allowlist`,
`AppState::seed_workspace_allowlist`), because writing the starter on
registration/opening turned every fresh workspace into a deny-all before its
owner asked for one. The pane's Create action is the only writer, and its copy
says that an empty file denies all egress. The reader must distinguish a
`NotFound` from any other read error and treat the latter as PRESENT (deny),
never as absent: an unreadable file must not widen the workspace.

ALLOW-ALL is honest here, and the honest answer is an ADDRESS rule: a bare `*`
classifies as a HOSTNAME, and `host_matches` only accepts an exact name or a
leading `*.` suffix, so `*` matches NO host - there is no wildcard-hostname
spelling. The nearest real equivalent the parser accepts is `0.0.0.0/0` / `::/0`,
which `net.cidr_contains` matches against any resolved address; the template
documents that pair as a commented example, and a test pins that `0.0.0.0/0`
classifies as an address while `*` classifies as a hostname.

The pane gained the affordances with handlers: the empty state distinguishes "no
file yet" from "a file that declares nothing" and offers Create starter allowlist
(writes the template) and Open file (reveals the path through the same
`settings::reveal` seam the settings file uses). Eight new locale keys, en+zh,
with parity enforced by the existing `the_two_locale_files_carry_the_same_keys`.

Round 171 made the permission chip's three consequence lines TRUE about the
network, DERIVED rather than written. The workspace `.bsh/egress-allowlist` is
tri-state, so every row appends a sentence built from `permission::NetworkFacts`
(the file's state + the preset's resolved `egress`): Read Only names web search
and the browser, Workspace Write names commands, Full access names tools, and a
preset that keeps the shell confined swaps in the sentence that names that
constraint. `WorkspaceEgress` is classified by the policy's own
`EgressRuleSet::parse` from facts AppState already cached - `egress_allowlist`
plus the new `egress_allowlist_present`, both set by the one
`refresh_egress_allowlist` read - so a render never stats the disk. Traps: the
resolved `sandbox.heel` answers for the mode IN FORCE, so the Full access row
claimed "this preset keeps the shell confined" on an ordinary Workspace Write
session; the row now asks the preset's own declaration
(`preset_forces_heel`). The kit's popup caps at 500px and each item is a fixed
26px high, so a longer row wraps and clips its own network clause; the permission
menu now sets `.max_w(px(820.0))`. Tests: all three modes x both locales x the
three file states, the preset-list and deny-all intersections, a negative that a
deny-all preset never reads as open, and two rendered `draw_until` tests reading
the rows the chip builds (absent file and deny-all preset).
Round 172 put two AUTHORING TEMPLATES into every workspace, at the only names
bsh ignores: `.bsh/policy.rego.tpl` and `.bsh/egress-allowlist.tpl`
(`workspace_templates.rs`). The enforcing files are still never seeded - a
present `egress-allowlist` with no rule would deny every destination, and a
policy is the reader's decision - so registration writes the templates and
nothing else; the templates are write-if-missing, so an edit survives every
launch, and a root that is not a directory is skipped rather than recreated.
They are ensured on Add workspace (state `adopt_workspace` and the shell's
already-registered path), on a workspace switch, and at launch for the active
workspace. The policy template is a complete `policy.workspace` module whose
verdict allows by default and whose four worked refusals (path prefix/segment,
egress port, egress host, tool) sit behind `examples := false`; tests compile
it, flip the switch and prove each refusal fires, and copy it to
`.bsh/policy.rego` through `policy_for_effective` to prove the loader ignores
the `.tpl` and applies the copy. The allowlist template is the Policy pane's
own `EGRESS_ALLOWLIST_TEMPLATE` under a copy-instruction header, so its rule
shapes cannot drift from the file the pane writes. Trap: the Rego comment
`a "# rule: <id>" comment` terminates an `r#"..."#` literal, so the constant
uses `r##"`; and a target path is normalised, so a test that injects a scratch
prefix into the template must canonicalise the root first.

Round 173 added the third authoring template, `.bsh/terminal-policy.rego.tpl`
(`TERMINAL_TEMPLATE_NAME`), for the interactive terminal gate. That gate was
real and used - `terminal.rs` reads `.bsh/terminal-policy.rego` for start, input
and output with redaction, hot reload, a last-good policy on a broken edit, an
audit ring and fail-closed on a parse error - but nothing pointed a reader at
it: the pane seeds a bare starter when it opens, and the contract lived only in
`docs/bsh-terminal-policy.md`. The template is discoverability, not enforcement.
Its body is NOT a second copy: it is `terminal::WORKSPACE_POLICY_TEMPLATE`, the
same text the pane seeds, so the file a reader copies and the file the pane
drops cannot drift (a test asserts `ends_with` on that constant). The body was
recast in the policy template's style - `package terminal` (the loader adds the
shipped module as `terminal.rego` and evaluates `data.terminal.verdict`), the
three input shapes, the boolean / `{allow, reason, redact}` answers, and three
worked examples (destructive command, output-redaction regex, refused shell)
guarded by `examples := false` over EMPTY lists, so the switch alone is inert.
The header carries what the body cannot say about itself: the `.tpl` is not the
enforced name, absence is OPEN while a file that fails to parse closes all three
gates and keeps the last good policy with the error recorded, and this gate is
the INTERACTIVE pane's while `.bsh/policy.rego` covers tool calls. Proven
through the loader rather than by reading: copying the template to the enforced
name leaves the start and input decisions equal to a missing file's, and
`gate_for_workspace` - a `#[cfg(test)]` wrapper on the `open_gate` the pane
itself calls - distinguishes the shipped `Default` gate from a `File` gate, so
"the .tpl is ignored" is asserted on the loader, not on file names. Trap:
`seed_policy` already writes the enforced name when a pane opens, so a test that
only checks that file's absence proves nothing about the loader.


Round 174 took the file tools from "works" to "fast and robust", measured rather
than asserted. The four tools now share one path resolver, one write path and one
read path, and each seam closed a failure the old code could reach:

- **Confinement is the deepest existing ancestor, not the immediate parent.**
  `resolve_path` normalises `.`/`..` first and then canonicalises the real path of
  the file (when it exists) or of the deepest ancestor that does (when it does
  not). The old check canonicalised only `joined.parent()`, so writing
  `bridge/deeper/new.txt` through a symlink whose `deeper` did not exist yet kept
  the lexical path, `create_dir_all` followed the link, and the write landed
  outside the workspace. An existing leaf now resolves to its real path, so a
  symlink inside the root reads/edits the file it points at and the policy sees
  that same file.
- **A read is bounded and can be windowed.** `offset`/`limit` are 1-based lines;
  a file past 8MiB is refused with the arguments that read a window instead of
  being loaded (a tool result travels through the request, so an unbounded read
  is a broken turn, not a slow one). The open is refused for anything that is not
  a regular file BEFORE opening it, because opening a FIFO hangs the turn.
  Binary content (a NUL in the head, or anywhere, via `find('\0')`) and invalid
  UTF-8 are named instead of returning mojibake.
- **A write is atomic and skips a no-op.** A temp file beside the target plus a
  rename leaves the old file intact on failure; the existing file's mode is
  copied onto the temp; a byte-identical write returns `unchanged: true` without
  moving the mtime, so a build watcher is not woken. Write and edit serialize on
  a per-path mutex, so two parallel calls cannot both read the original bytes and
  lose one update.
- **An edit searches once and explains a miss.** `replace_all` builds the result
  in one pass with memchr's finder; a miss names the line the first line matched
  and, for a CRLF file, says so. `old_string == new_string` is refused instead of
  silently rewriting the file.
- **A listing uses d_type and is ordered.** `entry.file_type()` answers dir/file
  with no stat (only a symlink costs one, to say whether it behaves like a dir),
  names sort bytewise, and 2000 rows are returned with `truncated` for more.

Measured on the debug build (M-series Mac, 5k entries, 4MiB file), through the
registry so the policy is included:

| call | before | after |
|---|---|---|
| read 4MiB | 0.66ms | 0.76-0.84ms |
| list_dir 5k entries | 45.6ms | 10-13ms |
| write_file 1MiB (real change) | 0.72ms | 0.85-0.88ms |
| edit_file 4MiB replace_all (65k matches) | 85.4ms | 52-59ms |

The read/write regressions are the safety: one extra bounded read for the
identical check, and the UTF-8/NUL diagnosis. A no-op write is much cheaper than
before (no temp, no rename). `bench_file_tools` is the checked-in manual
benchmark: `cargo test -p bsh --lib bench_file_tools -- --ignored --nocapture`.

Traps worth keeping: `bytes.iter().position(|b| *b == 0)` cost 10ms on 4MiB
(`content.find('\0')` is 0.18ms, and `str::lines()` is 5ms where
`matches('\n')` is 2ms); a lowercased comparison per sort cost 15ms more than a
plain byte sort on 5k names; `DirEntry::file_type` is free while
`path().is_dir()` stats every entry. Tests: symlink-bridge escape, missing-tail
`..` escape, link-out read + link-in resolve, ceiling/window/binary/directory
reads, identical-write mtime, mode preservation, edit miss hint, identical-edit
refusal, path-lock serialization, and the ordered listing.

Round 175 added the `glob` tool, the one file tool BSH was missing. It is a
discovery tool, not a content search: it answers "which files match this path
pattern" so an agent stops walking directories with `list_dir` and starts from
a list. The contract is the reference's - a pattern with no separator matches
the FILE NAME at any depth, a separator anchors it to the search base, `*`
stops at a separator and `**` crosses directories - and the walk is
ignore-aware: `.gitignore`/`.ignore`/`.rgignore` are honoured, hidden entries
are skipped, and the build/dependency trees are not descended. One switch,
`include_ignored`, lifts hidden + ignore + default-exclusion filtering for a
call; an explicit `path` is exempt from the default exclusions, so
`glob("*.rs", path="target")` still works when the reader means it. Results are
workspace-relative, sorted, capped at 2000 (limit max 1000), and `truncated`
says when more existed. `.git`/`.jj`/`.bsh` are never returned, symlinks are not
followed, the pattern is capped at 4 KiB, and the search root is policy-gated
as a Read before the walk starts.

Which backend, and why not a single dependency called "ripgrep" or
"zvec-grep": the `ripgrep` crate on crates.io is a binary-only package
(`autolib = false`, its only target is `rg`, no `[lib]` in the manifest), so
there is no library to call. Its reusable half - the crate that implements
`rg --files` and `rg -g` - is `ignore`, and that is what this tool depends on
(already in the workspace lock): `WalkBuilder` for the ignore-aware,
non-following walk and `overrides::OverrideBuilder` for the glob. zvec-grep's
Rust crates were the earlier candidate and are worse: `publish = false`, its
`file_selection` module is crate-private, and its public `ZvecGrep::files`
compiles the whole engine (measured: `cargo tree -p zg-engine` = 658 packages,
with a prebuilt zvec native library, ONNX Runtime, llama.cpp and eight
tree-sitter grammars). No one-walk tool may charge that to BSH's default build.
The tool adds only what ripgrep does not: the build/dependency exclusion list
(taken from zvec-grep's scan policy, minus its content exclusions such as
`locales`, `logs` and `tmp`, which a file search may legitimately want),
`.git`/`.jj`/`.bsh` always skipped, and pattern/result bounds.

Traps worth keeping: `Override::matched` returns `Ignore` for a non-match once
any whitelist exists, so the filter must test `is_whitelist()` rather than
`is_none()`; a leading `!` is ripgrep's exclusion syntax and a lone exclusion
would match nothing, so it is refused by name; `ignore` only applies
`.gitignore` inside a repository unless `require_git(false)` is set, which a
workspace with the file and no `.git` needs; a `filter_entry` closure sees the
walk root at depth 0, so skipping by name at every depth would make an explicit
`path="target"` walk nothing - the root is exempt. Tests: depth/basename/anchored matching, ignore
and hidden opt-in, default build/dependency skipping with root exemption,
metadata never returned, symlinked directory not followed, sorted/limited/
truncated results, invalid/empty/oversized patterns, a file root refused, and
the registry-level workspace-relative paths plus the outside-workspace policy
refusal.

Round 176 added `grep`, the content-search half of the pair. It is built on
ripgrep's own crates through the `grep` facade the `rg` binary is built from:
`grep::regex::RegexMatcherBuilder` for the pattern, `grep::searcher` for the
line-oriented scan and binary detection, and the same `ignore` walk the glob
tool uses (shared through `glob::build_walker`). A hit is
`{path, line, text}` with a 1-based line number; results are sorted by path and
line, and every axis is bounded: 4 KiB pattern, 4 MiB per file, 300 characters
per reported line, 50 hits by default (500 max), plus the glob tool's candidate
and ignore policy. Case folding is the explicit `case_insensitive` switch;
`include_ignored` lifts hidden/ignore/default-tree skipping; a file with a NUL
in its head is skipped, as is a symlinked file (the walk never follows links).
An empty result carries a hint naming the case sensitivity and the skipped
ignored files, so "no match" is not read as "not in the workspace".

Why the facade and not a binary: `ripgrep` on crates.io is binary-only, but the
`grep` crate is the library rg is built from and `ignore` is its walk. Both are
pure Rust and small; the facade pulls grep-matcher/regex/searcher/printer/cli.
The tool is registered in the `files` bundle beside glob, gated by the tool
switch and presets (the shipped read-only preset lists it), and the prompt and
stale-reference guards were updated so the model is told to use it for contents
and `glob` for paths - the exact confusion that made `glob hello` look broken
when `hello` lived only in file contents.

Tests: regex vs literal, case folding opt-in, ignored/hidden opt-in, binary and
oversized file skipping, bounded hits with `truncated`, invalid regex naming
itself, symlinked file not searched, the registry-level hit shape, the empty
hint, and the outside-workspace read refusal. Registry: 618+ tests passed with
clippy -D warnings clean before this round; the grep tests are in `grep.rs` and
`caps.rs`.

One cross-round wart: the shared working copy's tungstenite bump (0.24 -> 0.30)
landed without its code half, so the build failed on `Message::Text(String)`.
Adapting `browser/engine/cdp/wire.rs` to `Message::Text(.. .into())` is the
one-line fix needed to compile; it is unrelated to grep and belongs to the
browser round.

Round 177 turned the right pane's browser from "correct" into the crate's gold
requirement: EXTREMELY fast, with the correctness it already had left standing.
Three measured facts drove it, all probed against the prior browser engine
before a line was written (`docs/bsh-browser-substrate.md` carries the numbers):

1. **PNG is 2-9x faster than JPEG on this engine.** `Page.captureScreenshot` at
   jpeg q72 averaged 16.7 ms on the fixture and 74 ms on a dense page; PNG
   averaged 8.3 ms and 8 ms, and was SMALLER too (7 KB against 20 KB base64).
   The pump had been capturing JPEG at q72 with a 120 ms sleep: about eight
   frames a second no matter what the page did.
2. **`Page.startScreencast` is NOT a paint pump.** It answers OK and even
   advertised an activity-driven frame source, but it pushed frames only on
   NAVIGATION activity: a DOM change, a hover, a key, a wheel and a fully idle
   second all produced ZERO frames (measured). The polling pump stays; what
   changes is how it polls.
3. **The engine's own network signals cannot settle a page.** `Network.enable`
   emits NOTHING for a script-started subresource - a 900 ms-delayed image
   produced no `requestWillBeSent` at all - and `Page.lifecycleEvent
   networkIdle` fires WITH the load event while a 200 ms-later image was still
   in flight. Both were rejected as settle signals, with the measurement
   recorded in the doc so nobody re-tries them.

What the pane has now: the pump probes with PNG, compares the captured bytes
with the previous frame (an unchanged page decodes and stores NOTHING), and
adapts its cadence - a 33 ms BUDGET while the page changes (it sleeps only what
is left after the work, so a slow decode is never displayed slower than the
machine can decode it), 120 ms once four unchanged probes have gone by, woken
instantly by a shared activity stamp that navigation, input, a resize and the
request seam all bump and the pump's own probes deliberately do not. The budget
is not a guess dressed up as one: the first version slept a FIXED 33 ms on top
of the work, and the self-animating proof measured 4 frames in 400 ms in a debug
build where the frame's PNG decode is the slow half of a cycle; pacing to the
budget doubled it to 8. A claim about frame rate needs BOTH halves measured -
the engine's capture (8.7 ms still, 19.1 ms changing) and the local decode. The decode AND the RGBA->BGRA channel swap moved to
the pump thread, so the pane's render path is a plain copy of ready bytes (the
swap used to be a full per-pixel pass on the UI thread). The rest of the round
is latency: the idle window went 500 -> 400 ms with the load event now stamped
(without that stamp a quiet window that started with the document's own fetch
could beat a late subresource's stamp to the settle - the repo's own proof
would have gone flaky), and navigation grew TWO settles behind one seam: a TOOL
waits the full networkidle window because it reads the page's text, while the
PANE lands one paint beat after the load event because the frame it shows is
already live. MEASURED on one engine and page: `tool=443.95 ms pane=153.20 ms`.
The pane's viewport override also left the render path: a drag changes the
bounds every frame and each `Emulation.setDeviceMetricsOverride` was a blocking
round trip on the UI thread, so the want is recorded and applied off-thread at
most once per 150 ms.

Traps this cost, so the next round is cheap:

- `Engine::navigate` became dead code the moment the model passed a settle
  explicitly (`cargo build` warned, and `-D warnings` would have failed the
  gate). The engine now has ONE navigation entry that requires its settle, and
  every engine test says `Settle::Idle` out loud - which is exactly the
  documentation the wrapper was hiding.
- A model-level parameter named `settle` SHADOWS the module's own `settle()`
  function (the text-fallback path), and the error is `E0618: expected
  function, found Settle` at the far-away call site. The function is
  `settle_fetch` now.
- The pump must NOT bump its own activity stamp: `dispatch` bumps for every
  command, so a naive version made the pump's own captureScreenshot count as
  page activity and pinned the cadence at 33 ms forever. The exception is
  explicit and commented.
- Instrumenting the pump's cadence needs the FrameSlot HANDLE, and the shared
  `engine()` test helper builds one internally; the two pump proofs construct
  the engine directly with `launch_in` and their own `Arc<FrameSlot>`.
- Proving "an unchanged page publishes nothing" against a freshly navigated
  page races the first few renders (layout and fonts settle over a few probes),
  so the proof waits for the sequence to go STILL first - asserting immediately
  fails for the right reason on a loaded machine.
- The BGRA proof has to discriminate on the same byte it asserts: a red page
  arrives `[0, 0, 255, 255]` and turns blue as `[255, 0, 0, 255]`, so a
  missing swap fails the first assert and a mis-ordered copy fails the second.
- `cargo fmt` rewrites these files between reads, so an `edit` whose anchor was
  read before a fmt run is refused with "file changed since it was read" -
  re-read the region, then retry. It also reformats UNRELATED files in the crate
  (the whole of shell_tests.rs, in this round): restore those with
  `jj restore <path>` so the change set stays the round's own. Check that a
  "modified" file you never edited is only formatting BEFORE restoring it -
  `jj file show -r @- <path> | rustfmt --emit stdout` and a diff against the
  working copy proves it.
- This sandbox blocks `ps`, `sandbox-exec` and every confined shell, so 26 tests
  fail here for that reason alone and NOT because of a code change: the 2
  `browser::engine::reap` tests (the reaper shells out to `ps`, which answers
  rc=126), 7 `caps` tests and 14 `terminal` + 3 terminal-pane tests
  (`sandbox-exec: sandbox_apply: Operation not permitted`, exit 71). Prove it
  from the failed assertion's own stderr before touching any of that code.


Round 178 made the rendered browser DIRECT, by the owner's explicit decision,
and this note exists because the decision REMOVED an enforcement the previous
rounds had spent effort proving. What went: the loopback egress proxy
(`browser/engine/gate.rs`, deleted), the `--proxy` argument on the engine
launch, the pre-flight `Gate::preflight` before `Page.navigate`, the Fetch
seam's refusal branch, the `NetworkMode` vocabulary with its Proxied/Strict
setting and toolbar chip, the four locale keys behind them, `Tools`'
policy/session/preset fields, the `network_mode` payload fact, the pane's
`browser-seam-refused` banner and `engine_refusal`, and the four socket-level
refusal proofs (a refused URL, a refused subresource, a denied path, Strict
mode) that described a behaviour the pane no longer has.

What replaced them is one honest fact: the pane and every browser tool result
now say `network: "direct"`, and the substrate doc states the cost in its own
section - a URL outside the workspace allowlist loads here, and its origin sees
a real socket. The allowlist still governs the shell tool and the MCP transport,
which is where its refusals are still recorded and audited.

Traps and seams worth keeping:

- Deleting a seam deletes its proofs, and a proof deleted for the wrong reason is
  indistinguishable from a test relaxed to get green. Delete the test with the
  behaviour, name the behaviour in the round note and in the doc, and never
  leave the assertion behind pointing at something that cannot happen.
- The Fetch seam was NOT deleted with the gate. `Fetch.enable` is what bumps the
  navigation idle clock (`shared.touch` on `requestPaused`), so removing it would
  have silently reverted every settled navigation to load + 120 ms - a timing
  regression arriving through a security change. It now continues every paused
  request and refuses none: observation, and the comment says exactly that.
- `wait_network_idle` keeps its `inflight` ceiling and its counters; only the
  `failed` counter went (nothing fails a request now).
- The text view's fallback fetch used `agent::policy::fetch`, which REQUIRES an
  `EgressGate`. A direct browser needed a direct producer: `fetch_direct` is a
  small reqwest call driven by a current-thread tokio runtime, built only on that
  degraded path, and it is what keeps the pane working when the engine binary is
  absent. It replaces a gate-shaped API call, not a feature.
- The denial copy that named the reader's own setting (`right.browser.strictRefused`
  and the rule line) had to go with the setting: with nothing refusing, a failure
  sentence is always the producer's own prose.
- `Engine::client()` became test-only (`#[cfg(test)]`) the moment the refusal
  reader went: the tests still read the seam's counters, and `-D warnings` in a
  non-test build would otherwise call it dead.
- A concurrent writer shares this working copy: it committed this round's work
  mid-flight (the parent change id moved from `ozvmtnwr` to `sqlstmvv` and the
  edits appeared in `@-`), which shows up first as repeated
  `file changed since it was read` refusals from the edit tool. Re-read the file,
  check `jj log`/`jj diff --stat` to see whether the change already landed, and
  never re-apply a patch blindly over a commit that contains it.

Browser backend update (2026-10-27): BSH now launches installed Google Chrome
or Chromium over CDP; Chromium is the supported runtime. Set
`BSH_CHROMIUM_BIN` only when the executable is outside the normal PATH or macOS
application paths. The browser profile is temporary and page traffic is direct
(`--no-proxy-server)); the loopback socket is only the private CDP control
connection. Integration tests use the installed browser and direct local
fixtures.

Sidebar row actions (2026-10-27): the session and workspace rows drew their
hover controls as an ABSOLUTE overlay on the trailing edge, so a long title
painted straight through them and the session menu sat under its own label. The
reference does not overlay: `dsh-client-ui-workspace`'s own sheet swaps
`.time`/`.pinIndicator` for `.rowActions` (`display:none` until `:hover`)
inside the row's flex line, so the `flex:1` title truncates before them. Both
rows now carry one `hover_actions` slot - zero wide until the row is hovered,
then exactly the measured cluster width (20px boxes, the kit's 32px menu button
in a 4px pad, 2px gaps) - while the age, and the pinned row's 16px rest mark,
collapse in the same swap. Traps: a hover style that changes LAYOUT only lands
on a STATEFUL element (group hover on a plain `div` is paint-time only, because
`request_layout` computes style with `hitbox: None` and reads only the recorded
`hover_state.group`); a fixed 20px box inside a 0-wide slot came out 12px until
each box carried `.flex_none()`; and two older tests pressed the pin/ellipsis
without hovering - those controls are only in the flow under the pointer now, so
they hover first, as the workspace-row test already did.

Round 179 let `grep` take a file where it takes a directory. The tool refused a
file root with "is not a directory", so an agent that had just listed a path got
an error for naming one of the files it found; `search` now branches once - a
file to read, a directory to walk - and both go through the same per-file
`search_file`, so bounds, binary detection and the sink cannot drift.

Two rules the branch had to state rather than share. A file named in `path` is
read whatever its name or ignore status says, because naming it is the point;
and a named file that cannot be searched - binary, past the 4 MiB ceiling, not
regular - is REFUSED with its reason instead of returning no hits, since there
is no other file in the call that could answer, while the walk keeps silently
skipping those files (the others still answer). The empty hint drops its
"ignored and hidden files were skipped" sentence for a named file, which would
be false there.

Traps: `relative_to(base, path)` returns an EMPTY path when the two are equal,
so a file that is its own search base reports its parent-stripped NAME
(`hit_base`); and `std::fs::metadata` follows a symlink where the walker's
`entry.metadata` does not, which is fine only because a symlink never reaches
the helper from the walk. Tests: file root by name and by workspace-relative
path, ignore/hidden exemption, binary and oversized refusal, a missing path
that says `does not exist`, a socket that says `not a file or directory`, and
the registry-level `a_grep_searches_a_file_path`.

Round 180 put grep on ripgrep's SDK seam rather than beside it. The engine was
already ripgrep's (`grep` facade: grep-regex + grep-searcher, and `ignore` for
the walk), but the tool only used the `sinks::Lossy` closure and pre-probed the
file itself for a NUL in its first 8 KiB. `grep.rs` now implements
`grep::searcher::Sink` (`HitSink`) - `matched` writes a `Hit`, `finish` records
`SinkFinish::binary_byte_offset()` - so the line buffer, the line numbers and
the binary detection are the searcher's; `looks_binary`, `BINARY_PROBE_BYTES`
and the `std::io::Read` import are gone, and a named file that cannot be opened
now reports "could not be read" instead of "looks binary".

The reason the sink had to grow a `finish` is the one behaviour the pre-probe
had and `BinaryDetection::quit(0)` does not: with the generic reader the line
buffer scans each 64 KiB fill and quits at the NUL, and the searcher's
per-matched-line check only fires on the slice/mmap path (macOS never takes it -
`mmap::open` returns None there). So a file that matches BEFORE a NUL past the
first buffer has already written a hit when the quit arrives, and rg's answer is
"binary file matches", not those lines. `search_file` records
`found.hits.len()` before the search and truncates back to it when `finish`
reports a binary offset; a binary file also clears `limited`, so a rolled-back
file never sets `truncated`.

Traps: a `&mut S` is a `Sink`, so `search_path(matcher, path, &mut sink)` keeps
the sink readable afterwards; but `HitSink` holds `&mut found.hits`, so read its
flags into locals BEFORE touching `found` (the borrow lives as long as the sink
is used, and `found.hits.truncate(..)` is a second mutable borrow); `finish` IS
called when `matched` returns false, which is what makes the limit and the
binary rollback compose; and a test for the rollback has to put the NUL past
64 KiB, because inside the first buffer the buffer scan kills the file before
any line matches and the test would pass without a rollback. Proven both ways:
`a_match_before_a_nul_is_rolled_back_as_binary` fails with
`["clean.txt:1:needle", "late.bin:1:needle"]` when the truncate is removed.

Honest gaps of the SDK refactor, so the next round does not assume more than
shipped:

- No `grep::printer`. The tool answers typed hits, so `JSON`/`Summary` would be
  an encode-then-decode round trip. Consequence: rg's "binary file matches" line
  has no output row here - a walk drops the file's hits silently and a named
  file is refused by name.
- The dependency is the crates.io facade (`grep 0.4.1`, `ignore 0.4.32`), not a
  `git` dependency on the repository: the code is that repo's, but a git source
  would need network at build time and break the offline build. The `ripgrep`
  crate itself is still unusable (binary-only, no lib target).
- Binary detection became the searcher's, so it is EARLY-QUIT semantics, not a
  whole-file verdict: once the hit limit stops a search, a NUL later in the file
  is never met and the earlier hits stand. The old 8 KiB pre-probe would have
  dropped them. That is rg's own behaviour and it is stated rather than hidden.
- The refactor changed no output shape and no bound: `{path, line, text}`, the
  4 MiB / 300-char / 50-500 limits, the sort and the empty hint are as round 176
  and 179 left them; caps, prompt and reference docs needed no edit.
- Still absent as tool options, measured against rg rather than assumed: invert
  (`-v`), files-without-match (`-L`), context lines (`-A/-B/-C`), an `include`
  glob, and a text-mode override (`--text`). The reference's own grep tool
  carries `include` ("One glob filter for which files to search ... negation is
  not supported"), so that one is also a parity gap, not only an omission.

Round 181 put the results through ripgrep's printer and answered a binary file
the way rg answers one. Round 180's rollback is REVERSED: a file that matches
before its first NUL keeps those rows and gains a binary row, because that is
what `rg` prints (`path:1:needle` then `binary file matches (found "\0" byte
around offset N)`), and the rows a searcher already read are not a lie.

The pipeline is `grep::printer::JSON`, not a hand-written sink: `JSON::new(&mut
buffer)` plus `sink_with_path(matcher, &relative)` per file, then the buffer is
decoded line by line with serde into `Hit { path, line, text, kind }`. So the
line numbers, the `context` kind of a `-C` row, and the base64 `bytes` branch
the printer takes for a line that is not UTF-8 are ripgrep's, and the tool's own
`bounded_line` only trims the terminator and the width. `Emit` wraps the printer
for the one thing a printer has no opinion about - the call's ROW BUDGET: it
forwards `begin`/`matched`/`context`/`binary_data`/`finish` and answers `Ok(false)`
once the budget is spent, which is how the searcher is told to stop reading this
file. `binary_byte_offset()` on the sink is the binary verdict. The new
dependency is `base64 0.23.1`, the copy this crate's own arrow/parquet/reqwest
graph already builds, so naming it adds no package.

The five switches are rg's, with rg's own semantics measured rather than assumed:
`invert` (`-v`), `context` (`-C`, one integer, clamped to `MAX_CONTEXT` = 20 and
refused above it in caps), `files_without_match` (`--files-without-match`, NOT
`-L`, which is `--follow`), `include` (one glob for which files the WALK
searches), and `text` (`-a`, `BinaryDetection::none`). Measured: `-g` never
narrows an explicitly named file, so the tool's include is a walk filter and a
named `path` is read whatever it says; `--files-without-match` never lists a
binary file, and a file list spends its budget on files, not rows, so one
`matched` per file is all it needs; `-a` reports the lines before AND after a NUL
because it turns detection off rather than converting.

The binary rule is now uniform across a walk and a named file, and stated because
one half is a deviation: a file whose match precedes the NUL yields its rows plus
one binary row (`path`, `offset`, `message`) whether it was walked or named; a
named binary file with NO pre-NUL match is REFUSED by name ("is binary (a NUL
byte at offset N), so its text is not searched"), where rg would print `binary
file matches` - but rg reaches that sentence from a match, and there is none to
print; a walked file with no pre-NUL match is silently skipped, as rg skips it.

Evidence: `the_rows_match_the_installed_ripgrep` is `#[ignore]`d (it shells out
to `rg`, 15.2.0 here) and compares this tool's `(path, line, text, kind)` rows
against `rg --json` for the default, `-v`, `-C1`, `-g '*.rs'` and `-a` cases,
including the base64 branch; binary parity goes through `rg -n --with-filename`
instead, and text-mode parity on the same file through `rg -a --json`. It was
discrimination-checked: reporting a context row as a match fails it with "rows
differ from rg for context". Run it with `cargo test -p bsh --lib grep --
--ignored`.

Traps and honest gaps:

- `rg --json` and rg's standard printer DISAGREE about a binary file: the JSON
  printer converts (reporting matches past the NUL and putting the offset in
  `end.data.binary_offset`), the standard printer quits. The tool takes the
  standard printer's answer, so binary parity has to be measured on standard
  output, not on `--json`.
- The binary offset is the exact NUL offset, not a buffer boundary. Round 180's
  `71680` was that fixture's exact NUL position (`resize(70 * 1024)` then `push`),
  and the probe here reads `70007` from rg for a NUL at 70007 - so do not
  "round" it to a page size to match a memory of the old number.
- Not mirrored: the multi-file `WARNING: stopped searching binary file after
  match` wording. A row already names its own path, so the walk's binary row
  reuses the single-file sentence rather than emitting a second line shape.
- The budget is a ROW budget, where rg's `-m` counts matching LINES: with
  `context` set, a context row spends budget too, and a `files_without_match`
  call spends it on paths. Stated rather than changed, because an agent's real
  question is "how many rows will I read".
- Not offered, so nobody assumes otherwise: multi-line matches, separate
  before/after counts (`-A`/`-B`), repeated or excluded globs (`-g` lists and
  `!` patterns are refused by name), `--files-with-matches`, `--type`, and
  `--no-ignore-vcs`.
- The parity test parses rg's JSON with its own `serde_json::Value` reads rather
  than widening this module's `Data`: a `path` field added there is never read by
  the tool, and `cargo clippy --all-targets` would report it as dead code in the
  non-test build.

`Query` gained `invert`, `context`, `files_without_match`, `include`, `text` and
`Default`; `Found` gained `binary` and `unmatched`; `Skip::Binary` is gone
(refusal now comes from `FileResult::Binary { offset, matched: false }`); `hint`
takes `&Query` and the root and names include, invert and files_without_match.
The result JSON keeps its old shape, adds `kind: "context"` on a context row,
`binary: [{path, offset, message}]` and `files_without_match: [path]`, and only
carries a hint when nothing at all was found. `base64` is the round's one new
dependency; caps, the GREP prompt line and both rows of `docs/bsh-prompts.md` now
name the switches.

Round 182 fixed the blank body the browser pane drew when it opened. The report
was a white rectangle with no globe, no prompt, no loading line and no failure
line - indistinguishable from a crash, and it reported nothing the reader could
act on.

The cause was not the engine. `browser_live_body` chose between "draw the
capture" and "draw `browser_engine_notice()`", and that notice renders an EMPTY
div when `engine_error()` is None. So the healthy states - engine ready, no
capture yet - produced exactly nothing, while the empty and broken states each
had their own body the reader never saw.

Two rules came out of it, and both are now structural rather than copied prose:

- A pane with no drawable frame falls back to `browser_engine_notice()` PLUS
  `browser_body(view, cx)`. The text view is the pane's account of what it is
  doing (empty prompt, "loading <url>", or the failure and its next step), so
  there is no state left that can paint a bare rectangle.
- A capture is the reader's PAGE only when the browser holds a document for it:
  `frame_is_the_page(status)` is `status != Empty`, because the engine
  pre-warms `about:blank` and that white capture is indistinguishable from a
  broken pane. `browser_go` now drops the frame it replaces into
  `browser_render_stale` (released by the next render's `drop_image`), and
  `browser_history` does the same on success, so neither the pre-warm nor the
  previous page can stand in for the page being loaded.

The same gate then removed work nobody asked for: `sync_browser` asks the pump
for frames only while there is a page AND the pane is visible
(`set_streaming(visible && has_page)`), so an empty window stops probing and
PNG-decoding `about:blank` eight times a second and stops publishing a
full-viewport texture this pane does not draw. That forced one ordering change -
the cached pane view is refreshed BEFORE the engine block, because the gate's
`has_page` is read from it and a stale view would keep an emptied pane
streaming for a tick.

Draw the failure ABOVE the frame, not instead of it. A live pane swallowed every
failure: the frame stayed and the sentence the pane owed the reader was never
drawn. It now sits in the column above the live body, which is the rule the text
view already followed (keep the page you were reading, say why the load failed).

That exposed a copy bug worth naming. `Page.navigate` answers with Chromium's own
`net::ERR_*` when the PAGE cannot load - a name that does not resolve, a port
that is closed - and the engine is perfectly healthy there. Reporting it through
`Failure::from_engine` appended "the text view is still usable / reopen the tab to
start a new engine" to a reader whose only problem was a typo. `page_load_failure`
now classifies the navigation refusals into the transport vocabulary the pane
already has (`dns`, `tls`, `connect`, `redirect`, `scheme`) and carries the
engine's raw sentence as the detail; anything else (a dead socket, a CDP method
the engine does not implement) keeps the engine's message AND its install/restart
advice, because there the engine is what has to be fixed.

Evidence: two probes pin the pure halves -
`ui::right_panel::tests::an_empty_pane_never_draws_a_capture` (pre-warm is not a
page; loading/ready/failed are) and
`browser::tests::a_navigation_chromium_refused_is_a_load_failure` (eight
`net::ERR_*` texts to their kind, the sentence travelling unchanged, no fallback
advice; and a `Runtime.evaluate` refusal staying an engine failure). In the GUI,
with the window still live: a fresh launch shows the globe plus "Enter a URL to
open a page ..."; `example.com` draws the captured page; `nosuchhost.invalid`
shows "The name did not resolve / net::ERR_NAME_NOT_RESOLVED" above Chrome's own
error page; a local `http://127.0.0.1:8799/{a,b}.html` fixture renders A then B,
and Back shows A's text view.

Traps and honest gaps:

- Back/Forward restore the PANE's cached document, not the engine's history:
  the model's history holds only what it loaded, while the engine also holds
  `about:blank` and its own error pages. So the frame is dropped on a successful
  move and the restored document shows as a text view until the next navigation.
  Truth over a stale picture, and the visible cost is stated here rather than
  papered over.
- The pane-level gate has live evidence only. Test builds disable the engine, so
  there is no shell test that drives `set_streaming(visible && has_page)`; the
  engine tests prove `set_streaming(true)` publishes frames, including after a
  `false`. The pure `frame_is_the_page` is what is pinned headlessly.
- The LAST change (the `has_page` pump gate plus that reordering) is verified by
  build, clippy, docs and 1188 tests only. The GUI session went dark right after
  it: `ocu` answers `Apple event error -10005: cgWindowNotFound` for every app
  and `screencapture` shows wallpaper with no menu bar, so the empty-pane frame
  capture could not be re-taken. Re-verify live before trusting it further.
- `browser_frame_size` is still write-only: it is recorded from each frame and
  nothing reads it. Either a size-dependent decision appears, or it should go;
  it is not evidence that the viewport is checked.
- `set_streaming(false)` only clears an atomic (the pump thread stays alive and
  sleeps `FRAME_IDLE`), which is why gating it per tick is free and cannot strand
  the pump. Do not "optimise" it into a start/stop of the thread.

Round 183 fixed the browser pane's pointer HANDLING. The report was that the
right pane's page accepted no right click and no double click. Two independent
defects met, and the second one hid behind a plausible-looking mechanism:

- Only LEFT down/up were registered, and `mouse_params` stamped
  `button: "left"` on every event. A right press therefore never became the
  page's `contextmenu` (it was not delivered at all), and the middle button was
  equally absent.
- The pane timed double clicks itself in PAGE coordinates
  (`BrowserClick::next` over the mapped point) with a 500 ms / 4 px window.
  MEASURED with a live frame: the initial engine viewport is `(1000, 700)` until
  `Emulation.setDeviceMetricsOverride` lands, so two presses half a second apart
  mapped the same window point to (500, 350) and then (240, 502). The slop
  failed, the second press carried count 1, and `dblclick` never fired - and the
  press could also aim at the wrong spot while the frame on screen was captured
  at the old size.

The fixes are structural, not local tweaks:

- `PointerButton` (cdp) names the button and carries its mask. `pointer_down`,
  `pointer_up` and `pointer_move` take it, and `buttons` is the held set: the
  pressed button's mask on a press, 0 on a release, and the held mask on a drag.
  The pane registers down/up for Left, Right and Middle on both the frame and
  the window's captured-release listeners, so a release outside the frame still
  ends the right gesture it started.
- The count is now the PLATFORM's: `MouseDownEvent::click_count` is AppKit's own
  `clickCount`, measured in window coordinates against the real double-click
  time and point. It is clamped to 1..=3 for the page's `detail` and stored with
  its button, so a right press cannot continue a left press's sequence. The
  pane deleted `BROWSER_CLICK_MS`, `BROWSER_CLICK_SLOP` and the page-space
  `near` reset; a move beyond the slop is the platform's job.
- `browser_pressed` is now `Option<MouseButton>` and a release only ends the
  gesture of the button that is actually held; `browser_drag_start` is `Some`
  only for a left press, so a right drag does not touch the text selection.

Evidence: `browser::engine::tests::a_right_press_raises_contextmenu_and_a_middle_press_its_own_button`
drives a real Chromium page and sees `mousedown`/`contextmenu` with button 2,
then `mousedown`/`auxclick` with button 1. The unit test
`ui::right_panel::tests::a_press_uses_the_platform_count_and_restarts_for_another_button`
pins the count clamp and the button reset, and
`ui::shell_tests::a_double_click_and_a_right_click_reach_the_live_page` drives
the LIVE pane (no text view) through single, double and right clicks and reads
the page's own log: `click:1:0`, then `dblclick:2:0`, then
`mousedown:1:2`/`contextmenu:0:2` with no left click. That test calls
`cx.dispatcher.allow_parking()` because the pump thread's frame wake is real
cross-thread work, the same seam the turn tests use.

Traps:

- `MouseUpEvent::click_count` exists but the pane does not read it: the stored
  press count is what both halves of the gesture must name, and using the up's
  value would let a mismatched pair reach the page.
- `contextmenu`'s `detail` is 0, not the click count; a test that expects `:1:2`
  is asserting Chromium's event table, not the pane. The button is the field
  that matters there.
- A second press while one is held is ignored. The page cannot express two held
  buttons through CDP's `buttons` field meaningfully, and accepting it would
  release the drag in progress.
- The page point still maps through `engine.viewport()`, the last viewport the
  engine was ASKED for, so a frame captured before a resize can be on screen for
  a tick while the mapping already uses the new size. That transient aim is
  unchanged; what this round removed is its effect on the click COUNT, which is
  now measured in window coordinates by the platform.

Round 184 fixed text selection in the browser pane, and the cause was one field
missing from a mouse move. The report was that rendered page text could not be
selected at all.

MEASURED at the engine, with a real Chromium page: `Input.dispatchMouseEvent`
with `type: mouseMoved`, `buttons: 1` and `button: "none"` selects nothing -
before or after the release - while the same drag with `button: "left"` selects
the crossed range, spans block boundaries, and selects inside a `<textarea>`
(`selectionStart`/`selectionEnd` moved). `buttons` alone is not enough; the
renderer needs the move to NAME the held button to treat it as a drag rather than
a hover.

That is why the pane had a workaround, and why the workaround was what the reader
saw fail: `select_text_between` synthesized a range from
`document.caretPositionFromPoint` at the drag's two endpoints, and it returns
false - selecting nothing - whenever either endpoint's caret is not a TEXT node
(a margin, the gap between blocks, whitespace beside a block) and refuses editable
nodes outright, so no field's text could be selected at all. The script also cost
a `Runtime.evaluate` per captured-frame move, on the UI thread's queue.

The fix is in `pointer_move`, not in the pane: a held button rides the move
(`button` = the held button's name, `buttons` = its mask), and the page's own
default selection does the rest. `select_text_between`, `text_selection_script`,
`SELECT_AT_JS`/`select_at_script`, the pane's four call sites and
`Shell::browser_drag_start` are gone - one mechanism, no caret heuristic.

The same measurement retired the multi-click script: a press and release carrying
`clickCount: 2` select exactly the word, `3` the line/block, natively. The
triple-click assertion now trims the value, because Chromium's block selection
keeps the boundary's `\n\n` in `getSelection().toString()`.

Evidence, each re-injected and watched to fail:
`browser::engine::tests::dragging_over_page_text_selects_it_in_the_page` (a drag
across two paragraphs, then a later drag that must REPLACE it - empty string when
`button: "none"` is restored) and
`ui::shell_tests::a_drag_over_the_live_page_selects_the_text` (the live pane, a
press, six moves and a release; the page reports `drags >= 1` and a selection
containing the crossed words - empty when the field is reverted).
`a_double_click_selects_the_word_and_a_triple_click_the_line` now proves the
native path too, since nothing scripts the selection any more.

Trap worth keeping: a `.5px`-style "harmless" redundancy hides the real gap. The
scripted selection made double click look correct for rounds while the drag path
was broken, because the two shared a mechanism only by accident - the script ran
on both. Assert the PAGE's own `getSelection()`/`selectionStart`, never that the
pane sent events.

Round 185 gave the composer image input: a pasted screenshot, a dropped file and
the picker now feed one intake path, and the attached images draw the reference's
64px rail. A picked image already reached the model (Attachment -> `turn_input`);
what was missing was every way of getting one in, and any picture saying so.

MEASURED from the reference's own sheets (now in
`docs/ui-mirror/reference/dsh-css-inline/`): `ComposerAttachments.module.css` and
`AttachmentRail.module.css` - `.rail{margin-bottom:-6px;padding:2px 10px 0}`,
`.item{height:64px}`, `.thumbnail{width:64px;height:64px;border:.5px solid
border-l2-darkmode-thin;border-radius:radius-xl;background:
interactive-bg-hover}` with an `object-fit:cover` image, and `.remove{18x18 at
top:4px;right:4px;background:button-contrast-fill;color:
label-primary-inverted;opacity:0}` revealed by `.item:hover .remove`. The radius
is 20px (`--dsw-radius-xl:20px`, read from `dsh-client-ui-theme/lib/client.js`).
The ceilings are the shipped `dsh-attachment-local` defaults
(`maxImagesPerMessage` 20, `maxImageBytes` 20 MiB), and the refusals reuse the
reference's own sentences ("Image exceeds the configured byte limit.",
"Unsupported or malformed image data.", "Image limit: up to {count} images,
{size} each", from the conversation bundle's `attachment.dropDesc`).

The seam: the kit's `Input`/`Textarea` already ships `.on_paste(handler)`, and
its own doc names this case ("images and copied files belong in app-owned state
beside the input (e.g. Attachments)"). bsh wires it on the composer; text returns
`false` and falls through to the kit's insertion. Drops arrive as
`PlatformInput::FileDrop`: Entered parks an `ExternalPaths` on
`cx.active_drag`, Submit becomes a MouseUp, and every element whose hitbox is
hovered gets the drop on the bubble - so the listener sits on the shell frame,
this window's document. The picker, the paste and the drop share
`intake_paths`/`push_attachment`, so they cannot disagree about the ceilings or
about what counts (an image path attaches; anything else becomes an @mention,
bsh's existing file-reference vocabulary).

The rail draws a 128px PNG `Attachment::preview` built ONCE at attach, so a frame
hashes ~20 KB and GPUI's asset cache (keyed by the `Image` id, i.e. a content
hash) reuses the decoded tile; the original payload never reaches a frame. That
is also why the tile is `img(Arc<Image>)` and not a `RenderImage`:
`RenderImage::new` mints a fresh id per call and would leak one atlas tile per
frame - the trap the browser pane's frame cache documents.

Traps:
- `on_paste` is `Fn(&ClipboardItem, &mut Window, &mut App) -> bool` with no
  `Context`, so `cx.listener` cannot express it: the handler holds
  `cx.entity().downgrade()` and calls `paste_clipboard` through
  `WeakEntity::update`.
- `accepts_image_for` answers FALSE when the session resolves no model, so an
  intake test on a blank scratch state looks exactly like a paste that never
  fired. `state_with_image_model` seeds a provider and selects a leaf; without it
  both new intake tests failed with an empty list.
- `ExternalPaths` has no `FromIterator`; the field wants a `SmallVec`, so
  `ExternalPaths(vec.into())` is the construction a test can write.
- A clipboard carrying images AND text must not be fully consumed: the reference
  pastes the text too, so `paste_clipboard` returns `false` when a non-empty
  string entry exists and lets the kit type it.
- A refused image still consumes the paste: otherwise the kit's fallback types
  the file paths (its `text()` falls back to them) after the notice already said
  no.
- The SEND guard was the hidden half of "take images": the reference defines
  `empty = draft.trim() === "" && attachments.length === 0` (its InputBar), so an
  image with no words is a send. bsh's `Shell::send` returned early on an empty
  draft, which made an image-only message a silent no-op; `composer::can_send` is
  the pure rule now, `turn_input` writes no empty text block, and `display_text`
  has no leading blank line for an image-only row. A credential-refusal probe is
  NOT a usable observable for this (an anonymous provider opens the turn), so the
  test asserts the stored user row instead.

Honest gaps, each with what has no seam:
- The reference's full-viewport drop invitation (`DropOverlay`: blurred mask,
  115x84 illustration, 20px title over 16px/14px copy) is NOT drawn. bsh accepts
  a drop anywhere in the window but paints no cue over it.
- The rail's hidden scrollbar and its 24px edge arrows are not drawn; the rail
  scrolls horizontally with the platform's own overflow.
- A tile press opens the reference's original-image lightbox. bsh has no image
  viewer, so the tile is inert, the only action is the hover-revealed remove, and
  `cursor:zoom-in` is deliberately not set - the cursor must not promise a
  preview that does not exist.
- Only png/jpeg decode: `image` is built with those two features, so a webp/gif
  attachment gets a named tile instead of a picture.
- Non-image drops are @mentions, not the reference's 240x64 file cards: bsh's
  attachment model is an image.
- Stored history still carries only the display text (`[image: name]`) while the
  live request carries the blocks, and `caps::agent_messages` replays text only,
  so a later turn cannot re-read the image. Reported, not faked.
- The 128px preview is decoded synchronously at attach (one decode per image, on
  the input path), so a 12MP photo blocks that frame for the decode; the
  reference normalizes off-thread.
- The reference's tile carries `title: image.openOriginal` and its remove an
  `aria-label: image.remove {name}`; GPUI exposes no accessibility tree, so bsh
  carries neither, and the tile has no original to open.

Four axes:
1. Backend in Rust: the same `Attachment`/`turn_input` path a picked image used;
   the new intake only decides what becomes an attachment, and the model's image
   switch plus both ceilings are enforced in `push_attachment` before the
   payload is held.
2. Interaction: Cmd-V attaches a clipboard image; dropping attaches image files
   and mentions the rest; the hover-revealed remove drops one attachment; the
   picker takes the same ceilings. Text paste is unchanged (proved by a test).
3. Tips/help: no new tooltip (the reference's `+` keeps `input.commands`);
   refusals carry the reference's sentences.
4. Errors: no model / text-only, oversize, over-count and undecodable formats
   each show a notice and consume the paste, so nothing lands in the draft.

Evidence: `ui::shell_tests::pasting_an_image_attaches_it` (clipboard image,
64px tile), `pasting_text_still_reaches_the_draft`,
`dropping_files_attaches_images_and_mentions_the_rest` (a real
`FileDropEvent` pair), `the_remove_control_drops_an_attachment` (the 18px
control pressed inside the tile), `an_image_alone_can_be_sent` (Enter opens the
turn with the mark and no words), and
`state::tests::an_attachment_preview_is_a_square_png` /
`an_image_alone_is_a_sendable_turn`. Screenshot, sandboxed home at 1280x820: the
tile draws 64px with the 10px inset and 2px top pad in the composer card, and no
remove control until hover.

Round 126 made a turn cheaper and more tool-first, in the places the cost
actually lives. Measured first from a stored session (the .bsh history
parquet): the model streamed a long reasoning trace, and its preamble text was
DROPPED, because the engine recorded an assistant row only when the step had no
tool call - so the next step rebuilt a thought it had already had, and the
transcript showed words the session never kept.

Four seams:
- `crates/react/src/engine/mod.rs`: the preamble is recorded BEFORE the call it
  introduces (streaming and non-streaming loops), and a step that arrives with
  neither text nor a call gets one directed retry (`EMPTY_ANSWER_NOTE`) instead
  of closing the turn on silence. A reasoning model that spends a whole step
  thinking and emits nothing used to end the turn blank.
- `crates/agent/src/agent/agentic/engine.rs`: a skill is no longer a tool of its
  own. N skills used to mean N no-op entries in the request tool array, on top of
  the same catalog the vendor already appends to the system prompt; the single
  `load_skill` loader is the whole surface now.
- `crates/bsh/src/instructions.rs`: the rendered `AGENTS.md` baseline is capped at
  16,384 bytes rather than the reference's 65,536, because BSH re-injects it every
  turn and re-sends every step. A larger file is truncated most-specific-first
  with the named budget marker, and the rest stays readable with `read_file`.
- `crates/agent/src/agent/prompts.rs`: the standing guidance now opens with the
  harness bar - act now (call the tool in the same message you decide), think
  briefly (decide, then call) - before the plan/verify/report rules. The pinned
  rule test follows it.

Evidence: `react::tests::a_blank_stream_step_is_retried_instead_of_ending_on_silence`,
`react::tests::the_preamble_before_a_tool_call_stays_in_the_session`,
`agent::prompts::tests::the_default_prompt_pins_every_workflow_rule`,
`instructions::tests::the_rendered_batch_is_bounded_and_names_the_truncation`.

Round 127 made a submitted image durable and visible. The bug was not the live
request: `turn_input` already sent the exact binary block. `Shell::submit` and
`drain_inbox` then stored `display_text(...)` (the `[image: name]` mark) through
`begin_turn`/`begin_turn_in`, so the row's `images` stayed empty; a later turn
or reload replayed text only. Rows now keep the reader's clean words plus the
EXACT `data:<mime>;base64,<payload>` URL for normal, queued and steered turns.

- Backend in Rust: `state::attachment_images` is the one data-URL builder;
  `begin_turn_with_images` / `begin_turn_in_with_images` persist it, and
  `message_content` reconstructs the same `ContentPart::Binary` on replay. The
  MiniMax/OpenAI `image_url` shape is pinned in the vendor test.
- Interaction: the transcript draws the stored picture ABOVE the optional text
  bubble, right-aligned, with the reference `singleFit` (240px long edge, ratio
  clamped to 0.25..4, no upscaling, 44px frame floor) or 64px cover tiles for
  multiple images. Image-only rows draw NO bubble. The composer still previews
  its attachments, and the queue summary says `Image`, never a file name. A
  frame press opens a full-window lightbox of the cached original; its close
  control, the backdrop, and Escape dismiss it. GPUI has no zoom-in cursor, so
  the frame uses the close analogue (`cursor_pointer`), not a fake one.
- Tips/help: the frame carries the reference's "Open image" label as its
  accessible name and 500ms tooltip; the lightbox names "Image preview" and its
  close control "Close preview" in both locales.
- Errors: a malformed legacy data URL draws a quiet image icon in the same
  frame, with a localized "Image unavailable" tooltip; it does not print a
  filename or retry per frame. Send-time refusals (text-only model, oversize,
  undecodable format) are unchanged.

Performance: a slot-keyed cache (`loaded_from + local index`) decodes a stored
image once, capped at 64 entries and invalidated from the appended/rewound slot
on normal turns, queue drains, steers, goal attachments and rewind; a repaint
only clones an `Arc`.

Evidence: `ui::shell_tests::an_image_alone_can_be_sent` (stored URL, painted
44px frame, no bubble, lightbox opens; close control and Escape dismiss),
`a_queued_image_drains_with_its_original_bytes`,
`state::tests::an_ordinary_image_turn_persists_and_replays_the_same_block`,
`a_steered_image_row_replays_as_a_binary_block`,
`ui::transcript::tests::the_lone_image_box_matches_the_reference`, and
`react::llm::vendor::openai::tests::image_content_serializes_as_an_openai_image_url`.
Native screenshot: 1280x820 sandboxed home, the stored red image paints as the
rounded frame with no `[image: ...]` text anywhere in the transcript.

Round 127 answered the measured cause of slow, unfocused turns, and the cause was
not the model. A stored demo turn (`hello` workspace, request `create AGENTS.md`)
took 5 LLM round trips and ~4 minutes; the request bodies were 60 KB each, and
36,379 of the 41,762-byte system prompt was the skill catalogue - every installed
skill's full description, on every step, for a task that used none of them. A
direct probe of the configured endpoint showed ~23 s per call REGARDLESS of prompt
size (180 vs 9,175 prompt tokens), so the only lever that moves wall clock is the
number of calls, and the only lever that moves focus is what rides in the prompt.

Two changes, both in the request path:
- `react::llm::render_skill_catalog` ranks skills against THIS request: up to 8
  matching skills keep a 240-char description, every other skill is a name, and the
  whole block is capped at 4,096 bytes (measured on the installed set: 36,379 ->
  ~4,000). The Chat and Responses vendors both call it; `load_skill` still opens any
  of them by name. A machine that carries 178 skills no longer pays for them.
- The runtime snapshot now names the workspace layout: top level plus one level
  into up to three directories (`src/ (main.rs, lib.rs)`), bounded at 900 bytes and
  40 entries, with the remainder named. Step one can read the file it needs instead
  of spending a whole round trip on `list_dir`; the shared rule in the prompt also
  asks for every independent read in one message.

Evidence: `llm::types::tests::an_unrelated_catalogue_costs_names_not_descriptions`,
`a_matching_skill_keeps_its_description`, `the_catalogue_is_bounded_even_with_hundreds_of_skills`,
`wire_format_test::the_skill_catalogue_on_the_wire_is_ranked_and_bounded`,
`context::tests::the_snapshot_names_the_workspace_layout`, `the_layout_is_bounded`.
Not fixed here, stated: the endpoint's ~23 s fixed cost per call is the provider's,
and the skill ROOTS (`~/.agents/skills` 119, `~/.claude/skills` 22, `~/.dsh/skills` 1)
are the reader's own configuration; the pane can switch a source off.

The lightbox is the reference's interaction, measured by behaviour rather than
CSS (the primitives bundle is not in the extracted corpus): a modal over the
shell, the cached original fitted to the viewport without a second decode, and
the frame carrying a real button role, accessible name and tooltip. Escape is
handled before the composer's own key bindings and EVERY other key is swallowed
while it is open, so the modal cannot leave the composer editing behind it.
Because GPUI has no zoom-in cursor asset, the frame uses `cursor_pointer`; that
is the only cosmetic difference from the reference frame.

A session delete could be undone by the next LAUNCH, and the cause was a duplicate
document, not the delete: `Store::remove` removed `<root>/<id>.json` for the one
workspace the build resolved, while an older copy of the same id sat under another
registered root. `read` scans every root, `dedupe_by_id` kept one row, and the
surviving stray resurrected the chat on restart. Measured in the demo home:
`8d7f157e...` existed under both `hello/.bsh/sessions` and `bos/.bsh/sessions` with
different `workspace` ids.

`Store::remove` now sweeps every directory a session can live in (each registered
root, then the quarantine), removing the document AND its `history/<id>` log, and
logs a warning when a document cannot be removed instead of swallowing the error.
`Store::save` heals the same way (`drop_strays`): the root the document was just
written to is the authority, so the same id anywhere else is stale and is dropped.
An id is unique, which is what makes "another root's copy" a safe thing to delete.

Evidence: `store::raw_tests::deleting_a_session_sweeps_a_stray_copy_under_another_root`
(two roots, one id, delete, reopen -> empty) and
`store::raw_tests::a_save_drops_a_stray_copy_under_another_root`; the parked/restore
and purge flows still pass (`a_deleted_chat_is_parked_and_can_be_restored`,
`a_parked_chat_can_be_deleted_for_good`).

Halting or quitting mid-turn lost the turn, and the demo home showed it: the
`create AGENTS.md` session's JSON was untouched at 19:04 while a request was
logged at 19:26, so the running turn had never been written. Four separate holes:
- the debug log was TRUNCATED per launch (flexi_logger default), so "the log"
  after a restart was only the last run; the previous run sat beside it as
  `bsh_2026-10-28H19-00-27.log`. The logger now uses `.append()` with the same
  10 MB rotation, so `bsh.log` is continuous and rotation still bounds the set.
- nothing saved at window close: `QuitMode::LastWindowClosed` exits the process
  with the turn's rows still in memory. `AppState::shutdown` now stops every
  running stream and saves every session, wired to
  `Window::on_window_should_close`.
- `Store::save_history` treated "the same ROW COUNT" as "the same rows". A save
  that lands after the assistant placeholder was appended (a goal round, an
  automatic-compaction install, a stop after either) returned early and the
  turn's text/reasoning/tool results were never written. The tail is now compared
  by content (`same_transcript`) and rewritten when it changed.
- and a save from a window that does not reach the log's end (a list-only row a
  turn wrote into) used to take the truncation arm and rewrite the log down to
  that window. It is now refused with a warning instead.

`AppState::stop` also logs what the halted turn kept (text/reasoning chars, tool
calls), so a stopped request is not a dangling body in the log.

Evidence: `state::tests::a_stopped_turn_keeps_its_rows_on_disk`,
`a_shutdown_mid_turn_saves_the_transcript`,
`store::raw_tests::a_save_after_a_placeholder_rewrites_the_tail`,
`a_save_of_an_unhydrated_session_keeps_the_log`.
