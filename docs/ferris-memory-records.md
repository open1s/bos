# Ferris memory records — extensions on the Durable Record Kernel

Status: implemented. This document defines how ferris extensions read and
commit memory through the same kernel the host uses (issues #338-#346,
`docs/durable-record-kernel.md`), and how the BSH memory pane exposes the
same records to the reader.

## 1. One contract, host-neutral

Ferris must not depend on `agent`, `bsh` or GPUI, so the kernel's wire shape
is mirrored as a host-neutral contract in `ferris-core::memory` (re-exported
as `ferris_sdk::memory`):

- `MemoryRecordDto` — schema-2 record: id, revision, kind, scope, content,
  metadata, provenance, confidence, status, timestamps, validity, relations.
  The serde format is BYTE-COMPATIBLE with `agent::memory::MemoryRecord`;
  a BSH conformance test pins the JSON round-trip both ways.
- `RecallRequestDto` / `RecallOutcomeDto` / `RecallHitDto` — the hybrid
  recall seam (query, limit, token budget, kind filter, clock) with scores,
  explanations and provenance.
- `trait MemoryRecords` — the typed service: records / recall / add / update /
  supersede / remove / clear. Every mutation is commit-guaranteed by the
  kernel; errors are `ExtError` with stable `memory.*` codes
  (scope_denied, storage, not_found, conflict, corrupt, invalid, unavailable).
- `dispatch_memory_json(service, method, params)` — the JSON transport seam
  for out-of-process runtimes: the Node bridge's shim-to-host service frame
  (`service: "memory"`, `method`, `params`) dispatches through it, so JS/Python
  and process extensions use the same contract as in-process Rust ones.

Service identity: name `memory`, contract `bos.memory-records`, version
`1.0.0` (constants in the module).

## 2. Capabilities and governance

Two new capabilities in the ferris vocabulary:

| Capability | Risk | Meaning |
|---|---|---|
| `memory.read` | Medium | list and recall records in the granted scope |
| `memory.write` | High | commit, update, supersede, tombstone records |

Default trust rules apply unchanged: Builtin allows; Verified allows up to
Medium (so `memory.write` needs an explicit grant); Community/Local/Unknown
need explicit grants. Every decision is audited by the existing policy engine.

## 3. Host wiring (BSH)

- The host provides the service PER ACTIVATION at `Scope::Extension(id)`,
  only when the manifest declares a matching `services.requires` entry. The
  instance is bound to the build's resolved memory scope
  (`MemoryScopeResolver`: workspace store + session viewer) and to the
  extension's identity; its revocation token lives in the activation, so the
  service never outlives its consumer's build.
- `KernelMemoryService` implements the contract over `FileRepository`:
  - reads are viewer-scoped BEFORE ranking (the kernel's rule);
  - writes land at the viewer's scope; `memory.write` grant is re-checked at
    the service boundary (defense in depth beyond acquisition);
  - every committed record's provenance gains an `extension:<id>` evidence
    reference, so extension-written knowledge is always attributable;
  - kernel `MemoryError` maps to the stable `memory.*` ExtError codes.
- Acquisition goes through the standard `ServicePolicyAuthorizer`: the
  provider-declared capabilities for `memory` are checked against the
  consumer's grants at acquisition time.

### First-party consumer: com.bos.bsh.memory-tools

The extension-first rule is honored by the reference consumer: a linked-in
builtin extension declaring `memory.read` + `memory.write` and the service
requirement, contributing three agent tools — `memory_recall`,
`memory_remember`, `memory_forget` — each acquiring the typed service through
the registry at call time. It is the worked example third-party packages copy.

### Manifest example

```toml
schema_version = "2.0"
id = "com.example.research-notes"
name = "Research Notes"
version = "1.0.0"
runtime = "declarative"
capabilities = ["memory.read", "memory.write"]

[[services.requires]]
name = "memory"
contract = "bos.memory-records"
version = "^1.0"
scope = "extension"
# The host provides the service at activation time, and memory may be
# disabled: optional keeps the plan clean; acquisition succeeds when present.
optional = true

[[contribution]]
kind = "tool"
id = "save_note"
```

## 4. The BSH memory pane

A right-panel pane (`RightTab::Memory`, deep link `--panel memory`) over the
SAME kernel stores — the pane opens its own handle through the shared
canonical-identity registry, so it never holds a stale snapshot:

- list: content, kind, scope, status, revision, provenance source, updated time;
- search: the hybrid recall seam (multilingual), hits carry score explanations;
- filters: kind and status;
- actions: add (content + kind), edit (revision bump), supersede (history
  kept), delete (tombstone) — each runs on the runner runtime, never on the UI
  thread, and reports through one activity log (pending / succeeded / failed
  with the typed error label), the federation pane's pattern;
- the capture summary of the last completed turn is shown when present.

The pane is host management UI (like the vault or the extension inventory),
not a declarative extension panel: it manages host state. Extension-facing
functionality ships as the first-party extension above.

Reference gap check: the DSH reference has no memory pane; this is a
BOS-native surface drawn in the reference's card/row design language.

## 5. Permission semantics

The memory tools are capability-governed EXTENSION tools, like the inspector
and diagnostics - not permission-gated bundle tools like the shell or the
federation actions. A Read Only session therefore still carries them: their
writes are scoped, attributed, reversible host state (tombstones keep
history), confined by the `memory.write` grant and the activation's session
scope rather than the file permission mode. The file permission mode keeps
its own contract: it confines the filesystem and process families. The two
axes are deliberately separate, and the caps contract tests pin both.

## 6. Compatibility

- The DTO contract is additive; `agent::memory` keeps its own types. The
  conformance test is the guard against drift.
- The `MemoryStore`/`MemoryItem` legacy surface and the binding fixtures are
  untouched; `visible_records` is an additive public read seam.
- Legacy manifests without memory declarations are unaffected; the service is
  only provided when required.
- The kernel's durability, scope and migration policies are unchanged —
  extensions get the same guarantees as the host, never a bypass.
