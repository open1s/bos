---
module: bsh
tags: [extensions, conformance, design, checklist]
problem_type: reference
---

# BSH Extension Design Conformance Matrix

This file tracks every requirement in docs/BSH_Extension_Framework_Design.md
against the implementation in crates/bsh/src/extension. It is deliberately
exhaustive: a requirement is not "done" because a type exists; it is done when
the host performs the behaviour and a test proves it.

Status legend:

| Status | Meaning |
|---|---|
| done | implemented and covered by a test or an end-to-end CLI run |
| partial | implemented but at least one required behaviour is missing |
| missing | not implemented in this build |
| non-goal | the design explicitly defers this from the first release |

## 1. Executive baseline

| Design item | Status | Evidence / gap |
|---|---|---|
| Host + Extension Manager + Runtime Dispatcher | done | host.rs, runtime/mod.rs, runtime dispatcher by kind |
| Built-in Rust runtime | done | runtime/builtin.rs plus the shipped inspector |
| Wasmtime + Component Model runtime | done | runtime/wasm.rs; WIT host calls, fuel/memory/epoch limits, component fixture test |
| Extism plugin runtime | done | runtime/extism.rs; bsh_host bridge, policy-checked host calls, fuel/memory caps, plugin fixture |
| External process + JSON-RPC runtime | done | runtime/process.rs |
| WIT interface definition | done | crates/bsh/wit/extension.wit |
| Host-managed GPUI panels | partial | host::panel_surfaces + renderer; settings surface and right-panel Extensions mount; left/bottom/center still share those hosts |
| Declarative UI | done | contract, validation, GPUI renderer, action routing and stale-revision refusal |
| Unified tool/context/prompt contracts | done | agent.rs |
| Capability authorization, quotas, audit | partial | authorization and audit done; quotas partial |
| Directory package | done | package.rs |
| Archive distribution | missing | tar.gz reader not yet wired to install |
| Local/Git/official/private registries | missing | local directory only |
| Native dynamic libraries | non-goal | design section 8.4 |

## 2. Design principles

| Principle | Status | Gap |
|---|---|---|
| Extension-first contribution model | partial | first-party bundles still use the older plugin registry; the inspector uses the new one |
| Runtime-independent contracts | done | no GPUI/Wasmtime types in manifest, policy or UI schema |
| Capability declaration is a request | done | policy.rs |
| Least privilege and scopes | done | path/domain/secret scopes |
| Agent-native contributions | done | tools/prompts/contexts/workflows; model/agent/preset descriptors are selectable |
| UI-hosted | done at contract level; renderer pending | no GPUI object crosses the boundary |
| Command/event/stream distinction | done | RPC, events and live process/MCP/WASM progress through run_streaming |
| Versioned contracts | done | manifest 1.0, API 1.0, UI 1.0, WIT package 1.0 |
| Fault containment | done | timeout/lazy runtime/cancel token + bounded process/MCP startup retry |
| Observable by default | partial | status/audit/doctor + calls/events/startup/failed/active/fuel (WASM/Extism)/memory ceilings/live memory (WASM/process)/crash history; Extism memory usage not reported |
| Reproducible installation | done | lockfile, sources, upgrade/rollback, reproduce from remote or recorded local path |
| Data survives code upgrades | done | data root separated; migrations executed on version change |

## 3. Taxonomy

### 3.1 Contribution types

| Contribution | Status | Gap |
|---|---|---|
| UI extension (panel) | done | declarative renderer, actions, themes, settings host |
| Tool extension | done | |
| Agent extension | done | agent descriptor maps to a selectable Preset (plugins, permission, level) |
| Model extension | done | descriptors merge into settings.plugin_providers and are selectable |
| MCP extension | done | runtime/mcp.rs + mcp-echo fixture |
| Workflow extension | done | runtime/workflow.rs sequential executor with capability union |
| Context extension | partial | static Markdown only, no scoped provider call |
| Prompt extension | done | Markdown appended to system prompt |
| Command extension | done | registered and inserted from the composer slash palette |
| Event extension | done | WASM/process subscriptions, workspace/model/agent/tool publishers |
| Theme extension | done | panel theme plus an app-wide palette override selected in Settings |
| Language extension | missing | no language contribution kind |
| Storage extension | done | namespace API + WASM/WIT and Extism bsh_host bindings |
| Integration extension | missing | no GitHub/browser/terminal adapter contribution |

### 3.2 Execution runtimes

| Runtime | Status |
|---|---|
| Built-in Rust | done |
| Trusted native module | non-goal |
| WASM Component | done |
| Extism plugin | done |
| External Process | done |
| MCP server | done |

## 4. System architecture

| Boundary | Status | Gap |
|---|---|---|
| GPUI host owns App/Window/Entity | done | extension module imports no GPUI |
| Agent runtime owns tool discovery/invocation/cancel | done | discovery/invocation/cancel token + runtime stop/trap |
| Extension manager discovers, validates, resolves, dispatches, tracks health | done | host.rs |
| Capability manager evaluates, scopes, audits, revokes | done | capability.rs |
| Runtime dispatcher routes lifecycle/RPC/command/event | done | command/panel/pump delegated through LazyRuntime |
| Dependency direction independent of GPUI | done | |
| Dependency direction independent of Wasmtime outside runtime | done | wasmtime will live in runtime/wasm.rs only |

## 5. Package format and manifest

| Requirement | Status |
|---|---|
| extension.toml, assets, schemas, wasm, prompts, workflows, migrations directories | done | migrations run on upgrade |
| All directories except manifest optional | done |
| Manifest schema 1.0 fields | done; model_providers/agent_providers/workflows present |
| Compatibility validated before runtime init | done |
| Requested capabilities never grants | done |
| Contribution ids unique and globally qualified | done |
| Entry paths package-relative and traversal-safe | done |
| Resource requests capped | done |
| Unknown mandatory fields and unsupported schemas fail | done |
| SemVer and reverse-domain ids | done |
| Optional forward-compat fields | done via serde defaults |
| Canonical hash | done SHA-256 |
| Publisher identity | done | package.sig + trust.toml |
| Signature metadata and verification | done | Ed25519, install verification, CLI keygen/sign/trust/verify |
| Source registry and revision recorded | done | Git revision and registry URL recorded |
| Exact version recorded | done |
| Dependency lock recorded | done |
| Installation timestamp and verification result | done | InstalledRecord.verification |

## 6. Extension API

| Layer | Status |
|---|---|
| Core API: lifecycle, metadata, context, errors, logging, cancellation | done |
| Domain API: agent, tools, workflow, events, storage | done: workflows/tools/storage/events and selectable model/agent/preset providers |
| UI API: panels, commands, declarative views | done | renderer + commands + actions + composer palette |

### 6.1 API modules

| Module group | Status |
|---|---|
| core/lifecycle, metadata, context, errors, logging | done |
| core/cancellation | done | AsyncTool token -> Runtime::cancel |
| capabilities/filesystem | done | WASM read_file/write_file host functions with policy |
| capabilities/network | done | WASM fetch host function with policy |
| capabilities/process | partial | policy + spawn + rlimits + macOS write profile; Linux namespace absent |
| capabilities/storage | done | WASM storage_get/set/list |
| capabilities/secrets | done | WASM read_secret |
| agent/tool, prompt, context_provider | tool/prompt done, context static |
| agent/agent_provider | done | descriptor -> selectable preset |
| agent/model_provider | done | descriptor -> settings.plugin_providers |
| workflow/trigger, action, execution | done | sequential workflow executor; no triggers |
| events/publisher, subscriber | done | publisher + WASM and process subscribers |
| ui/panel, view, command, notification, theme | done | panels/views/commands/notifications + panel and app-wide theme |

### 6.2 Contract rules

| Rule | Status |
|---|---|
| No GPUI types across boundary | done |
| No host file descriptors | done |
| Explicit request/response schemas | done | tool input JSON schemas, UI/event schema versions, typed JSON command/event envelopes with depth checks |
| Correlation ids and cancellation | done | correlation + cancel token + runtime stop |
| Bounded payload and stream rate | done | event/tool caps + a 200-per-second progress window |
| Version every transported/persisted schema | done for manifest, UI, events, lock, state; grants/registry format 1 |
| Host implementation types private | done |
| Runtime transport mapped to domain semantics | done | process, wasm, mcp, declarative |

## 7. Lifecycle

| Requirement | Status |
|---|---|
| Discovery and static validation | done |
| Compatibility and dependency resolution | done |
| Install into staging, atomic promotion | done |
| Lazy loading / on-demand activation | done via LazyRuntime |
| Graceful deactivate/shutdown | done | runtime trait delegated through LazyRuntime |
| Cancellation of outstanding work | done | Runtime::cancel stops process/MCP children and traps a WASM guest |
| Health monitoring and crash handling | partial: health/status; crash backoff retry missing |
| Retry with bounded backoff | missing |
| State persistence independent of package | done |
| Explicit uninstall and retention | done |
| Startup policies on-startup/on-demand/on-workspace-open/manual | done |
| Failure stops new requests, times out work, releases resources, diagnostics, isolation | partial: new requests skipped, timeout per call, resources lazy; startup retry present, crash isolation partial |

## 8. Runtimes

| Requirement | Status |
|---|---|
| Built-in statically linked, internal interface, lifecycle conventions | done |
| WASM load/validate, host binding, memory/fuel/time limits, authorized host functions, cancellation, error normalization, metrics | done |
| Process JSON-RPC stdio, handshake, structured stderr | done |
| Process executable allowlist/user approval | done | allowlist + explicit grant; Settings > Extensions approves the declared process executable |
| Environment filtering, cwd restriction, timeout, output limits | done |
| OS-level sandbox | partial | macOS profile tested; Linux bwrap namespace plan implemented but not executed here; process network bridge absent |
| Native dynamic libraries | non-goal |

## 9. UI

| Requirement | Status |
|---|---|
| Mode A panel contribution with id/title/icon/location/visibility | partial | fields recorded and shown; panels mount in Settings and the right panel, not one dock per location |
| Mode B declarative components | done | ui/extension_panel.rs; image draws source label, not pixels |
| Mode C embedded guest GPUI | non-goal |
| Validate schema/payload, depth/node/text limits | done |
| Route action ids to owner | done | dispatch -> owning Runtime::command |
| Apply state patches, revisions, stale rejection | done | UiSession + renderer |
| Focus, keyboard, accessibility | done | role/aria labels; actions, apply, checkbox, select options, actionable list rows and tabs are focusable kit controls |
| Loading/error/unavailability states | done | renderer loading/error/message |
| No JS/HTML | done |
| Theme contributions | done | tokens apply to extension panels and, when selected, override the shell palette |

## 10. Agent-native API

| Requirement | Status |
|---|---|
| Tool provider | done | JSON and rich ToolContent |
| Context provider | partial: static Markdown |
| Prompt provider | done |
| Model provider | done | descriptor -> settings.plugin_providers |
| Agent provider | done | descriptor -> settings.plugin_presets |
| Preset provider | done | descriptor -> settings.plugin_presets |
| Workflow provider | done | declarative executor |
| Tool content: text, images, structured, file refs | done | ToolContent + MCP mapping |
| Streaming output, progress, cancellation | done | process $/progress, MCP notifications/progress, WASM host progress; cancel token |
| Timeouts, error categories, correlation ids | done |
| MCP tool/resource/prompt mapping | done | mcp-echo test |
| MCP lifecycle/reconnect/timeout/cancel | done | lazy start, timeout, close, fresh session on next call |

## 11. Events

| Requirement | Status |
|---|---|
| Envelope | done |
| All listed topics | done | workspace, model, agent turn and tool topics published |
| Topic allowlists and subscription filters | done | filters + WASM/Extism publishes allowlist; process children receive events only |
| Schema versioning, correlation ids | done |
| Bounded queues/backpressure | done |
| Payload and rate limits | done | 256 KiB payload + 200 events/second per publisher |
| Cancellation propagation | done | tool token -> runtime.cancel |
| Delivery semantics documented | done in standard |
| Error isolation | done |
| No unbounded history | done |
| Separate streams/app events/audit paths | done |

## 12. Security

| Requirement | Status |
|---|---|
| Capability categories including terminal.execute | done | process.execute plus the terminal.execute alias |
| Network domain/method policy | done | domain scope and optional method-restricted domain scope |
| Process executable/argument policy | done | allowlist, explicit grant, program and program+args scopes checked per spawn |
| CPU/fuel and wall clock | done | wall clock + wasm fuel + process RLIMIT_CPU |
| Memory/concurrency | partial | wasm StoreLimits + process RLIMIT_AS; concurrency per runtime mutex |
| Host-call authorization | done | WASM host functions call PolicyEngine |
| IPC authentication and message validation | done | private stdio, per-session handshake token echo, line/JSON/depth validation |
| Secret redaction | done | read_secret + audit without values |
| Storage isolation | done |
| Permission revocation | done via grants store |
| Audit trail | done |
| Package hash/signature verification | done | install + doctor |
| Disable/quarantine | done | CLI quarantine + directory |
| Trust levels verified/unknown | done | Ed25519 trust store |

## 13. Registry, installer, dependencies

| Requirement | Status |
|---|---|
| Official registry | done | HTTP index source |
| Git repository source | done | git+URL#revision |
| Local directory | done |
| Local package archive | done | tar.gz extractor |
| Enterprise/private registry | done | named registry CLI |
| Install pipeline download/checkout, hash/signature, manifest, compatibility, resolve, staging, promotion | done | sources + store |
| Directory layout installed/cache/staging/quarantine/data/state/logs | done |
| Data separation | done |
| Uninstall data policy | done |
| Upgrade must not overwrite data | done by versioned dirs |
| Lockfile id/version/hash/source/revision/deps/api/signature/installation metadata | partial | signature/revision/metadata recorded in registry; lockfile fields partial |
| Upgrade and rollback | done | versions/set_active/rollback CLI |
| Reproduce from lockfile | done | bsh ext reproduce reinstalls Git/registry locks or the recorded local source path, then checks hashes |
| Package integrity verification on load | done | doctor + install signature/hash |

## 14. Rust workspace proposal

The design proposes 17 crates. This build keeps one crate (crates/bsh) with
module boundaries matching those responsibilities. Splitting now would add
compilation and ownership overhead without changing a contract. This deviation
is deliberate and does not remove a feature.

## 15. Technology selection

| Area | Status |
|---|---|
| Wasmtime + Extism | done |
| WIT | done |
| Tokio | done |
| Serde/TOML/JSON | done |
| JSON schema for config/tool | done | config.rs |
| BLAKE3 or SHA-256 | SHA-256 done |
| Ed25519 signatures | done |
| SemVer | done |
| petgraph or custom resolver | custom resolver |
| JSON-RPC 2.0 | done |
| Cancellation tokens | done | tool token + runtime trap/stop |
| tracing | done |
| storage | JSON/TOML per-extension |

## 16. Observability and developer experience

| Requirement | Status |
|---|---|
| Installed version/source, runtime kind, lifecycle, capabilities, errors, dependencies | done |
| Startup duration, request count/latency, memory/fuel, active tasks/subscriptions, crash history | partial | startup/calls/events/subscribers/failed/active/fuel (WASM/Extism)/memory ceilings/live memory (WASM/process)/retained crash history; Extism memory usage not reported |
| CLI new/validate/build/test/package/install/list/info/enable/disable/update/uninstall/logs/doctor | done | plus versions/rollback/quarantine/keygen/sign/trust/config/registry/metrics |
| Templates, WIT bindings/examples, local dev, mock host, integration harness, logs/traces, compatibility checker, package builder, security diagnostics | done | templates/WIT/examples/builder/doctor + `bsh ext mock` for every runtime |

## 17. Testing

| Requirement | Status |
|---|---|
| Unit: manifest, ids, resolver, lifecycle, capability, events, UI, paths, lock determinism | done |
| Integration: built-in registration, WASM load/invoke, process start/stop, tools, cancellation, UI action routing, patches, denial, crash isolation, upgrade/rollback, data preservation | partial | cancellation and scan/load crash isolation covered; cross-platform execution remains partial |
| Security tests: traversal, unauthorized fs/network, process denial, oversized messages, nested UI, event flood, resource exhaustion, malformed payloads, stale actions, revoked capability, package tampering/signatures | partial | signature/UI/traversal covered; network flood/process exhaustion missing |
| Cross-platform matrix | not run in this environment |

## 18. Roadmap

| Phase | Status |
|---|---|
| Phase 1 core | done |
| Phase 2 WASM + Extism | done |
| Phase 3 agent, event, MCP, process | done | MCP/events/process/tools and live progress |
| Phase 4 GPUI UI | partial | renderer/panels/actions/themes/keyboard focus done; multiple mount points still partial |
| Phase 5 ecosystem | done | registry/upgrade/rollback/signatures/builder/Extism/lock reproduction |

## 19. User stories

The implementation covers the type/manifest/store/policy/lifecycle/process/CLI
stories. Resolved: Wasmtime/host functions, Extism, WASM limits, WASM/process
subscriptions, MCP, UI renderer/actions, installer sources, upgrade/rollback,
CLI build/test/package/update, signatures, quarantine, process/MCP/WASM progress,
model/preset providers and local/remote lock reproduction. Still open: app-wide
theme application, Linux filesystem sandboxing, cross-platform coverage and
retry backoff.

## 20. Remaining gaps

The first-release feature set is implemented except for these explicit gaps.
Each is a behaviour, not a missing type.

1. Agent provider descriptors are selectable presets; there is no separate
   code-level agent adapter API beyond that descriptor contract.
2. Filesystem sandbox: macOS profile is tested; the Linux bubblewrap plan is
   implemented and unit-tested but cannot be executed in this macOS environment.
3. Cross-platform matrix: the suite is green on macOS (this environment);
   Windows/Linux process limits and path handling are not exercised here.
