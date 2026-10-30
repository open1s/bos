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
| Wasmtime + Component Model runtime | missing | runtime/wasm.rs is the current work item |
| External process + JSON-RPC runtime | done | runtime/process.rs |
| WIT interface definition | done | crates/bsh/wit/extension.wit |
| Host-managed GPUI panels | partial | panel id/title registered; renderer missing |
| Declarative UI | partial | contract/validation done; GPUI renderer missing |
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
| Agent-native contributions | partial | tools/prompts/contexts; models/agents/workflows missing |
| UI-hosted | done at contract level; renderer pending | no GPUI object crosses the boundary |
| Command/event/stream distinction | partial | RPC and events; streaming tool output missing |
| Versioned contracts | done | manifest 1.0, API 1.0, UI 1.0, WIT package 1.0 |
| Fault containment | partial | timeout/lazy runtime; cancellation and backoff retry missing |
| Observable by default | partial | status/audit/doctor; per-request metrics partial |
| Reproducible installation | partial | lockfile and hashes; registry sources and upgrade/rollback missing |
| Data survives code upgrades | partial | data root separated; migrations not executed |

## 3. Taxonomy

### 3.1 Contribution types

| Contribution | Status | Gap |
|---|---|---|
| UI extension (panel) | partial | registration and validation; renderer missing |
| Tool extension | done | |
| Agent extension | missing | no agent-role contribution |
| Model extension | missing | no model/provider adapter contribution |
| MCP extension | missing | MCP runtime/adapter not implemented |
| Workflow extension | missing | declared but no executor |
| Context extension | partial | static Markdown only, no scoped provider call |
| Prompt extension | done | Markdown appended to system prompt |
| Command extension | partial | registered in the host plugin registry; composer palette not wired |
| Event extension | partial | tools publish; packages cannot subscribe to host events |
| Theme extension | missing | themes recorded only |
| Language extension | missing | no language contribution kind |
| Storage extension | partial | namespace API exists; no guest host-function binding |
| Integration extension | missing | no GitHub/browser/terminal adapter contribution |

### 3.2 Execution runtimes

| Runtime | Status |
|---|---|
| Built-in Rust | done |
| Trusted native module | non-goal |
| WASM Component | missing |
| External Process | done |

## 4. System architecture

| Boundary | Status | Gap |
|---|---|---|
| GPUI host owns App/Window/Entity | done | extension module imports no GPUI |
| Agent runtime owns tool discovery/invocation/cancel | partial | discovery/invocation yes; cancellation no |
| Extension manager discovers, validates, resolves, dispatches, tracks health | done | host.rs |
| Capability manager evaluates, scopes, audits, revokes | done | capability.rs |
| Runtime dispatcher routes lifecycle/RPC/command/event | partial | lifecycle/RPC yes; command/event routing partial |
| Dependency direction independent of GPUI | done | |
| Dependency direction independent of Wasmtime outside runtime | done | wasmtime will live in runtime/wasm.rs only |

## 5. Package format and manifest

| Requirement | Status |
|---|---|
| extension.toml, assets, schemas, wasm, prompts, workflows, migrations directories | partial: accepted; migrations not executed |
| All directories except manifest optional | done |
| Manifest schema 1.0 fields | done; models/agents/workflows contributions absent |
| Compatibility validated before runtime init | done |
| Requested capabilities never grants | done |
| Contribution ids unique and globally qualified | done |
| Entry paths package-relative and traversal-safe | done |
| Resource requests capped | done |
| Unknown mandatory fields and unsupported schemas fail | done |
| SemVer and reverse-domain ids | done |
| Optional forward-compat fields | done via serde defaults |
| Canonical hash | done SHA-256 |
| Publisher identity | missing |
| Signature metadata and verification | missing |
| Source registry and revision recorded | partial: source enum; no registry/Git revision |
| Exact version recorded | done |
| Dependency lock recorded | done |
| Installation timestamp and verification result | timestamp done; verification result missing |

## 6. Extension API

| Layer | Status |
|---|---|
| Core API: lifecycle, metadata, context, errors, logging, cancellation | partial: cancellation missing |
| Domain API: agent, tools, workflow, events, storage | partial: workflow/model/agent missing |
| UI API: panels, commands, declarative views | partial: validation done, renderer/action routing missing |

### 6.1 API modules

| Module group | Status |
|---|---|
| core/lifecycle, metadata, context, errors, logging | done |
| core/cancellation | missing |
| capabilities/filesystem | partial: policy only, no guest host function |
| capabilities/network | partial: policy only, no guest host function |
| capabilities/process | partial: policy + spawn; no OS sandbox |
| capabilities/storage | partial: API; no guest binding |
| capabilities/secrets | partial: policy only |
| agent/tool, prompt, context_provider | tool/prompt done, context static |
| agent/agent_provider | missing |
| agent/model_provider | missing |
| workflow/trigger, action, execution | missing |
| events/publisher, subscriber | publisher done; subscriber API not exposed |
| ui/panel, view, command, notification, theme | view contract; the rest partial/missing |

### 6.2 Contract rules

| Rule | Status |
|---|---|
| No GPUI types across boundary | done |
| No host file descriptors | done |
| Explicit request/response schemas | partial: tool/UI yes; command/event payloads not schema-checked |
| Correlation ids and cancellation | partial: correlation id done; cancellation missing |
| Bounded payload and stream rate | partial: event/tool caps; stream absent |
| Version every transported/persisted schema | done for manifest, UI, events, lock, state; grants/registry format 1 |
| Host implementation types private | done |
| Runtime transport mapped to domain semantics | done for process; wasm pending |

## 7. Lifecycle

| Requirement | Status |
|---|---|
| Discovery and static validation | done |
| Compatibility and dependency resolution | done |
| Install into staging, atomic promotion | done |
| Lazy loading / on-demand activation | done via LazyRuntime |
| Graceful deactivate/shutdown | partial: runtime trait; host does not expose suspend/deactivate |
| Cancellation of outstanding work | missing |
| Health monitoring and crash handling | partial: health/status; crash backoff retry missing |
| Retry with bounded backoff | missing |
| State persistence independent of package | done |
| Explicit uninstall and retention | done |
| Startup policies on-startup/on-demand/on-workspace-open/manual | done |
| Failure stops new requests, times out work, releases resources, diagnostics, isolation | partial: new requests skipped, timeout per call, resources lazy; backoff missing |

## 8. Runtimes

| Requirement | Status |
|---|---|
| Built-in statically linked, internal interface, lifecycle conventions | done |
| WASM load/validate, host binding, memory/fuel/time limits, authorized host functions, cancellation, error normalization, metrics | missing |
| Process JSON-RPC stdio, handshake, structured stderr | done |
| Process executable allowlist/user approval | partial: allowlist + explicit grant; no per-command approval UI |
| Environment filtering, cwd restriction, timeout, output limits | done |
| OS-level sandbox | missing |
| Native dynamic libraries | non-goal |

## 9. UI

| Requirement | Status |
|---|---|
| Mode A panel contribution with id/title/icon/location/visibility | partial: id/title/icon; location/visibility missing |
| Mode B declarative components | contract done all listed components; renderer missing |
| Mode C embedded guest GPUI | non-goal |
| Validate schema/payload, depth/node/text limits | done |
| Route action ids to owner | missing (renderer) |
| Apply state patches, revisions, stale rejection | contract done; renderer missing |
| Focus, keyboard, accessibility | missing |
| Loading/error/unavailability states | missing |
| No JS/HTML | done |
| Theme contributions | missing |

## 10. Agent-native API

| Requirement | Status |
|---|---|
| Tool provider | done, but result content types limited to JSON |
| Context provider | partial: static Markdown |
| Prompt provider | done |
| Model provider | missing |
| Agent provider | missing |
| Workflow provider | missing |
| Tool content: text, images, structured, file refs | missing (only structured JSON) |
| Streaming output, progress, cancellation | missing |
| Timeouts, error categories, correlation ids | done |
| MCP tool/resource/prompt mapping | missing |
| MCP lifecycle/reconnect/timeout/cancel | missing |

## 11. Events

| Requirement | Status |
|---|---|
| Envelope | done |
| All listed topics | partial: workspace.changed, model.requested/completed absent as constants/publishers |
| Topic allowlists and subscription filters | partial: filters done; publisher allowlist not enforced |
| Schema versioning, correlation ids | done |
| Bounded queues/backpressure | done |
| Payload and rate limits | payload done; rate limit missing |
| Cancellation propagation | missing |
| Delivery semantics documented | done in standard |
| Error isolation | done |
| No unbounded history | done |
| Separate streams/app events/audit paths | done |

## 12. Security

| Requirement | Status |
|---|---|
| Capability categories including terminal.execute | partial: process.execute only; alias missing |
| Network domain/method policy | partial: domain scope; method policy not enforced |
| Process executable/argument policy | partial: allowlist and args; no argument policy |
| CPU/fuel and wall clock | partial: wall clock; fuel missing |
| Memory/concurrency | partial: declared, not enforced |
| Host-call authorization | missing (no guest host calls yet) |
| IPC authentication and message validation | partial: length/JSON; no auth |
| Secret redaction | partial: audit does not log values; no secret API |
| Storage isolation | done |
| Permission revocation | done via grants store |
| Audit trail | done |
| Package hash/signature verification | hash done; signature missing |
| Disable/quarantine | disable done; quarantine directory exists but no flow |
| Trust levels verified/unknown | partial: enum; no verification path |

## 13. Registry, installer, dependencies

| Requirement | Status |
|---|---|
| Official registry | missing |
| Git repository source | missing |
| Local directory | done |
| Local package archive | missing |
| Enterprise/private registry | missing |
| Install pipeline download/checkout, hash/signature, manifest, compatibility, resolve, staging, promotion | partial: local copy + hash + manifest + compatibility + resolve + staging + promotion |
| Directory layout installed/cache/staging/quarantine/data/state/logs | done |
| Data separation | done |
| Uninstall data policy | done |
| Upgrade must not overwrite data | done by versioned dirs |
| Lockfile id/version/hash/source/revision/deps/api/signature/installation metadata | partial: id/version/hash/source/deps/api/host; signature/revision/installation metadata missing |
| Upgrade and rollback | missing |
| Reproduce from lockfile | missing |
| Package integrity verification on load | partial: hash recorded; no re-verify per load |

## 14. Rust workspace proposal

The design proposes 17 crates. This build keeps one crate (crates/bsh) with
module boundaries matching those responsibilities. Splitting now would add
compilation and ownership overhead without changing a contract. This deviation
is deliberate and does not remove a feature.

## 15. Technology selection

| Area | Status |
|---|---|
| Wasmtime | dependency added; runtime missing |
| WIT | done |
| Tokio | done |
| Serde/TOML/JSON | done |
| JSON schema for config/tool | tool schema stored; config schema not enforced |
| BLAKE3 or SHA-256 | SHA-256 done |
| Ed25519 signatures | dependency added; verification missing |
| SemVer | done |
| petgraph or custom resolver | custom resolver |
| JSON-RPC 2.0 | done |
| Cancellation tokens | missing |
| tracing | done |
| storage | JSON/TOML per-extension |

## 16. Observability and developer experience

| Requirement | Status |
|---|---|
| Installed version/source, runtime kind, lifecycle, capabilities, errors, dependencies | done |
| Startup duration, request count/latency, memory/fuel, active tasks/subscriptions, crash history | partial: startup/calls/failures; latency/memory/fuel/tasks missing |
| CLI new/validate/build/test/package/install/list/info/enable/disable/update/uninstall/logs/doctor | partial: new/validate/hash/install/uninstall/list/info/enable/disable/grant/revoke/capabilities/logs/doctor; build/test/package/update missing |
| Templates, WIT bindings/examples, local dev, mock host, integration harness, logs/traces, compatibility checker, package builder, security diagnostics | partial: examples, local dev, validation, doctor; build/test/package/mock host missing |

## 17. Testing

| Requirement | Status |
|---|---|
| Unit: manifest, ids, resolver, lifecycle, capability, events, UI, paths, lock determinism | done |
| Integration: built-in registration, WASM load/invoke, process start/stop, tools, cancellation, UI action routing, patches, denial, crash isolation, upgrade/rollback, data preservation | partial: built-in, process, tools, denial, patches at contract level; WASM/cancel/UI routing/upgrade/rollback missing |
| Security tests: traversal, unauthorized fs/network, process denial, oversized messages, nested UI, event flood, resource exhaustion, malformed payloads, stale actions, revoked capability, package tampering/signatures | partial: many unit tests; signature/network/process-exhaustion missing |
| Cross-platform matrix | not run in this environment |

## 18. Roadmap

| Phase | Status |
|---|---|
| Phase 1 core | done |
| Phase 2 WASM | missing |
| Phase 3 agent, event, MCP, process | partial: tools/prompts/process/events; MCP and streaming missing |
| Phase 4 GPUI UI | partial: contract; renderer/panels/actions/themes missing |
| Phase 5 ecosystem | partial: local install/lock/CLI; registry/upgrade/rollback/signatures/package builder missing |

## 19. User stories

The implementation covers the type/manifest/store/policy/lifecycle/process/CLI
stories. Open items map to EXT-008/009/010 (WIT host functions, Wasmtime),
EXT-012 (WASM limits), EXT-013 (event subscriptions), EXT-014/015 (agent
providers), EXT-016 (MCP), EXT-018/019/020 (UI), EXT-021/022 (installer,
resolver upgrade), EXT-023 (CLI build/test/package/update), EXT-024
(compatibility/security integration).

## 20. Open items this file exists to close

1. Wasmtime Component runtime and host functions, with limits, cancellation and metrics.
2. GPUI declarative renderer, panel location/visibility, action routing, patches, themes, command palette.
3. MCP runtime/adapter.
4. Model, Agent and Workflow providers.
5. Registry sources (Git/HTTP/archive/private), upgrade/rollback, lock reproduction.
6. Ed25519 publisher signatures and quarantine.
7. Guest host functions for filesystem/network/secrets/storage in WASM.
8. Process OS sandbox and memory/concurrency limits.
9. Richer tool results, streaming and cancellation.
10. Event subscriptions, publisher allowlists and rate limits.
11. Config schema enforcement and migrations.
12. Per-request metrics and the remaining CLI/SDK commands.
13. Integration/security/cross-platform tests for all of the above.
