# Browser substrate: Servo

Status: decided, blocked on the host toolchain, not on design.
Measured 2026-10-24 on this machine (rustc 1.98.1, CommandLineTools SDKs
MacOSX26.5 / MacOSX27.0).

## What the user asked for

Servo (https://github.com/servo/servo) as the built-in browser's substrate -
a real engine instead of the shipped text/DOM view.

## The substrate exists and its API is the right shape

servo 0.7.0 is published on crates.io (rust-version 1.88) and is genuinely
embeddable. Its tree is 2,276 packages; the parts an embedder needs are all
exported from the crate root:

- SoftwareRenderingContext::new(PhysicalSize) -> Result<Self, Error>: a
  headless software-GL context. No window, no display handle, no surfman
  surface of our own - exactly what a toolkit that paints its own pixels needs.
- RenderingContext::read_to_image(DeviceIntRect) -> Option<RgbaImage>: the
  frame, as pixels.
- ServoBuilder::default().event_loop_waker(..).protocol_registry(..).build()
  plus Servo::spin_event_loop().
- WebViewBuilder::new(&servo, context).url(..).delegate(..).build(),
  WebView::paint(), and WebViewDelegate::notify_new_frame_ready.
- ProtocolRegistry (custom scheme handlers) and submit_resource_reader
  (embedder-side responses).

GPUI's side is ready too: gpui::RenderImage::new(Frame),
ImageSource::Render(Arc<RenderImage>) and gpui::img(..) exist in
gpui-pre 0.3.9, and Servo hands back an image::RgbaImage - a direct
conversion. The frame path is therefore:

    SoftwareRenderingContext (headless)
      -> webview.paint() on notify_new_frame_ready
      -> read_to_image(rect) -> RgbaImage
      -> RenderImage::new(Frame::from(rgba))
      -> img(ImageSource::Render(..)) inside the Browser pane

and the clock is spin_event_loop() driven from a GPUI task, with
EventLoopWaker::wake requesting the next one.

## The blocker: SpiderMonkey does not build here

servo 0.7.0 depends unconditionally on mozjs / mozjs_sys 153.3.0, and its build
script dies in configure's compiler probe:

    ld64.lld: error: could not load TAPI file at
      /Library/Developer/CommandLineTools/SDKs/MacOSX27.sdk/usr/lib/libSystem.tbd
    libSystem.tbd:4:20: error: unknown architecture
      targets: [ x86_64-macos, x86_64-maccatalyst, arm64e-macos, arm64e-maccatalyst ]
    clang: error: linker command failed
    mozjs_sys-153.3.0-1/build.rs:289: assertion failed: result.status.success()

The SDK's TAPI stubs list the arm64e slice; the ld64.lld this toolchain ships
cannot map arm64 onto it. Reproduced twice - default SDK, then
SDKROOT=.../MacOSX26.5.sdk - in a scratch project outside this repo
(servo = { version = "0.7", default-features = false }, 579 crates compiled
before the failure). It is a host toolchain/SDK mismatch: nothing in bsh
reaches it, and no Cargo flag reaches a configure probe that runs before the
crate compiles.

Workarounds, all host-level (none is a repo change):

1. Install full Xcode and select its SDK, whose Apple ld understands modern
   TBDs (sudo xcode-select -s /Applications/Xcode.app), or
2. use a rust-lld new enough to read arm64e TAPI slices, or
3. point mozjs's configure at Apple's linker instead of -fuse-ld=lld.

The optional-dependency route was considered and rejected: an optional = true
dependency still adds the whole graph to Cargo.lock for every build, and the
feature cannot compile here, so it would poison a shared working copy without
producing a single rendered page.

## What a Servo round must do, in order

1. Land servo as an optional dependency behind a servo feature and prove
   cargo check -p bsh --features servo on a host whose linker passes the step
   above. Nothing lands until that command is green.
2. crates/bsh/src/browser/servo_host.rs: one SoftwareRenderingContext, one
   Servo, one WebView, a waker that posts a GPUI task, spin_event_loop() on
   that task, paint() + read_to_image on frame notification, and a RenderImage
   published into BrowserHandle (the pane's existing revision counter already
   repaints on change).
3. Policy, before the first page. Servo's own fetcher must not run. Route every
   request through agent::policy::fetch (the one gated GET shipped this round)
   with a ProtocolRegistry handler, and refuse the round if a request can be
   observed bypassing it. A rendered page that skips the egress gate is worse
   than the text view.
4. Refs: replace parse_document's element table with Servo's accessibility tree
   (forward_accessibility_action; accesskit is re-exported), keeping the ref
   format eN, the monotonic ids and the stale-ref sentence the tools already
   return.
5. Input: forward pointer/key events through WebView::notify_input_event, and
   keep the text view as the fallback for hosts where the feature is off.

Until step 1 is green on the host, the pane keeps the text/DOM view - the only
honest surface this crate can build and test here.


## The reference's own solution (measured)

DSH ships the browser as its own package:
@deepseek-ai/dsh-client-ui-sidebar-browser (61 KB client.js; README + en/zh
locale in the asar). Measured from that README, which is DSH's own design note:

| Axis | DSH's solution |
|---|---|
| Carrier | Desktop: an Electron webview tag (native history, retained pages). Web: a sandboxed iframe with app-managed history. |
| Protocol policy | HTTP/HTTPS only, loopback included; rejects file:, script/data/blob, embedded credentials, the app origin, malformed addresses |
| Toolbar | Back, Forward, Reload, Go, Open in system browser; the observed page title |
| Lifecycle | Browser tabs declare keepMounted: the page survives tab changes, Session switches, collapse and floating. After a restart it shows the saved title and URL and only reloads on request |
| Open paths | The right-Sidebar guide, sidebarRight.openTab('browser', {params:{url}}), a chat link when the link preference is In-App Sidebar, and browser.new on Cmd+T |
| Model surface | None - Browser tabs are user-facing presentation state and register no tool, prompt section, or Session event |
| Security posture | Never injects Electron or Node into visited content; the main process approves guest leases and enforces attachment, navigation and permission policy |
| History | A navigation state machine: the first controlled load is known, a later unreadable load marks the address (Back/Forward/external disabled, Reload returns to the controlled URL) |

### The BSH port of that solution

Same shape: a platform web view owned by the right-panel tab, not a page painted
by the shell. On macOS that is WKWebView, and every seam is already in this tree:

- objc2-web-kit 0.3.2 is in the local cargo cache (same objc2 0.6 /
  objc2-app-kit 0.3.2 / objc2-foundation 0.3.2 family bsh already depends on).
- gpui_pre Window implements raw_window_handle::HasWindowHandle
  (gpui-pre-0.3.9 window.rs:7573), so a view can obtain the AppKit ns_view and
  host the web view as a subview.
- NSView::addSubview, NSView::setFrame, NSView::setHidden (objc2-app-kit),
  WKWebView::loadRequest, goBack, goForward, reload (objc2-web-kit).

Mapping, one for one:

| DSH | BSH |
|---|---|
| Electron webview in the right-Sidebar tab | WKWebView subview of the GPUI window's ns_view, framed to the Browser pane |
| guest lease and navigation policy in the main process | EgressGate::authorize(host, port) before loadRequest; a denied URL is never loaded and the pane shows the rule |
| iframe sandbox flags (Web) | not applicable (native carrier) |
| Back/Forward/Reload/Go/Open-in-browser toolbar | the pane's existing toolbar, driving the carrier when one is attached |
| keepMounted | the browser state already lives on AppState; the carrier is hidden, not destroyed, when the tab changes |
| Cmd+T | already wired (the browser.new equivalent) |
| no model tools | bsh keeps its nine tools as an additional surface, reading the same URLs |

Two honest gaps for that round: WKWebView's own subresource fetches do not pass
the egress gate (only top-level navigation is vetted before the request), and
the carrier must be hidden whenever the Browser pane is not the one on screen.


## UPDATE: it builds and renders on this machine now

The blocker is defeated, in a scratch project (no repo change):

    servo = { version = "0.7", default-features = false }   # 2,276 packages
    cargo build  ->  Finished dev in 1m48s (after mozjs)

Two host-level fixes were required, both outside the repo:

1. A compiler wrapper that drops the forced lld while preserving argument
   boundaries (no eval: a quoted -D must stay one argument). Put it first on
   PATH and point CC/CXX at it. Without it, mozjs configure dies on
   libSystem.tbd (arm64e) and, with a naive eval-based wrapper, icu_data.S
   dies on '.incbin' because the quoted -DICU_DATA_FILE was split.
2. A PATH WITHOUT Homebrew LLVM. With /opt/homebrew/bin on PATH, mozjs mixed
   Homebrew's libc++ headers into an SDK build (unknown type name ldiv_t) and
   picked /opt/homebrew/opt/llvm/bin/ld64.lld for the linker. Apple's clang
   must be the only one visible:

    PATH=/tmp/ccwrap:<rust bin>:/usr/bin:/bin:/usr/sbin:/sbin

Result of the headless probe (400x300, no window):

    SoftwareRenderingContext::new(400x300)          ok
    ServoBuilder + WebViewBuilder + delegate         ok
    rendering_context.read_to_image(rect)            Some(400x300, 120000 opaque px)
    page load                                        panicked: "No resource reader registered"

The last line is the useful one: a data: URL needs submit_resource_reader, and
that is the SAME seam the egress gate must use. So the next step is not "make
Servo build" but "register one resource reader that calls
agent::policy::fetch" - one function that makes the rendered page and the
policy the same path.