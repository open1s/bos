# BSH Extension Platform

Status: normative for BSH 3.x, Ferris framework, manifest 2.0.
Authority: this document and the source under crates/ferris-core, crates/ferris-runtime,
crates/ferris-sdk and crates/bsh/src/extension. If they disagree, the source wins and
this document is fixed in the same change.

Related documents:

- Ferris framework architecture and phase status: [ferris-framework.md](ferris-framework.md)
- Author SDK, host facade and examples: [ferris-sdk.md](ferris-sdk.md)
- Extension WIT contract: [../crates/bsh/wit/extension.wit](../crates/bsh/wit/extension.wit)
- Runnable examples: [../examples/extensions](../examples/extensions)
- Chinese course-style textbook: [bsh-extension-guide.zh.md](bsh-extension-guide.zh.md)

## 1. What an extension is

An extension is a package directory with a validated extension.toml (manifest 2.0),
optional runtime files and optional signatures. BSH installs packages into a hashed
store, resolves dependencies and services, grants capabilities, activates runtimes
through the neutral ExtensionManager, and attaches the resulting tools, commands,
panels, prompts, contexts, skills and other contributions to one agent build.

The stack is deliberately layered:

~~~text
ferris-core      manifest, package store, manager, capability policy, services,
                 events, UI contract, terminal, signing, lifecycle
ferris-runtime   declarative, process, MCP, Extism, Wasm component, Cordis runtimes
ferris-sdk       author API + host facade (the only Ferris entry for BSH)
crates/bsh       host adapter: FerrisHostSpec, FerrisRuntimeResolver, UI, agent wiring
~~~

BSH has exactly one Ferris dependency in Cargo.toml:

~~~toml
ferris-sdk = { path = "../ferris-sdk", features = ["host-runtime"] }
~~~

The SDK host facade re-exports the core modules and the standard runtime crate as
ferris_sdk::host_runtime. BSH source does not import ferris_core or ferris_runtime
directly.

## 2. Package layout

~~~text
my-extension/
  extension.toml          required, schema 2.0
  package.sig             optional Ed25519 signature
  tools/<id>.json         declarative tool descriptor
  commands/<id>.md        declarative command template
  panels/<id>.json        declarative panel document
  prompts/<id>.md         declarative prompt text
  contexts/<id>.md        declarative context text
  skills/                 SKILL.md skill directories
  workflows/<id>.json     recorded workflow payload
  themes/<id>.json        recorded theme payload
  presets/<id>.json       recorded preset payload
  providers/models/<id>.json
  providers/agents/<id>.json
  languages/<id>.json
  integrations/<id>.json
~~~

Only files declared by a contribution are read. Paths are package-relative and must
not escape the package root. Manifests and contribution files are size-limited.

## 3. Manifest 2.0

The only accepted schema generation is 2.0. A 1.x package fails at load time with
"unsupported manifest schema" and must be migrated with bsh ext migrate.

~~~toml
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "A minimal declarative extension"
authors = ["Example"]
license = "MIT"
repository = "https://example.com/hello"
homepage = "https://example.com"

[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[runtime.config]
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]

[configuration]
schema = "schemas/config.schema.json"

[configuration.defaults]
verbose = false

[resources]
max_memory_mb = 256
max_execution_ms = 30000
max_concurrent_calls = 4
max_output_bytes = 1048576

[dependencies]
"com.example.base" = "^1.0"

[services]
provides = []
requires = []
~~~

### 3.1 Top-level fields

| Field | Required | Meaning |
|---|---:|---|
| schema_version | yes | Exactly "2.0" |
| extension | yes | Identity block |
| compatibility | yes | Host and extension API ranges |
| runtime | yes | Runtime kind and opaque config |
| capabilities | no | Required-only capability list |
| contribution | no | Array of generic contribution entries |
| configuration | no | Schema path and defaults |
| resources | no | Memory, execution, concurrency, output ceilings |
| dependencies | no | Extension id to version requirement |
| services | no | Provided and required service contracts |

Unknown top-level fields are rejected. Unknown contribution kinds are rejected when
the manifest is projected into the host's typed contribution view.

### 3.2 Identity

| Field | Required | Meaning |
|---|---:|---|
| id | yes | Reverse-domain extension id |
| name | yes | Human-readable name |
| version | yes | Semantic version |
| description | yes | One-paragraph description |
| authors | no | Display strings |
| license | no | SPDX expression |
| repository | no | Source URL |
| homepage | no | Project URL |

The id is stable, globally unique and used for package directories, storage
namespaces, capability grants, audit records and qualification.

### 3.3 Compatibility

host is the supported BSH version range. extension_api is the extension API
generation. Requirements use semver syntax. A package outside either range is refused
before activation.

### 3.4 Runtime and startup

runtime.kind must be one of the registered strings:

| Kind | Payload | Config keys used by the standard adapter |
|---|---|---|
| builtin | Rust runtime registered by id | none |
| declarative | Package files | startup |
| process | JSON-RPC subprocess | entry, args, env, protocol, startup |
| mcp | MCP server over stdio | entry, args, env, startup |
| extism | Extism PDK plugin | entry, startup |
| wasm-component | WebAssembly component | entry, startup |
| cordis | DSH/Cordis apply(ctx, config) package | entry, startup |

startup values are on-startup, on-demand, on-workspace-open and manual. The default
is on-demand. runtime.config is opaque to the manifest parser and validated by the
runtime adapter.

### 3.5 Capabilities

Capabilities are required-only declarations. Declaring a capability does not grant
it; the host applies policy and the user can grant or revoke each capability per
scope. The complete vocabulary is:

| Capability | Risk | Meaning |
|---|---|---|
| workspace.read | low | Read files inside the workspace |
| workspace.write | high | Write, create or remove files inside the workspace |
| network.http | high | Make outbound HTTP requests |
| process.execute | critical | Start and manage an external process |
| computer.use | critical | Read the screen or drive keyboard and pointer |
| secrets.read | critical | Read a named secret from the vault |
| extension.storage | low | Use the extension's private storage namespace |
| agent.tools.register | low | Register tools the agent can call |
| agent.context.provide | low | Supply scoped context |
| agent.prompts.provide | low | Supply prompt fragments |
| agent.skills.provide | medium | Supply SKILL.md skills |
| agent.models.provide | medium | Register model or provider adapters |
| agent.agents.provide | medium | Register agent roles or strategies |
| agent.workflows.provide | medium | Register reusable workflows |
| events.publish | low | Publish events on the host bus |
| events.subscribe | low | Subscribe to events on the host bus |
| ui.panels | low | Contribute host-managed panels |
| ui.commands | low | Contribute commands and keybindings |
| ui.theme | low | Contribute visual theme resources |
| hooks.observe | high | Receive observe-hook batches |
| agent.intercept | high | Intercept and rewrite host events or requests |

terminal.execute is accepted as a historical alias for process.execute. Run
bsh ext capabilities for the live list.

### 3.6 Contributions

Each entry is:

~~~toml
[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
description = "Echo the input"
~~~

Payload is opaque TOML validated by the kind's host adapter. The standard kinds and
their declarative file conventions are:

| Kind | File / meaning |
|---|---|
| tool | tools/<id>.json |
| command | commands/<id>.md |
| panel | panels/<id>.json |
| prompt | prompts/<id>.md |
| context | contexts/<id>.md |
| skill | skills directory entry |
| workflow | workflows/<id>.json |
| theme | themes/<id>.json |
| preset | presets/<id>.json |
| model-provider | providers/models/<id>.json |
| agent-provider | providers/agents/<id>.json |
| language | languages/<id>.json |
| integration | integrations/<id>.json |
| event.subscribe | Event topic subscription |
| event.publish | Event topic publication |
| hook | Observe-hook point |

Contribution ids are unique within an extension and follow the dotted contribution id
grammar.

Which surface a kind reaches is part of the contract, not an implementation detail:

| Surface | Kinds | Where it lands |
|---|---|---|
| Model-callable | tool | the build registry, as a namespaced live tool - withheld from the request's array until `find_tool` selects it |
| Model-readable | prompt, context, skill | the system prompt, or the skill catalogue |
| Reader-only | command, panel, theme, workflow | the host's plugin registry and inventory |

A package's tools are DEFERRED: registered and callable, but not part of the
request's tool array, because one install contributes a family and every schema
is re-sent on every step. The build advertises `find_tool`; the definitions it
reports join the next request, and the model calls those tools directly. The
Tools pane still lists them (it reads the registered set, not the advertised
one). A tool the model should see without searching is a
tool the prompt should name; the deferral exists for the long tail.

A command is never promoted to a tool: the reader invokes it, and the model
operating the reader's shell is a different grant. A package that wants the model
to run the same effect declares a `tool` too and shares the handler, so one
effect has two front doors and the extension author decides which ones exist.

The composer carries the command's ANNOTATION, not its template. Choosing the
row writes `/name ` into the draft and the reader keeps typing; the send then
composes one message for the model - the template in an `<extension_command
name="...">` block (the framing a skill uses), a `---` divider, then those words -
and the leading annotation leaves the request once its block is attached. So the
transcript keeps the line the reader wrote, and a command's prompt body never
masquerades as text they typed. The token is the qualified name the palette
draws (`namespace__contribution`), which is why the draft and the resolver
cannot disagree about the spelling. The block also carries that name, so a child
run - whose row is flattened from its own agent, i.e. the model-facing request -
can be shown back as `/name words` instead of the template.

The stored row stays the annotation, so every later turn replays it AS THE SAME
BLOCK (`Runner::seed_session`), not as a bare `/name` the model has no
instruction for. The transcript is never rewritten: only the request's view of
the history is. The palette only offers loaded commands; a hand-typed qualified
name that resolves to none is reported by name (`slash.commandUnavailable`) and
the line travels as text, so a typo or a switched-off package does not look like
a command that silently did nothing.

A skill reaches the model in one of two forms, and both feed the same set the
catalogue and the `load_skill` tool read:

- a declared file, `skills/<id>/SKILL.md`, which the host loads at build time;
- a runtime registration. A Cordis package calls `ctx.skills.register(..)` from
  `apply(ctx)`; the runtime materializes each registration under its data
  directory and advertises that directory to the host. The runtime's set wins
  when it holds a skill, and an empty directory falls back to the declared files
  so a package is never worse off for having declared them.

### 3.6.1 Declarative panel documents

A `panel` contribution ships `panels/<id>.json`:

~~~json
{
  "title": "LLM Sniffer",
  "location": "right",
  "default_visibility": "visible",
  "theme": "midnight",
  "theme_dark": "midnight-dark",
  "refresh_ms": 1500,
  "view": { "schema_version": "1.0", "root": { "type": "column", "children": [] } }
}
~~~

The view is validated at load: at most 1000 nodes, depth 32, 16 KiB per text leaf,
256 KiB of text, 500 table rows, 2048 characters per cell, 1 MiB serialized. A
component the host cannot honour is a load error, not a silent fallback.

| Component | Shape |
|---|---|
| container, column, row | `children` |
| text, heading | `text`, heading `level` 1-6, optional `tone` |
| markdown | `text` (rendered as markdown) |
| button | `label`, `action`, `style` (primary/secondary/ghost/danger) |
| input | `label`, `value`, `placeholder`, `action` - a real editable field |
| select | `label`, `value`, options `[{label, value}]`, `action` |
| checkbox | `label`, `checked`, `action` |
| list | items `[{title, detail, action?}]` |
| table | `columns`, `rows`, optional `widths` (one relative weight per column) |
| tree | `nodes` |
| tabs | `tabs` `[{id, label, content}]` |
| section | `title`, `open` (start state), `children` - collapsible, the reader's choice is kept |
| progress | `value` 0-1, `label` |
| code | `language`, `value`, optional `tone` |
| image | `src` must be `asset/...`; remote URLs are refused |
| notification | `level` (info/success/warning/error), `text` |
| divider, spacer | - |

`tone` is a semantic role, never a colour: `neutral`, `muted`, `faint`,
`accent`, `success`, `warning`, `error`. The host resolves it against the
panel's palette, so the same document follows the reader's light/dark choice and
a package's own theme tokens.

**Actions.** An action id is a contribution id owned by the package. A press
calls the tool with that id, falling back to the command seam when no tool owns
it. Inputs send their text (`{"value": ...}`), selects their option value,
checkboxes `{"checked": bool}`. Every action carries the revision it was
rendered from; a stale action is refused rather than applied to a newer frame. A
handler that returns a UI document replaces the card at the next revision; any
other JSON is shown as a message under it.

**Live refresh.** `refresh_ms` (250..60000) asks the host to re-read a live card
on that interval while the pane is visible. The timer is armed from a render, so
a hidden pane stops refreshing by itself.

**Theming.** `theme` and `theme_dark` name themes the same package contributes;
the host picks the dark one in dark mode and falls back to the light one, and to
the host palette when neither is named. The palette is resolved per frame, so a
light/dark switch lands without rebuilding the pane.

**A declared view is a placeholder.** The host draws it immediately, then asks
the runtime for its live card once per selection (and on Refresh). If the
runtime cannot answer, the declared view stays and the panel says why.

### 3.6.2 Observe hook points

A `hook` contribution names one point; the host pushes a bounded batch of
events to the guest's `bsh_hook` export, shaped `{point, batch: [event, ...]}`.
Hooks are observe-only: a guest cannot deny or replace the call.

| Point | Emitted when | Event fields |
|---|---|---|
| `turn.start` | a user input opened a turn | `session, agent, input_chars` |
| `turn.end` | the turn settled (done or failed) | `session, agent, failed, events` |
| `model.request` | before one LLM step | `agent, data: {model, request_id, request}` (`request` is serialized JSON, host-capped at 8 KiB) |
| `model.response` | after one LLM step | `agent, data: {request_id, response_type, response}` or the stream fields |
| `tool.before` | before every tool call, including namespaced MCP tools and `load_skill` | `agent, data: {tool_name, call_id, tool_args}` |
| `tool.after` | after every tool call | `agent, data: {tool_name, call_id, tool_result}` |
| `skill.load` | a skill body is attached to a turn's prompt | `skill, bytes` |
| `http.exchange` | every LLM vendor call, and every HTTP fetch a sandboxed runtime makes through its bridge | metadata, masked headers and bounded bodies (see the tap's caps) |

Scope of the coverage, stated rather than implied:

- An MCP **tool call** is a tool call: it arrives at `tool.before`/`tool.after`
  with a namespaced `tool_name`. The MCP **wire** (initialize, tools/list, the
  JSON-RPC frames) is not observed yet.
- Network activity is observed where the host mediates it: LLM vendors, the
  extism/process HTTP bridge, and anything that goes through the tap. A shell
  command's own sockets are outside the host and are not visible.
- Skill *bodies* are observed when attached (`skill.load`); the skill catalogue
  in the system prompt is not a per-turn event.

### 3.7 Services

~~~toml
[services]
provides = []
requires = []

[[services.provides]]
name = "com.example.search"
contract = "com.example.search-api"
version = "1.0.0"
scope = "workspace"
capabilities = []

[[services.requires]]
name = "com.example.config"
contract = "com.example.config-api"
version = "^1.0"
scope = "extension"
optional = true
~~~

Scope is host, workspace, session or extension. Service discovery is not
authorization: the consumer must still hold the capabilities the provider declares.
A required service that cannot be satisfied blocks activation unless optional is true.

### 3.8 Dependencies, resources and configuration

Dependencies are a map of extension id to semver requirement. Cycles and unsatisfied
requirements are refused before activation.

Resource ceilings are requests subject to host limits. Defaults are 256 MiB memory,
30000 ms execution, host concurrency and output limits when omitted.

Configuration uses a package-relative JSON Schema and optional TOML defaults. Values
are read and written with bsh ext config <id> [set <json|file>|schema].

## 4. Lifecycle and activation

The package lifecycle is:

~~~text
available -> installed -> resolved -> applied -> active
                    \-> failed / quarantined
~~~

Installation stages the package, verifies hashes and signatures, records the version
and dependency lock. Activation is driven by ExtensionManager in ferris-core:

1. plan resolves dependencies, services and activation order.
2. apply creates each runtime, starts it and collects contributions.
3. the host ContributionSink installs agent-facing contributions and returns leases.
4. ActivationSet owns runtime and contribution leases.
5. unload revokes leases and shuts the runtime; reload reconciles a new plan.

BSH calls the manager from apply_with_fibers and stores one Activation per applied
extension. Agent contribution leases are revoked through BshLeaseHost, so the manager
owns both runtime and contribution lifecycles. A failed apply or reload rolls back
already-started activations.

## 5. Package store, signatures and market

The store is content-addressed by package hash and keeps one directory per version.
Installed, active, previous and quarantined versions are tracked; uninstall keeps the
data namespace by default and --purge-data removes it.

Publisher trust is Ed25519:

- bsh ext keygen <publisher> creates a key pair.
- bsh ext sign <path> --publisher <name> --key <file-or-base64> writes package.sig
  over id, version and package hash.
- bsh ext trust <publisher> <public-key> records trust.
- bsh ext verify <path> checks the signature; unsigned packages can still install
  when policy allows them.

The market is a Git repository, not a service. The official provider is
https://github.com/open1s/bsh_extensions with this layout:

~~~text
bsh_extensions/
  index.json
  packages/<id>/<version>/...
~~~

bsh ext market lists entries, bsh ext market install <id> [version] installs a
version (the newest compatible one when none is named), and registry add/list
configures private providers. An index may advertise several versions of one
id; the settings Market tab draws one row per extension and offers the versions
on that row, while installed versions that the index no longer carries stay
selectable because the store keeps them for rollback.

## 6. CLI reference

All commands are under bsh ext:

| Command | Purpose |
|---|---|
| new <id> | Scaffold a 2.0 declarative package |
| validate <path> | Parse and validate a package |
| migrate <path> [--write] | Convert a 1.x manifest to 2.0 |
| hash <path> | Print the canonical package hash |
| build / package <path> | Validate and build a .tar.gz release |
| test <path> | Validate the package and declared contributions |
| mock <path> [tool] [json] | Run one tool against a permissive mock host |
| install <path|archive|git+url> | Install into the managed store |
| uninstall <id> [--purge-data] | Remove a package |
| list | List installed and built-in extensions |
| plan [--json] | Print manager activation order |
| graph [--json] | Print the service/dependency graph |
| why <id> | Explain why an extension is or is not applied |
| effects <id> | List the declared effect surface |
| reload <id> | Rebuild the host view of one extension |
| mount <file> | Apply a Cordis/profile patch |
| info <id> | Show one extension |
| enable / disable <id> | Toggle activation |
| versions <id> | List installed versions |
| update <path> | Install and activate a new version |
| rollback <id> | Activate the previous version |
| reproduce <lock> | Reinstall exactly the locked versions |
| retry <id> | Clear a persisted failure and reload |
| quarantine <id> [--reason] | Quarantine code and disable |
| grant / revoke <id> <capability> | Change capability grants |
| capabilities | List the capability vocabulary |
| logs <id> | Audit and runtime logs |
| config <id> [set|schema] | Read or write configuration |
| metrics | Call, event and subscriber counters |
| providers / languages / integrations | List declared descriptors |
| language-check <id> <lang> <path> | Run a language service diagnostic |
| integrate <id> <integration> [json] | Run an integration action |
| market [query] / market install <id> [version] | Git market |
| doctor | Check store, hashes, dependencies and policy |
| keygen / sign / verify / trust / untrust / publishers | Signing and trust |
| registry add/list | Registry configuration |

## 7. Authoring with the SDK

ferris-sdk is the only author entry point. There is no macro DSL. An author builds a
validated manifest, implements Extension and optionally uses the test harness:

~~~rust
use ferris_sdk::prelude::*;

struct Echo;

#[async_trait]
impl Extension for Echo {
    fn manifest(&self) -> ExtensionManifest {
        ManifestBuilder::new("com.example.echo", "Echo", "1.0.0", "Echo input")
            .capability("agent.tools.register")
            .contribution(ContributionSpec::tool("echo"))
            .build()
            .expect("manifest")
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
        Ok(vec![ContributionSpec::tool("echo")
            .into_contribution(json!({ "description": "Echo" }))])
    }

    async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}
~~~

ManifestBuilder covers identity, compatibility, runtime, dependencies, capabilities,
services, resources, configuration, startup and every standard contribution kind.
See docs/ferris-sdk.md and crates/ferris-sdk/examples/echo.rs and
crates/ferris-sdk/examples/toolkit.rs.

Quick start with the host CLI:

~~~bash
bsh ext new com.example.hello --dir hello
bsh ext validate hello
bsh ext test hello
bsh ext install hello
bsh ext list
bsh ext info com.example.hello
~~~

## 8. Migration from 1.x

Manifest 2.0 hard-cuts the old format. bsh ext migrate <path> prints a 2.0
manifest; --write replaces extension.toml and keeps extension.toml.v1 as a backup.
The migration maps:

- compatibility.bsh -> compatibility.host
- runtime kind and entry/args/env/protocol/startup -> runtime.config
- contributes.* arrays -> [[contribution]] entries
- events -> event.subscribe, publishes -> event.publish, hooks -> hook

All shipped examples and the bsh ext new scaffold are already 2.0. Legacy test
fixtures live under crates/ferris-core/testdata/legacy and are intentionally 1.x.


## Network egress

`network.http` grants are public-egress grants. For every hop (the initial URL
and each redirect) the host is resolved once, **every** returned address is
classified, and the validated addresses are pinned into the client so a second
DNS lookup cannot swap in a private answer (DNS rebinding). Loopback, private,
link-local, CGNAT, multicast, unspecified, broadcast, documentation and
benchmark ranges are refused by default - including a hostname that mixes a
public and a private answer. The one explicit exception is a grant that names
the destination as a literal IP (for example `domain:127.0.0.1` for a local test
server): that is the separate, address-scoped private-network grant. TLS
verification and the HTTP `Host` header still use the URL hostname.
## 9. Security limits

- Capability checks are per call and scoped; registration is not authorization.
- Process and MCP children are **not** OS-sandboxed on any platform: a ferris
  runtime controls its own confinement, so `process.execute` is a trust
  decision about the package, not a container. `network.http` still gates
  host-mediated HTTP calls and tools, but the host does not intercept sockets a
  child opens for itself.
- Package paths, contribution sizes, manifest size, UI document depth, event rates
  and storage writes are bounded.
- Service discovery, event delivery and plugin mounting never grant filesystem,
  network, process, credential or Computer Use access.
- Signing proves publisher identity and package integrity; it does not replace
  capability review.

## 10. Verification and gates

The executable acceptance gates are:

- cargo fmt --all -- --check
- cargo clippy --workspace --exclude jsbos --exclude nbos --all-targets -- -D warnings
- RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
- cargo test -p ferris-core --features tokio
- cargo test -p ferris-runtime --features "wasm extism cordis"
- cargo test -p ferris-sdk
- cargo test -p bsh --lib extension::
- scripts/ci/check-extension-gold-standard.sh
- scripts/ci/check-ferris-independence.sh

Current green baseline at this revision:

- core: 159 passed, 8 ignored
- runtime: 49 passed
- SDK: 4 passed
- BSH extension: 56 passed
- gold conformance: RC 0
- Ferris independence: ok

The full workspace suite passes when the local Chrome-only UI drag test is skipped;
CI has no Chromium and skips that test through discover_binary.

## 11. Deliberate limits

- workflows, themes and presets are recorded contributions; there is no workflow
  executor in this document's scope.
- The declarative runtime executes tool bodies as pure transformations; it does not
  provide a general scripting language.
- Hot reload is host-driven through bsh ext reload; it is not a file watcher.
- The market is Git-backed and has no bespoke registry service.
- A host may add capability names, contribution kinds and runtime kinds, but BSH's
  standard vocabulary is the one listed here.
