# Browser substrate: a native view for the pane, Chromium over CDP for the tools

Two consumers, two backends, because they need different things.

- **The pane** renders a native web view: `gpui-webview` hosts Wry in the GPUI
  window - WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux
  (X11/XWayland only). The platform composites the page, so scrolling, text
  selection, keyboard, IME, links and video are the platform's own. Nothing is
  captured, forwarded or re-encoded.
- **The agent tools** (`browser_navigate`, `_snapshot`, `_click`, `_type`,
  `_press`) drive the installed Chrome/Chromium over CDP, because a page an
  agent reads and clicks needs a programmable protocol, not a picture.

## The pane's native view

The view is built lazily, from a deferred task on a frame with a live window
(`browser_ensure_webview`), and only once the pane has an address: a native
view on `about:blank` would cover the pane's empty state with a white
rectangle. `sync_browser_webview` keeps it sized to the pane and visible only
while the Browser tab is the one showing.

A native child view is drawn ABOVE the GPUI tree on this backend, so it covers
popovers, dialogs, menus and tooltips in its bounds, and it cannot be clipped.
The pane hides the view while an overlay (the settings dialog) is open, which
is what keeps that dialog visible. A pane that never navigates never builds the
view at all, so the cost is paid by the pane that is used.

The view is given a REAL browser user agent, per platform. Wry's default is a
sparse `AppleWebKit/605.1.15` with no `Safari` token, and sites that sniff it
serve a stub: MEASURED, baidu.com answered with `readyState: complete` and NO
`document.body` at all (so its `#kw` search field did not exist), while the same
view with a Safari-style UA loaded the real page (`百度一下，你就知道`, body
445 KB) and the full search flow worked: typed `Python` into `#kw`, clicked
`#su`, and the results page arrived as `Python_百度搜索` with `#content_left`.

A link that asks for a NEW window (`target="_blank"`, `window.open`) is a
separate case: the pane has one page, so Wry's new-window handler DENIES the
window and records the URL, and the pane loads it in its own view. Without
that, every search result looked dead - MEASURED on Baidu, whose results are
all `target="_blank"`: clicking one did nothing at all, while the same click
with the handler in place landed on the destination
(`https://python.bd.cylsoft.cn/?...`).

The address bar navigates on ENTER as well as on the Go control: the field's
`PressEnter` is what a reader actually uses, and both routes call
`browser_submit`, which normalizes a bare host to `https://` and then takes the
pane's path. The FIRST navigation is special: with no view built yet, the clock
must not start Chromium or fetch the text - the render builds the native view
from the address the Enter just set. Before that rule the first Enter launched
the CDP engine, and where it could not start the pane fell back to the text
view, which is what made the key look inert.

The address bar follows the page through Wry's navigation handler, which
reports every navigation - a link click, a redirect, a history move. This is
deliberately NOT `WebView::url()`: that unwraps a nil `NSURL` and ABORTS the
process when the view has no committed URL yet (MEASURED: polling it from the
pane clock killed the app while a page was still loading).

The headless test suite keeps the text/capture path (`cfg!(test)`), because a
native view cannot be exercised in the test harness; the view itself is verified
by running the app.

**Do not enable `gpui-fast`.** `gpui-webview`'s default features turn on
`gpui-kit/gpui-fast`, which SWAPS the whole GUI backend: it compiled and ran,
but `gpui_kit::TestAppContext` disappears, so the entire test suite stops
compiling. The dependency is pinned to `default-features = false` for that
reason. On Linux the application must also start on X11 (see the crate's
guide); `WebView::build` refuses a Wayland window.

## The agent tools drive the pane's own view

The tools talk to the SAME page the reader sees. The view is `!Send` and lives
on the UI thread, so `browser/script.rs` is the bridge: a tool queues an
`Eval` or `Load` command and blocks on its reply, and the pane's own clock
drains the queue into `evaluate_script_with_callback` / `load_url`. Nothing is
re-implemented per backend: the tools run the SAME snapshot script the engine
uses, so refs, element kinds and payloads are identical.

A tool's sequence is therefore: navigate (the view loads, the tool waits for
`readyState === "complete"` on a page that moved), snapshot (the script reads
the live DOM and stamps refs), then type/click/press. Two behaviours are worth
stating because they cost a measurement each:

- `browser_type` records the focused ref AFTER it re-reads the page, because
  the read replaces the document with the page's own `focused` and would
  otherwise clear the field the call had just focused - which made the
  documented `browser_press Enter` refuse with "no element is focused".
- `browser_press Enter` waits up to 3 s for the URL to move: a form submits
  asynchronously, and sampling immediately reported the focus payload while
  the page had already left (MEASURED on Baidu, `rsv_enter=1` arrived a
  fraction of a second later).

VERIFIED on Baidu through these functions (a temporary probe, since the
headless suite cannot host a native view): `browser_snapshot` returned 60
element refs around the real page (title `百度一下，你就知道`, 615 characters of
text), `browser_type` stored `Python` into the search field, `browser_click`
activated a submit ref (`via: "script"`) and the page navigated to a `/s?wd=`
result URL, and `browser_press Enter` navigated with the same shape.

Chromium over CDP remains the fallback for a handle with NO view - the
headless test suite, or a pane that was never opened - so the tool surface is
unchanged when there is no native view to drive.

### The runner seam, and the coverage it buys

`ScriptRunner` is the seam (`browser/script.rs`): the pane supplies the queue
that the native view drains, and the TEST harness supplies `EngineRunner`,
which answers the same `Eval`/`Load` commands through Chromium over CDP. That
is what makes the tool path testable without a native window:
`the_tools_drive_a_page_through_an_injected_runner` runs `load`,
`snapshot_live`, `type_live` and `click_live` against a local page - the real
JavaScript, refs and payloads - and asserts that the page's own click handler
ran.

That test earned its keep immediately: it caught a REF-STABILITY bug. The
snapshot script used to re-stamp every element whose ref was below the floor,
so the re-read that `browser_type` performs moved every ref, and the documented
`browser_type` then `browser_click` sequence was refused as stale (MEASURED on
Baidu as well). The script now KEEPS an element's existing ref and mints ids
above the floor only for unmarked elements, which is what mints a fresh range
after a NAVIGATION and leaves it alone on a re-read.

### What the page sees, and what it does not

Wry exposes no input dispatch, so on the native view a tool's input is
DOM-synthetic: `browser_click` uses `element.click()` and reports
`via: "script"`, `browser_type` sets the value and fires `input`/`change`,
and `browser_press` dispatches `keydown`/`keypress`/`keyup` and calls
`form.requestSubmit()` for Enter. Those events are NOT `isTrusted`: a page
that checks `event.isTrusted` can tell them apart, and a page that only reacts
to real OS input will not see them. The CDP path still has real input dispatch,
so a handle with no view keeps that behaviour.

A snapshot lists elements in DOM order with their kind and label; choosing
which ref to act on is the caller's job. On a page with promotional or
trending links (Baidu's home page is one) several refs look plausible, and the
tool acts on the one it is given.

## Discovery

Set `BSH_CHROMIUM_BIN` to the browser executable to override discovery.
Otherwise BSH looks for `chromium`, `chromium-browser`, `google-chrome`,
or `google-chrome-stable` on `PATH`. On macOS it also checks the standard
Google Chrome and Chromium application paths.

If none is found, the Browser pane reports the searched locations and asks for
Chrome or Chromium to be installed.

## The sandbox fallback

The first launch keeps Chromium's own process sandbox, which is the guard a
process rendering untrusted web content should have. Some hosts cannot give it
what it needs - a container, a CI runner, a sandboxed harness - and there
Chromium's sandbox fails to initialize and its GPU and network services exit
(MEASURED: `sandbox initialization failed: Operation not permitted`, `GPU
process exited unexpectedly: exit_code=6`, then the CDP websocket is reset).
Without a fallback the pane could only report a closed socket.

So a launch that fails this way is retried once with `--no-sandbox
--disable-gpu-sandbox --disable-dev-shm-usage`. The retry is not the default:
a normal desktop keeps the sandbox, and the fallback is remembered for the
rest of the process so the next pane opens directly in the mode that works.

## Process and network

Each pane starts Chrome with its own temporary profile, an ephemeral CDP port,
and headless rendering. The selected port is read from Chrome's
`DevToolsActivePort` file. BSH attaches to the browser websocket, creates or
attaches to a page target, then enables Page, Runtime, and Fetch domains.

Web pages and their resources load directly from Chrome. BSH does not start or
use a loopback proxy. The loopback CDP socket is only the local control
connection between BSH and its own browser process. The browser profile is
removed when the engine shuts down.

Flattened CDP events carry a target session id. Fetch requests are continued
immediately on their originating session, so navigation tracking does not
intercept or rewrite page traffic.

## Rendering and media

The capture pump probes `Page.captureScreenshot` in JPEG, compares successive
captures as text, and publishes only changed frames, dropping to the idle
cadence once a page becomes still. While video is playing, it keeps the motion
cadence so movement remains visible in the pane. Chromium supplies native DOM
rendering, text selection, keyboard and pointer input, and media decoding.

Three measured facts shaped the pump (480x744@2x, Chrome 156, macOS):

- One capture costs ~50 ms and is CONSTANT: it does not shrink with the
  format, the scale, or the region captured (`clip` included), and
  `--disable-gpu` does not move it. The cost is the compositor readback round
  trip, not the pixels.
- What follows the round trip is NOT constant: JPEG decodes in ~8 ms where
  PNG needs ~17 ms, and the gap widens with the viewport (40 ms vs 26 ms at
  960x1488@2x). That is why the probe is JPEG - the frame is compared as text
  and shown as a screen capture, so nothing needs losslessness.
- The ~50 ms is a per-request queue wait, not work: with two captures in
  flight the effective cost halves (~25 ms each) and eight in flight cost
  ~10 ms each. The pump therefore keeps `PIPELINE_DEPTH` (2) captures
  outstanding - the cheap part of that curve; more depth buys little and is
  another frame of staleness between a change and the pane showing it.

The decode runs on its own thread (`bsh-browser-decode`), so the pump starts
its next capture as soon as the bytes are handed over: the cycle is
`max(capture, decode)` instead of `capture + decode`. A hand-off that finds
the decoder busy is DROPPED, and the pump only advances its "what the pane is
showing" marker when the hand-off was accepted - otherwise a dropped frame
would be remembered as displayed and the pane could settle on an image it
never received.

`Page.startScreencast` is deliberately NOT the frame source. MEASURED on this
engine, it acknowledges its start and then misses paints: a typed key published
NO frame (a search box showed one character while every keystroke reached the
page), and a page animating at 25 fps was published at ~10 fps. A capture probe
answers every change, and the activity stamp (`Shared::activity`) is what makes
an input, a navigation or a resize publish without waiting for the idle cadence.

## Input

The pane maps the window's coordinates into page coordinates and forwards each
event as CDP input; the page handles selection, focus and its own scrollers.

Input is fire-and-forget (`fire: true`) everywhere, so no CDP round trip runs on
the UI thread: a keystroke, a wheel tick or a drag move is queued and the next
frame continues immediately. The exceptions are the clipboard chords, whose
answer is the text being copied, and the tool-driven activation path.

The held CHORD rides every mouse and wheel event (`modifiers`, in CDP's bits),
which is what a page reads as `shiftKey`/`ctrlKey`/`metaKey`: Shift+click
extends a selection instead of replacing it, Alt+click and Cmd/Ctrl+click reach
the page's own handler, Shift+drag keeps extending, and Ctrl/Cmd+wheel is the
zoom a map or an editor implements itself. A drag that leaves the frame keeps
its chord too, because the pane stores it with the rest of the gesture state.

A keystroke is forwarded the way a browser dispatches it: `rawKeyDown`, then a
`char` event, then `keyUp` for a printable key (so both the page's `keydown`
handler and its field see one character), and `Input.insertText` for text with
no US-keyboard identity - a composed CJK character, an emoji. Tab and
Shift+Tab are forwarded as real `Tab` keys, so the DOCUMENT's focus order
walks rather than the shell's.

An IME commit is text the platform also hands the pane's input handler, so the
two routes are deduplicated: the last text the key path inserted is remembered
and its echo is dropped. MEASURED, without that a two-character commit landed
TWICE (`你好` in a field read `你好你好`); a single-character echo was already
handled, which is why the defect only showed on composition input.

Two sign and scale conventions are worth stating, because getting either wrong
is a silent behaviour bug rather than an error:

- A window wheel delta moves the CONTENT (positive scrolls the reader up) while
  a CDP `mouseWheel` moves the VIEWPORT (positive `deltaY` scrolls the page
  down), so both axes are negated before forwarding. Both axes are forwarded:
  a horizontal wheel reaches a horizontally scrolling page.
- A mouse notch arrives in whole lines and is gained (the transcript's own
  `MOUSE_GAIN`) so one detent is a comfortable step; a trackpad's precise pixel
  delta passes through untouched because its hardware already carries momentum.

A press forwards the button it was made with. CDP names the button in `button`
and reports the held set in `buttons` (left 1, right 2, middle 4), so a right
press reaches the page as a secondary click and its `contextmenu`, and a middle
press as `auxclick`. One button is held at a time.

A drag move carries BOTH: `button` names the button being dragged with and
`buttons` is the held set. That pair is the whole selection mechanism, and it is
not optional - MEASURED, a move with `button: "none"` and `buttons: 1` reads as a
hover, so Chromium selects nothing while dragging and nothing after the release,
while the same drag with `button: "left"` selects the crossed text, works across
block boundaries, and selects inside a textarea. The page's own default
selection is therefore what the pane relies on: the pane does not synthesize a
range from carets.

The click count rides the press AND its release, because that is what becomes
the page's `event.detail` and its `dblclick`. The count is the platform's
(`MouseDownEvent::click_count`), measured in window coordinates against the
real double-click interval, so a viewport resize between two presses cannot
reset it; the pane does not time it itself. Chromium turns that count into its
own default selection - `1` places the caret, `2` selects the word, `3` the
line/block - so no script supplies the word or the line either. A triple click's
native string keeps the block boundary's newlines, which is why a test compares
it trimmed.

Keys carry their physical modifier mask instead of folding Shift into the key
name, so Shift+Arrow extends a selection and Cmd/Ctrl+Arrow jumps a line.
Composed characters are typed: macOS `Option`+`a` produces `å` and is forwarded
as that character, while an Option chord whose character equals its key is an
accelerator and is not typed. Control and Cmd chords belong to the page or the
application, never to the text path.

## Tests

Browser integration tests use the installed Chrome/Chromium binary and a
direct local fixture origin. They skip with a clear message if no browser is
installed. Set `BSH_CHROMIUM_BIN` when the executable is outside the standard
search locations.
