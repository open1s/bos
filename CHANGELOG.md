# Changelog

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
