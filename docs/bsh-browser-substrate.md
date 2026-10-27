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

## Tests

Browser integration tests use the installed Chrome/Chromium binary and a
direct local fixture origin. They skip with a clear message if no browser is
installed. Set `BSH_CHROMIUM_BIN` when the executable is outside the standard
search locations.
