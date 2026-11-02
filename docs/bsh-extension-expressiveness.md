# BSH Extension Expressiveness

This note records the expressiveness target for the BSH extension framework:
**no BSH capability should be out of reach of an extension, and the composition
model should be at least as expressive as Cordis while keeping BSH's capability
and sandbox boundaries.**

The implementation lives in crates/bsh/src/extension. The design rationale is
docs/RFC-EXT-003-cordis-composable-extension-runtime.md; the acceptance standard
is docs/bsh-extension-gold-standard.md.

## 1. The three seams

Every BSH aspect is exposed through exactly one of three seams. An extension
does not need a new core match arm for each feature.

| Seam | Use | Ownership |
|---|---|---|
| Contribution | a declared artifact (tool, command, panel, prompt, skill, provider, theme, workflow) | manifest + ContributionLease |
| Service | a request/response capability | Context + ServiceRegistry generation |
| Event | observation or interception (emit, waterfall, ...) | Context + EventDispatcher listener |

A host subsystem that can be named can be exposed without changing the
extension runtime:

~~~rust
// Request/response seam.
let terminal = ctx.acquire::<TerminalService>("terminal")?;
terminal.run(command).await?;

// Observation/interception seam.
dispatcher.on("tool.before", DispatchMode::Waterfall, 0, |request, next| {
    // inspect, rewrite, or short-circuit
    next.next(request)
});

// A registration that disappears with its context.
ctx.effect("subscribe", |_| {
    let subscription = dispatcher.on("model.request", DispatchMode::Emit, 0, handler);
    EffectCleanup::sync(move || drop(subscription))
})?;
~~~

## 2. Aspect-to-seam map

| BSH aspect | Extension seam | Status |
|---|---|---|
| Agent turn / loop | agent.turn waterfall + agents service | implemented via AgentPlugin on_turn_start/end |
| Model request/response | model.request / model.response waterfalls | implemented (DispatcherPlugin bridges AgentPlugin stages) |
| Tool before/after | tool.before / tool.after waterfalls | implemented (ExtensionTool + DispatcherPlugin) |
| Tools | manifest contribution + tools service | implemented |
| LLM providers | provider descriptor + llm service | descriptor implemented |
| Prompts and context | prompt/context contributions | implemented |
| Skills | skill contribution / inline skill body | implemented (inline body pending) |
| Commands | command contribution | implemented |
| UI panels | declarative UI + host GPUI adapter | implemented |
| Native UI slots | host-owned slot registry | refused by design for native GPUI |
| Terminal | terminal service + capability | implemented (HostTerminal, ProcessExecute-gated) |
| Settings / config | ConfigHandle + JSON Schema | config primitive implemented |
| Storage | per-extension namespace | implemented |
| Workspace / session scope | Context scope path | implemented (host/workspace/session inheritance) |
| Cross-extension services | ServiceRegistry contracts | implemented, extension scope enforced |
| Events | EventDispatcher five modes | implemented |
| Workflows | workflow contribution | implemented |
| MCP | MCP runtime adapter | implemented |
| Themes | theme contribution | implemented |
| DSH Cordis bundles | Cordis runtime adapter | tools/commands/skills + profile mount; HMR owed |

The "not yet" rows are host-adapter work, not missing expressiveness in the
composition model: the service, event, and effect seams they need already exist.

## 3. Why this is at least Cordis's expressiveness

| Cordis mechanism | BSH equivalent |
|---|---|
| Context | Context with parent, kind, scope path, effects, services |
| Fiber | Fiber with state, epoch, inject, effects, async dispose |
| inject / reactive coeffects | Inject + Epoch + Reconciler + CompositionRuntime |
| ctx.effect | EffectScope with sync/async/nested cleanup |
| ctx.on / waterfall | EventDispatcher with five modes |
| ctx.isolate | per-service named realms with realm-carrying reactive notifications |
| Loader entries / groups | CompositionPlanner profiles, groups, patches |
| HMR | package update/reload; live HMR is owed |
| Dynamic property access | explicit typed services, contracts, and events |
| Unrestricted JS services | sandboxed runtimes + capability policy |

The BSH model is deliberately stricter where Cordis is loose: service access is
typed and auditable, and registration never grants a capability. That is
expressiveness without losing the security boundary.

## 4. Adding a new aspect

A host subsystem that wants to be extensible follows four steps:

1. **Name the seam.** Request/response goes to a typed service; observation or
   policy goes to an event.
2. **Declare the contract.** Give the service a name, contract id, version, and
   required capabilities; give the event a name, mode, and payload schema.
3. **Register the implementation.** Put the service in the host ServiceRegistry
   at Host/Workspace/Session scope, or put the emitter on the EventDispatcher.
4. **Expose it through a Context.** Extensions acquire the service or register an
   event listener. The effect scope removes both on unload.

No change to the extension runtime is required. This is the practical meaning of
"extend any aspect of BSH".

## 5. Performance rules

The design targets better than Cordis on the hot path:

- Service lookup is a read-lock hash lookup, not a proxy trap chain.
- Scope inheritance is at most four entries (Extension -> Session -> Workspace ->
  Host); exact lookup is O(1).
- Dependency changes update an index; the reconciler never scans all fibers.
- A burst of changes coalesces by fiber id into one reload.
- Sync effects avoid a boxed future; async effects are opt-in.
- Context is an Arc clone; it is not rebuilt per call.
- The reconciler is event-driven; there is no polling loop.

Known cost to watch: the dispatcher clones the JSON payload per listener. A hot
per-token event should use an Arc payload or a typed Rust event before it is
wired into the model stream.

## 6. Honest remaining work

These are the gaps that still need code, not just a seam:

1. **Legacy feature migration** -- the annotations provider is now a
   first-party package (`com.bos.bsh.annotations`) that gates the NotesStore,
   and disabling it disables the store. The `Notes` value type and UI callsite
   remain in core as a thin view; a process-package provider is still separate.
2. **Live HMR** -- package update reloads the host; in-place module replacement
   is not implemented.
3. **Native UI slots** -- the DSH client plane is browser JS; BSH draws native
   GPUI, so ctx.slots stays refused rather than faked.
4. **Cordis patch HMR** -- rows mount by writing config and rebuilding the host;
   an in-place composition swap is not implemented.

The tests in crates/bsh/src/extension/composable/ pin the
semantics that the wiring must preserve.
