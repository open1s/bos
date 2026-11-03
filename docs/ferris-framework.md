# Ferris Extension Framework

Status: Phase 5 complete — manifest 2.0 is the only accepted package schema;
1.x packages must run `bsh ext migrate` first; every shipped example and the
`bsh ext new` scaffold are already 2.0. Migration preserves runtime
entry, args, env, protocol, startup policy, event subscribe/publish and hook
contributions; migrated packages validate, `bsh ext validate` reports
`schema: 2.0` plus the generic contribution count, and `bsh ext graph` reads
effects from the 2.0 generic contribution list. The independent `ferris-smoke`
binary parses a 2.0 manifest with no BSH dependency, and the SDK carries unit
tests for its author runtime adapter. The author guide is
[ferris-sdk.md](ferris-sdk.md).

BSH imports Ferris through `ferris-sdk` only: `crates/bsh/Cargo.toml` has one
Ferris dependency, `ferris-sdk` with the `host-runtime` feature, and BSH source
uses `ferris_sdk::*` and `ferris_sdk::host_runtime::*`. The SDK host facade
re-exports the core modules, manager, package/store, capability, manifest, UI,
terminal and standard runtime crates. The Phase-1 composable/runtime/core
re-export modules are deleted, and the lifecycle state machine now lives in
ferris-core (only the terminal/hooks adapters and the `topics` convenience alias
remain in BSH). `FerrisHostSpec` and `FerrisRuntimeResolver` are the host
adapters. `statuses()` merges packages the
neutral manager sees with the host's loaded set, and install, uninstall,
enable/disable, versions, rollback, retry, quarantine and `bsh ext plan` go
through the manager.

`LegacyAdapter` bridges the old tool-shaped runtime contract to the generic
Runtime trait. The production `ExtensionHost::rebuild` takes its package order
from `ExtensionManager::plan`, builds every runtime through
`ferris_runtime::factory::build` with a `RuntimePorts` bundle, and activates the
applied set through `ExtensionManager::apply` with the host's own runtimes
registered on the resolver. The host no longer names a concrete runtime
constructor and no longer owns activation order. The manager calls `HostContributionSink` (a `ContributionSink`) to install the
agent tools/commands/panels/prompts/skills, and the returned leases are held by
the manager's `Activation` and revoked through `BshLeaseHost` when the build
unloads, so the manager owns both runtime and contribution leases. A
failed activation rolls back the already-started set instead of leaking runtimes
or leases.

## Goal

Extract BSH's extension platform into an independent Rust framework and SDK:

~~~text
crates/ferris-core     0.1.0
crates/ferris-runtime  0.1.0
crates/ferris-sdk      0.1.0
crates/ferris-smoke    0.1.0
crates/bsh              host adapter only
~~~

Dependency direction:

~~~text
ferris-core
  ↑
ferris-runtime   # standard runtimes + Node bridge (Phase 2)
  ↑
agent             # uses the bridge, does not own the framework
  ↑
bsh               # Host adapter
~~~

Core, runtime and SDK must not depend on bsh, agent or GPUI. CI enforces this with
scripts/ci/check-ferris-independence.sh plus the smoke crate.

## Frozen decisions

- Host-neutral manifest 2.0 with a generic contribution envelope.
- Host registers contribution kinds, capabilities, runtime kinds, service
  contracts, scope kinds and event specs.
- Contribution is { kind, id, payload } with a host-registered schema/sink.
- Runtime is describe() + invoke(kind, id, input).
- Capability manifests are required-only; unknown capabilities and contracts are
  rejected at validation.
- Service identity is the contract id, not a local name.
- Scope is { kind, id } with a host-registered path.
- Errors are ExtError { code, message, details }; host codes are extensible.
- ExtensionManager owns install/plan/apply/reload; Host is a bundle of small
  traits (HostSpec, ContributionSink, EventSink, AuditSink, Spawner,
  ProcessSandbox, TrustRoots, Registry).
- Contributions use async sink/lease and reverse-order teardown.
- install only stores; plan is side-effect free; apply is atomic; reload is a
  plan diff with per-activation cancel/revoke.
- One manager per root with internal serialization and a cross-process lock.
- Canonical store layout under the host-provided root.
- SDK ships a secure default process sandbox; hosts may replace it but must not
  silently disable it.
- Core is runtime-agnostic; Tokio is confined to standard runtime features.
- Ed25519 signing/verification and the trust-store format live in the SDK.
- The resolver is service-aware (package DAG + provider/consumer edges).
- Services are typed in-process by default, with optional JSON transport.
- EventSpec is host-registered and carries the permitted modes and interceptor
  capability.
- Runtime config is opaque and validated by the RuntimeKindSpec schema.
- Authors implement the Extension trait; there is no macro DSL.
- Manifest contributions are a generic [[contribution]] list.
- 1.x packages are hard-cut: migrate before use.
- Migration is phased with temporary BSH re-exports; every phase keeps tests
  green.
- SDK owns framework conformance; BSH keeps adapter and integration tests.

## Migration phases

~~~text
Phase 0  new crates + public types/traits + independence gate
Phase 1  core primitives move to ferris-core; BSH re-exports
Phase 2  runtime + bridge move to ferris-runtime; agent uses bridge
Phase 3  Host trait and ExtensionManager wired into BSH
Phase 4  manifest 2.0, migrate command, remove 1.x parser
Phase 5  remove re-exports, BSH imports ferris_*; CI gate final
~~~
