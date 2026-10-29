# Browser substrate: Chromium over CDP

The BSH Browser pane uses the installed Google Chrome or Chromium executable.
It starts a private headless browser process and controls it through the
standard Chrome DevTools Protocol (CDP). No browser engine is linked into the
Rust build.

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
