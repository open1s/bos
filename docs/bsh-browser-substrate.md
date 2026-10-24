# Browser substrate: Obscura over CDP

Status: shipping. The Browser pane's rendered view is a real left-to-right,
styled, script-running page, produced by an EXTERNAL
[Obscura](https://github.com/h4ckf0r0day/obscura) process (v0.2.5) spoken to
over its standard CDP endpoint. Servo is gone: no crate, no feature, no vendor
patch, no code.

Measured 2026-10-25 on this machine (arm64 macOS, rustc 1.98.1).

## What the user asked for

A real browser substrate for the built-in browser, replacing the text/DOM view,
with the egress gate kept. The owner's decision was Obscura over Servo, driven
DIRECTLY through its CDP endpoint.

## Install (the binary is NOT vendored)

The release tarball is 67 MB and unpacks to two binaries (`obscura` 73 MB,
`obscura-worker` 69 MB). Nothing in this repo carries it.

    # 1. download and extract
    curl -L -o /tmp/obscura.tar.gz \
      https://github.com/h4ckf0r0day/obscura/releases/download/v0.2.5/obscura-aarch64-macos.tar.gz
    tar -xzf /tmp/obscura.tar.gz -C /tmp/obscura

    # 2. either put it on PATH ...
    install -m 755 /tmp/obscura/obscura ~/.local/bin/obscura

    # ... or name it explicitly
    export BSH_OBSCURA_BIN=/tmp/obscura/obscura

    # ... or copy it to the documented development location
    mkdir -p target/obscura && cp /tmp/obscura/obscura target/obscura/

Discovery order, in code (`browser/engine/cdp/client.rs`):

1. `BSH_OBSCURA_BIN` - a path that is not a file is an ERROR naming the
   variable, never a silent fall-through;
2. `obscura` on `PATH`;
3. `<checkout>/target/obscura/obscura` - the development location above.

A missing binary is a NAMED failure in the pane ("The rendered view needs the
Obscura binary; it is not at ...") with the install hint, and the text/DOM view
keeps working. It is never a panic and never a blank rectangle.

### Fonts: the CJK fix

MEASURED: without `--font-dir`, Chinese renders as TOFU BOXES in the
screenshot. With `--font-dir /System/Library/Fonts` the same page renders
Chinese, Japanese and Korean correctly (the engine test asserts the CJK string's
presence in the live DOM, and the captured PNG shows the glyphs).

The shim passes every font directory that exists, by default:

    /System/Library/Fonts  /System/Library/Fonts/Supplemental  /Library/Fonts
    /usr/share/fonts  /usr/local/share/fonts

`BSH_OBSCURA_FONT_DIRS=/a:/b` overrides the list; `BSH_OBSCURA_FONT_DIRS=none`
turns the scan off (faster startup, tofu for CJK). No other flag fixes it.

## The measured CDP surface (what the docs do not say)

Neither of these is in Obscura's docs; both cost a probe:

1. **The page target endpoint is dead.** `/json/list` advertises
   `ws://127.0.0.1:P/devtools/page/page-1`, and EVERY command on it answers
   `{"code":-32601,"message":"No page for session"}` - before and after a page
   exists. The working route is the BROWSER endpoint (`/json/version` ->
   `webSocketDebuggerUrl`) plus `Target.createTarget` (which emits
   `Target.attachedToTarget` carrying the `sessionId`) and then every page
   command with that `sessionId`. The shim still DISCOVERS the target through
   `/json/list`, tries `Target.attachToTarget` first, and falls back to
   `createTarget` - which is the path that works. `Page.enable`,
   `Runtime.enable`, `Fetch.enable`, `Page.navigate`,
   `Runtime.evaluate`, `Page.captureScreenshot`, `Input.*`,
   `Page.getNavigationHistory`, `Page.navigateToHistoryEntry` and
   `Emulation.setDeviceMetricsOverride` all answer on that session.
2. **`Host` must carry the port.** A raw `GET /json/version HTTP/1.1` with
   `Host: 127.0.0.1` is answered `403 {"error":"request refused"}`; the same
   request with `Host: 127.0.0.1:<port>` is `200`. The discovery client sends
   the port.

Also measured and relied on: `--port 0` works with `--ready-file <path>`,
which is written ATOMICALLY once the engine is listening
(`{"pid":..,"host":..,"port":..,"websocket_path":..}`). The shim prefers the
ready file and parses the `CDP server: ws://...` banner as a fallback.
`--quiet` suppresses the banner entirely, so a banner-only client cannot use
it.

## The egress gate, and why it is NOT the Fetch domain

The Fetch domain LOOKS like the right seam. It is not a gate in 0.2.5:

- `Fetch.enable {patterns:[{"urlPattern":"*"}]}` then `Page.navigate` to a
  page that serves after a 1.2 s delay produced this order:
  `[3138 SOCKET] [3139 REQ /slow]` at the origin, `Page.navigate` returning
  at `4347`, and `Fetch.requestPaused` at `4348`. **The pause arrives AFTER
  the response was fetched.** A server that never responds produced the socket
  and the request and NO pause at all.
- Consequence: `Fetch.failRequest` refuses the DELIVERY (the renderer treats it
  as blocked) but the origin socket already exists. It cannot carry "a refused
  URL produces zero origin sockets".

The seam that DOES run before the socket is `--proxy`: Obscura's fetcher
honours it for the document AND every subresource (measured: a page with two
images, a script and a stylesheet produced four proxied requests and zero direct
ones). The shim therefore launches the engine with
`--proxy http://127.0.0.1:<gate-port>` and the gate is a loopback HTTP proxy in
`browser/engine/gate.rs`:

- a PLAIN HTTP forward-proxy request carries the absolute URI, so it is
  authorized on `host:port` **and the full path/query** through
  `EgressGate::authorize_path` - the SAME `PolicySet` the shell tool and the
  MCP transport ask. `PolicyData::egress_deny_paths` is the policy vocabulary
  (literal prefixes) and the baseline's `egress-path-denied` rule decides it;
- a refusal is answered `403 Forbidden` carrying the policy's own rule and prose
  (HTML, escaped), and opens no origin socket - a path refusal now means zero
  origin sockets for plain HTTP, not only a failed delivery;
- a `CONNECT` tunnel (https) is authorized by host:port and then tunnelled: the
  policy sees the destination, not the individual URLs inside TLS. That is a
  transport limit, so it is the reader's choice (next section), not a silent one;
- The CDP `Fetch` seam asks the same path door, so its delivery refusal and
  the proxy's pre-socket refusal cannot disagree about a URL;
- `allowed`/`refused` counters and the last refusal are the test's observable.

### The network mode: Proxied or Strict

`Settings -> Built-in plugins -> Shell -> Browser network` (the Shell card, or
`--settings=shell`; en + zh, default **Proxied**) is the explicit choice the
CONNECT limit requires:

- **Proxied** (default): https is allowed and authorized at host:port; a path
  rule inside an encrypted page is not enforceable and the setting's hint says
  so.
- **Strict**: the CONNECT tunnel is refused before it exists. Navigation
  pre-flight refuses an https URL with the rule `browser-strict-network` and a
  sentence saying TLS is not terminated; the loopback proxy answers a
  `CONNECT` `403` with the same rule, so an https SUBRESOURCE is refused too.
  Plain http keeps working in Strict, path rules included.

The active mode is named in the Browser pane's toolbar
(`browser-network-mode`) and every browser tool result carries
`network_mode` ("proxied" | "strict") beside `sandbox`/`network`. The mode is
handed to the proxy on each navigation, so a change applies to the NEXT
navigation; a request a page starts on its own between the change and the next
navigation still runs under the previously applied mode.
The refusal's user-facing sentence is localized in the pane; the tool payload's
message is the engine's English prose, like every other engine failure.

### Sandbox: the browser never runs under heel

The rendered browser is deliberately OUTSIDE the heel sandbox the shell tool and
the terminal pane use. The engine is a DIRECT child of bsh (not a
`SandboxSpec` consumer), it reaches the network only through its own loopback
egress gate, and every browser tool result states `sandbox: "none"` beside the
network fact. There is no heel profile, no `sandbox-exec` wrapper and no
turn-level sandbox on this path.

One leak was possible and is now closed: a bsh started from a SANDBOXED shell
inherits heel's environment, and heel's `HTTP(S)_PROXY` would sit in front of
(or beside) the browser's own gate while its private `TMPDIR` points at a
scratch the engine was never granted. The launch therefore removes the
sandbox's reserved variables (`RESERVED_ENV` in `agent::sandbox`: the proxy
family and `TMPDIR`/`TEMP`/`TMP`) before spawning Obscura, and a unit test
(`the_engine_launch_removes_the_sandbox_environment`) pins it. One list, two
readers, so the scrub cannot drift from what the sandbox reserves.

Residual, stated: an OS-level sandbox around the bsh PROCESS itself is inherited
by every child it starts, and an unprivileged process cannot leave it. Run bsh
outside heel when the rendered browser needs the network.

The CDP `Fetch` seam is still enabled and every `requestPaused` is handled
(continue, or fail with `BlockedByClient` and the policy's prose recorded), as a
second line: it fails the delivery of anything the policy refuses and it records
the refusal for the pane. It is described as delivery control, not as the gate.

### The proof (live listeners, discrimination)

`cargo test -p bsh --lib browser::engine::tests` with the binary installed:

- **refusal**: navigating to `http://127.0.0.1:<refused-port>/` under a policy
  that allows loopback only on `<allowed-port>` returns `Navigation::Refused`
  whose rule names the port, and the refused origin's LIVE listener stays at
  **0 hits**;
- **discrimination**: the same engine then navigates the allowed URL and the
  SAME listener counter shape moves to **>= 1** - without this the zero would be
  meaningless;
- **subresource**: a page whose `<img>` comes from the refused origin loads its
  allowed parts (text and layout present) while the refused origin stays at
  **0 hits**, `gate.refused() >= 1` and `gate.last()` carries the policy prose;
- **path (plain HTTP)**: the origin records every request path it sees. A page
  at `/` whose `<img>` points at `/secret/pixel.png` loads its allowed parts
  while the recorded path list has the document and NO `/secret/` entry -
  `gate.last().rule == "egress-path-denied"`;
- **path, without an engine**: `browser::engine::gate::tests` drives the proxy
  with a raw `GET` and proves `403` + rule + prose + **0 origin sockets** for
  the denied path and `200` + **1 socket** for an allowed path on the same
  origin;
- **strict**: `NetworkMode::Strict` refuses `https://127.0.0.1/...` with
  `browser-strict-network` before any request, keeps `http` working on the
  same engine, and the proxy refuses `CONNECT` after the mode is switched (the
  `Proxied` gate opened it, proving the switch is what refused it);
- **lifecycle**: after `Drop`, the engine's port refuses connections: the child
  was killed and reaped, not orphaned.

## The module map

    crates/bsh/src/browser/engine/
      mod.rs        the facade: Engine/EngineError/Navigation/Page, FrameSlot
      gate.rs       the loopback egress gate (proxy) + preflight + counters
      reap.rs       the orphan reaper: marker, pidfile, ppid==1 + argv check
      cdp/
        mod.rs      the page vocabulary: navigate, snapshot, click, type, press,
                    history, reload, capture, resize, pointer, scroll, keys
        client.rs   process launch, binary discovery, ready-file/port, target
                    discovery, the ONE socket thread, lifecycle
        wire.rs     JSON-RPC 2.0 frames, typed RpcError, the RFC 6455 socket,
                    base64 for captures
      tests.rs      the live proofs above

`browser.rs` keeps the model and the nine tools UNCHANGED in shape: same tool
names and arguments, same `e1..` refs (monotonic), same caps (4000 text / 60
elements / 6000 html), same payload keys, same stale-ref sentence. Only the
producer changed. Three tool DESCRIPTIONS were corrected because they claimed no
page scripts run - a claim the rendered engine makes false - and the payload
shapes were not touched.

### The CDP method behind each tool

| Tool | CDP |
|---|---|
| `browser_open` | `Page.enable` (once), `Page.navigate`, wait `Page.loadEventFired`, then **networkidle** |
| `browser_snapshot` | `Runtime.evaluate` over the LIVE DOM, stamping `data-bsh-ref` |
| `browser_click` | `Input.dispatchMouseEvent` moved/pressed/released at the element's centre (Range-union fallback; `via: pointer\|script`) |
| `browser_type` | focus + `Input.insertText`; a `select` sets its value and fires `input`/`change` |
| `browser_press` | `Input.dispatchKeyEvent`; the PAGE moves focus |
| `browser_back`/`forward` | `Page.getNavigationHistory` + `Page.navigateToHistoryEntry` |
| `browser_reload` | `Page.reload` |
| `browser_close` | `Target.closeTarget`, then the child |
| pane frame | `Page.captureScreenshot` on a 120 ms pump |
| pane resize | `Emulation.setDeviceMetricsOverride` |

### Refs

Refs are stamped into the page as `data-bsh-ref="eN"` from the browser's
monotonic counter. A second snapshot keeps an element's ref (measured in the
test: the two reads return the same ids), and a new document starts a new range
because the stamps went with the old DOM. A ref that is not on the current
document still gets the model's own stale-ref sentence, because `click_live`
resolves against the model BEFORE it asks the engine.

## Navigation, idle and activation

- **networkidle is real now.** Every request that reaches the Fetch seam bumps a
  monotonic activity stamp; the seam is enabled once at bootstrap and its
  `requestPaused`/`continue`/`fail` path keeps an in-flight counter. A
  navigation is settled when no request has been seen for **500 ms** and at most
  **2** are in flight, capped by the navigation timeout; if the Fetch domain was
  refused, the old load + 120 ms settle is the fallback and says nothing it did
  not measure. Proof: a page whose script starts an `<img>` 300 ms AFTER its
  load event is read immediately after `browser_open` and the snapshot already
  contains the text that image's `onload` wrote (`networkidle_reads_content_a_late_subresource_wrote`).
- **Click hit points**: `getBoundingClientRect()` first, then the union of the
  element's own client rects and a `Range` over its contents, with a
  `scrollIntoView` before the point is read. A pointer event is dispatched
  whenever a non-zero box exists anywhere in that union; `element.click()` is
  kept ONLY for a genuinely boxless target. The result names the path it took
  (`via: "pointer"` or `via: "script"`), for a click and for a focus, so a
  caller can tell a trusted pointer activation from a scripted one.

## Pointer and keyboard input (measured against Baidu)

The rendered page is a BITMAP in the pane, so input is not a fake UI over it:
the pane forwards real CDP `Input` events in PAGE coordinates.

### Pointer

- Press and release are SEPARATE: `mousePressed` on the down event,
  `mouseMoved` on hover and drag, `mouseReleased` on up. Sending
  press+release on the down event (the old behaviour) made every drag a burst
  of clicks and left no way to select text; hover did not reach the page at all.
  Hover is throttled to ~60 Hz because each move is a CDP round trip on the UI
  thread; a drag is never throttled.
- The wheel is `Input.dispatchMouseEvent` `mouseWheel`.
- **Click count is timed by the pane.** A second press within
  `BROWSER_CLICK_MS` (500 ms, the platform double-click default) and
  `BROWSER_CLICK_SLOP` (4 page px) of the first sends `clickCount: 2`; a
  third sends 3 (further rapid presses stay at 3). The count rides BOTH
  `mousePressed` and `mouseReleased`, which is what becomes the page's
  `event.detail` and its `dblclick`; a move beyond the slop resets the
  window, so two ordinary clicks never read as one double click.
- **Word and line selection are supplied by bsh, because the engine does not.**
  MEASURED on Obscura 0.2.5: `clickCount: 2` fires `dblclick` (detail 2) and
  a third press fires a third click, but the DOM selection stays EMPTY - the
  engine implements no default selection for a multiple click, and a drag never
  selects document text either. So on the release of a counted click the pane
  asks the engine to select at that point: `caretRangeFromPoint` for document
  text (the word for a double click, the surrounding block/line for a triple).
  Residual: a double or triple click inside an `<input>`/`<textarea>` selects
  NOTHING, because a click focuses the field but the engine leaves
  `selectionStart` null (MEASURED), so there is no caret offset to expand;
  bsh will not guess one and select the wrong word.
- **The pointer is captured for the length of a drag.** The pane records that a
  press began inside the page frame. While it is held, the window's own move
  handler forwards the CLAMPED page point after the pointer leaves the frame
  (the frame's own hit-test handler stops at its edge), and the window-level
  mouse-up - including a release outside the window, which macOS still delivers
  to the window that took the press - sends a final `mouseMoved` plus
  `mouseReleased` at the clamped edge and clears the flag. Without it a drag
  released over the sidebar or outside the window left the page's own drag
  state pressed forever. A release that is lost entirely is recovered from the
  next move that arrives with no button held.

### Keyboard

- Keys: Enter, Tab/Shift+Tab, the four arrows, Backspace, Delete, Home, End,
  PageUp, PageDown and Escape become `Input.dispatchKeyEvent`. Enter also
  dispatches a CHAR event, because a page's keypress/submit handler listens for
  that rather than the DOM keydown alone.
- **Printable characters are a real key sequence.** One printable character
  sends `rawKeyDown` + `char` + `keyUp`, with the US-keyboard identity a
  browser uses (`key`, `code`, `windowsVirtualKeyCode`,
  `nativeVirtualKeyCode`) and the modifier mask. The character is inserted
  ONCE, by the `char` event; `Input.insertText` is not also called by this
  path, so a page listening for `keydown` sees `a`/`b`/`c` AND the field
  reads `abc`, not `abcabc` (the input handler below recognises and drops
  the platform's echo of the same key).
- Composed text - CJK/IME output, an emoji, anything with no US-keyboard
  identity - still goes through `Input.insertText`, the composition route;
  inventing a key identity for it would lie to the page.
- **An IME commit has a route now.** The page frame installs an input handler at
  paint (the same `ElementInputHandler`/focus seam the terminal pane uses), and
  a committed composition is inserted with `Input.insertText`. Because the
  platform also reports an ordinary key's character to that handler, the pane
  remembers the character it just typed: an identical single-character commit
  is the echo and is dropped (otherwise every typed character doubled -
  MEASURED), while a different commit supersedes it (the typed key is removed
  with a Backspace, then the commit is inserted). Which surface receives a
  commit is decided by FOCUS, so the terminal and the page never both take it.
- **Editing chords are forwarded with the protocol's modifier bits** (Alt 1,
  Ctrl 2, Meta 4, Shift 8) as `rawKeyDown` + `keyUp`: Cmd/Ctrl+C, +V, +X,
  +A, +Z, +Shift+Z (redo) and +F. On macOS Cmd is Meta; Ctrl elsewhere is Ctrl;
  both come from the keystroke's physical modifiers, so there is one path.
- **Paste, route stated.** The pane reads the system clipboard ONLY when the
  reader presses the paste chord, and hands that string to the engine. The
  engine dispatches a `paste` event carrying it as `clipboardData` (a
  synthetic event - `isTrusted` is false, which a CDP client cannot fake) and
  inserts the SAME text through `Input.insertText` only when the page did not
  prevent default. One source of truth, one insertion; a page that owns the
  paste prevents default and inserts its own text, and nothing is injected at
  any other moment.
- **Copy and cut** dispatch the chord and the page's own `copy`/`cut` event
  (with a `clipboardData` object the page may write), then the pane reads the
  selection - the page's clipboard text if its handler wrote one, else the
  field's `selectionStart..selectionEnd` or `window.getSelection()` - and
  writes it to the SYSTEM clipboard. Cut removes the field selection and fires
  an `input` event.
- **Select-all** dispatches the chord, then selects the focused field's whole
  value or the document (`setSelectionRange` / a Range over `body`): Obscura
  implements no default action for a chord. MEASURED: CDP's `commands:
  ["selectAll"|"paste"]` is IGNORED by this build, and
  `document.execCommand` is a stub that returns false for
  selectAll/copy/cut/delete/undo.
- **Undo and redo are forwarded only.** The engine has no editing history and
  bsh will not keep a second copy of the page's text, so a page that implements
  undo sees the chord while a plain field's text does not change. +F is
  forwarded too; this pane has no find bar.

MEASURED 2026-10-25 in the real window against <https://www.baidu.com/>
(reproduce the page capture with
`BSH_PROOF_URL=https://www.baidu.com/ crates/bsh/scripts/browser-proof.sh`;
input is posted through CoreGraphics and System Events, so the app received real
input, not a test hook): clicking the search box focused it; typing `rust` put
the query in the box exactly once and opened Baidu's suggestion list; Backspace
removed the final `t`; Cmd+A then Cmd+C put exactly `rus` on the SYSTEM
clipboard (checked with `pbpaste`), so the chord, select-all and copy paths run
end to end in the real app. A drag across the field changed the page's own
state, and Enter triggered Baidu's search (an earlier sitting). The page capture
path is `ocu call get_app_state` -> a base64 PNG in the JSON reply -> decoded to
a file (the same path `browser-proof.sh` uses); the click and keystroke posting
is a scratch harness (CoreGraphics `CGEventPost` plus System Events
`keystroke` / `key code`), not a repo script, because it hard-codes the window
layout.

CJK, stated plainly rather than claimed: a CJK string posted as a synthetic
Unicode keystroke under the ABC layout types the KEYCAP character (`中` arrives
as `a`), because GPUI derives `key_char` from the physical keycode and
discards the event's Unicode string. That is pre-existing, not a regression of
this round (the old `insertText` path typed the same keycap character). With a
real input method selected, GPUI routes printable keys into the IME and the
frame's input handler forwards the COMMITTED composition text to the page; that
path is wired but the harness could not drive a candidate commit reliably
(switching input sources mid-session left the synthetic events unobservable), so
it is reported as unproven in this round, not as working.

The deterministic proofs are engine tests (each SKIPS cleanly with no binary):

- `editing_chords_reach_the_page_and_paste_inserts_once` - the page's keydown
  log carries `a+meta`, `c+meta`, `v+meta`, `x+meta`, `z+meta+shift`;
  copy returns the selection; paste carries the clipboard text in its event and
  the field holds it once; cut removes it;
- `printable_keys_fire_keydown_and_insert_exactly_once` - the keydown log is
  `a,b,c` and the field is `abc中`, so no character is doubled and composed
  text still lands;
- `a_double_click_selects_the_word_and_a_triple_click_the_line` - the page's
  `mousedown` details are `[1, 2, 3]` and `window.getSelection()` is
  `beta` after the second click and `alpha beta gamma` after the third, while
  a single click selects nothing;
- `a_release_at_the_clamped_edge_clears_the_page_drag` - the page reports
  `dragging`, then one release at the clamped `(799, 599)` clears it, with no
  move after the release.

The pane-side timing and mapping are unit tests too:
`a_press_counts_double_and_triple_clicks_by_time_and_slop`,
`a_move_beyond_the_slop_ends_the_click_window` and
`an_outside_release_maps_to_the_page_edge_only_when_clamped`.

A measurement note: `ocu call click` cannot drive this app. The bare binary
is not registered with LaunchServices, and the click tool answers
`appNotFound("bsh")`. The events above were posted with CoreGraphics
(`CGEventCreateMouseEvent` / `CGEventPost`) and the keyboard went through
System Events (`keystroke`, `key code`), which is real input to the app.
`ocu call get_app_state` still works for captures.

## The pane

- The rendered view is **pre-warmed**: opening the Browser tab (or the
  `--browser=` deep link) starts the engine then, not on the first Go. The
  capture pump is LAZY - it starts on the first `set_streaming(true)` - so a
  pane that is shown and never used has no pump thread and no window; closing
  the pane stops the process. MEASURED here: launch 163-234 ms, then a first
  navigation on the warm engine 538-540 ms of which 500 ms is the networkidle
  window (protocol + settle ~40 ms). Before pre-warm the same first open paid
  launch (163-185 ms) + navigation (159-163 ms, including the old 120 ms settle);
  pre-warm removes the launch from the navigation, and the idle window is the
  correctness cost of the section above.
- `Page.captureScreenshot` runs on the engine's own pump at ~8 fps; the JPEG is
  decoded to RGBA ON THE PUMP THREAD and the GPUI thread only builds a
  `RenderImage` from bytes that already exist (no decode on the render path).
- The one channel transform is at the GPUI boundary: `RenderImage` wants BGRA
  and a capture is RGBA. MEASURED: handing RGBA over unchanged draws a blue
  heading orange and a red box blue - a byte-order bug that looks like a
  renderer bug.
- Pointer presses/moves, the wheel and typed keys are forwarded in PAGE
  coordinates (window position minus the frame's laid-out bounds, scaled by
  viewport/geometry); a press takes keyboard focus for the pane.
- The viewport follows the pane's geometry
  (`Emulation.setDeviceMetricsOverride`) and an invisible tab stops the pump
  entirely.
- The text/DOM view remains the fallback: without a binary, or with
  `BSH_BROWSER_ENGINE=off`, the pane draws it and says why in the engine's own
  words (`browser-engine-<kind>`), never a blank rectangle.

## The windowed proof (one command)

`crates/bsh/scripts/browser-proof.sh` is the documented dev path. It builds the
shell, writes the fixture page (blue `#4176e6` heading, red 200x80 box, the
script's text, Chinese glyphs, a link/button/input), starts a loopback fixture
server on a free port, launches the shell at it with a SCRATCH `HOME` under
`target/`, waits for the engine's pidfile, captures the window through
`ocu call get_app_state`, and re-runs the engine capture/assertion:

    crates/bsh/scripts/browser-proof.sh
    # target/bsh-obscura-proof.png   the engine image, ASSERTED non-blank with the
    #                                blue heading and red box
    # target/bsh-browser-window.png  the real GPUI window, evidence to inspect

    # A real site instead of the fixture (no server is started):
    BSH_PROOF_URL=https://www.baidu.com/ BSH_PROOF_SKIP_ENGINE_TEST=1 \
      crates/bsh/scripts/browser-proof.sh

What it does NOT do, deliberately: assert the window's pixels. The harness
cannot photograph its own GPUI surface, so the automated assertion stays on the
engine image and on the live-DOM checks; the window capture is reproducible
evidence for a human re-running the proof, not a pixel test.

## What was deleted (Stage 1)

- `crates/bsh/src/browser/servo_host.rs`, `servo_gate.rs`, the whole
  `browser/engine/{client,host,protocol,wire}.rs` host tree and
  `src/bin/bsh-servo-host.rs`;
- the `servo-render` feature, its eight optional dependencies, the
  `bsh-servo-host` binary, both `[patch.crates-io]` entries and `vendor/`
  (`zed-font-kit`, `tree-sitter-sequel`);
- the shell's `ServoPane` and the pane's `servo_body`, and the `servo` field.

Package counts: `Cargo.lock` **1496 -> 1227** packages (-269). The default
`cargo tree -p bsh` package set is **834 before and after** (Servo was
optional, so its graph never entered the default build), and
`cargo tree -p bsh | grep -c servo\|mozjs` is **0**.

One resolver fact found on the way: `tree-sitter-sequel` (pulled by
`gpui-component`) pins `cc ~1.2.1` while `embed-resource` (pulled by
`gpui-pre`) wants `cc ^1.2`, and a build-dependency is unified to ONE
version. The lock is pinned to `cc 1.2.62` (`cargo update -p cc --precise
1.2.62`), which satisfies both, so neither vendor patch is needed.

## Honest gaps (updated this round)

Fixed this round: **path-level egress for plain HTTP** (GAP 1), the **network
mode choice** for the CONNECT limit (GAP 2), **click hit-point discrimination**
(GAP 3, partially), **networkidle** (GAP 4), **engine pre-warm** (GAP 5),
**orphan reaping** (GAP 6) and the **one-command windowed proof** (GAP 7).

The next round closed the four INPUT gaps: the **editing chords** (I1), a
**printable keydown** for every printable character (I2), the **timed click
count with the word/line selection the engine does not do** (I3), and the
**captured pointer release** for a drag that leaves the pane or the window (I4).
Its new residuals, stated rather than assumed:

- a multiple click inside an `<input>`/`<textarea>` selects nothing: the
  engine leaves `selectionStart` null after a click (MEASURED), so bsh has no
  caret offset to expand to a word and refuses to guess;
- undo/redo are forwarded but not implemented for a plain field (the engine has
  no editing history and `document.execCommand` is a stub, MEASURED);
- IME COMPOSITION events are not synthesized: committed CJK text (when a real
  input method delivers it) reaches the page through `Input.insertText`, so a
  page that only reacts to `compositionstart`/`compositionupdate`/
  `compositionend` does not see a composition. A synthetic Unicode keystroke
  still types its keycap character, because GPUI discards the event's Unicode
  string (MEASURED); the real-IME commit route is wired but was not proven end
  to end in this round;
- the page's `paste`/`copy`/`cut` events are SYNTHETIC
  (`isTrusted: false`); their `clipboardData` behaves like the real object,
  but a page that reads `event.isTrusted` can tell;
- moves are not delivered once the pointer is outside the whole application
  window (macOS does not route them to a hit test there); the release IS, so
  the gesture still ends;
- text selection by dragging is still the engine's own, and the engine
  implements none (MEASURED: a drag across document text leaves the selection
  empty). Word/line selection is the counted-click path above.

1. **`Fetch` is still post-fetch** (measured above); it remains delivery
   control. The SOCKET guarantee for a path refusal is the proxy, and it now
   covers plain HTTP fully: `host:port:path` is asked before connecting.
   **Residual:** the human-facing `.bsh/egress-allowlist` file has no PATH
   syntax yet, so a path rule is expressible through `PolicyData`
   (`with_egress_deny_paths`) and the preset/compile seam, not by typing a line
   in that file. No syntax was invented for it in this round.
2. **`CONNECT`/https is gate-at-host:port, and TLS is NOT terminated.** This is
   not fixable without a MITM CA the reader must trust, which this round did not
   add. It is now the reader's explicit **choice**: `Proxied` accepts the
   host:port granularity for https, `Strict` refuses the tunnel with the rule
   named. Residual risk in Proxied: a path rule inside an https page is not
   enforced at the socket; the pane and the setting say so.
3. **An inline `<a>` still has no usable box in Obscura 0.2.5.** MEASURED after
   the fix: the fixture's inline link reports zero `getBoundingClientRect()` AND
   zero `getClientRects()`/`Range` rects, so `via` is `"script"` there and the
   DOM's own `click()` is what runs. The fix that IS in place: scroll into view,
   element box, then client-rect/Range union, and a synthesized pointer event
   whenever any non-zero box exists; `via` reports which path ran, so a caller
   can tell. Residual: for this shape the engine gives no point to hit.
4. **networkidle is implemented** (500 ms quiet, <= 2 in flight, capped; load +
   settle fallback when the Fetch domain is unavailable). Residual: the
   `requestPaused` event arrives AFTER the fetch in 0.2.5 (measured), so the
   counter observes activity late; the idle STAMP is what makes the wait real,
   and the 500 ms window is a bounded guess rather than an event.
5. **No CJK without fonts** unless the font directories exist on the host;
   `BSH_OBSCURA_FONT_DIRS=none` reproduces the tofu. Unchanged.
6. **The engine still cold-starts**, but a shown pane PRE-WARMS it: the launch
   (~0.16-0.23 s measured here; the 2-3 s figure is a cold filesystem) happens
   when the Browser tab appears, not on the first Go. Residual: a
   `--browser=URL` deep link navigates on the same frame it shows the pane, so
   that first-ever open still shares the launch; and the 500 ms networkidle
   window is now part of every settled navigation.
7. **Windowed capture is reproducible, not asserted.** `browser-proof.sh`
   re-captures both images in one command, but no test asserts the GPUI window's
   pixels: the harness cannot photograph its own surface. The assertion stays on
   the engine image and the live-DOM checks. Unchanged, and stated rather than
   faked.
8. **Orphan reaping is deliberately narrow.** Every child is launched with
   `--storage-dir <data>/bsh-obscura-<pid>-<nanos>-<seq>` and a pidfile at
   `<data>/browser/engines/<marker>.pid` records `<pid>\n<marker>`. A child is
   killed only when its pidfile parses, its marker matches, its parent is 1, and
   its OWN argv (read from `ps`) carries the marker. Residual: an orphan reparented to a supervisor
   rather than to 1, or one whose argv cannot be read, is left alone - that is
   the "never kill what we cannot identify" rule, not an oversight.
   MEASURED on 0.2.5: the engine IGNORES SIGTERM (a manual `kill <pid>` left it
   alive), so the reaper's TERM-then-SIGKILL escalation (2 s grace) is what
   actually stops it. This was proved end to end: killing a proof-run shell with
   SIGTERM left a real obscura child at ppid 1 with its marker and pidfile, and
   the NEXT `bsh --browser=...` launch logged
   `reaped 1 orphaned browser engine process(es): [pid]` and the process was
   gone.

## Servo: removed

The previous revision of this file records the Servo attempt: the in-process
engine faulted (SIGSEGV in `SystemFontService::fetch_font_keys_if_needed`, then
a host-process crash in the bsh link graph), the build needed a wrapper C
toolchain and two vendor patches, and the graph was 2,276 packages with
SpiderMonkey. None of it is in the tree now.