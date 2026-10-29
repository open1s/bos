# Embedded browser in Rust: solution research (performance-first)

**Status:** research note, 2026-10-29. Not an implementation plan yet.
**Question:** how to embed a browser in a Rust application when performance is the
key requirement.
**Scope note:** this repo already ships a browser pane, and it already made the
central decision. Any answer that ignores `crates/bsh/src/browser/` is answering a
different question, so the existing engine is the baseline every option is
measured against, not a background section.

---

## 0. The finding that reframes the question

`crates/bsh` already embeds a browser, and it does **not** link an engine:

- `docs/bsh-browser-substrate.md` — the pane launches the installed
  Chrome/Chromium, controls it over CDP, one private profile and ephemeral port
  per pane, no loopback proxy, no browser crate in the build.
- `crates/bsh/Cargo.toml:124-126` — "No browser crate is linked, so the default
  build carries no browser engine code." The only dependency the shim adds is
  `tungstenite`, which is *already* in the graph via zenoh, so it adds no
  package to `Cargo.lock`.
- `crates/bsh/AGENTS.md` states the crate's gold requirement is to **win on
  performance**, and that "performance is a requirement, not a polish pass."

So the real question is not "which engine should we embed?" but **"does moving
from an out-of-process Chromium over CDP to a linked Rust engine beat what we
already measured, on the axis we already said wins?"** Everything below is
organised around that.

### What the CDP path costs, measured here

The pane's own notes record the numbers, and they are specific rather than
hand-wavy:

- **Screencast is not the frame source, deliberately.** `Page.startScreencast`
  "acknowledges its start and then misses paints: a typed key published NO frame
  ... and a page animating at 25 fps was published at ~10 fps." A capture probe
  answers every change instead, and an activity stamp makes input/navigation/
  resize publish without waiting for the idle cadence.
- **The capture path is decode-bound, not render-bound.** `engine/mod.rs:18-25`:
  the pump probes with a **PNG capture**, decodes it, and swaps channels to BGRA
  on its own thread, so the GPUI thread only builds a `RenderImage` from ready
  bytes. A still page is probed *without a decode at all*.
- **The cost is a PNG encode + PNG decode per frame, plus a full-viewport
  copy over the CDP transport**, at roughly 30 fps while a page moves.

That is the honest bottleneck: on the CDP path, every visible frame is
compressed, sent over a local websocket, and decompressed. It is fine for a
side pane and it is exactly what a linked engine would remove.

---

## 1. The candidate field, as it actually stands

Versions and dates below are from crates.io / crates.io-index and the projects'
own releases, read on 2026-10-29.

| Option | What it is | Current state | Links an engine? | Cross-platform rendering |
|---|---|---|---|---|
| **Chrome/Chromium over CDP** *(what bsh does now)* | External process, CDP control | Shipping in this repo | No | Yes, identical Chromium everywhere the user has it |
| **CEF** via `cef-rs` / `tauri-runtime-cef` | Full Chromium, embedded | `cef` **156.2.0+156.0.14** published **2026-10-25**; Tauri 3.0 alpha ships `tauri-runtime-cef` | Yes (huge) | Yes |
| **Servo** | Rust engine, embeddable | `servo` **0.7.0** on crates.io, published **2026-10-05**; 0.1.0 was 2026-04-13, so ~6 minor releases in 6 months | Yes (large) | Yes |
| **Blitz** (DioxusLabs) | Rust HTML/CSS renderer, **no JS engine** | `blitz-dom` **0.2.4**, **2026-10-16**; self-described **alpha** | Yes | Yes |
| **wry** | Wrapper over the OS webview | `wry` **0.57.1**, **2026-10-23**; 43M downloads | Never — OS engine | **No** (WebKit / WebView2 / WebKitGTK) |
| **Ultralight** | Commercial WebKit fork | Rust bindings `ultralight` **0.1.7**, last published **2024-04-28** — ~2.5 years stale | Yes | Yes, proprietary licence |

### 1.1 Servo — now genuinely a crate, and this is new

The most consequential change since this repo last looked: **Servo is on
crates.io.** `servo 0.7.0` ships with `servo-embedder-traits`,
`servo-constellation`, `servo-script`, `servo-layout`, `servo-paint`,
`servo-net`, `servo-media`, `servo-webgl`, `servo-webgpu`, plus
`webrender`/`webrender_api` 0.70 and `stylo` 0.22. The published package is
~714 kB of source; average successful docs.rs build is ~15m44s, which is itself a
useful cost signal.

The embedding API was reworked through 2025 into something idiomatic — a
`WebView` handle with methods for navigation and input, and a `WebViewDelegate`
trait for engine→embedder callbacks, explicitly modelled on Apple's WebKit
delegates. The Servo blog describes the motivation concretely: getting Servo
running *at all*, without wiring input or resizes, "took over 200 lines of Rust
code", versus "under 50 lines of C" for a minimal WebKitGTK kiosk app. That gap
is what the rework closed. Servo also has a trait for **alternative rendering
contexts** (#35052, #34813, #34780) — the seam that matters if we ever want to
paint into our own GPU surface rather than a system window.

Caveats that matter for a production pane: it is still labelled a prototype
engine; WPT conformance is improving fast but is not Chromium; and security/
process-isolation maturity is the usual objection to embedding a young engine in
something that renders untrusted web content.

**Servo is the only option here that is both Rust-native and a real, complete
engine with JS.** That is its whole case.

### 1.2 Blitz — the performance-interesting one, with a hard disqualifier

Blitz is a Rust HTML/CSS renderer built on **Stylo** (the CSS engine Servo and
Firefox share) and **Taffy** (the layout engine also used by Zed and Bevy UI),
painting through **wgpu**. Its stated aims are modularity, embeddability and API
flexibility, and rasterisation runs on the GPU via Vello/wgpu.

Two things make it genuinely interesting for a performance-first pane:

1. **No JS engine** is a design decision, not an oversight — the project
   "defer[s] work on complex features like JavaScript execution, browser-grade
   network caching and security, and process-isolation" to reach production
   quality sooner for what it does support.
2. It supports **rendering HTML to an image/texture** as a first-class use case.

And one hard disqualifier for *this* pane: bsh's Browser pane renders **arbitrary
user web pages**, which require JavaScript. Blitz cannot execute it. Blitz is
therefore out for the general Browser pane and remains relevant only for
rendering *our own* trusted HTML/CSS (a help view, an e-mail or Markdown
surface) where shipping no engine and no JS is a security win rather than a
limitation. It is self-described **alpha**, targeting beta by end of 2025 and
production "sometime in 2026" — i.e. not now.

### 1.3 CEF — the "it just works, and you pay for it"

CEF is now a first-class, supported runtime option in the Rust ecosystem rather
than a bindings science project. Tauri 3.0 alpha ships `tauri-runtime-cef` as a
drop-in swap for `tauri-runtime-wry`, selected by the dependency line alone. The
`cef` crate is actively released (156.2.0, 2026-10-25), and the Tauri docs are
unusually candid about the costs, which mirror what we would hit:

- **Size:** ships its own Chromium; bundles are "hundreds of megabytes larger",
  and more memory at runtime.
- **Build:** first build downloads ~1 GB of CEF binaries per platform, and on
  Windows/macOS compiles CEF's C++ wrapper (CMake + Ninja required).
- **Desktop only.**

The architectural detail worth stealing from the docs regardless of which engine
we pick: a CEF app is **one executable that is also its own renderer, GPU,
network and utility process** — Chromium re-launches the same binary with a
`--type=` switch, which is why the entry point must check for that switch first
and run the helper-process path before building any UI. That is a real trap for
anyone embedding Chromium, and it is already solved in the CDP design we have
(Chromium is a separate, unmodified binary).

### 1.4 wry — the option that gets *worse* on the axis we care about

`wry` is the default Rust answer and it is excellent at what it optimises for:
small binaries, zero engine to ship. But for a **performance-first** requirement
it fails on the axis itself, because it does not choose the renderer — the OS
does, and the OS engines are not equivalent:

- macOS → WKWebView, Windows → WebView2, **Linux → WebKitGTK**.
- WebKitGTK is the weak link, and it is measured: wry issue **#1315**, "wry's
  webkitgtk2 performance is noticeably slower than previous versions". There is a
  long-running WebKit bug (#245783) tracking slow GTK scrolling, animations, drag
  & drop and input.
- The Linux graphics situation leaks into shipping apps: Tauri maintains a
  "Linux Graphics Issues" page whose main cause is the **WebKitGTK DMABUF
  renderer requesting buffer formats the NVIDIA driver does not provide**, with
  `WEBKIT_DISABLE_COMPOSITING_MODE=1` and `__NV_DISABLE_EXPLICIT_SYNC=1` as
  workarounds — i.e. the documented fix for a rendering bug is **to turn GPU
  compositing off**. There is even a published `webkit2gtk-nvidia-quirk` crate
  that detects the session and applies the workaround.

Read that carefully against the gold requirement: the path that is cheapest to
build is the path where, on one of our three desktop platforms, the documented
remedy for visual breakage is disabling hardware acceleration. That is not a
performance-first choice; it is a performance-by-platform-lottery choice.

### 1.5 Ultralight — the small-engine option, now dormant

Ultralight is a proprietary, lightweight WebKit fork aimed at games and apps,
with a surface-based model that composites into your own renderer — the closest
shape to "engine as a library, paint into my GPU surface". The Rust bindings
(`ultralight`) were last published **2024-04-28**, ~2.5 years ago. Combined with
a commercial licence and a small user base, this is a dependency-and-licensing
risk, not a performance win. Several competing "small engine" products in this
niche (Sciter, Ekioh Flow) are also proprietary, which is why Blitz's permissive
licensing is called out as its own advantage on Blitz's comparison page.

---

## 2. The decision that actually matters: process model, not engine

The single highest-leverage observation in this research is that **the engine
choice and the performance profile are almost decoupled; the frame-transport
choice is what dominates.**

Compare the two shapes:

**Shape A — engine in another process, pixels over a control channel (today).**
Pros: crash isolation is free, the page's renderer cannot corrupt or stall our
UI, one unmodified binary, no build burden, identical Chromium on every
platform, and devtools/CDP come free. Cons: **every visible frame is a PNG
encode → websocket → PNG decode → channel swap**, which the code comments
already describe as the deliberate cost centre, and Chrome's own screencast
path is unusable so a polling capture probe replaces it.

**Shape B — engine linked in-process, painting to our surface.** Pros: frames
can be produced directly into a GPU texture; no compression, no transport, no
per-frame decode; the seam already exists in Servo (alternative rendering
contexts) and in principle in Blitz (wgpu output). Cons: the engine becomes part
of our address space and our build; conformance and security maturity are now
ours to own; and a linked Servo still needs its full dependency tree (GStreamer
for media, a large build).

For a pane rendering **untrusted arbitrary web content**, Shape A's isolation is
not a nice-to-have — it is the reason the pane can survive a hostile page at all.
Any move to Shape B must answer what replaces that isolation.

### 2.1 The middle path that is cheap and probably right

If the goal is "stop paying PNG per frame", that does not require abandoning the
CDP architecture. It requires changing the **frame transport**:

- **Keep** the external Chromium process, its isolation, and the CDP control
  channel.
- **Replace** the PNG capture probe with a zero-copy path to the page's
  compositor output. The candidates are Chrome's own frame-production surfaces
  (the GPU/Viz shared-image path) rather than a screenshot RPC — i.e. get a
  shared texture/handle instead of an encoded buffered image.
- **Keep** the existing `FrameSlot` mailbox contract either way: it already
  decouples decode/publish from the GPUI thread, coalesces bursts to the newest
  frame, and signals a coalescing wake instead of a per-frame poll. That design
  survives a transport change untouched, which is why it is the right place to
  cut.

This is the option that preserves everything the current design gets right
(isolation, one Chromium, no build burden, free devtools) while removing the one
thing it pays for: the per-frame encode/decode. It should be evaluated before
any engine swap, because an engine swap pays the isolation and build costs up
front and *still* has to solve frame transport.

### 2.2 Where a linked engine genuinely wins

A linked engine is the right answer when one of these is true, and not
otherwise:

1. **Frames must be composited into our scene graph** (3D overlay, transforms,
   video-wall, per-pixel effects over page content) — no encode/decode hop can
   be tolerated at all.
2. **Frame rate must be display-synced, not capture-synced.** The measured
   ceiling on the current path is the capture cadence, the pump's "about thirty
   frames a second", and Chrome's screencast's ~10 fps on motion. A pane welded
   to a 120 Hz display cannot get there through a screenshot RPC.
3. **The user may not have Chrome installed.** This is the real product risk in
   the current design: discovery looks for `chromium`, `chromium-browser`,
   `google-chrome`, `google-chrome-stable`, and if none is found the pane
   reports the searched locations and asks the user to install one. A linked
   engine removes that dependency; CEF does so at hundreds of MB; Servo does so
   in Rust.
4. **Input latency is on the critical path.** Today every pointer/wheel/key event
   crosses a websocket as JSON before the page sees it. The notes show how much
   care this required (wheel axis negation, button/drag semantics, click counts,
   IME, accelerators). A linked engine shortens that path to a function call.

---

## 3. Recommendation

**Do not swap the engine yet. Measure the transport first.** In order:

1. **Instrument what the current path actually costs**, per frame, in the two
   phases the code already separates: engine-side capture+encode, and host-side
   decode+swap. The comments assert PNG decode dominates and that a still page
   skips decode entirely; that is a testable claim and it decides everything
   downstream. If decode+swap is small next to encode+capture, optimizing the
   transport is pointless and the engine swap is the only lever left.
2. **If the frame path dominates, pursue the shared-surface transport (§2.1)**
   before touching the engine. It keeps the isolation property the pane must
   have for untrusted content and keeps the build free of an engine.
3. **Adopt Servo as the strategic Rust-native target and track it, don't ship it
   yet.** It is the only Rust-native complete engine, it is now on crates.io at a
   real cadence (0.1.0 in April → 0.7.0 in October 2026), and its embedder API
   was reworked specifically to make embedding tractable, with an
   alternative-rendering-context seam that is exactly what rule 1 and 2 above
   need. Ship it when it can render the pages we must render and when its
   isolation story is settled — not before.
4. **Use Blitz only for trusted, engine-owned HTML** (help, Markdown, mail-like
   surfaces), where "no JS engine, no shipped engine, no Chromium dependency" is
   a win. Never for the arbitrary-web Browser pane. It is alpha.
5. **Treat wry as a non-option under this requirement**, and say so plainly. It
   optimises binary size and build cost, and it hands the renderer to the OS;
   on Linux that means WebKitGTK, whose performance regressions are documented
   and whose GPU path has a known NVIDIA incompatibility whose standard fix is
   to disable compositing.
6. **Treat CEF as the fallback that trades size for certainty** — a supported,
   maintained, identical-everywhere Chromium, at hundreds of MB and a 1 GB
   first-build download. Choose it if "the user must not need Chrome installed"
   and "Servo isn't ready" are both true at once.

### The one-line answer

For a performance-first embedded browser in Rust today, the winning move is
**not a different engine — it is removing the per-frame image encode/decode from
the path between a Chromium we already trust and our own GPU surface**, with
Servo as the Rust-native endgame and Blitz confined to engine-owned HTML.

---

## 4. Sources

- This repo: `docs/bsh-browser-substrate.md`; `crates/bsh/src/browser/engine/mod.rs`
  (FrameSlot/capture design, lines 18-25, 48-114); `crates/bsh/Cargo.toml` lines
  124-142; `crates/bsh/AGENTS.md` (gold requirement).
- Servo embedding rework and new webview API:
  https://servo.org/blog/2025/02/19/this-month-in-servo/
- Servo monthly status (keyboard scrolling handled by the engine, WebView API):
  https://servo.org/blog/2025/10/24/this-month-in-servo/
- `servo` crate on crates.io (0.7.0, 2026-10-05, dependency list, build time):
  https://crates.io/crates/servo · https://docs.rs/crate/servo/latest
- Servo repository: https://github.com/servo/servo
- `servo` crate versions/releases: https://github.com/servo/servo/releases
- Blitz about/status page (alpha, no-JS strategy, comparisons):
  https://blitz.is/about · https://github.com/dioxuslabs/blitz
- Blitz description as a wgpu/Stylo/Taffy renderer:
  https://daily.dev/posts/dioxuslabs-blitz-high-performance-html-and-css-renderer-powered-by-wgpu-3xd9v3jri
- Tauri CEF runtime guide (size, build, `--type=` helper process, desktop-only):
  https://v3.tauri.app/develop/cef
- `cef` crate (156.2.0+156.0.14, 2026-10-25): https://crates.io/crates/cef
- `cef-rs`: https://github.com/tauri-apps/cef-rs
- wry WebKitGTK performance regression: https://github.com/tauri-apps/wry/issues/1315
- Tauri Linux graphics issues (WebKitGTK DMABUF/NVIDIA, compositing workarounds):
  https://v2.tauri.app/develop/debug/linux-graphics
- WebKitGTK slow-performance tracker: https://bugs.webkit.org/show_bug.cgi?id=245783
- `webkit2gtk-nvidia-quirk` crate: https://docs.rs/webkit2gtk-nvidia-quirk
- `wry` crate (0.57.1, 2026-10-23): https://crates.io/crates/wry
- Ultralight site/blog: https://ultralig.ht/blog/ultralight-1-4-out-now
- `ultralight` Rust bindings (0.1.7, last published 2024-04-28):
  https://crates.io/crates/ultralight
- Verso (a browser built on Servo; what an embedder must supply that the engine
  does not — windowing, menus, IME, notifications):
  https://wusyong.github.io/posts/verso-ui/
- Servo benchmark report (project's own benchmark wiki):
  https://github.com/servo/servo/wiki/Servo-Benchmarking-Report-(December-2024)
- Embedding Servo in Qt via CXX-Qt (alternative rendering-context precedent):
  https://www.kdab.com/embedding-servo-in-qt
