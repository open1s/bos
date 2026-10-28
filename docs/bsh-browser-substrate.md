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

The capture pump compares successive PNG captures, publishes changed frames,
and reduces capture frequency after a page becomes still. While video is
playing, it keeps the fast capture cadence so motion remains visible in the
pane. Chromium supplies native DOM rendering, text selection, keyboard and
pointer input, and media decoding.

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
