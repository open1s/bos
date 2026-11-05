# Ferris from Python and Node.js

Status: authoring and full host bindings implemented in `nbos` and `jsbos`.
The host is enabled by default; build with `--no-default-features` for the
authoring-only addon.

## Question

Can `jsbos` and `nbos` bind the Ferris SDK so Python and JavaScript can use
Ferris, and what would a full host binding require?

## Verdict

Yes. Ferris was designed host-neutral: `ferris-sdk` depends on `ferris-core`
only, and the author-facing `Extension` trait is plain Rust with an async
`start`/`invoke`/`shutdown` surface. That is directly adaptably to a Python
object or a JavaScript object, and the SDK's `ExtensionRuntime` already provides
the adapter seam.

Two very different scopes exist:

1. **Authoring and in-process runs** (shipped): a Python/JavaScript object
   implements the SDK's Extension interface, declares a schema-2.0 manifest, and
   runs under `FerrisHarness`, the binding equivalent of the Rust
   `TestHarness`. This needs only `ferris-sdk` with its default features — no
   WASM, no Extism, no host manager.
2. **Package host** (shipped behind the default `ferris-host` feature): install,
   list, plan, apply and unload packages, run the standard runtimes
   (declarative, process, MCP, Cordis, WASM/Extism), inspect contributions and
   forward invocations. This adds a `HostSpec`, a `RuntimeResolver`, a
   `ContributionSink` and the standard runtime features.

## Shipped API

Both bindings expose `FerrisManifest`, the chainable
`FerrisManifestBuilder` and `FerrisHarness`.

```python
from nbos import FerrisHarness, FerrisManifest

manifest = FerrisManifest.parse(open("extension.toml").read())

async def start(ctx):
    return [{"kind": "tool", "id": "echo", "payload": {"description": "Echo"}}]

async def invoke(call):
    return call["input"]

harness = FerrisHarness(manifest.to_toml(), "/tmp/echo", start, invoke)
await harness.start()
result = await harness.invoke("echo", {"text": "hi"})
await harness.shutdown()
```

```javascript
const { FerrisHarness, FerrisManifest } = require('@open1s/jsbos');

const manifest = FerrisManifest.parse(manifestToml);
const harness = new FerrisHarness(
  manifest.toToml(),
  '/tmp/echo',
  async (ctx) => [{ kind: 'tool', id: 'echo', payload: { description: 'Echo' } }],
  async (call) => call.input,
);

await harness.start();
await harness.invoke('echo', { text: 'hi' });
await harness.shutdown();
```

Callback payloads are plain JSON-shaped values:

| Callback | Receives | Returns |
|----------|----------|---------|
| `start(ctx)` | `{ id, root, data_dir, runtime_kind }` | array of `{ kind, id, payload }` |
| `invoke(call)` | `{ contribution: { kind, id }, input, context: { correlation, timeout_ms } }` | any JSON value |
| `shutdown()` | no arguments from the high-level wrapper (the native JS callback receives an empty object) | ignored |

Callbacks may be synchronous or return a promise / coroutine.

## Host API

`FerrisHost` owns one canonical Ferris store and drives the package manager.
It is the binding equivalent of BSH's extension host, without a UI.

```python
from nbos import FerrisHost

host = FerrisHost("/tmp/ferris-store", workspace="/tmp/workspace")
host.install("/path/to/package")
print(host.plan())                 # ["com.example.echo"]
contributions = await host.apply() # { "com.example.echo": [...] }
result = await host.invoke("com.example.echo", "tool", "echo", {"x": 1})
await host.unload()
```

```javascript
const { FerrisHost } = require('@open1s/jsbos');

const host = new FerrisHost('/tmp/ferris-store', '/tmp/workspace');
host.install('/path/to/package');
host.plan();
const contributions = await host.apply();
const result = await host.invoke('com.example.echo', 'tool', 'echo', { x: 1 });
await host.unload();
```

Host surface:

| Area | Members |
|------|---------|
| Store | `install`, `installPackage(source, origin)` (origin: `installed`, `development`, `builtin`), `uninstall`, `installed`, `versions`, `activeVersion`, `activate`, `rollback`, `setEnabled`, `clearFailure`, `quarantine`, `packageRoot` |
| Activation | `plan`, `apply`, `reload`, `unload`, `contributions` |
| Invocation | `invoke(extension, kind, id, input)` |
| Linked packages | `registerExtension(manifest, root, start, invoke, shutdown)` |
| Events | `subscribeEvents(topics, callback)`, `unsubscribeEvents(id)`, `publishEvent(topic, payload)` |
| Policy | `grant`, `revokeGrant`, `grants`, `auditDegraded` |
| Agent tools | `registerAgentTools(agent)` |

Runtimes: declarative, process, MCP, Cordis, Extism and WebAssembly Component.
A `builtin` manifest is refused because no host-linked builtin is registered by
the binding. Contributions are collected as JSON; the caller decides whether to
register them with an agent.

Policy defaults to `enforce`; grant the capabilities a package needs before
invoking it:

```python
host.grant("com.example.echo", "process.execute", "program:node")
```

`scope` accepts `any`, `workspace`, `path:<prefix>`, `domain:<name>`,
`program:<name>` and `secret:<name>`. Pass `policy_mode="advisory"` for a
local development session.

## Shared host assembly

The non-FFI host pieces — `BindingHostSpec`, `BindingRuntimeResolver`,
`AppliedContributions`, the lease/sink pair, parsers and `ShellTerminal` —
live in `crates/ferris-bindings`. The JS and Python crates own only napi/pyo3
classes and callback adapters, so the two bindings cannot drift. The `host`
feature is opt-in, keeping the authoring-only addon free of wasmtime/Extism.

## How it works

```text
Python object / JS object
        │  pyo3 / napi callbacks
        ▼
PyForeignExtension / JsExtension   (implements ferris_sdk::Extension)
        ▼
ferris_sdk::ExtensionRuntime       (the Rust TestHarness seam)
        ▼
ferris_sdk::Runtime                (describe + invoke)
```

- `FerrisManifest.parse` calls `ExtensionManifest::parse`, so the same schema-2.0
  validation that guards Rust extensions guards bindings.
- Python async callbacks are awaited through the same
  `call_python_callback_maybe_async` bridge the Python tool adapter uses; the
  coroutine runs with `asyncio.run` on a blocking thread, so the GIL is not held
  while it runs.
- JavaScript callbacks use a `ThreadsafeFunction` and await a returned promise
  through `PromiseRaw`, the same path as the JavaScript tool adapter.
- The harness itself is `Arc<ExtensionRuntime<...>>`, so start/invoke/shutdown
  can run concurrently without borrowing the language object.

## Constraints

- **In-process means trusted.** A Python/JavaScript extension runs with the
  host process's privileges. The Ferris capability list is declared in the
  manifest but the harness does not sandbox the code. Untrusted packages must go
  through the process/WASM runtimes and a host policy engine.
- **Builtin manifests are host-linked.** An installed package whose runtime is
  `builtin` is refused because the binding has no host-linked builtin table.
  `FerrisHost.registerExtension` covers in-process language extensions, and
  all installed package runtimes (declarative, process, MCP, Cordis, Extism,
  WASM component) are supported.
- **Contributions are data first.** `FerrisHost` records each package's
  contributions and can call `onContributions` / `onRevoke`; the caller still
  registers tools/prompts/skills with its own agent. There is no automatic
  language-side lease that removes them.
- **Process sandboxing is on by default.** `processSandbox: false` (JS) /
  `process_sandbox=False` (Python) disables the platform sandbox for process
  and MCP runtimes; it exists for hosts whose nested sandbox cannot start. It
  is a development escape hatch, not a security boundary.
- **Binary size.** The host build links `wasmtime` and `extism`, so the addon is
  much larger than the authoring-only build. Measured on macOS arm64 release:
  19.4 MB before Ferris, 25 MB for the authoring-only build, 45 MB for the full
  host build. Use `--no-default-features` when only authoring is needed.

## Host design

- `HostSpec` uses the store root, an empty scope path and a capability registry
  seeded from `Capability::ALL`.
- `RuntimeResolver` calls `ferris_runtime::factory::build` with
  `RuntimePorts` (policy, event bus, workspace, data root, `{}` config, trust,
  shell terminal) and wraps the legacy runtime in `LegacyAdapter`.
- `ContributionSink` records `{ kind, id, payload }` per extension and returns a
  lease that removes the entry on revoke.
- `apply` plans in dependency order, starts each runtime, takes the
  `Activation` out of the manager's set and owns it so `invoke` and `unload`
  can address it later. `unload` revokes leases and shuts runtimes down in
  reverse order.
- Policy defaults to enforce. Grants are persisted under
  `<root>/policy/grants.toml` and audited to `<root>/policy/audit.jsonl`.

## Remaining work

- Host contribution callbacks deliver `{ extension, contributions }`;
  `registerAgentTools` / `register_agent_tools` adds applied tools to an
  Agent/AgentBuilder and forwards calls to the owning runtime. Prompts, skills
  and context still belong to the caller's agent surface.
- Host event callbacks are delivered, but host-command callbacks are not bound.
- `registerExtension` hosts an in-process language extension as a linked
  package; it is not part of the dependency plan or the installed store.
- Cross-platform release packaging for the WASM-enabled addon.

## Test coverage

- `crates/jsbos/test/ferris.test.js`: manifest parse/builder, async/sync
  callbacks, start/invoke/invokeContribution/shutdown, callback errors.
- `crates/jsbos/test/ferris_host.test.js`: install/plan/apply/invoke/unload,
  enable/disable, grants, contribution/revoke callbacks, event subscription,
  linked JS extension, linked replacement teardown, rejected-callback cleanup,
  unknown-scope refusal, agent tool registration, and
  declarative/process/MCP/Cordis/Extism/WASM end-to-end runs.
- `crates/jsbos/test/ferris-parity.test.js`: static JS/Python surface parity for
  `FerrisManifest`, `FerrisManifestBuilder`, `FerrisHarness` and `FerrisHost`.
- `crates/nbos/tests/test_ferris.py`: the authoring matrix plus coroutine
  shutdown.
- `crates/nbos/tests/test_ferris_host.py`: the host matrix, including the same
  runtime coverage.
- MCP and Cordis are covered end-to-end with `process_sandbox=False` in this
  environment; the default sandbox remains the production path.

## Recommendation

Ship both surfaces. Keep `FerrisHarness` for extension authors and
`FerrisHost` for programs that manage packages. Expose host activation as an
explicit call (`apply`) rather than at construction, so a program decides when
to start installed code.
