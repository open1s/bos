---
module: ferris-core, ferris-sdk, bsh::extension, bsh::ui
tags: [extensions, ferris, memory, service-contract, capabilities, ui-pane]
problem_type: feature
issues: []
---

# Extensions and a reader UI on the Durable Record Kernel

## Problem

The Durable Record Kernel (issues #338-#346) gave the host commit-guaranteed,
scoped, evidence-backed memory - but extensions could not reach it, and the
reader had no surface to see or manage what the agent remembers.

## Solution

### 1. A host-neutral ferris contract

`ferris-core::memory` (re-exported as `ferris_sdk::memory`) mirrors the
kernel's schema-2 wire shape as DTOs and defines `trait MemoryRecords`
(records / recall / add / update / supersede / remove / clear) plus
`dispatch_memory_json` for out-of-process runtimes. Ferris never depends on
agent/bsh; a BSH conformance test pins the JSON round-trip both ways so the
shapes cannot drift.

Two capabilities join the vocabulary: `memory.read` (Medium) and
`memory.write` (High), under the existing trust rules and audit.

### 2. Per-activation host provision

BSH provides the service at `Scope::Extension(id)` during activation, only
when the manifest declares the requirement AND the consumer holds a memory
grant. The instance (`KernelMemoryService`) is bound to the build's resolved
workspace store and the activation's session viewer:

- reads are viewer-scoped before ranking;
- writes are HOST-BOUND to the viewer scope (a caller's declared scope is
  advisory - an extension can never publish wider than its session);
- every committed record gains an `extension:<id>` evidence reference;
- the write grant is re-checked at the service boundary;
- the revocation token lives in the activation, so the service never outlives
  the build.

The planner lesson: a host-provided service must be declared
`optional = true` in `services.requires` - the planner resolves service
dependencies BEFORE activation and a non-optional missing provider fails the
whole plan. Optional models "the host may provide this" correctly.

### 3. The first-party consumer

`com.bos.bsh.memory-tools` (builtin) declares the capabilities and the
requirement like any third-party package and contributes `memory_recall`,
`memory_remember`, `memory_forget` - each acquiring the typed service
through the registry at call time. It is the worked example external packages
copy, and it honors the extension-first rule.

### 4. The reader's pane

`RightTab::Memory` (deep link `--panel memory`) over the SAME kernel stores:
the pane opens its own handle through the canonical-identity registry, so it
never holds a stale snapshot. List / search (hybrid recall with score
explanations) / kind filters / history toggle / add / edit / supersede /
delete, each mutation on the GPUI background executor and reported through one
activity log with the kernel's typed error label. The last capture summary is
shown when present. i18n in both locales; theme accessors throughout.

**Visual design (round 2).** The first cut was a flat list of grey boxes; the
pane now follows the federation pane's card stack and the reference's own
design language:

- a gradient **hero** (icon tile, title, subtitle, live count pill, refresh
  and add buttons), so the pane opens with an identity rather than a toolbar;
- a **search card** whose kind filters are colour-coded chips (each kind owns
  one hue; the selected chip wears the kind's tint);
- **record cards** with a coloured left accent bar, a tinted kind glyph tile,
  readable pills (kind with glyph, scope level + shortened id, provenance
  source, lifecycle status), the revision and age, and hover-washed icon
  actions with tooltips;
- an **activity card** with one coloured dot per outcome;
- the pane background is the soft page tint with white cards, the same
  inversion the federation pane uses.

Two layout defects were found and fixed while measuring the result: the
scroll container and its content must be separate elements (folding them into
one `flex_1` scroller floated the content in dead space), and a zero
`updated_at_ms` must read as unknown instead of "20772d" measured from the
epoch.

The pane is host management UI (like the vault or the extension inventory);
extension-facing functionality ships as the first-party extension. The DSH
reference has no memory pane - this is a BOS-native surface in the reference's
card/row design language (honest gap, not a fake mirror).

## Lessons

- The GPUI test scheduler refuses activity from a shared tokio runtime's
  threads. Window-started kernel work runs on `cx.background_executor()`
  with a private current-thread tokio runtime INSIDE the future - the same
  rule the comparison pane documents.
- `tr` takes `&'static str`: dynamic kind/status labels map through static
  key functions, never `format!` into `tr`.
- Service registry generics are Sized: a trait-object service travels as a
  sized `MemoryServiceHandle(Arc<dyn MemoryRecords>)` with Deref.
- Extension tools live in the agent's LIVE overlay (`agent.live_tools()`),
  not the base registry - tests must read the overlay.

## Verification

- ferris-core 246, ferris-sdk 64, ferris-runtime 4 tests green; contract and
  dispatch unit tests included.
- BSH: adapter tests (commit/recall/supersede/tombstone, scope binding, grant
  refusal, error codes), three end-to-end activation tests (provision /
  acquire / revoke, disabled, grant-gated), and two GPUI click tests driving
  the real pane (list/add/delete and search/supersede) against real stores.
- Full bsh lib suite, workspace clippy -D warnings, fmt, rustdoc, and the
  Python/JS binding suites.
