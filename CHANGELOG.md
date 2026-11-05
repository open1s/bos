# Changelog

## Unreleased

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
