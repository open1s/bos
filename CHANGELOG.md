# Changelog

## v4.1.3 (2026-11-11)

- **Declarative extension panels**: a `panel` contribution ships
  `panels/<id>.json` - title, location, default visibility, a light and a dark
  theme, a refresh interval, and a view tree the host renders. The document is
  validated at load with explicit ceilings (1000 nodes, depth 32, 16 KiB per
  text leaf, 256 KiB of text, 500 table rows, 2048 characters per cell, 1 MiB
  serialized), and a component the host cannot honour is a load error rather
  than a silent fallback. The extension pane mounts these documents in the
  right-hand pane.
- **Wasm-component extensions**: a `wasm-component` guest now gets WASI preview
  2's clock and random interfaces, so `std::time` and `getrandom` work without
  a host bridge; stdout and stderr are inherited, while the filesystem,
  environment, sockets and stdin stay behind the `bsh:extension/host` imports
  and their capability checks. A wasm component can also observe hooks, which
  the extism runtime already could, under the same `hooks.observe` capability.
- **Hook points are explicit**: the host dispatches eight of them -
  `model.request`, `model.response`, `tool.before`, `tool.after`,
  `http.exchange`, `turn.start`, `turn.end` and `skill.load` - and a manifest
  naming anything else is rejected as "not a hook point this host dispatches"
  instead of silently never firing.
- **An abandoned stream still reports its exchange**: a response body the caller
  drops now reports through a drop guard that fires exactly once, so the view
  behind the vendor tap shows a truncated stream as truncated instead of
  showing nothing.
- **Right-pane deep links**: `bsh --panel=<files|terminal|diff|browser|extensions|federation|tasks>`
  opens that pane on launch, and `--browser=<url>` opens the Browser pane on a
  URL.

## v4.1.2 (2026-11-10)

- **Linux ships one AppImage**: the `.deb`, `.rpm` and portable tarball are
  gone, replaced by `bsh-<version>-linux-<arch>.AppImage`. The deb could not be
  installed on Ubuntu 24.04 at all: its `Depends` came from the release
  binary's full `ldd` closure, which included WebKitGTK's transitive
  `libicu70`/`libunistring2` because the package was built on 22.04, and apt on
  24.04 has neither. Packaging the app instead of declaring its dependencies
  removes that whole class of failure.
- **What the image carries, and why not WebKit**: bsh plus the libraries the
  host's own WebKitGTK does not already load, computed at package time from
  `ldd` - so the set maintains itself. WebKitGTK is a host prerequisite on
  purpose: it resolves its helper processes and injected bundle at paths
  compiled into the library, which no environment variable relocates, so a
  bundled copy cannot be found without a mount namespace over `/usr`.
- **Readable start-up failures**: `AppRun` checks glibc and every unresolved
  library before the exec, and prints the one command the detected distribution
  needs (`apt`, `dnf`, `pacman`, `zypper`) instead of a loader error. FUSE 2
  is optional via `--appimage-extract-and-run`.
- **Desktop integration without a package manager**: `--integrate` writes a
  menu entry pointing at the image where it sits and `--uninstall` removes it,
  compensating for the deleted deb's `.desktop` file and uninstall path.
- **Verified on clean systems**: the release job runs the image in
  `ubuntu:22.04`, `ubuntu:24.04`, `debian:12` and `fedora:41` containers with
  WebKitGTK installed, and in one without it to prove the preflight fires.

## v4.1.1 (2026-11-10)

- **Tool and skill discovery in the prompt**: the system prompt now opens with
  a discovery section derived from the request's own tool array. Its heading
  names `find_tool` and/or `find_skill` only when the request carries them, so
  a withheld tool family is never hidden and a search the request does not carry
  is never promised. The section still leads the prompt, and a build that
  withholds nothing says nothing about a search it does not have.
- **Workspace ignore file**: `.bsh/.ignore` (gitignore syntax) is honoured by
  every file tool - `list_dir`, `read_file`, `read_files`, `write_file`,
  `edit_file`, `apply_patch`, `glob` and `grep` - with the harness's own
  `.bsh` and `.bos` trees always excluded. A fresh workspace gets the file with
  every rule commented out, so nothing changes until its reader uncomments a
  line; an existing file is never overwritten.
- **File preview**: a Markdown tab opens on its rendered document with a
  Source/Preview switch, an image tab adds a `W x H - size` caption, and SVG is
  rasterized instead of painted as a tinted mask. The source buffer remains the
  editable truth for save, undo and comments.
- **Extensions pane**: tools, skills and installed extensions are searchable
  from the pane, and a declarative extension panel picks which installed
  extension it mounts.
- **Transcript scrolling**: a long, scrolled transcript keeps only its visible
  rows built and bounded instead of laying out the whole history.

## v4.1.0 (2026-11-09)

- **Intelligent Review (MVP)**: file tools now capture exact before/after
  snapshots into a content-addressed store under .bsh/review; text and
  tree-sitter structural diffs are derived from those snapshots, a new Review
  tab renders changes, findings, verification placeholders, tool provenance and
  digest-bound decisions, task attempts plus the Audit tab link to the captured
  changeset, and Audit shows every review decision across changesets as a
  timeline. Any changed file can open the full Diff pane (split/unified,
  syntax, wrapping) from its snapshots, structural rows keep their parent
  indentation, and findings open the change they name. Binary changes report
  byte counts, and images also report their dimensions. Shell output remains
  explicitly unattributed, verification evidence has no producer yet, and
  PDF/document/archive support is metadata-level in this cut.
- **Pane fit**: the Trajectory, Audit and Review tabs now fit the centre column
  at any width. Their side columns take a share of a narrowed pane instead of
  holding their designed 220/330/380/390, the step and event detail cards float
  over the body (keeping their own close) when the body would fall under a
  readable width, and the run header, filter rows and decision buttons truncate
  or wrap instead of painting past the pane's edge. An expanded right panel used
  to squeeze the run timeline to 44px, clip the session line under the header
  buttons, and put Audit's export control outside the pane.
- **Trajectory liveness**: a step now reads 进行中 only while it is still
  producing data. A tool call whose result has landed is 已完成 even while the
  rest of its turn streams, the model-inference row hands off to a call in
  flight, and a historical run no longer reports running because its session is
  still streaming.

## v4.0.3 (2026-11-09)

- **BSH interface refresh**: the settings shell now uses the macOS-style
  blue-white design system across Account, General, Models, Web Search,
  Extensions and Agent Presets, with a fixed navigation rail, runtime status,
  recent activity and the unified account/data paths.
- **Chat and trajectory**: conversation rows, composer and message actions use
  the new visual language. Trajectory gained a run navigator, step timeline,
  progress header, step inspector and rich Markdown/code/diff/image previews.
  The measured timeline remains available from the same tab.
- **Audit**: a flat event table with overview statistics, event/result/time
  filters, inline event inspector and a direct jump to the related Run.
- **Tasks**: the right dock now draws the execution tree as real work units
  with Main Agent, Subagent, Peer and Task nodes, distinct run states, a
  selected-unit inspector, a work-unit List and the durable task board.
- **Tool discovery**: the model-facing `call_tool` dispatcher was removed.
  `find_tool` now returns each tool's name, description and full JSON Schema,
  and those definitions are added to the next model step so discovered tools
  are called directly by name.
- **Federation and runtime**: additional federation work, extension runtime
  fixes, scheduling and agent-loop corrections, and test coverage for deferred
  tools and the new BSH surfaces.

## v4.0.2 (2026-11-05)

- **Ferris SDK bindings**: `nbos` and `jsbos` expose `FerrisManifest`,
  `FerrisHarness` and `FerrisHost`. A Python or JavaScript object can
  implement the Ferris SDK `Extension` interface (manifest +
  start/invoke/shutdown callbacks) and run in process through the Rust
  `ExtensionRuntime`. `FerrisHost` adds the full package host: install, list,
  versions, enable/disable, rollback, quarantine, dependency-ordered plan,
  activation of the declarative/process/MCP/Cordis/Extism/WASM runtimes,
  contribution collection, invocation, policy grants and audit. The host also
  accepts contribution/revocation callbacks, exposes event subscription and
  publication, and can register an in-process language extension as a linked
  package. A chainable `FerrisManifestBuilder` covers the full schema-2.0
  builder. The host is enabled by default through the `ferris-host` Cargo
  feature; build with `--no-default-features` for the authoring-only addon.
  Hardened after review: unknown grant scopes are refused instead of silently
  becoming `any`, contribution leases are token-scoped so a stale lease cannot
  delete a replacement's entry, a rejected contribution callback leaves no
  phantom entry, replacing a linked extension tears the old runtime down, and
  event subscription ids are no longer truncated. `registerAgentTools` /
  `register_agent_tools` registers applied tool contributions on an agent, and
  MCP/Cordis runtime coverage was added. Host assembly now lives in the shared
  `ferris-bindings` crate so Python and JavaScript cannot drift. See
  `docs/ferris-bindings.md`.
- **Python/JS bindings**: catch up with the agent-runtime surface that landed
  with builtin tools 2.0. Both bindings now expose the working plan
  (`planItems`/`setPlan`/`clearPlan`/`addPlanTool`), durable goals
  (`Goal`, `GoalStore`, goal tools), mid-turn steering, deferred tools
  (`deferTool`/`deferToolPrefix`), live prompt sections, inline skills, and
  background jobs (`JobRegistry`, job tools, job-aware bash). Agent config
  gains `personaComplete`, `compactionInputTokens`, `toolOutputCap`,
  `parallelToolCalls`, `webSearch`, `contextBudget`, `skillCatalogue` and
  `fallbacks`.
- **Python bindings**: a run now writes its state back to the stored agent, so
  conversation history survives across calls and steering reaches the running
  turn. Inline skills declared with `skill(name, content)` are registered
  instead of silently ignored, and `with_bash` works (the config object
  accepts dynamic attributes).
- **Templates**: both bindings expose `renderTemplate`/`render_template`,
  `withMemory`/`with_memory` and `estimateTokens`/`estimate_tokens` from
  `react::template` and the agent context estimator.

- **Windows**: the bridge resolves `node.exe` on `PATH`, so the Cordis runtime
  and its end-to-end coverage run on Windows CI; the Cordis entry tests that
  still hit the drive-letter path bug are skipped there with that reason.
- **Release automation**: every pipeline releases only from a `vX.Y.Z` tag
  whose version matches Cargo, jsbos, nbos and the Python `__version__`, and
  skips when it does not; the macOS dmg is built through an explicitly sized
  image, linux-arm64 builds with thin LTO so it fits a free arm64 runner, and
  a dispatch publishes with the gh CLI instead of retagging.

## v4.0.1 (2026-11-04)

**Breaking**: extension manifest 2.0 is now the only schema the host accepts, so
1.x packages must be migrated with `bsh ext migrate`. That break is why the
platform moves to 4.x.

- **Extensions**: a host-neutral `ExtensionManager` now owns install,
  activation, rollback, retry and quarantine; packages load through the
  manifest 2.0 envelope with legacy migration; `bsh ext plan` previews the
  activation plan; versioned service contracts resolve into the activation
  graph with generation-scoped leases, drain handles and fiber-owned
  revocation.
- **Ferris SDK**: the standalone `bos-ext` crates (core, runtime, SDK, smoke)
  land with a manifest surface, an author API and examples; BSH runs on the
  SDK host facade.
- **Windows**: Process and MCP children launch in an AppContainer and their
  memory is capped with Job Objects; a Windows lane covers the extension
  runtimes.
- **Agent tools**: `find_tool`/`find_skill` expose full schemas, tool-call
  errors are schema-aware, batched file reads are bounded, and a multi-file
  patch tool applies atomically; tool arguments are enforced against their
  JSON schemas.
- **BSH workspace**: installed extensions can be searched and uninstalled,
  installed themes render as cards, extension commands appear in the slash
  menu, and image paste is fixed.
- **Runtime safety**: rkyv archives arriving over the network are validated,
  session ids stay out of logs, MCP discovery and writes are bounded and run
  off executor workers, WASM/Extism cancellation interrupts pure compute,
  child process groups are terminated, and per-id lifecycle work is serialized
  so failed reloads stay retryable.
- **Release**: a tagged pipeline builds and publishes installers for macOS
  (dmg/app zip), Linux (deb/rpm/tar.gz) and Windows (msi/zip), with signing and
  notarization hooks, a tagged-tree test gate and artifact verification.

## v3.0.2 (2026-10-07)

- **Fix**: rate-limiter and retry waits are now recorded on the live
  LLM path — `rate_limit_waits`, `total_rate_limit_wait` and
  `total_resilience_time` no longer stay at zero in production runs
- **Fix**: `react()`/`stream()` record per-run token, tool-call and
  tool-time deltas instead of engine-lifetime cumulative totals; the
  `total_tokens` hook reports the run's real token sum and
  `last_stream_*` values reset at the start of each stream
- **Fix**: jsbos publish workflow serializes concurrent publishes,
  refuses mismatched tags, repairs a stranded `latest` dist-tag, and
  stays idempotent for the root and per-platform packages
- **Docs**: root, nbos and jsbos READMEs refreshed for the 3.0.2
  baseline release

## v3.0.1 (2026-10-07)

- **Memory**: in-memory and file-backed memory store with metadata
  filters and a capacity cap; `Agent::remember`/`recall`/`forget`
  ergonomics; a `{{memory}}` placeholder places recalled context in
  the system prompt
- **Templates**: `react::template` renders `{{name}}` prompt
  placeholders with typed errors
- **Bindings**: Python and JS gain a `Memory` class (shared ranking
  fixture, JSON-lines persistence) and recall-on-run
- **Tools**: `AsyncFunctionTool::from_fn` for async closures that
  own their arguments
- **Resilience**: retry classification by error type
  (`LlmError::is_retryable`, `execute_with`); the rate limiter now
  honours `retry_backoff` and `auto_wait`
- **Observability**: per-call minimum/maximum LLM wall time in
  metrics, surfaced by both bindings
- **Cleanup**: removed dead collectors and unused module code

## v2.3.7 (2026-07-13)

- **Fix**: MCP tool namespaces now use `_` separator instead of `/` to comply with OpenAI function naming restrictions (only `[a-zA-Z0-9_-]` allowed)

## v2.1.1 (2026-05-13)

- **Discoverability**: Rewrote README with problem-first hook, badge row, "Why BOS?" comparison table, hero image
- **Discoverability**: GitHub Discussions enabled, topics updated (8 tags added)
- **Discoverability**: Wiki published (12 pages — Python, JS, Rust guides, architecture, config, MCP, skills, API ref)
- **Discoverability**: PyPI metadata updated (keywords, author, classifiers, project URLs)
- **Discoverability**: npm keywords updated (agent-framework, multi-agent, llm, mcp, brainos, tools, skills)
- **Feature**: Custom OG social card image for GitHub sharing
- **Release**: v2.1.0 release notes published

## v2.1.0 (2026-05-10)

- **Python rename**: `pybos` → `nbos` published on PyPI (`pip install nbos`)
- **Python**: Resilience config API (circuit breaker + rate limiter)
- **JS**: Hooks, plugins, session management in fluent BrainOS API
- **JS**: Performance metrics (`getPerfMetrics()` / `resetPerfMetrics()`)
- **JS**: Enhanced MCP client (`listPrompts()`, `listResources()`, `readResource()`)
- **JS**: TypeScript definitions expanded
- **Docs**: Updated user guides and API references for all languages
