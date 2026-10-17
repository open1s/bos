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
increment reports the four axes too, and `docs/ui-parity-requirements.md` §3C keeps the
running status:
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
3. `docs/ui-parity-requirements.md` §11 records the reference's settings
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

BSH's colours come from the active gpui-kit theme. `theme.rs` holds 16
`static AtomicU32` tokens (page, sidebar_bg, selected_row, new_session_fill,
hover, text, muted, faint, border_soft, border, border_strong, accent, error,
success, warn, user_bubble) exposed as accessors. `sync(cx)` copies the kit's
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
metrics are in `docs/round-history.md`, and the click test proves the
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
  point the store at `target/bsh-scratch-home`, so a click that saves a session never
  writes under the user home. Other tests assert the `settings::dir()` string, so
  overriding `HOME` or a `BSH_HOME` process-wide would make them race.
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
traps - lives in `docs/round-history.md`, moved there when this file passed the
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
