# Ferris Dynamic UI RFC: An Agent-Native Composable UI Runtime

- **Status:** Proposed
- **Date:** 2026-11-12
- **Scope:** BOS capability and federation contracts, Ferris (`ferris-core`, `ferris-runtime`, `ferris-sdk`), the BSH host and its GPUI presentation layer
- **Decision class:** Architecture and migration direction; no implementation authorization is implied
- **Runtime:** cordis-rs hosts surface providers (pinned `cordis-core = 0.6.1`); the contract stays provider-family neutral (§2.4)
- **Extends:** [Ferris Cordis-Inspired Evolution RFC](ferris-cordis-evolution-rfc.md)
- **Related:** [BSH Extension Platform](bsh-extension.md) · [Ferris Framework](ferris-framework.md) · [Ferris SDK](ferris-sdk.md) · [BSH 2.0 Architecture RFC](bsh-2.0-architecture-rfc.md) · [Architecture](../ARCHITECTURE.md)

Normative words are used in the RFC 2119 sense. Code blocks marked *illustrative* fix a shape for review; they are not a committed API. File references name the current implementation, not a target.

## 1. Executive summary

Ferris already has composition, generation-scoped effects, typed services and dependency convergence — the direction set by the Cordis evolution RFC and implemented natively in `ferris-core`. That work deliberately stops at the UI boundary: BSH renders, and Ferris contributes `panel` documents. The boundary is correct about **pixels**. It is incomplete about **capability**.

The surface framework **runs on cordis-rs**: a provider is admitted as a Cordis Plugin, its registration is owned by the apply generation, and teardown is Fiber disposal. The contract is not a Cordis type, though. A surface document, contract and frame are Ferris values, and Cordis types stay inside the adapter. Other provider families lower into the same provider seam, so the runtime is Cordis while the contract remains provider-family neutral (§2.4).

Today a panel is one package's document, mounted by one host, driven by one package's tools. Nothing in the runtime can express "this region is rendered by that capability", "this agent built this interface for this turn", "these two BSH nodes each supply half of this view", or "this control is a capability invocation that must be authorized again when pressed". The composable runtime stops one layer short of the user, and that is the layer where agent products are actually judged.

This RFC closes the gap by making **UI a capability**:

> **Ferris evolves into an Agent-Native Composable Runtime with Dynamic UI and Distributed Capability Federation.**

The result is not a second framework. It is one more contract plane over the composition primitives that already exist:

1. **A surface is a capability.** It has a contract, a version, a provider, a scope realm, declared actions and declared guarantees. It is discovered, resolved, placed, owned and disposed like every other capability, and every action it exposes is authorized like every other call. There is no separate UI permission: placement is host policy and effects are capability decisions.
2. **Composition is by reference, not by merge.** A surface embeds another surface through a *slot*. The host resolves the slot to a provider. Documents are never concatenated, so revisions, authorization and failure stay per owner.
3. **Ownership is a region and a generation.** One presenter owns a surface's root; every embedded provider owns only its region. Revoking a generation revokes exactly that region and nothing else.
4. **Agents author surfaces as proposals.** A turn can present, patch and close an interface under an explicit lifecycle, budget and provenance rule. Agent output is untrusted input, never authority.
5. **Presentation stays native and host-owned.** Ferris never draws a pixel and never learns GPUI types. BSH owns layout, focus, input, accessibility, theming and every privileged affordance.
6. **Distribution projects surfaces as capabilities.** A remote BSH node supplies a surface contract and frames; the local host renders native and revalidates everything. No pixel protocol is required for the default path.

The target sentence is:

> **One Runtime, Multiple Interfaces, Composable Capabilities, Dynamic Experiences.**

### 1.1 What changes, and what does not

| Concern | Today | After this RFC |
|---|---|---|
| Who renders | BSH, per panel | BSH, per surface instance over a host slot catalogue — unchanged owner, finer unit |
| What a package contributes | One `panel` document | A surface contract plus frames; still one contribution kind family |
| Surface providers | Panels from builtin/declarative runtimes, plus the Cordis JS adapter | Any provider family can supply a surface, including Cordis; none is privileged (§2.4) |
| Cross-extension UI | A package must own the whole panel | A slot names another capability; the host mounts it |
| Agent and UI | An agent is a panel *consumer* | An agent is a surface *producer* under a bounded lifecycle |
| Multiple agents | One agent per UI | Presenter/contributor/observer roles over one surface session |
| Multiple machines | No UI contract | Remote surface providers with local validation and dual authorization |
| Ferris role | Extension and capability runtime | Also the UI **contract** runtime; still not a renderer, scheduler or agent |
| BSH role | GPUI host and product workspace | Unchanged, plus a surface host and composition engine |

This RFC does **not** repeal ownership: BOS still owns agents, tasks, workflows and federation membership; BSH still owns the workspace, GPUI and user interaction; Ferris still must not become a UI host, an agent runtime or a remote-node manager. It adds the missing **contract plane** between them.

## 2. Context and current baseline

### 2.1 What exists

| Area | Evidence | What it already gives us |
|---|---|---|
| Declarative UI contract | `ferris-core/src/ui.rs` — `UiDocument` schema `1.0`, ~20 node kinds, `UiLimits` (1000 nodes, depth 32, plus text, list, table, cell and tab limits) and a 1 MiB document ceiling, `UiSession` with a monotonic revision, `accept_action` refusing a stale action | A validated, bounded, host-neutral document with revision-based staleness checks |
| GPUI renderer | `bsh/src/ui/extension_panel.rs` — path-keyed nodes, per-frame palette, panel picker, `render_revision` captured per frame | A renderer already separated from the document model; interaction runs on the GPUI thread while provider calls are scheduled |
| Panel host record | `PanelSurface` in `bsh/src/extension/host.rs` — owner, runtime, trust, policy, tool descriptors, workspace, package root | **Per-action capability authorization and audit already exist** for UI-triggered calls |
| Panel contribution | `docs/bsh-extension.md` §3.6.1 — `panels/<id>.json`, declared view placeholder, `refresh_ms`, theme tokens, asset-only images | A package-shipped view with declared limits and a live read path |
| Composition primitives | `ferris-core/src/composable/` — `Context`, `EffectScope`, `ServiceRegistry`, `Scope`, `Inject`/reconciler, the live runtime | Generation-safe effects, exact service identity, dependency convergence ready to be reused |
| Capability model | `capability/src/` — descriptors, kinds, side effects, risk, invocation limits, federation transport | One vocabulary for discovery, assembly, authorization and remote projection |
| Component ABI | `bsh/wit/extension.wit` — `panel: func(id: string) -> result<string, host-error>` | A portable contract that already carries a UI call; changing it is an ABI version bump |
| Federation | `federation/src/` — identity, membership, delegation, remote capability proxy | Remote execution and delegation; **no** remote UI contract |

### 2.2 Structural limits of the current model

1. **One panel belongs to one package.** A view that needs data from two capabilities must be re-implemented by a package that holds both, or be split into two unrelated panes. There is no composition unit.
2. **A document is replaced, never refined.** `UiSession::apply` swaps the whole tree at a higher revision. A streaming producer re-sends everything, a large surface re-lays out everything, and there is no base-relative update.
3. **Node identity is positional.** The renderer keys nodes by a generated path, and only tree and tab nodes carry ids. Focus, selection, scroll offset and IME state therefore cannot survive a document swap, which is exactly what a dynamic surface does most often.
4. **Actions are package-local names.** A control names a tool or command contribution id of its own package. A composed surface cannot route a control to another provider's capability without making one package the owner of everything.
5. **An agent cannot produce UI.** Panels are declared at install or served by a runtime; a turn has no way to present an interface, and no lifecycle exists for one that ends with the turn.
6. **Providers declare no UI guarantees.** Nothing states whether a provider can stream, patch, work offline, honour accessibility roles or accept theme tokens. Composition cannot negotiate what a provider can actually render.
7. **Refresh is a timer, not convergence.** `refresh_ms` re-reads a card while visible. There is no reactive invalidation from capability state and no defined behaviour when a dependency disappears.
8. **Remote UI has no contract.** There is no statement of what a remote node may project, how it is validated, or which node authorizes an action pressed in a remote-supplied view.
9. **A naming collision is waiting to happen.** `capability::SurfaceName` already means the model-facing *callable* name of a capability. If "surface" becomes the UI term without a rename, `descriptor.surface` will be read as a UI surface by the next contributor.

### 2.3 Behaviour worth preserving

These are not incidental; a new design that loses one of them is a regression:

- **Validated, bounded documents.** A component the host cannot honour is a load error, not a silent fallback.
- **Semantic colour roles in a document.** `tone` is a role resolved against the host palette, so a document never freezes a colour. Package themes are a separate, declared mechanism: a theme contributes hex tokens that are parsed and validated as a palette, not a document colour.
- **No ambient content.** Remote images, HTML, script and arbitrary URLs are refused today and must stay refused.
- **Revision gating.** A control acts on the revision it was drawn from; a stale action is refused rather than applied to a newer frame.
- **Per-call authorization and audit.** A UI action passes the same capability decision as an agent tool call.
- **Host-owned chrome and layout.** Providers describe content; the host owns focus, sizing, scrolling and accessibility mapping.
- **Declared limits.** Node count, depth, text and bytes are bounded before rendering, and the refusal names the limit.

### 2.4 The runtime, and why the contract stays neutral

The surface framework **runs on cordis-rs**. Provider admission is a Cordis Plugin prepare/apply, registration is owned by the apply generation, and teardown is Fiber disposal. The dependency is pinned exactly: `cordis-core = "=0.6.1"` (MSRV 1.88; the workspace is on a newer toolchain). The version was checked by compiling the adapter and running its tests, not by reading a changelog; a bump is a deliberate change that must keep those tests green.

The contract is **not** a Cordis type. A surface contract, document, frame and instance are Ferris values; Cordis types stay inside `ferris_runtime::surface_runtime` and never cross the `ferris-sdk` boundary. That split is what lets one contract hold for every provider family while one runtime owns their lifecycle:

| Surface provider family | How it reaches the runtime | Where its UI work runs |
|---|---|---|
| Native / builtin | Implements the `SurfaceProvider` seam directly | Host process |
| Declarative package view | The shipped `panels/<id>.json` document, validated and lowered | Host process, no code |
| Cordis-compatible runtime | The package's `apply(ctx)` registers UI; its adapter lowers it into frames | cordis-rs host process |
| Process / WASM / Extism | A host-side adapter implements `SurfaceProvider` over the guest call | Provider process or sandbox |
| MCP / workflow | The adapter lowers their output into a frame | Adapter |
| Remote BSH node | Frames and actions over the federation transport | Remote node |
| Agent turn | A `bos.ui/present` document | Host process |

Rules that keep the runtime canonical and the contract neutral:

1. **The contract cannot name a provider family.** No field selects "a Cordis surface", and no node kind is reserved for one runtime.
2. **No family gets a shorter path.** Every frame is validated by the same host validator; every action goes through the same pipeline; every instance gets the same generation, revision floor and disposal report.
3. **Cordis owns provider lifecycle.** A provider is admitted as a Plugin, its registration is removed by a generation-owned effect, and disposal goes through the Fiber. An adapter may not hand a frame to the host around the instance, because that would bypass generation and disposal — the split-brain the runtime exists to prevent.
4. **Other families lower in; they do not replace the runtime.** A process, WASM, MCP or remote adapter implements the provider seam and is hosted by the same Cordis runtime. Its non-UI contributions are untouched: the same package's tools, services and events remain ordinary Ferris capabilities.
5. **Unsupported is declared, not worked around.** A family that cannot honour a profile or a guarantee says so in its contract (§6.1); the host picks a compatible profile or refuses. It does not get a private fallback.
6. **Cordis types stay private.** The SDK re-exports `ferris_core::surface`; it never re-exports `cordis_core`. Swapping the runtime must not change a contract or a document.

The first implementation is the native seam plus the Cordis runtime, with the declarative path lowering into a document. That proves the split before a second runtime adapter exists.

## 3. Goals and non-goals

### 3.1 Goals

- Define a host-neutral **surface capability contract** with identity, version, profiles, actions, guarantees and limits.
- Make **composition** a first-class operation: host slots, provider slots, exact resolution, deterministic tie-breaking, bounded depth and fan-out.
- Define **surface lifecycle and ownership** so that every mounted region has one generation, one owner and one disposal report.
- Define **agent-authored surfaces** with admission, budgets, provenance and a lifetime, without granting the agent ambient UI authority.
- Define **multi-agent surface sessions** with presenter/contributor/observer roles and conflict rules that do not require document merging.
- Specify the **interaction pipeline** that turns a press into an authorized capability invocation and a normalized result back into a frame.
- Specify the **GPUI presentation plane**: caching, coalescing, focus and IME survival, virtualization, theming, accessibility and performance budgets.
- Make **distributed surfaces** expressible: declarative projection by default, dual authorization, staleness and offline rules.
- Keep Manifest 2.0, `ferris-sdk` and existing schema-`1.0` panels working throughout migration.
- Keep the framework testable without BSH, Agent or GPUI dependencies.

### 3.2 Non-goals

Ferris will not own:

- A renderer, layout engine, focus manager, IME implementation or accessibility tree. Those stay in BSH/GPUI.
- A pixel or video protocol. A snapshot projection exists as a fallback, not as the model.
- Agent reasoning, task decomposition, delegation or conversation semantics. Those stay in BOS.
- A UI definition language with control flow, expressions or arbitrary code. The document stays a bounded value.
- A second lifecycle authority. Surface instances are owned by the same composition runtime that owns effects and services.
- An automatic trust rule. A package signature proves integrity, not that its UI is safe to mount.
- Turning the UI into an authorization source. Rendering never grants what a capability check would deny.

### 3.3 Alternatives considered

| Alternative | Why it is attractive | Why it is rejected |
|---|---|---|
| Every surface is an HTML view in the web escape hatch | Fastest path to arbitrary UI; one rendering stack | Loses native performance, theming and accessibility; adds a second trust boundary and a second renderer; kept only as a declared `native-embed` escape hatch (§12.6) |
| Pixel or desktop streaming for remote surfaces | Any renderer works; no schema | Bandwidth, latency, no accessibility tree, no theming, no local composition; kept only as labelled P2 snapshot (§14.1) |
| One global document that every provider patches, with a CRDT merge | One tree; concurrent edits | Turns presentation into a distributed merge problem, blurs authorization per region, and produces conflicts that surface as visual corruption; replaced by slots (§11.2) |
| An agent emits GPUI/Rust code that the host compiles | Unlimited expressiveness | Arbitrary code execution from untrusted output, unreviewable, no lifecycle ownership; rejected outright |
| An agent emits HTML/CSS that the host renders | Familiar authoring | A second styling system enables host impersonation and inaccessible output; rejected |
| A new top-level UI capability kind outside the capability model | Seems simpler | Duplicates discovery, authorization and federation projection; the surface is a capability family with one contract id (§6.1) |
| The UI owns the business state it displays | Fewer round trips | Two sources of truth; the UI becomes authoritative and stale after any external change; the capability owns state (§15.1) |
| Ferris owns the renderer so behaviour is uniform | One implementation | Violates the ownership boundary, pulls GPUI into core and breaks the independence gate; presentation stays in BSH (§4.2) |

## 4. Ownership and planes

### 4.1 Four planes

```text
Intent plane      BOS agents: who wants a surface, for which task, with which grants
                  |
Contract plane    Ferris: what a surface is, who may mount it, what it owns,
                  how it is versioned, resolved, placed and disposed
                  |
Presentation      BSH/GPUI: layout, focus, input, IME, accessibility, theming,
plane             privileged chrome, the surface host and its composition engine
                  |
Distribution      BOS federation: which node supplies a capability, remote
plane             identity, delegation, transport, dual authorization, audit
```

A rule follows from the split: **the contract plane never holds a widget, and the presentation plane never holds an authority.** A GPUI element cannot be serialized, and a surface document cannot grant a capability.

### 4.2 Authoritative ownership

| Resource or lifecycle | Owner | Ferris responsibility | BSH responsibility |
|---|---|---|---|
| Surface contract | Ferris | Define, validate, version, resolve, project | Declare which host slots and profiles it supports |
| Surface instance | Ferris (composition runtime) | Admit, generation, revision floor, state, bindings, disposal | Hold a render cache keyed by instance id, request transitions, draw the state it is given |
| Surface document/frame | Provider | Validate, bound, reject unknown nodes | Validate again as untrusted input, never trust the producer |
| Host slot catalogue | BSH | Treat as host-declared targets; do not invent regions | Declare, version, constrain, document |
| Provider slot | Surface provider | Declare in the contract, bound depth and fan-out | Resolve, mount, contain failure |
| Action dispatch | BOS capability layer, invoked by Ferris/BSH | Build the request, enforce generation and binding | Capture principal, press context, show pending state |
| Privileged affordance (permission, consent, trust) | BSH | Refuse provider-declared impersonation | Render from host copy only, always outside a surface frame |
| Agent turn | BOS Agent Runtime | Expose the surface-contribution capability | Present and dispose with the turn |
| Remote node and remote task | Federation protocol, provider node | Export local surface descriptors | Render the projection, enforce local policy |

Rules:

1. **One presenter per surface instance.** A second provider cannot take a mounted root; it must request a handoff, which is host-mediated and audited.
2. **One generation per region.** A region's authority ends with its generation, even if an old caller still holds a reference.
3. **The host owns every pixel it did not delegate.** Chrome, empty states, skeletons, error cards, confirmations and provenance badges are host-rendered.
4. **The provider owns only its region.** A provider cannot resize a sibling, move a host region or overlay host chrome.

### 4.3 A naming collision to resolve before Phase 1

`CapabilityDescriptor::surface` (`capability/src/descriptor.rs`) is a `SurfaceName`: the name a *model* calls. This RFC uses "surface" for a *renderable region*. Two meanings on one word will produce a wrong reading in exactly the code that joins the two systems.

Decision: rename the callable-name type to `CallableName`, keep a deprecated alias for one release, and reserve **Surface** for the UI contract:

```rust
// illustrative
pub struct CallableName(String);          // was SurfaceName: the model-facing name
#[deprecated(note = "renamed: this is a callable name, not a UI surface")]
pub type SurfaceName = CallableName;
```

The field on the descriptor becomes `pub callable: CallableName` with a serde alias for the wire name, so no serialized descriptor breaks. This is a prerequisite, not a preference.

## 5. Domain model

### 5.1 Definitions

| Term | Meaning |
|---|---|
| **Surface** | A named, versioned, addressable interactive region supplied by one provider |
| **Surface contract** | The typed description of that surface: profile support, state model, actions, slots, limits, guarantees |
| **Surface provider** | Whatever answers the contract: a native host component, a declarative package view, a Cordis-compatible runtime, an agent turn, a process/WASM/Extism extension, an MCP server or workflow adapter, a remote node |
| **Surface instance** | One mounted occurrence of a surface in one scope, with a generation and a revision floor |
| **Surface document** | The validated, bounded value the provider returns for one revision |
| **Surface frame** | A document plus its envelope: instance, generation, revision, digest, truncation and budget use |
| **Surface action** | A declared binding from a control interaction to one capability reference and an input mapping |
| **Surface session** | The host-side object that owns one surface instance, its presenter, contributors and observers |
| **Host slot** | A host-declared mount point with layout constraints, a required profile and a documented purpose |
| **Provider slot** | A mount point a surface declares in its own contract, so other capabilities can extend it |
| **Profile** | The rendering affordance level a host asks for: `full`, `compact`, `text`, `machine` |
| **Presenter / contributor / observer** | The provider that owns the root / a provider that owns one slot / a principal that only reads |

Terminology: the UI term is **ActionBinding**. `capability::composition::Binding` keeps its existing meaning — a capability-to-provider binding, including `Binding::Remote { node, grant }` — and an unqualified "binding" in this RFC means an action binding.

### 5.2 Identity

```rust
// illustrative
pub struct SurfaceId {        // provider-qualified, stable across restarts
    pub provider: ProviderId,
    pub name: String,         // provider-local, stable for the provider's life
}
pub struct InstanceId(uuid::Uuid);   // host-scoped, created at admission
pub struct Generation(u64);          // incremented when the provider is replaced
pub struct Revision(u64);            // incremented per frame within one generation
```

- A **package id never identifies a live instance**. One package may mount the same surface in two scopes; two packages may each contribute a surface to the same slot.
- A **generation replaces a revision silently at the caller's peril**. A frame, patch or action names both; a generation mismatch is refused before any other check.
- A **revision is monotonic per instance**, not global. Patches are base-relative and never rebase silently.
- Instance ids are opaque to providers. A provider that stores an instance id across a restart must treat it as stale.

### 5.3 State ownership

| State | Owner | Survives a frame? | Survives a remount? |
|---|---|---|---|
| Authoritative domain state (a diff, a job list, a session) | The capability, never the UI | Yes, it is re-read | Yes |
| Surface view state (which section is open, selected tab) | Provider, in its frame | Yes | Provider's choice |
| Ephemeral interaction state (focus, caret, IME composition, scroll offset, hover, drag) | Host, per instance | Yes, keyed by node id | Yes, keyed by slot identity (contract + host slot + owner) |
| Pending state of an action (spinner, disabled) | Host, per binding | Yes, until the call returns | No |
| Provenance, presenter, contributors | Host, per session | Yes | Rebuilt from the descriptor |

The rule that makes this testable: **the host never mirrors business state into the UI model, and the provider never observes host focus state.** A provider that wants to know what the reader selected asks through an action or declares the selection in its own frame.

### 5.4 The composition unit: a slot

A slot is a node in a document that names a *contract*, not a package and not a URL:

```json
{
  "type": "slot",
  "surface": "acme.git/status",
  "version": "^1.2",
  "required": true,
  "props": { "repo": "/work/bos" },
  "sizing": { "weight": 1, "min": 180, "max": 520, "overflow": "scroll" },
  "fallback": { "type": "text", "text": "Status view unavailable" }
}
```

- The host resolves the slot at mount, not at render. A **required** slot that resolves to nothing is a composition error, reported with the candidates considered; an **optional** slot draws its declared fallback, labelled as a placeholder.
- The resolved child is a **separate instance** with its own generation, revision, action bindings, authorization context and disposal report.
- A slot is not a capability call. Rendering does not invoke the child's capabilities; it mounts the child's surface.
- Slot props are data, bounded and typed by the **child contract's props schema**; they are not code and are not interpolated into text. A slot without a declared props schema accepts no props at all.
- Props select *which* already-authorized call a capability performs; they never widen authority. A capability that accepts a security-sensitive selector (a path, a host, an account) must authorize it as part of its own scope, because otherwise a hostile parent could mount a child and aim the child's grants at a target the parent chose — a confused deputy reached sideways.

## 6. Contracts

### 6.1 Surface contract

```rust
// illustrative
pub struct SurfaceContract {
    pub contract: ContractId,          // "bos.ui.surface" — one contract family
    pub contract_version: Version,     // the family's version, not the provider's
    pub surface: SurfaceId,            // provider + stable name
    pub display_name: String,          // untrusted label: picker and catalogue only, never host chrome
    pub description: String,           // untrusted prose, shown muted
    pub profiles: ProfileSet,          // full | compact | text | machine
    pub state: StateModel,             // static | on_demand | stream | snapshot
    pub actions: Vec<ActionSpec>,      // every binding a frame may name
    pub slots: Vec<SlotSpec>,          // mount points this surface offers
    pub limits: SurfaceLimits,         // frame, patch, action and instance budgets
    pub guarantees: ProviderGuarantees,
    pub trust: TrustLevel,             // provenance: builtin | verified | community | local | unknown
    pub isolation: IsolationClass,     // in_process | process | wasm | remote
}
```

StateModel is the part the current contract lacks, and it decides how the host updates a region:

| State model | Host behaviour | Typical provider |
|---|---|---|
| `static` | Render the shipped document; never re-read | A declarative package view |
| `on_demand` | Read once per mount and on explicit refresh | Today's panel runtime |
| `stream` | Subscribe; apply frames and patches as they arrive | An agent turn, a live job list |
| `snapshot` | Fetch a bounded, cacheable projection; no local interaction | A foreign or remote renderer |

ProviderGuarantees is mandatory metadata, not documentation:

| Property | Required declaration |
|---|---|
| Isolation | in-process native, extension process, Cordis/Node bridge, WASM guest, remote node |
| Update | `frame_only` or `frame+patch`; maximum patch ops |
| Streaming | supported or not, and the maximum frame rate |
| Offline | `last_known_good` or `none` |
| Determinism | whether the same props and revision always yield the same document |
| Theming | accepted tone tokens; no raw colours are ever accepted |
| Accessibility | the roles the provider promises to set for its interactive nodes |
| Interaction | `read_only` or `interactive`; which action modes it may emit |

A provider that cannot honour a property declares it unsupported. The host then chooses a profile or refuses the mount; it does not silently degrade an interaction.

### 6.2 Interface profiles

"One runtime, multiple interfaces" is not a slogan if the contract can say what a surface looks like when there is no window.

| Profile | Node vocabulary | Actions | Used by |
|---|---|---|---|
| `full` | Every node kind | Binding table dispatched locally | BSH/GPUI |
| `compact` | Text, heading, list, table, button, input, select, checkbox, notification, divider | Same binding ids, host-drawn compact controls | Narrow docks, small windows |
| `text` | Text, heading, list, table, code, progress, notification | Numbered choices the reader types or an agent selects | CLI, TUI, a terminal-only session |
| `machine` | No layout: the frame is returned as structured data | Action descriptors only, no dispatch | An agent consuming another agent's surface, a test harness |

Rules:

- A provider **MAY** support a subset of profiles; it **MUST NOT** claim one it cannot render.
- Action ids are identical across profiles. A profile may hide an action only by declaring it in `hidden_actions: [{ action, reason }]`. A destructive action may not appear in that list at all: its confirmation is part of the action, and a profile that cannot draw the confirmation must refuse the action rather than hide it.
- The host picks the highest mutually supported profile and records the choice in the session, so a support report can answer what this reader actually saw.
- The `machine` profile is what makes a surface a normal capability for an agent: an agent can read another agent's interface as data without a window.

### 6.3 Surface document 1.1

Schema `1.1` is a **strict superset** of `1.0`, so every existing panel document remains valid. Three additions carry the dynamic model.

**Stable node ids.** Every node may carry `id`; every interactive node **MUST**. Ids are unique within a document, stable across frames of one instance, and the key for host-owned focus, selection, scroll and IME state. A patch addresses nodes by id. `Insert` and `Replace` may introduce new ids; an existing node's id may not change, and an id may not be reused after `Remove`, so a stale focus key or caret can never land on a different control. Attaching an id to a node that already exists therefore requires a full frame.

**A binding table.** Controls name a binding key; the document carries the table.

```json
{
  "schema_version": "1.1",
  "bindings": {
    "refresh": { "capability": "acme.git/refresh", "input": { "repo": "$props.repo" } },
    "open":    { "capability": "acme.git/open_file", "input": { "path": "$event.value" }, "required": true }
  },
  "root": {
    "type": "column",
    "children": [
      { "type": "button", "id": "refresh", "label": "Refresh", "action": "refresh" },
      { "type": "slot", "surface": "acme.git/diff", "version": "^1", "sizing": { "weight": 1 } }
    ]
  }
}
```

The table exists for three reasons: it is a fraction of the bytes of repeating a capability reference on every control (a real cost when a model emits the frame), it is resolved **once at mount** so a dead control is a mount error rather than a click that fails, and it keeps the set of capabilities a surface can reach enumerable for authorization.

**The slot node**, defined in §5.4.

The mapping language is deliberately not a language:

| Expression | Meaning |
|---|---|
| `$event.value` | the submitted text of the control |
| `$event.checked` | the checkbox state |
| `$event.option` | the selected value |
| `$props.<name>` | the mount prop named by the suffix |
| any JSON literal | that literal, unchanged |

No attribute access beyond one level, no arithmetic, no interpolation, no conditionals. A string that begins with `$` and is not one of the forms above is refused rather than passed through; a literal that must begin with `$` is written as `{ "$text": "$..." }`. A provider that needs a computed argument computes it in its capability and returns it in the next frame, or declares it in props.

### 6.4 Action bindings and principals

Every action binding names one of exactly two **dispatch principals**. They are not a new identity system: they select which existing principal performs the call (§13.3), and the call is built as the existing `InvocationContext` over `capability::Principal`.

| Principal | Meaning | What it may name |
|---|---|---|
| `producer` (default) | Runs with the **producer's** grants — the package's, or the agent session's for an agent-authored surface, like today's panel action | A capability the producer holds **now**. A manifest requirement governs what may be *declared*; it never becomes a grant |
| `reader` | Runs with the **reader's** own authority, which is the host session's principal | Only a host-owned allowlist of chrome actions (focus, open settings, copy, close) |

A binding may never name an arbitrary command as the reader, and may never run with the union of both authorities. This single rule removes the most attractive escalation path in an agent-authored UI: a surface that visually says "click to approve" while the call underneath runs with the producer's or the agent's authority.

The existing panel dispatch leaves the call's session empty, so a reader-principal binding cannot be authorized today. Phase 2 must carry the host session into `CallContext`; until then, `reader` bindings are refused at mount rather than guessed.

Mount checks for the whole table:

1. Every binding parses and maps only the allowed expression forms.
2. Every required binding resolves to a capability that exists, is visible in this scope, and is held by the dispatching principal **now**.
3. A non-required binding that cannot resolve mounts disabled, with host copy naming why; it never mounts enabled and fails at press time.
4. A binding whose capability risk is critical and whose principal is `reader` is refused unless the capability is in the host allowlist with an explicit confirmation class.
5. A binding may not use a free-text input to feed a capability that reads secrets or declares the `auth` side effect. A typed field that asks for a credential is the one interface an agent-authored surface must not own, and refusing the binding is the control — a provenance badge is not.

### 6.5 Frames and patches

```rust
// illustrative
pub struct SurfaceFrame {
    pub instance: InstanceId,
    pub generation: Generation,
    pub revision: Revision,
    pub base_revision: Option<Revision>, // None: full document, Some: base of a patch
    pub doc: Option<SurfaceDocument>,    // None on a patch-only frame
    pub patch: Vec<PatchOp>,             // empty on a full frame
    pub digest: [u8; 32],                // content digest of the resulting document
    pub truncated: bool,                 // the provider hit its own budget and says so
}

pub enum PatchOp {
    Replace { id: NodeId, node: SurfaceNode },
    Insert  { parent: NodeId, index: u32, node: SurfaceNode },
    Remove  { id: NodeId },
    SetText { id: NodeId, text: String },
    SetProps { id: NodeId, props: serde_json::Value },
    SetBindings { changed: BTreeMap<BindingKey, Option<ActionBinding>> },
}
```

Rules:

- `base_revision` **MUST** equal the instance's current revision. A mismatch is refused as `StaleRevision { current }`, returned to the producer so it can re-read and retry. Silent rebasing is forbidden: it would let a producer overwrite state it never observed.
- A patch is applied to a staged copy, validated in full (ids, limits, profile, bindings), and committed atomically. A patch that fails validation changes nothing.
- A structure change and a binding change in one frame are ordered: bindings first, so a newly inserted control cannot be pressed against an old table.
- `truncated` is an explicit provider statement, not an error. The host draws a host-owned partial-view notice; it never pretends the frame is complete.
- The host **MAY** coalesce frames that arrive within one frame interval; it **MUST NOT** coalesce across a generation change.

### 6.6 Validation and compatibility

Validation runs twice: once in the provider's own process (so a mistake is caught near its cause) and always again in the host (because the provider is not trusted). The host check is the normative one.

Checks: schema version, node vocabulary, node count/depth/text/bytes, unique ids, binding grammar, binding resolution, slot graph (cycles, depth, fan-out), profile conformance, digest and declared truncation.

Compatibility:

- The document schema version and the contract version advance independently. A host reads `1.0` and `1.1`; a `1.2` document is refused with `SchemaNewer`, never partially rendered, because skipping an unknown node would skip its authority.
- Within a major version, the node vocabulary is additive; a node kind is never redefined.
- A contract's profile set is part of its identity. A provider that adds a profile need not change its name; a provider that drops one is making a breaking contract change.

## 7. Composition and resolution

### 7.1 Host slots

The host publishes a slot catalogue. It is the *only* set of places a surface may appear, and each entry carries layout constraints, a required profile and a purpose. Providers cannot invent a host region; adding one is a host release, reviewed like any other seam.

| Slot | Constraints | Purpose |
|---|---|---|
| `workspace.main` | weight 1, min 320x240 | The primary work surface |
| `workspace.left` / `workspace.right` | min 220, max 640 | Side panels |
| `workspace.bottom` | max height 40% | Docks, terminals, logs |
| `workspace.statusbar.leading` / `.trailing` | height 24, inline | Small persistent indicators |
| `transcript.inline` | full width, host row rhythm | A turn's own result surface, between messages |
| `settings.page` | fills the dialog body | A settings page |
| `command.palette.row` | one row | A command source |
| `empty.hero` | centred, max 640x480 | First-run and blank-session content |

A host slot has an **occupancy rule**: `single` (one presenter; a second mount is a conflict), `stacked` (ordered, host-tabbed) or `inline` (many small rows). The rule is declared once and enforced by the host, not negotiated per provider.

A host slot also declares a **minimum provenance** and whether agents may mount there. `workspace.*`, `transcript.inline` and `empty.hero` accept community packages, and `transcript.inline` and `workspace.main` accept agent-authored surfaces. `settings.page` and `command.palette.row` are first-party/builtin only, because filling them *is* impersonating the host's own dialog or command surface. A slot with no declared rule accepts no agent surface.

### 7.2 Resolution

- A slot names a **surface contract** (provider-qualified id) and a semver range. It does not name a package: renaming or repackaging a provider must not break composition.
- Resolution checks existence, version range, profile support, scope realm, whether the provider's package (or the presenting tool) is enabled for this workspace and preset, and the host slot's occupancy and provenance rule. None of these is a UI-specific grant: enablement is the existing package/preset/tool policy, and placement is the host slot rule.
- **There is no second resolver.** A slot lowers to the existing `CapabilityRequirement` and goes through the host resolver, so pinned-peer, priority and spec order keep their current meaning (`capability/src/composition.rs`). The RFC adds no new ordering rule.
- **Ambiguity is an error.** When the resolver cannot decide, the host reports every candidate with its version, provider and scope. It never picks one by directory order or hash order.
- **No implicit fallback.** A slot never silently resolves to a nearby contract, a different version or a parent scope. The declared `fallback` node is drawn only when the slot is optional and unresolved, and the host labels it as a placeholder.
- Scope realms are exact, by the same rule as service lookup in Ferris (evolution RFC §6.5). A workspace-scoped surface is not visible in another workspace unless the host declares an explicit projection.

### 7.3 Failure containment

Composition exists so that one provider's failure is a region, not a window.

| Failure | Host behaviour |
|---|---|
| Slot unresolved, slot required | The instance stays Pending; the region draws a host-owned skeleton with the reason, keeps its min size, and mounts on provider convergence |
| Slot unresolved, slot optional | Draw the declared fallback, labelled as a placeholder |
| Provider refuses to mount | Host error card in the region, with a retry action and a host-only detail line |
| Provider frame invalid | Keep the last good frame, mark stale, disable bindings that changed |
| Provider exceeds its budget | Refuse the frame with the specific limit, keep the last good frame |
| Provider panics | Unmount the instance, draw the error card, emit an audit event; siblings unaffected. For an in-process native provider the adapter boundary must be unwind-safe (`catch_unwind`) and the contract must declare `unwind_safe: true`; a provider that cannot make that promise runs in a process runtime instead |
| Provider returns an error message | Show it muted and escaped, attributed to the provider; never inside host chrome |

The host error copy is host-owned and translated. A provider cannot supply text that appears in host chrome, because that is how a surface would impersonate the host. The same rule covers a surface's `display_name` and `description`: they are provider text, escaped, host-styled, attributed, and never allowed to stand in for a host label such as a permission prompt or the provenance badge.

### 7.4 Dependency convergence

Slots are dependencies, and convergence is a **runtime** decision, not a host one: the host renders the region state that convergence produces.

- Surface requirements are composition dependencies over surface contracts — the same mechanism as service-keyed `Inject`, generalised from a service name to a `(contract, version, realm)` requirement. They are resolved and re-evaluated inside `ferris-core`.
- A required slot with no provider leaves the instance **Pending**, not Live with an empty region.
- When a provider appears, eligible consumers mount under the same policy and version checks.
- When a provider disappears or is revoked, consumers are notified **before** their next interaction, and their bindings to the vanished surface are disabled. The host keeps the last good frame visible with a stale marker.
- Provider replacement does not expose a window in which a consumer renders frames from an unvalidated provider: the new generation is admitted and mounted, then bound atomically, then the old one drains.
- Convergence is serialized per scope and generation-checked in the runtime. The host observes one state value per instance per change and never a dependency graph; it must not reimplement the reconciler. `Reconciler` is currently crate-private in `ferris-core/src/composable`, so Phase 3 exports a surface-aware view of it rather than copying its logic into BSH.
- The `refresh_ms` timer is legacy compatibility for `on_demand` providers, not the convergence mechanism.

### 7.5 Bounds

| Bound | Default | On violation |
|---|---:|---|
| Host slot depth | 4 | Refuse the mount, name the path |
| Provider slot fan-out | 8 per slot, 32 per instance | Refuse the extra mount, keep the rest |
| Instances per scope | 32 | Refuse with the scope and the count |
| Frame nodes / bytes | 1000 / 1 MiB | Refuse, keep last good frame |
| Patch ops / bytes | 256 / 64 KiB | Refuse, return current revision |
| Bindings per document | 64 | Refuse at mount |
| Actions per instance per second | 20 | Refuse with `RateLimited`, keep pending state honest |
| Frames per instance per second | 10 coalesced, 30 declared maximum | Refuse the excess frames, keep the latest |

All bounds are host policy with declared defaults; a package cannot raise them.

## 8. Lifecycle

### 8.1 Instance states

```text
Declared -> Resolved -> Admitted -> Pending -> Mounted -> Live
                                                           |  ^
                                                    Degraded  |
                                                           v  |
                                                      Unmounting -> Disposed
Any non-terminal state -> Failed(cause)
```

- **Declared**: a slot names a contract; nothing is running.
- **Resolved**: exactly one provider is chosen and its contract is validated.
- **Admitted**: the composition runtime created the instance, its generation and its effect scope. No live effect exists yet.
- **Pending**: a required slot dependency is unsatisfied; the region is admitted and drawn as a host skeleton, not Live. This is a state, not an error, and it is reported to the reader as the reason it is waiting.
- **Mounted**: the host has a first valid frame and the region has bounds.
- **Live**: frames and actions flow.
- **Degraded**: a dependency vanished or a frame was refused; the last good frame is shown, bindings may be disabled.
- **Unmounting -> Disposed**: bindings revoked, subscriptions stopped, provider resources released, report emitted.

A retry creates a new attempt; it never resurrects a disposed generation.

### 8.2 Generation replacement

1. Validate the new contract and prepare it without live effects.
2. Resolve its dependencies and authorize its bindings.
3. Admit the new generation in a non-serving state.
4. Mount it; verify a first valid frame and its digest.
5. Atomically rebind the region to the new generation.
6. Stop accepting actions on the old generation.
7. Drain in-flight actions to a deadline.
8. Revoke old bindings, subscriptions and child mounts.
9. Dispose the old generation and emit a replacement report.

If steps 3-5 fail, the old generation stays authoritative and the new attempt is reported. If old-generation disposal fails after the switch, the new generation stays authoritative while the manager tracks the leaked resources for reconciliation — the same rule as the extension lifecycle.

### 8.3 Refusals

| Refusal | When | Reader sees |
|---|---|---|
| `StaleRevision { current }` | A frame, patch or action names an old revision | Control disabled, host refresh affordance |
| `StaleGeneration` | Any message from a replaced generation | Nothing; logged |
| `UnknownBinding` | A control names a binding not in the table | Host error card at mount |
| `Denied { capability }` | A binding failed authorization at mount, or a dispatch was denied | Disabled control with host policy copy |
| `NoProvider` / `AmbiguousProvider` | Slot resolution | Skeleton or error card with candidates |
| `ProfileUnsupported` | No common profile | Text fallback, labelled |
| `BudgetExceeded { limit }` | Frame, patch, action rate | Last good frame plus a partial notice |
| `SchemaNewer` | Document newer than the host reads | Error card, no partial render |
| `Untrusted` | A profile or node class the trust level forbids | Error card |

Every refusal is one value with one host copy key, so the UI never invents an explanation and the audit record and the screen agree.

### 8.4 Teardown

Teardown uses the existing `EffectScope` ownership model end to end, not a parallel one:

1. Refuse new actions and frames for the generation.
2. Cancel or drain in-flight actions to the deadline.
3. Unmount children (deepest first), then stop subscriptions.
4. Release provider resources.
5. Release the instance's effect scope and emit a disposal report with failures and timeouts.

Disposal is idempotent, reverse-ordered, attempts every cleanup after one fails, and never relies on `Drop` for asynchronous work. A host closure (window close, workspace switch) disposes the whole scope in one report.

### 8.5 Crash isolation

A provider panic is caught at the adapter boundary, never around the render pass: the document is validated data, so the render path has no untrusted control flow to guard. A panicking provider unmounts its own instance and nothing else. A panicking *host* render is a host bug and fails loudly; swallowing it would leave a half-painted frame that looks like a provider error.

## 9. Interaction and the action pipeline

Every press follows one logical sequence. It mirrors Ferris's capability invocation pipeline (evolution RFC §6.9), because a press **is** a capability invocation:

1. Capture the rendered revision, the node id, the event and the reader's principal.
2. Look up the binding in the frame's table.
3. Check instance generation and revision are current; refuse stale.
4. Resolve the dispatch principal (producer or reader) and the requested scope, and build the call on the existing `InvocationContext`/`capability::Principal` rather than a host-local identity.
5. Authorize the exact capability, resource and action **at dispatch time** against current grants.
6. Confirm when the action's risk class or its `confirmation: host` requires it: the host draws its own confirmation outside the frame, records the decision, and stops here if the reader declines. A surface cannot draw, skip or pre-answer this step.
7. Reserve concurrency and the action-rate budget.
8. Materialize the input from the mapping and enforce size and depth limits.
9. Dispatch through the capability layer with a deadline and a cancellation token.
10. Normalize the result: frame, patch, message, error or denial.
11. Update the instance's revision (frame/patch), the host pending state, and the audit record.
12. Release the reservation and any action-owned effect.

Host-owned interaction rules:

- **Pending state is host state.** A pressed control shows a host-drawn pending affordance; the provider does not need to re-render to say "working".
- **Double dispatch is refused by default.** A binding declares `mode: once` (default) or `repeat`; `once` ignores a second press until the first settles.
- **Change events are debounced.** An input or select declares `debounce_ms` (0-500); a keystroke never becomes a capability-call storm.
- **Cancellation reaches the provider.** Unmounting cancels in-flight actions through the provider's declared cancellation mode; a dropped future is not evidence the work stopped.
- **Every outcome is visible.** A denial, a timeout and a provider error each have distinct host copy; none is silent.

Outcome handling:

| Result | Host action |
|---|---|
| A frame at a higher revision | Apply as a full frame |
| A patch with a valid base | Apply atomically, advance revision |
| Plain text | Show as a message under the control, at the current revision |
| A structured refusal | Restore the control, show the mapped host copy |
| A newer generation | Ignore; the region already moved |

## 10. Agent-authored surfaces

### 10.1 Capability surface

An agent gets one capability family, not a private UI back door. The ids below are capability ids (provider `bos.ui`):

```text
bos.ui/present   open a surface in a host slot for this turn or session
bos.ui/patch     apply a base-relative patch to a surface this agent owns
bos.ui/close     dispose a surface this agent owns
bos.ui/catalogue return the component catalogue, profiles and limits this host accepts
```

They are ordinary capabilities: named, described, risk-classed, authorized per call, audited. They are contributed by a first-party BSH extension through the normal manifest and effect path, which satisfies the extension-first rule rather than adding a privileged in-core shortcut.

The gating chain has four links and **no fifth, UI-specific permission**: whether the host offers the tool at all is ordinary tool policy (the preset/bundle switch that already decides what a build carries); where a surface may appear is the host slot's provenance rule (§7.1); what it may do is each action binding's per-call capability decision (§9); and how much it may spend is the surface budget. A `ui.surface.*` grant would be a second permission system for decisions those four already make, so this design deliberately has none. The existing `ui.panels` manifest capability keeps its current meaning for declared panel contributions; the surface contract adds no requirement of its own.

An agent **composes** capabilities; it does not **create** them. A turn may build a new surface out of capabilities it already holds, but it cannot mint a capability, widen its own grant or invent a host affordance. "Dynamic interaction capability" therefore means a dynamically composed experience over statically authorized capabilities — the distinction is the whole security model, and it is not negotiable.

`bos.ui/present` input (illustrative):

```json
{
  "slot": "transcript.inline",
  "lifetime": "turn",
  "title": "Review 3 changed files",
  "profile": "auto",
  "document": { "schema_version": "1.1", "bindings": {}, "root": {} }
}
```

### 10.2 Admission

A presented document is a **proposal**. The host admits it only if all of the following hold:

1. The host offers `bos.ui/present` to this build or session — ordinary tool policy, not a UI permission. A tool the preset switched off cannot present, and the refusal names the tool.
2. The slot exists, is free under its occupancy rule, and supports a common profile.
3. The document validates under schema, limits and profile.
4. Every required binding resolves to a capability the calling agent holds **now**, or the agent omits it.
5. The agent's active-surface budget allows another mount.
6. The target slot accepts agent-authored surfaces, and the slot belongs to the caller's session and window — a turn cannot present into another session's `transcript.inline`.
7. The agent is not attempting a host-native affordance, and no binding uses a free-text input to reach a secret- or auth-effect capability (§6.4).

Refusals are structured and immediately usable: the error names the failed limit, the current revision, or the unresolved capability, so the model can correct in one retry instead of guessing.

### 10.3 Lifetimes

| Lifetime | Ends when | Restored on restart |
|---|---|---|
| `turn` (default) | The turn that created it completes or is stopped | No |
| `session` | The session closes, the reader closes it, or the app restarts | No |

A turn surface is disposed with the turn, including a stopped or failed turn. It is not left behind as a dead pane and is not silently promoted to a longer lifetime; keeping an interface means promoting it to a package surface or a stored workspace view, which is an explicit, reviewed act rather than a flag on a prompt.

The second row is deliberately "No". An agent-authored surface's provider was a turn's capability; after a restart nothing can re-serve its frames, and a cached document without a provider would be a view whose actions cannot be authorized. The host therefore restores package and host surfaces only, and tells the reader when a session-scoped agent surface is gone instead of drawing a dead frame.

### 10.4 Provenance and privilege

- Every agent-authored surface is drawn inside a host frame that names the producing agent, the turn, and its lifetime. The badge is host chrome and cannot be covered, styled or removed by the document.
- An agent-authored surface **cannot** render a permission, consent, trust or policy prompt. Those are host affordances, drawn outside any surface, from host copy.
- An agent-authored surface cannot claim to be another principal, another extension or the host. The title it supplies is displayed as untrusted content, never in the host title bar.
- The reader can always close a surface. A surface cannot trap focus, suppress its close control or open another surface on its own authority.
- Text that claims host authority ("permission required", "allow access", "system") is refused at admission through a declared label class. This is defence in depth behind the binding rule in §6.4, not the primary control: the structural refusal of credential-shaped bindings is what actually closes the hole.

### 10.5 What agent-authored UI is for, and what it is not

Legitimate and valuable: a diff review surface with per-file actions, a clarification form with typed options, a job dashboard a turn produced, a result table that beats prose, a debugging view of the agent's own plan.

Not legitimate: a fake permission dialog, a fake error that asks for a credential, a control that triggers a capability the agent does not hold, an interface that hides the fact that it is agent-produced, or a surface whose only purpose is to consume reader attention beyond the task.

### 10.6 Prompt economy

A UI contract that is expensive to emit will be emitted badly. The host therefore provides:

- `bos.ui/catalogue`: the exact node vocabulary, field names, limits and available slots, with a minimal example, so a model does not invent adjectives or guess limits.
- The binding table instead of per-control capability references.
- A refusal that states the failed check and the current state, so the retry is a correction, not a rewrite.
- An **enforced** default budget for agent-authored surfaces: 120 nodes and 16 KiB in `SurfaceLimits`, not a recommendation. Host policy may loosen it; the agent cannot. A larger interface declares `truncated` and pages through actions rather than shipping a 1000-node frame.

## 11. Multi-agent surface sessions

### 11.1 Roles

| Role | Power | Drawn as |
|---|---|---|
| **Presenter** | Owns the root document; declares slots; can add and remove contributors | The surface's primary attribution |
| **Contributor** | Owns exactly one slot assigned by the host; patches only its own instance | A secondary attribution on its region |
| **Observer** | Reads frames and actions; cannot mutate | Nothing, or a presence mark |

A surface session has exactly one presenter at a time. The presenter's agent is the one whose turn presented the surface; a contributor is admitted by the presenter's explicit call or by the host when a package's surface is resolved into a declared provider slot.

Admission is an operation, not an implication: the presenter calls `bos.ui/contribute { instance, slot, producer }` and `bos.ui/revoke { instance, slot }`, each authorized and audited like every other surface capability. Presence is an expiring heartbeat — a contributor renews it with its frames or a keep-alive — and an expired contributor is unmounted without touching its siblings. A contributor that cannot be reached is reported as absent, never assumed to be live.

### 11.2 Why there is no document merging

Two agents patching one document is a conflict problem with no good answer at the UI layer: last-write-wins loses work, and a merge algorithm turns presentation into a distributed system. The slot model removes the problem instead of solving it:

- Each contributor's document is its own instance with its own revision.
- The presenter cannot edit a contributor's subtree; it can only unmount the slot.
- A contributor cannot insert siblings outside its slot.
- The host never interleaves two documents, so there is no merge, no rebase and no silent overwrite.

### 11.3 Handoff

Presenter transfer is explicit and audited:

1. The incoming presenter calls `bos.ui/present` with `handoff: <instance>`.
2. The host checks the caller's tool enablement and the slot's provenance rule, validates that the incoming presenter can resolve the root's bindings, and asks the outgoing presenter to consent. A caller cannot take another principal's root by naming it: consent, or an explicit host policy limited to the same owner and provenance class, is required, and a refusal is a recorded state the reader can see.
3. The outgoing presenter drains; its in-flight actions complete or time out.
4. The new generation is admitted, mounted and bound atomically; contributor slots survive because their contracts, not their parents' documents, define them.

A contributor whose slot the new presenter does not declare is unmounted with a host notice, not silently dropped.

### 11.4 Concurrency rules

- Revisions are per instance and monotonic; the host serializes applies per instance.
- A late patch is refused with the current revision, so the producer can re-read. There is no last-write-wins at any level.
- A contributor's action binds to its own provider principal; it does not inherit the presenter's grants.
- Presence is a host fact (which contributor is live), not document content; a contributor cannot draw another contributor's badge.
- Terminating one contributor disposes only that slot; the rest of the surface keeps its revision.

### 11.5 Distributed contributors

A remote agent can contribute to a local surface session only through an explicit federation delegation grant, with its patch channel crossing the node boundary. Its frames are revalidated locally as untrusted input, its bindings may target only capabilities hosted on its own node, and the local host never lets a remote contributor mutate the root (§14).

## 12. GPUI presentation plane

### 12.1 Structure

```text
Shell (existing GPUI entity)
  └─ SurfaceHost (entity, one per window)
       ├─ slot map: host slot -> ordered instance ids
       ├─ cache: InstanceId -> RenderCache
       │    ├─ instance handle (generation, revision floor) from ferris-core
       │    ├─ frame cache: current + last known good
       │    ├─ host view state: focus key, scroll, selection, pending actions
       │    └─ no lifecycle record: convergence and disposal live in the runtime
       └─ dirty set + a single per-frame notify
```

The `SurfaceHost` is a **host adapter over the composition runtime**, not a second lifecycle authority. The instance record — identity, generation, revision floor, state, action bindings and disposal report — lives in `ferris-core`. `SurfaceHost` holds a render cache keyed by `InstanceId`, the host-only view state, and the requests it issues. It owns no service registry, no event bus and no dependency resolution; it owns pixels, focus and interaction. The cache above is exactly that: if it ever disagrees with the runtime, the runtime wins and the cache is rebuilt.

### 12.2 Render pipeline

1. A frame begins; the host takes an immutable snapshot of the dirty instances.
2. For each dirty instance, rebuild its element tree from the cached document. GPUI consumes elements each frame, so the cache unit is the **document**, not a subtree; an unchanged instance still builds its elements from cached data, with no provider read and no validation.
3. The composition step places slot children at their declared sizing.
4. The frame paints.
5. Background work (provider reads, action results) arrives on the executor, updates the instance under `cx.update`, marks it dirty and requests **one** notify, never one per item.

Rules that follow from the BSH gold requirement:

- **No provider call in the render path.** Reads are scheduled ahead, the way the current panel asynchronously loads and refreshes; a render never blocks on I/O.
- **No per-frame allocation proportional to provider count.** Slot resolution is cached until a provider change invalidates it.
- **No state re-read per frame.** The frame cache is the state; the provider is re-read only when its state model says so.
- **One notify per host per frame.** A stream of patches that lands in one interval repaints once; GPUI already coalesces a burst of `cx.notify`, so the measurement that matters is element build plus layout time, not the notify count.

### 12.3 Focus, IME and interaction state

- The host owns focus. A focus key is `(instance, node id)`, and the id must be stable across frames.
- A patch that removes the focused node moves focus to the nearest surviving ancestor and announces the move through accessibility.
- A patch that changes only text or props of the focused input must not recreate the element; the caret, selection and IME composition survive.
- A patch that changes the value of a field the reader is composing in is **deferred** until composition ends; overwriting an IME composition is a visible product defect, not a nicety.
- Scroll offset and disclosure state are per instance and survive frames; they are not provider state and not round-tripped through a document.
- Value updates replace a range, never the whole field: `set_value` on the focused field resets the selection and any marked range, so a patch applies the smallest range replacement that reaches the declared value.
- The current renderer shows where the rule lands: `ExtensionPanel::input_for` caches one `InputState` per node path and calls `set_value` whenever the declared value differs. That comparison must become composition-aware, because a provider that re-sends an older value — an easy bug in a streaming producer — otherwise overwrites the reader's typing and the input method's composition buffer.

### 12.4 Layout, virtualization and size

- The host owns every scroll container, dock boundary and pane resizing. A document declares a slot's sizing constraints, never an absolute position.
- A slot's min size is authoritative: a provider cannot collapse below it, and the host draws the skeleton at that size.
- The host virtualizes over the bounded document (today's table ceiling is 500 rows) instead of asking the provider for a viewport. A frame never depends on scroll position, so a provider never observes the reader; larger data sets page through actions.
- A surface that resizes its region does not rebuild its document; it re-lays out the cached frame.

### 12.5 Theming and accessibility

- Providers name tones, never colours; the host resolves tones against the active palette per frame, as the panel renderer already does, so a light/dark switch lands without a remount.
- Accessibility is a host mapping over the node vocabulary, with provider-declared roles checked at mount. Minimum promises: every button has a label, every input has a label and a value, every table has headers, every image has alt text, every error is announced, and an agent-authored surface announces its producer.
- A provider that cannot meet the declared role for a node must declare `read_only` for it; a checkbox that announces nothing is worse than text.

### 12.6 The web escape hatch

The existing native web view stays available as a `native-embed` surface class with an explicit trust level. It is not the default, it does not inherit host cookies or session state, it never draws host chrome, and it must be declared by a trusted native provider, never by a package or an agent. Its accessibility and focus behaviour are declared honestly as limited.

### 12.7 Budgets

| Budget | Target | Measured how |
|---|---|---|
| Element build + layout, 1000 nodes | under 4 ms | Release build, instrumented, on the reference machine |
| Patch apply and validation | under 500 µs | Unit benchmark on the validator |
| Notifies per host per frame | at most 1 | Counter in `SurfaceHost`; the gate is build time, not the count |
| Allocations per rebuild | no growth with instance count | Allocation counter in a test build |
| Mount (excluding provider read) | under 16 ms | Timing around admit + first frame |
| Provider read deadline | 2 s default, 250 ms - 30 s policy | Action/read harness |
| Cached frames per instance | 2 | Assertion in the instance |
| Memory per instance | under 4 MiB | Frame size accounting |

These are gates, not aspirations: a change that breaks one is a regression even if every test passes.

### 12.8 Testability

The existing click-test harness is the acceptance tool: seed a surface, `draw_until` the first frame, press a control by debug selector, assert the state the capability wrote. Surface tests must also cover: focus survival across a patch, IME-safe value updates (unit level), stale-revision refusal, a provider error card without a panicking sibling, and a text-profile render of the same surface naming the same action ids.

## 13. Security model

A UI is both an output surface and an input surface. Agent-authored and remote-authored content makes it the most exposed part of the system. The model is simple: **rendering grants nothing**.

### 13.1 Threats

| Threat | Example | Control |
|---|---|---|
| Host impersonation | A frame that looks like the permission dialog | Host chrome is outside any frame; providers cannot draw chrome, confirmations or badges |
| Capability escalation | A surface's control runs with the provider's authority while claiming to be the reader's | Two-principal model (§6.4), authorized at mount and at dispatch |
| Clickjacking | A surface overlays or embeds a privileged control | Host controls are drawn by the host, never mounted from a document; slots that *are* host chrome (`settings.page`, `command.palette.row`) admit first-party providers only and never an agent surface |
| Data exfiltration | An image URL, a link or a remote frame carrying state outward | No remote images, no URL navigation, no implicit network from a document |
| Prompt-injected UI | Content tells the reader to approve something destructive | Destructive actions require host confirmation outside the frame; provenance is always visible |
| Resource exhaustion | A 1000-node frame per keystroke, a patch storm | Node/byte/patch/rate budgets; debounce; coalescing |
| Remote trust abuse | A remote node supplies a frame that triggers local actions | Remote action bindings are typed `Binding::Remote { node, grant }`; a local capability id in a remote frame is refused |
| Stale authority | An old generation's patch or action lands after replacement | Generation and revision checks before any other work |
| Silent degradation | A profile hides a confirmation | Hidden actions declared with a reason; destructive actions never hidden |

### 13.2 Invariants

1. A document never grants, implies or caches a capability.
2. Every dispatch authorizes the exact capability, resource and scope at press time.
3. A mount authorizes the whole binding table up front; a dead control is a mount error.
4. Host-native affordances are host-rendered, from host copy, outside every frame.
5. Provider prose is untrusted: escaped, attributed, muted, never in chrome.
6. No document may cause a network request, a process launch or a file read by rendering.
7. Unknown node kinds, unknown profiles and newer schemas are refused, never skipped.
8. A revocation disables affected bindings immediately; in-flight calls follow invocation policy.
9. Every surface binding check and every dispatch runs with `PolicyMode::Enforce`. Advisory mode converts a deny into an allow, so it must never authorize a surface action.
10. A stale generation cannot publish frames or dispatch actions.
11. Failure of the policy engine fails closed: no dispatch, a host-owned explanation.
12. Provider text never appears in host chrome, and a provenance badge is never the only control protecting a privileged action.

### 13.3 Principals

| Principal | Used for | Authority |
|---|---|---|
| Reader (host session) | Chrome actions, confirmations | The reader's own grants plus the host allowlist; the existing `Principal::Local { session, preset }` |
| Producer: package | Its declared surface actions | The package's grants, checked per call, as today |
| Producer: agent | A turn's authored surface actions | The agent session's grants at that moment |
| Remote node | Remote-supplied frames | Its own node's policy; local policy additionally gates every local effect |

The two *dispatch modes* in §6.4 select which of these performs the call; this table is the identity substrate, not a second set of authorities. No principal inherits from another: there is no ambient host authority in a frame, and no union of reader and producer authority in one binding.

### 13.4 Audit

Every mount, rebind, disposal, refusal, dispatch and denial records: principal, surface id, instance, generation, revision, host slot, capability, scope, decision, package provenance, isolation class, correlation id and policy version. The audit record and the on-screen refusal must be derivable from the same value, so they cannot disagree.

## 14. Distributed surfaces

### 14.1 Three projection modes

| Mode | What crosses the node boundary | Trade-off | Default for |
|---|---|---|---|
| **P1 Declarative projection** | Contract + frames + patches, rendered locally | Native look, small bandwidth, themable, no remote code; requires schema compatibility | Every remote surface |
| **P2 Snapshot projection** | A bounded, cacheable image or serialized frame | Works for foreign renderers; not themable, not accessible, coarse interaction | A surface the local renderer cannot honour |
| **P3 Native embed** | A remote window, view or stream | Highest fidelity, highest risk, no local composition | Never across nodes; local trusted native only |

P1 is the architecture. P2 exists so a capability that cannot be re-expressed is not forced through a broken interpreter, and it is always marked as a snapshot: not a component in the composition, not focusable content, not eligible for a slot's own layout.

### 14.2 Rules

1. **Remote frames are untrusted input.** The local host validates schema, limits, profile, ids and bindings exactly as it validates a local package.
2. **A remote frame's action bindings are remote bindings.** They are represented with the existing `Binding::Remote { node, grant }` and a `remote_provider_id(node)`-qualified capability, so "local or remote?" is a type check rather than a comparison of two strings that can collide. A local capability id in a remote frame is refused; that is the rule that stops a peer from using a local reader's click as local authority.
3. **Dual authorization.** The provider node authorizes the invocation under its own policy, and the local node authorizes its outbound grant, the placement and the reader's visibility (the package/preset allowlist). Neither decision is inferred from the other.
4. **Remote identity and provenance are visible.** The host badge names the node and, when the node reports one, the agent. A remote surface is never drawn as local.
5. **Budgets are local.** Frame size, patch count, action rate and timeouts are enforced by the receiving host regardless of what the provider declares.
6. **Cancellation and revocation reach the provider node**, with a verifiable outcome; a dropped local future is not evidence the remote work stopped.
7. **Correlation and provenance cross the chain**, and both nodes keep an independent audit record.
8. **A local instance never owns a remote task or agent lifecycle.** A remote surface handle is a lease proxy with an explicit expiry.

### 14.3 Topologies

| Topology | Flow | Key constraint |
|---|---|---|
| Peer panel | Node B's capability mounts in node A's `workspace.right` | B's frames validated by A; actions dispatched to B and re-authorized there |
| Remote contributor | Node B's agent contributes a slot to node A's surface session | Explicit delegation grant; patch channel crosses the boundary; B may not touch A's root |
| Read-only mirror | Node A renders a B surface as P2 | Explicitly labelled, no actions except a host "open on B" affordance |
| Offline replay | Node A shows the last known good frame from B | Stale banner, bindings disabled unless the contract declares offline-safe actions |

### 14.4 Transport

Frames, patches, action requests and results travel as capability payloads over the existing transport (bus topics locally, the federation protocol between nodes). The UI contract adds no transport of its own; it defines message kinds on the transport that already carries invocations. A frame is bounded, digestible and idempotent to re-send; an action carries an idempotency key when it mutates.

### 14.5 What never leaves the node

- Host chrome, confirmations, policy prompts, provenance badges and focus state.
- The reader's identity beyond what the delegation grant names.
- Local capability descriptors a remote node is not granted to discover.
- Any local action binding from a remote frame (§14.2 rule 2).

## 15. State, persistence and offline

### 15.1 Authority

The capability owns authoritative state. A surface instance owns a cache of the last validated frame and its own ephemeral view state. Nothing in the UI model is a second source of truth, and no provider is expected to reconstruct domain state from a frame.

### 15.2 Persistence

| Persisted | Keyed by | Restored as |
|---|---|---|
| Pinned/session surface descriptors | workspace + session + slot + contract | A fresh mount, re-read from the provider |
| Host view state (disclosure, scroll, selection) | instance identity (contract + slot + owner) | Applied on the first frame if the ids still resolve |
| Last known good frames | instance identity, with a TTL | Degraded render while the provider is unreachable |
| Nothing else | - | - |

Frames are a cache, never truth. A restart with a stale cache shows a stale marker and re-reads before enabling actions.

### 15.3 Offline behaviour

| Situation | Behaviour |
|---|---|
| Provider unreachable, contract says `last_known_good` | Last frame, stale badge, actions disabled except declared offline-safe ones |
| Provider unreachable, contract says `none` | Host error card with retry |
| Capability vanished while its surface is mounted | Region degrades, bindings to it disable, other regions unaffected |
| Node disconnected | Remote surfaces degrade to their last frame; a reconnect revalidates before rebinding |
| Action sent while disconnected | Refused with a host copy that names the disconnect; never queued and replayed silently |

No action is replayed automatically on reconnect. A mutating action that must survive a disconnect carries an idempotency key, and the retry is the reader's or the agent's explicit decision.

## 16. Observability and developer surface

Agent-authored and composed UI is only debuggable if the runtime can explain itself. Three tools are part of the deliverable, not optional polish.

### 16.1 Events

`surface.mounted`, `surface.frame`, `surface.patch`, `surface.refused`, `surface.degraded`, `surface.disposed`, `surface.rebound`, `action.dispatched`, `action.denied`, `action.completed`, `slot.unresolved`, `slot.ambiguous`. Each carries instance, generation, revision, provider, slot, principal and the refusal value where one applies.

Frame and patch events are not a firehose: each instance keeps a bounded ring buffer (default 64 entries, 256 KiB), high-rate streams are sampled, and the buffer is the replay source. Retention is declared host policy, because an unbounded per-frame log would spend the exact budget §12.7 defends.

### 16.2 Metrics

Mount latency, first-frame latency, apply latency, frames and patches per second, refuse counts by reason, provider errors by provider, action latency and rate-limit hits, degraded time, and the number of live instances per scope. These extend the existing host metrics rather than creating a second counter family.

### 16.3 Surface inspector

A developer pane over the live `SurfaceHost`:

- The slot tree with each instance's provider, generation, revision, profile and state.
- The frame tree with node ids, the binding table and the resolved principal per binding.
- The action log with decisions, denials, refusals and correlation ids.
- The composition decisions: why this provider, why this profile, why this slot was unresolved.
- A replay of recorded frames for one instance, so a defect is reproducible without the live producer.

### 16.4 Explaining a disabled control

Every disabled control has one host-owned reason reachable from the control itself (a tooltip or the inspector, depending on view). "Greyed out" without a reason is treated as a defect: it is the most common way a composed UI becomes unmaintainable, and the most common way an agent's mistake is mistaken for a policy decision.

## 17. Migration plan

Migration is additive. Every phase leaves the product shippable, keeps Manifest 2.0 and schema-`1.0` panels working, and adds no new crate before its contract is proven. The three-crate Ferris boundary and the one-dependency BSH rule (BSH imports `ferris-sdk` only) are unchanged.

### Phase 0 — Contract freeze and rename

- Publish the surface vocabulary, the schema-`1.1` draft and the host slot catalogue.
- Rename `capability::SurfaceName` to `CallableName` with a deprecated alias and a serde alias, so no serialized descriptor changes (§4.3).
- Instrument the current panel renderer: mount time, frame time, notify count, action latency. These are the baseline the budgets in §12.7 are measured against.
- Write the gap register from §2.2 as tracked items.

**Exit gate:** the rename lands with green tests; the baseline numbers are recorded; the schema and slot catalogue are reviewed and frozen for Phase 1.

### Phase 1 — Document 1.1 and the component ABI

- Add node ids, the binding table, and the `slot` node as an additive schema `1.1`; keep the `1.0` reader and the `1.0` limits.
- Validate twice: provider-side in `ferris-sdk`, host-side in BSH, with a shared validator and a test that a document accepted by one is accepted by the other.
- Publish **`bsh:extension@1.1.0`** as a new package that exports `surface(id)`, `patch(id, base, ops)` and `close(id)` in a separate interface beside the unchanged `1.0` extension. Adding exports to the existing world would not degrade to a handshake failure — the host binds the whole world, so every `1.0` guest would fail to instantiate. Generate both bindgens, handshake on the package version, keep the `1.0` world instantiable, and keep the two WIT copies (`crates/bsh/wit/extension.wit`, `crates/ferris-runtime/wit/extension.wit`) byte-identical under a parity test, with one named as normative.
- No host behaviour change yet: panels keep rendering as today.

**Exit gate:** a schema-`1.0` package loads unchanged; a `1.1` document round-trips through both validators; the ABI handshake refuses version skew explicitly; no measurable render regression.

### Phase 2 — SurfaceHost and instance ownership

- Introduce the instance record in `ferris-core` (identity, generation, revision floor, state, disposal report) and the `SurfaceHost` entity in BSH as a render cache over it; model each mounted panel as one instance.
- Publish region state to the host as one typed value per change. The host never computes lifecycle, convergence or authorization, and the `UiSession` revision floor becomes the instance's, not the host's.
- Close today's command fallback: a panel action that is not a tool descriptor currently reaches `runtime.command` without a capability decision. Every action must take the same authorization path before Phase 3 can build on it.
- Implement caching (current + last known good), the single per-frame notify, and the budgets in §12.7 as assertions.
- Panels continue to work exactly as before; the diff is internal.

**Exit gate:** every existing panel renders identically; frame time and notify counts meet the budget on the reference machine; a disposed instance leaves no effect, listener or cached frame; a stale action is still refused.

### Phase 3 — Composition

- Add the host slot catalogue with occupancy rules and profile requirements.
- Implement provider slots, exact resolution, ambiguity diagnostics, the slot-depth and fan-out bounds, and failure containment (skeleton, error card, fallback).
- Wire slot dependencies into the existing `Inject`/epoch reconciler so a missing provider is Pending and a vanishing provider degrades.
- Migrate one real multi-capability surface (a review or diff surface embedding a second provider) as the proof.

**Exit gate:** a surface embeds another provider's surface and the two keep independent revisions, authorization and disposal; ambiguity and cycles are refused with the candidates named; the child's failure does not blank the parent; disposing the parent disposes the child exactly once.

### Phase 4 — Patches, streaming and convergence

- Implement base-relative patches, staged validation and atomic commit, with `StaleRevision` returning the current revision.
- Implement the `stream` state model with coalescing and a per-instance frame rate cap; keep `refresh_ms` for `on_demand` providers.
- Add the action-rate budget, debounce and the double-dispatch guard.
- Prove convergence with a provider that mounts, is replaced and is removed while mounted.

**Exit gate:** a 200-patch burst produces one repaint and a correct final document; a stale patch changes nothing and names the current revision; a replaced provider never serves a frame under its old generation; a vanished provider degrades the region without an error cascade.

### Phase 5 — Agent-authored surfaces

- Ship the first-party `bos.ui` extension: `present`, `patch`, `close`, `catalogue`, with risk classes, limits and audit, gated by ordinary tool enablement rather than a UI permission.
- Implement admission (§10.2), lifetimes (§10.3), the provenance frame, the agent-active-surface budget, and the refusal messages the model can act on.
- Implement the `text` and `machine` profiles, so the same surface is usable from the CLI and by another agent.
- Forbid host-native affordances in agent frames and add the test that proves it.

**Exit gate:** a turn presents a surface bound to its own capabilities, the reader acts, the turn patches it, and the turn's end disposes it; a binding to a capability the agent does not hold is refused at mount with a usable message; the surface cannot draw a confirmation; the same surface renders in the text profile with the same action ids.

### Phase 6 — Multi-agent surface sessions

- Implement presenter/contributor/observer roles, host-assigned contributor slots, confined patches and explicit handoff.
- Draw attribution per region, not per document.
- Prove that one contributor's termination, error or revocation disposes only its slot.

**Exit gate:** two agents compose one surface without document merging; a contributor cannot mutate the root or a sibling; a handoff preserves contributor slots whose contracts the new presenter declares and unmounts the others with a notice; concurrent patches are serialized with no lost update.

### Phase 7 — Federation and the developer surface

- Implement P1 projection over the federation protocol, with local revalidation, remote-only binding targets, dual authorization, deadline and cancellation.
- Add P2 snapshot projection for a surface the local renderer cannot honour, clearly labelled and non-composable.
- Implement offline rules, the idempotency key for mutating actions, and the no-silent-replay rule.
- Ship the surface inspector, the disabled-control explanation and the metrics from §16.

**Exit gate:** a peer node's surface mounts locally and its actions are authorized on both sides; a remote frame attempting a local binding is refused; a disconnect degrades rather than fabricates; the inspector reconstructs one instance's history from events and recorded frames.

## 18. Work breakdown

Each story is scoped to no more than three working days and is independently reviewable.

| ID | Deliverable | Acceptance criteria | Est. |
|---|---|---|---:|
| FER-UI-001 | Callable-name rename | `SurfaceName` deprecated alias, serde alias, workspace green | 1d |
| FER-UI-002 | Schema 1.1 validator | Ids, bindings, slot node; `1.0` unchanged; dual-validator parity test | 3d |
| FER-UI-003 | ABI 1.1 handshake | `surface`/`patch`/`close` in WIT; explicit version refusal; `1.0` guest still runs | 3d |
| FER-UI-004 | SurfaceHost skeleton | Instances, generation, revision floor, fallback to panels unchanged | 3d |
| FER-UI-005 | Frame cache and notify budget | Two frames, one notify per interval, budget assertions | 2d |
| FER-UI-006 | Focus and IME survival | Ids, focus key, deferred value update during composition, tests | 3d |
| FER-UI-007 | Host slot catalogue | Declared slots, occupancy rules, profile requirement, validation | 2d |
| FER-UI-008 | Slot resolution | Exact contract/version/realm, ambiguity diagnostics, bounds | 3d |
| FER-UI-009 | Failure containment | Skeleton, error card, fallback, stale, panic isolation | 3d |
| FER-UI-010 | Patch engine | Staged validation, atomic commit, stale refusal, op ordering | 3d |
| FER-UI-011 | Streaming and coalescing | Stream state model, rate cap, burst test | 3d |
| FER-UI-012 | Action pipeline | Two principals, dispatch checks, pending, debounce, rate limit | 3d |
| FER-UI-013 | `bos.ui` extension | Present/patch/close/catalogue, admission, provenance, budget | 3d |
| FER-UI-014 | Profiles | `compact`, `text`, `machine` renderers and parity tests | 3d |
| FER-UI-015 | Surface sessions | Presenter/contributor/observer, handoff, confined patches | 3d |
| FER-UI-016 | P1 federation | Remote provider, local revalidation, remote-only bindings, dual audit | 3d |
| FER-UI-017 | Inspector and metrics | Live tree, decisions, action log, replay, counters | 2d |
| FER-UI-018 | Conformance suite | The gates in §19 as automated tests where testable | 3d |

## 19. Conformance gates

Ferris must not be described as offering a dynamic UI runtime until these are demonstrated. Each gate names a test, not an intention.

### Contract

- A schema-`1.0` document and a schema-`1.1` document both validate; a `1.2` document is refused with `SchemaNewer`.
- Two surfaces produced by different provider families (at minimum a native provider and a declarative one, then a Cordis adapter) pass the same validation corpus and the same action pipeline; no document or contract field identifies a provider family.
- The provider-side and host-side validators accept and reject the same corpus.
- An interactive node without an id is refused; duplicate ids are refused.
- A binding with an unknown expression form is refused; a binding to a missing capability is refused at mount.
- The mapping language cannot express a computation (property test over the grammar).

### Composition

- Slot resolution is deterministic for identical inputs and independent of map iteration order.
- Ambiguous, cyclic, too-deep and over-fan-out composition each produce a distinct, tested refusal.
- A missing required provider yields Pending; a missing optional provider yields the declared placeholder.
- A child surface's failure leaves the parent Live and the sibling regions unaffected.
- Disposing a parent disposes each child exactly once and emits one report.

### Lifecycle and ownership

- A stale generation cannot publish a frame, apply a patch or dispatch an action.
- Patch application is atomic: a rejected patch leaves the previous revision byte-identical.
- Revocation disables affected bindings before the next interaction.
- Every mounted instance has one effect scope, and disposal is idempotent.
- Replacement never serves a frame from the old generation after the rebind.

### Interaction and security

- Each dispatch performs the capability decision at press time; a mid-session revocation is honoured.
- A `reader`-principal binding cannot name an arbitrary command or a non-allowlisted capability.
- An agent-authored frame cannot render a confirmation, a badge or host chrome.
- An action at a stale revision is refused and the control returns to a defined state.
- Action rate limits and debounce hold under a producer that fires continuously.
- No node kind causes a network request, a process launch or a file read by rendering.

### Presentation

- Focus, selection, scroll and IME survive a full frame and a patch that does not remove the focused node.
- Element build, layout and patch apply meet the §12.7 budgets in a release build on the reference machine.
- Tone-only theming: a light/dark switch lands without a remount and no document can set a raw colour.
- Every interactive node maps to an accessibility role with a label; an agent-authored surface announces its producer.
- The `text` profile of a surface names the same set of action ids as the `full` profile; a declared hidden action is absent from both the profile and any confirmation it would have carried.

### Agent

- A turn presents, patches and disposes a surface; the disposal happens on success, failure and stop alike.
- A presented binding to a capability the agent does not hold is refused at mount with the failing capability named.
- The agent's active-surface budget is enforced and reported.
- There is no UI permission kind: switching the presenting tool off is sufficient to make a turn unable to present, and no surface path bypasses that switch.
- The catalogue response matches the host's actual vocabulary (asserted against the validator).

### Federation

- A remote frame is validated locally exactly like a local package frame.
- A remote frame binding a local capability is refused.
- Both nodes record an authorization decision for one remote action, and they are independently auditable.
- A disconnecting node degrades its surfaces without fabricating a frame, and reconnection revalidates before rebinding.
- No action is replayed silently after a reconnect.

## 20. Risks and mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| UI becomes a second authority for actions | Privilege escalation through a control | Two-principal model, mount-time table authorization, dispatch-time reauthorization |
| Two lifecycle authorities (host vs composition runtime) | Split-brain disposal and leaks | SurfaceHost is an adapter; instances are effect scopes, never a parallel registry |
| Document merging between agents | Lost updates, unreviewable behaviour | Ownership by region and slot; no merge, explicit handoff |
| JSON specs are too expensive to emit | Poorly authored interfaces, retries, token waste | Binding table, catalogue capability, precise refusals, size guidance |
| Agent UI becomes noise | Reader fatigue, dark-pattern risk | Turn lifetime default, active-surface budget, provenance, reader close always available |
| Composition explodes (depth, fan-out, cycles) | Unbounded render cost | Declared bounds, refusal with the path named, host-declared slots only |
| Patch churn burns CPU | Dropped frames on a fast producer | Coalescing, per-instance frame cap, rate limits, budgets as assertions |
| Provider frames are treated as trusted | Host compromise via spoofing or exfiltration | Host revalidation, no remote content, host chrome outside frames |
| Remote UI becomes a phishing surface | Credential theft through a peer node | Provenance badge, no privileged controls, no local bindings, snapshot labelling |
| Accessibility regresses under dynamic content | Unusable for screen-reader users | Declared role promises, id-based focus, announcement on failed focus move |
| Schema drift breaks packages | Installed extensions stop loading | Additive vocabulary, dual-read, explicit `SchemaNewer`, ABI handshake |
| Too many crates too early | Dependency churn, slower review | Stay in the three existing crates plus the BSH adapter; no new crate in Phases 0-4 |
| The renderer becomes the framework | GPUI types leak into the contract | Contract has no renderer types; CI guard on Ferris independence already enforces it |

## 21. Decisions requested

This RFC asks maintainers to approve the following before implementation:

1. **UI is a capability.** A surface has a contract, provider, version, realm, generation, limits and audit, and is discovered, placed, owned and disposed like any other capability. Its actions are authorized like any other call.
2. **Composition is by reference.** A slot names a surface contract; documents are never merged, and each mounted region keeps its own generation and revision.
3. **Ownership is by region and generation**, with one presenter per instance and one disposal report per generation.
4. **Schemas are additive.** Schema `1.1` extends `1.0`; unknown newer schemas are refused, never partially rendered; Manifest 2.0 and `ferris-sdk` stay compatible.
5. **Callable names are renamed** before the surface vocabulary is adopted (`SurfaceName` to `CallableName`), so one word has one meaning.
6. **Two dispatch modes over the existing principals.** An action binding runs as the producer (package or agent session) or as the reader (the host session), never as a union; reader bindings are allowlisted, and both are expressed with `capability::Principal` rather than a UI-local identity.
7. **Agents author surfaces as proposals** through the ordinary `bos.ui/present` tool, an admission check, a budget and a lifetime, with host-drawn provenance and no privileged affordances.
8. **Multi-agent composition uses slots, not shared documents**; handoff is explicit and audited.
9. **The presentation layer stays BSH/GPUI.** Ferris holds no widget, renderer or focus state; GPUI types never cross the contract.
10. **Federation projects surfaces as capabilities**, declares remote bindings as remote-only, enforces dual authorization and fails closed when policy or transport is unavailable.
11. **Budgets are gates.** Build time, patch time, allocation growth, memory, frame rate and action rate are enforced as tests, not reviewed as intentions.
12. **No production migration** beyond the rename starts until Phase 0 baselines and the schema/slot freeze are complete.
13. **There is no UI-specific permission.** A surface is admitted by ordinary tool policy (is the presenting tool offered?), placed by the host slot's provenance rule, limited by the surface budget, and its effects are authorized per action binding by the existing capability policy. No `ui.surface.*` grant kind, no UI permission store, and no second decision path is introduced.
14. **Surface binding checks and dispatches require `PolicyMode::Enforce`.** Advisory mode can turn a deny into an allow, so it must never authorize a surface action.
15. **The component ABI advances as a new package**, `bsh:extension@1.1.0`, with the `1.0` world still instantiable, dual bindgen, and one normative WIT copy guarded by a parity test.
16. **Reader-principal dispatch needs the host session in `CallContext`.** Until that lands, `reader` bindings are refused at mount rather than authorized by guesswork.
17. **cordis-rs is the runtime, and the contract stays neutral.** Surface providers are admitted as Cordis Plugins (pinned `cordis-core = 0.6.1`) with generation-owned registration and Fiber disposal; every other provider family lowers into the `SurfaceProvider` seam. Cordis types never cross the SDK boundary, and no contract field, node kind or host shortcut may select a provider family.

## 22. Implementation status (2026-11-12)

Landed in this change, in `ferris-core` and re-exported by `ferris-sdk`:

| Area | Where | State |
|---|---|---|
| Surface identity, limits, refusal vocabulary | `surface/mod.rs` | Implemented |
| Contract, profiles, guarantees, limits | `surface/contract.rs` | Implemented |
| Schema 1.1 node vocabulary, ids, slot node, 1.0 lowering | `surface/node.rs` | Implemented |
| Document, binding table, validation against contract | `surface/document.rs` | Implemented |
| Frames, base-relative patches, atomic application | `surface/frame.rs` | Implemented |
| Catalogue, exact resolution, ambiguity, cycles and bounds | `surface/compose.rs` | Implemented |
| Instance engine: generation, revision, refusal rules, disposal | `surface/engine.rs` | Implemented |
| Provider seam | `surface/runtime.rs` | Implemented |
| cordis-rs runtime: install, refresh, uninstall, generation-owned registry | `ferris-runtime/src/surface_runtime.rs` | Implemented |
| BSH host slot catalogue and one surface instance per mounted panel | `bsh/src/extension/surface.rs` | Implemented |
| Panel renderer draws the surface document and revision the engine holds; live cards and action results apply through it | `bsh/src/ui/extension_panel.rs` | Implemented |
| Package panels may ship a schema 1.1 surface: bindings, slots and limits are validated, actions derived into the contract, and the lowered 1.0 tree keeps drawing | `ferris-runtime/src/declarative.rs`, `bsh/src/extension/surface.rs` | Implemented |
| A 1.1 panel control resolves its binding's capability to the tool that implements it; no tool means a named refusal, not an inert control | `bsh/src/ui/extension_panel.rs` | Implemented |
| Generation renewal when a panel's provider is rebound | `bsh/src/extension/surface.rs` | Implemented |
| Bounded, versioned wire envelope for frames, patches, actions and close | `surface/wire.rs` | Implemented |
| Remote-binding refusal: a remote frame may bind only its own node | `surface/document.rs` | Implemented |
| Surface instance count in host metrics | `bsh/src/extension/host.rs` | Implemented |
| Slot composition in the renderer: a slot draws the resolved child surface, with fallback, missing-child and cycle behaviour | `bsh/src/extension/surface.rs` | Implemented |
| Inspector rows (slot, generation, revision, state, refusal) and a refused count | `bsh/src/extension/surface.rs` | Implemented |
| Agent surfaces: validated store, `present`/`patch`/`close`/`catalogue` tools, panel rendering under an agent badge | `bsh/src/extension/agent_surface.rs` | Implemented, actions read-only |
| Local bus transport: `surface.message` topic, publish and ingest of frames and close messages | `bsh/src/extension/host.rs` | Implemented, federation not wired |
| Composed-slot controls route to the panel that owns the region | `bsh/src/extension/surface.rs`, `bsh/src/ui/extension_panel.rs` | Implemented |
| Focused-input guard: a live card cannot overwrite the field the reader is typing in | `bsh/src/ui/extension_panel.rs` | Implemented |
| Composition budget: an expanded slot tree over the node/byte ceiling keeps the panel's own document | `bsh/src/extension/surface.rs` | Implemented |
| Surface inspector rows in the Extensions pane | `bsh/src/ui/settings.rs` | Implemented |

Evidence: `cargo test -p ferris-core surface` runs 37 surface tests; the full crate is 164 passed, 1 ignored; `cargo clippy -p ferris-core --all-targets -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc -p ferris-core --no-deps` are clean. BSH: `cargo test -p bsh --lib extension::` is 127 passed and `cargo test -p bsh --lib ui::extension_panel` is 9 passed; `cargo clippy -p bsh --lib -- -D warnings` is clean. `cargo test -p ferris-runtime` is 58 passed. The extension-point inventory is in §23.

Still open, and the phases in §17 remain the plan:

- A composed slot's controls now route to the panel that owns the region, and the focused input is protected from a value overwrite. An **agent surface's** controls are still read-only: the panel dispatch path does not yet route a binding key to its capability.
- A runtime rebind now renews the generation, but the renderer keeps the last good document rather than re-reading it from the new provider until the next refresh.
- The agent tools exist and render behind an agent badge, but they are not yet a first-party extension package with its own manifest, and their actions are not authorized through the capability pipeline.
- The local bus carries frames and close messages; federation topics, correlation and dual audit are still ahead, and an action arriving over the transport is refused rather than authorized.
- Frame budgets are enforced structurally (node, byte, patch and composition ceilings); per-frame timing instrumentation is not wired into the metrics yet.
- The inspector shows each mounted surface's slot, generation, revision, state and refusal reason in the Extensions pane; a dedicated pane with per-binding decisions and a disabled-control explanation is ahead.
- The surface grant question is settled the other way: no new UI permission kind is introduced (§10.1, decision 13).

## 23. BSH UI extension points

A UI extension point is two things: a **placement** (a stable region id the host draws) and a **contract** (who may put what there, what it may do, and who disposes it). A placement without a contract is a hole; a contract without a placement is dead code.

Every point must declare, in one place:

1. A stable **id** and occupancy rule (`single`, `stacked`, `inline`).
2. Whether an **agent-authored** surface may mount there.
3. Whether it is **first-party only** (host chrome cannot be filled by a package).
4. The **profile** the host will render (`full`/`compact`/`text`/`machine`) and the limits.
5. One **lifecycle owner**: admitted through the surface host, with a generation, a revision floor and a disposal report.
6. The **authorization** of every action it exposes: a binding, never a raw command, and host chrome only through the reader allowlist.

### 23.1 The current set

| Point | Shape | State |
|---|---|---|
| Host slot | `HostSlot` registry: id, occupancy, agent acceptance, first-party rule | Implemented and enforced at admission (`bsh/src/extension/surface.rs`) |
| Panel contribution | Manifest `panel` plus `panels/<id>.json`, schema 1.0 | Implemented (the shipped contract) |
| Provider slot | A document mounts a child through a `slot` node; the host expands it and routes its actions | Implemented for mounting and routing. A package can be the *target* (`bsh.panel/<panel id>`) but cannot yet declare a named sub-slot another surface mounts into |
| Surface contribution | A panel file's `view` may be schema 1.1, carrying a binding table and slot nodes; the host mounts the surface and the renderer draws its lowered 1.0 form | **Implemented** through `panels/<id>.json` (no separate `surfaces/` kind yet) |
| Agent surface | `bos.ui/*` tools, agent limits, provenance badge, panel rendering | Implemented; actions are read-only (no binding-to-capability dispatch) |
| Cordis JS `ctx.slots` | A bridge host API that lowers a Cordis plugin's UI registration into frames | **Missing**: `ferris-runtime/src/bridge/dsh-host-shim.js` still refuses `ctx.slots` |
| Component ABI | `surface(id)`, `patch`, `close` beside `panel(id)` in `bsh:extension@1.1.0` | **Missing**: the WIT exports `panel(id)` only |
| Reader host services | A small allowlist of chrome actions (focus, open settings, copy, close) a surface may request | **Missing** |
| Composer, statusbar, palette, transcript, settings producers | Slots are declared in the registry; producers exist only through panels and agent surfaces | Partial: ids are stable, package/Cordis producer paths are not |

The registry deliberately carries slots that have no producer yet — `workspace.statusbar.leading`, `workspace.statusbar.trailing`, `composer.row`, `command.palette.row`, `settings.page` — so an extension can target a stable id once its producer lands instead of guessing a region name. The two host-chrome slots are first-party only, and no slot accepts an agent surface unless the registry says so.

### 23.2 Rules for adding a point

1. Add the id to `HostSlot::ALL` (or declare it as a provider slot in a contract) before any consumer exists; no implicit region names.
2. Enforce occupancy at admission, with a refusal that names the holder.
3. Route every control through an action binding and the host pipeline; a new point does not get a private invocation path.
4. Admit through the surface host so generation and disposal are inherited, not parallel.
5. Ship tests for admission, occupancy refusal, missing provider, and disposal.

### 23.3 What "enough" requires

A package, an agent and a runtime plugin must each be able to: place UI in a declared region; offer a slot another surface can mount into; declare actions with their target capability; carry provenance; and be disposed with their owner. Today the agent path meets all five (except live actions), the package path meets placement and ownership but cannot offer slots or ship a 1.1 document, and the Cordis JS path meets none because its UI seam is refused.

A package can now ship a 1.1 surface, and its controls resolve a binding to the tool whose id is the capability's local name (or its namespaced form). A binding with no tool behind it is refused with the capability named. The remaining difference from the agent path is that an agent surface has no tool registry of its own to resolve against.

The next three deliveries, in dependency order:

1. **Surface contribution kind.** `contributes.surfaces` with `surfaces/<id>.json` carrying the contract (profiles, slots, actions, limits) and a schema 1.1 document; the host admits it through the same `SurfaceHost` and offers its `SlotSpec`s to other surfaces.
2. **Cordis `ctx.slots` bridge.** Replace the refusal in the DSH shim with a bounded registration call that lowers to a surface provider; the plugin does not receive a renderer, only a place to declare a surface.
3. **ABI 1.1.** Export `surface`/`patch`/`close` in a new `bsh:extension@1.1.0` package so process and WASM guests have the same point a native plugin has.

## Appendix A - Worked example

A review surface composed across an agent, a package and a peer node.

**1. A turn presents a root surface** into `workspace.main` with lifetime `turn`:

```json
{
  "schema_version": "1.1",
  "bindings": {
    "approve": { "capability": "bos.review/approve", "input": { "id": "$props.review" }, "required": true },
    "reject":  { "capability": "bos.review/reject",  "input": { "id": "$props.review" }, "required": true }
  },
  "root": {
    "type": "column",
    "children": [
      { "type": "heading", "text": "Review 3 changed files", "level": 2 },
      { "type": "slot", "surface": "acme.git/diff", "version": "^1.2",
        "props": { "repo": "/work/bos", "rev": "HEAD" },
        "sizing": { "weight": 1, "min": 240 } },
      { "type": "row", "children": [
        { "type": "button", "id": "approve", "label": "Approve", "action": "approve", "style": "primary" },
        { "type": "button", "id": "reject", "label": "Reject", "action": "reject", "style": "danger" }
      ] }
    ]
  }
}
```

**2. The host admits it.** This build offers `bos.ui/present`; the slot is free and accepts agent surfaces; the document validates; both bindings resolve to capabilities the agent holds; the agent's active-surface budget allows one more. The instance is admitted with generation 1, revision 1.

**3. The slot resolves.** `acme.git/diff` resolves to one package provider at `1.2.3`. It is mounted as a second instance with its own generation, its own bindings and its own effect scope. The peer node is not involved yet.

**4. The agent patches.** The agent's capability finishes reading and returns a patch with `base_revision: 1`:

```json
{ "base_revision": 1, "patch": [
  { "op": "set_text", "id": "heading", "text": "Review 3 changed files - 42 insertions" }
] }
```

The heading in step 1 has no `id`, so this patch is **refused** with `UnknownNode` — a patch addresses nodes by id, and the host has no target. The refusal names the missing id, and because an id cannot be introduced by a patch, the agent re-presents a corrected full document at revision 2 with `id: "heading"` and the new text. This is the intended feedback loop: one precise refusal, one correction, and the surface is never left half-applied.

**5. The reader presses Approve.** The host captures revision 2, resolves binding `approve` to `bos.review/approve` with the agent session as the dispatch principal, authorizes it, reserves the action, dispatches, receives a frame at revision 3 with a success notification and a disabled Approve control, and records the audit line. The pending state never required the agent to re-render.

**6. A contributor joins.** The agent presents a contributor into the diff provider's declared slot `acme.git/diff/annotation` for a peer node's "CI status" surface. The peer's frames cross the boundary, are revalidated locally, and may bind only `ci.status` capabilities on the peer. The local reader can read the CI status; clicking "rerun" dispatches to the peer, which authorizes it under its own policy, and the local node records the outbound decision.

**7. The peer disconnects.** The CI region degrades to its last frame with a stale badge; its bindings disable; the rest of the review surface keeps working. Nothing is queued for replay.

**8. The turn ends.** The turn surface is disposed, the diff instance is disposed with it, the contributor slot is disposed first (deepest first), and one disposal report lists all three. The audit record names the agent, the turn, the package provider and the peer node.

## Appendix B - Schema 1.0 to 1.1

| Concern | 1.0 | 1.1 |
|---|---|---|
| Schema field | required, must be `1.0` | `1.0` or `1.1`; produced frames declare their own |
| Nodes | existing vocabulary | same, plus optional `id` on every node and a `slot` node |
| Controls | `action` is a contribution id | `action` is a binding key; the binding table names the capability |
| Limits | nodes, depth, text, bytes | same, plus patch ops, bindings, slots, ids |
| Validation | structural | structural plus binding resolution, id uniqueness, slot graph, profile |
| Action request | `{action, revision, payload}` | `{instance, generation, revision, binding, event, principal}` |
| Result | a replacement document or a message | a frame, a patch, a message, an error or a denial |

A `1.0` panel is mounted as a surface with generated ids, an empty binding table and its actions lowered to provider-principal bindings using today's contribution-id resolution, so behaviour is preserved exactly while the code path is unified.

### B.1 Field reference (1.1)

The fields a producer and a validator must agree on. A field not listed here does not exist in `1.1`.

**Document**

| Field | Type | Required | Rule |
|---|---|---|---|
| `schema_version` | string | yes | `"1.0"` or `"1.1"` |
| `bindings` | map<string, ActionBinding> | no | at most 64 entries; keys unique |
| `hidden_actions` | array<{action, reason}> | no | every entry must name a declared action and a non-empty reason; destructive actions are refused |
| `root` | node | yes | the node vocabulary of `1.0`, plus `id` and the `slot` node |

**ActionBinding**

| Field | Type | Default | Rule |
|---|---|---|---|
| `capability` | string | — | provider-qualified `provider/name`; resolved at mount |
| `operation` | string | provider default | optional named operation on that capability |
| `input` | value | `{}` | only literals and the §6.3 mapping forms |
| `principal` | `producer` \| `reader` | `producer` | `reader` requires the host allowlist |
| `required` | bool | `true` | `false` mounts the control disabled when unresolved |
| `mode` | `once` \| `repeat` | `once` | `once` blocks a second press until the first settles |
| `debounce_ms` | u16 | `0` | 0-500; change events only |

**Slot node**

| Field | Type | Default | Rule |
|---|---|---|---|
| `surface` | string | — | provider-qualified surface id |
| `version` | semver range | `"*"` | resolved through the host resolver |
| `required` | bool | `true` | unresolved required slot leaves the instance Pending |
| `props` | object | `{}` | must satisfy the child contract's `SlotSpec.props` schema; no schema means no props |
| `sizing.weight` | f32 | `1.0` | 0.1-20 |
| `sizing.min` | u32 | `0` | device-independent pixels; authoritative |
| `sizing.max` | u32 | `0` | `0` means unconstrained |
| `sizing.overflow` | `scroll` \| `clip` \| `fit` | `scroll` | host-owned container |
| `fallback` | node | — | required when `required: false`; drawn only while unresolved |

**ActionSpec (contract side)**

| Field | Type | Rule |
|---|---|---|
| `id` | string | stable, referenced by a control's `action` |
| `label` | string | host may localize; untrusted when provider-supplied |
| `capability` | string | the only capability this action may reach |
| `risk` | `low`..`critical` | from `capability::Risk`; `critical` requires host confirmation |
| `confirmation` | `inherit` \| `host` | `host` always draws the host confirmation, even at low risk |

A validator that meets an unlisted field refuses the document at `1.1`; the vocabulary grows only with a schema revision.

## Appendix C - Provider checklist

An extension author implementing a surface answers these before shipping:

1. Which profiles does the surface render, and does the `text` profile name the same actions?
2. Which state model: `static`, `on_demand`, `stream` or `snapshot`?
3. Which slots does it offer, and is each one required or optional for its consumers?
4. Which capabilities do its bindings name, and is each covered by a grant the package actually holds? A manifest requirement only makes it declarable.
5. Which node ids are interactive, and are they stable across frames?
6. What are its limits, and does it report `truncated` honestly?
7. What are its guarantees: isolation, patch support, offline, determinism, theming, accessibility roles?
8. What happens on failure: which regions can it degrade without losing the rest?
9. Does it ever need a host affordance? If yes, it is doing something the host must own instead.
10. Does it work in the `machine` profile, so an agent can read it without a window?

## References

- [Ferris Cordis-Inspired Evolution RFC](ferris-cordis-evolution-rfc.md) - composition, effects, services, events, lifecycle and the ownership boundary this RFC extends
- [BSH Extension Platform](bsh-extension.md) - Manifest 2.0, contribution kinds, declarative panels, limits and gates
- [Ferris Framework](ferris-framework.md) - implementation status and the frozen decisions
- [Ferris SDK](ferris-sdk.md) - author and host API
- [BSH 2.0 Architecture RFC](bsh-2.0-architecture-rfc.md) - capability, composition, workspace and federation model
- [Architecture](../ARCHITECTURE.md) - BOS/BSH system boundaries
- [cordis-rs](https://github.com/dshbox/cordis-rs) - the runtime that hosts surface providers; its types stay inside the adapter
