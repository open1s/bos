# BOS Extension Framework (bos-ext)

Status: Phase 2 in progress — core primitives are in bos-ext-core; runtime
contract, builtin, declarative and process runtimes are in bos-ext-runtime.
Hooks, Extism and WASM moved to runtime; Cordis and the Node bridge remain in
BSH.

## Goal

Extract BSH's extension platform into an independent Rust framework and SDK:

~~~text
crates/bos-ext-core     0.1.0
crates/bos-ext-runtime  0.1.0
crates/bos-ext-sdk      0.1.0
crates/bos-ext-smoke    0.1.0
crates/bsh              host adapter only
~~~

Dependency direction:

~~~text
bos-ext-core
  ↑
bos-ext-runtime   # standard runtimes + Node bridge (Phase 2)
  ↑
agent             # uses the bridge, does not own the framework
  ↑
bsh               # Host adapter
~~~

Core, runtime and SDK must not depend on bsh, agent or GPUI. CI enforces this with
scripts/ci/check-bos-ext-independence.sh plus the smoke crate.

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
Phase 1  core primitives move to bos-ext-core; BSH re-exports
Phase 2  runtime + bridge move to bos-ext-runtime; agent uses bridge
Phase 3  Host trait and ExtensionManager wired into BSH
Phase 4  manifest 2.0, migrate command, remove 1.x parser
Phase 5  remove re-exports, BSH imports bos_ext_*; CI gate final
~~~
