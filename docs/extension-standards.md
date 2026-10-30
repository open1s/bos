---
module: agent
tags: [extensions, dsh, codex, plugins, bridge, trust, sandbox]
problem_type: reference
---

# Extension standards: what DSH and Codex plugins can actually do here

> **Historical reference.** This document measures the Codex and DeepSeek
> Harness plugin formats and the legacy shim in `crates/agent/src/extensions`.
> It is kept for the old inventory pane and for comparison. BSH's own extension
> standard, implemented by `crates/bsh/src/extension`, is
> `docs/bsh-extension-standard.md`; new packages target that manifest
> (`extension.toml`), not the DSH or Codex formats described here.

Measured 2026-10-23 against the shipped artefacts on this machine, not against
the fixtures. Every number and quotation below is reproducible with the recipe
in the appendix.

The subject is `crates/agent/src/extensions/`. One substrate serves both
standards: a package is parsed by the shim for its standard, lowered onto one
`Extension` model, and its contributions are registered once in
`ExtensionRegistry`. There is exactly one adapter seam for imperative entries
(`extensions/bridge.rs`) and one for declarative files
(`extensions/declarative.rs`).

---

## 1. The DeepSeek Harness standard, as measured

### 1.1 Extraction recipe

`/Applications/DeepSeek Harness.app/Contents/Resources/app.asar` is 182,833,898
bytes. Header layout: `[u32 ?][u32 pickle][u32 jsonLen][json file index]`, with
file data at `8 + pickle` (= 4,780,404 here). The index is a TREE whose `files`
may be a **list** of `{name, size, offset, ...}` or a **dict** keyed by name;
both shapes occur in one archive, so a walk that assumes either one returns
zero entries without erroring. `json.JSONDecoder().raw_decode` is required:
`jsonlen` (4,780,392) overruns the JSON by six bytes.

```
total entries            18,312
distinct @deepseek-ai packages   307
packages with a `dsh` field      100
  dsh.client                     78
  dsh.bundle                     18
  dsh.sessionFormatMigration      4
  dsh.manifestVersion             0
```

**The manifest declares almost nothing.** `dsh.manifestVersion` appears on
**zero** of the 100 packages that carry a `dsh` object; the field exists in the
format but no shipped package writes it. There is no `dsh.tools`,
`dsh.commands`, or `dsh.skills` field anywhere. Everything a package delivers
is registered by executing its JavaScript host entry.

### 1.2 The host entry

`main` resolves for 301 of the 307 packages (`lib/index.js` for almost all).
The export is a Cordis plugin: an `apply(ctx, config)` function. From
`@opencode2dsh/dsh-plugin` (a real third-party package installed here at
`~/.bos/bsh/extensions/opencode2dsh_dsh_plugin/`):

```js
const inject = ["llm"];
function apply(ctx, config = {}) {
  const resolved = config;
  if (resolveConfig(config).mode === "sidecar") applySidecar(ctx, config);
  else applyAdapter(ctx, resolved, () => readVolatile(resolved.ipPool));
}
```

and inside `applyAdapter`:

```js
if (!ctx.llm || typeof ctx.llm.registerAdapter !== "function") {
  logger.error("opencode2dsh: llm service unavailable; adapter mode cannot register");
  return { ready };
}
// ...
registration = ctx.llm.registerAdapter([PROVIDER_ID], adapter);
```

The package also declares `inject = ["llm"]`, which is the Cordis dependency
injection contract: the fiber does not activate until `llm` exists.

### 1.3 The registration seams, quoted

**`ctx.tools.register(definition)`** — `@deepseek-ai/dsh-tools/lib/index.js`:

> `* Register globally or in the calling agent scope. Scoped tools shadow`
> `* globals; duplicates within one layer and the reserved `run_code` name fail.`
> `* @param definition - tool schema, execution, and optional finalization/presentation callbacks.`
> `* @returns the exact disposer that unregisters the tool.`

It validates hard: `tool "${name}" must declare output { schema, render, presentationMeta? }`,
and the name `run_code` is *"reserved for the PTC mode presentation transport and
cannot be registered or shadowed"*. `defineTool(options)` takes `name`,
`description`, `parameters`, `output.schema`, `output.render`, `execute`,
`finalizeContent`, `projectContent`, `presentCall`, `presentResult`,
`isConcurrencySafe`, `deferLoading`, `timeoutMs`.

**`ctx.commands.register(definition)`** — `@deepseek-ai/dsh-commands/lib/index.js`:

> `* Register a global or calling-agent-scoped command.`
> `* @param definition - discovery metadata and direct UI handler.`
> `* @returns the exact effect disposer that unregisters this definition.`

The validator is the contract:

```js
if (!COMMAND_NAME.test(definition.name)) throw new TypeError(`command name "${definition.name}" must match ${String(COMMAND_NAME)}`);
if (typeof definition.description !== "string") throw new TypeError(...);
if (definition.description.trim().length === 0) throw new TypeError(`command "${definition.name}" description must not be empty`);
if (typeof definition.handler !== "function") throw new TypeError(`command "${definition.name}" handler must be a function`);
```

plus an optional `input: { hint: string, attachments?: boolean }`. The handler
must return a `CommandResult` with a `kind` discriminator.

**`ctx.skills.register(skill)`** — `@deepseek-ai/dsh-skill/lib/index.js`:

> `* Register a borrowed readonly runtime skill into the calling context's layer.`
> `Project entries outrank runtime entries, which outrank user entries, within one layer.`
> `Same-name runtime entries in one layer are first-wins; a duplicate logs a warning`
> `and receives a no-op disposer so it cannot remove the winner.`

Required: `name` (`SKILL_NAME`), non-empty `description`. Optional: `whenToUse`,
`invocation` (defaulting to `{ modelInvocable: true, userInvocable: true }`),
`content`, `source`, `provider`, `path`.

**`ctx.on(name, listener, options)`** — the Cordis event bus, from
`@deepseek-ai/cordis/lib/index.js`:

> `* Register an event listener owned by the current fiber.`
> `* The listener is removed automatically when the fiber unloads. Throws`
> `` `CordisError('INACTIVE_EFFECT')` `` `if the fiber is already disposed.`

The dispatcher supports five modes — `emit` (concurrent), `serial`, `bail`,
`waterfall` — where a waterfall listener that does not call `next()` vetoes the
rest of the chain.

**`ctx.effect(execute, label = "anonymous")`** — Cordis's disposer tree. The
callback returns a disposer (or a generator/async-iterator of them); it is what
makes `register` reversible, and every registration seam above is implemented
in terms of it (`this.layers.effect(this.ctx, ..., { label: "tools.register()" })`).

**`ctx.slots.register` / `ctx.slots.inject`** — the CLIENT UI seam. Measured in
the 143 `dsh-client-*.js` bundles in the asar:

```
ctx.slots.register   229 occurrences
ctx.slots.inject     164 occurrences
ctx.locale.register   56 occurrences
```

These live in the browser bundle, never in the host entry.

### 1.4 The minimum host surface, by usage

Counting `ctx.<member>` across the 301 host entries, by **number of packages**
that touch it (a count of packages, not of call sites, because one package
repeating a member 40 times is still one dependency):

| member | packages | calls |
|---|---|---|
| `ctx.effect` | 93 | 155 |
| `ctx.on` | 80 | 193 |
| `ctx.logger` | 76 | 252 |
| `ctx.get` | 64 | 208 |
| `ctx.inject` | 44 | 53 |
| `ctx.fiber` | 34 | 69 |
| `ctx.tools` | 34 | 91 |
| `ctx.llm` | 11 | 24 |
| `ctx.commands` | 5 | 5 |
| `ctx.skills` | 4 | 11 |
| `ctx.slots` | **0** | 0 |

The five members a package needs to contribute anything are `ctx.tools`,
`ctx.commands`, `ctx.skills`, `ctx.llm` (for providers), and `ctx.on`/`ctx.effect`
(the lifecycle pair every one of them is built on). `ctx.slots` is absent from
host entries entirely, which is why the client entry is a separate plane.

---

## 2. The Codex standard, as measured

From `~/.codex/.tmp/plugins/plugins/` (64 manifests) and
`~/.codex/plugins/cache/openai-api-curated/` (12 more), read-only.

### 2.1 `.codex-plugin/plugin.json`

Field frequency across the 64 manifests:

```
name 64   version 64   description 64   author 64   interface 64
homepage 52   repository 52   license 53   keywords 53
skills 47      apps 36      mcpServers 9      hooks 1
```

`author` is an **object** — `{ name, email, url }`:

```json
"author": {
  "name": "HeyGen",
  "email": "hyperframes@heygen.com",
  "url": "https://hyperframes.heygen.com"
}
```

`interface` field frequency (all 64 unless noted):

```
displayName 64   shortDescription 64   longDescription 64   developerName 64
category 64      capabilities 64       defaultPrompt 64
websiteURL 64    privacyPolicyURL 61   termsOfServiceURL 61
logo 60          composerIcon 57       screenshots 46
brandColor 49    supportURL 15         logoDark 10   brandColorDark 10
```

### 2.2 The companion files

A plugin root carries (counts over the 64):

```
.codex-plugin  64   assets 60   skills 48   .app.json 36   .mcp.json 31
README.md 30    agents 12   scripts 9   commands 6   mcp 4
plugin.lock.json 3   hooks.json 1
```

**`.mcp.json`** — 31 files, always `{ "mcpServers": { ... } }`. Server keys by
frequency: `type` 26, `url` 26, `command` 5, `args` 5, `cwd` 4, `oauth` 8,
`oauth_resource` 4, `scopes` 3, `env` 1. Two transports:

```json
{ "mcpServers": { "airtable": { "type": "http", "url": "https://mcp.airtable.com/mcp",
                    "oauth": { "client_id": "<AIRTABLE_PUBLIC_CLIENT_ID>" } } } }
```

**`.app.json`** — 36 files, always `{ "apps": { "<local key>": { "id": "asdk_app_…" } } }`.
The `id` is a vendor-side application identity, not a command.

**`plugin.lock.json`** — 3 files. A pack-time provenance record:

```json
{ "lockVersion": 1, "pluginId": "com.openai.notion", "pluginVersion": "0.1.0",
  "generatedBy": "codex plugin pack (draft)",
  "skills": [ { "id": "notion-knowledge-capture",
                 "vendoredPath": "skills/notion-knowledge-capture",
                 "source": { "type": "github", "repo": "openai/skills",
                             "path": "skills/.curated/notion-knowledge-capture",
                             "ref": "c207989…" },
                 "integrity": "sha256-<fill-during-pack>" } ] }
```

**`hooks.json`** — 1 file (Figma), a Claude-Code-shaped hook list:

```json
{ "hooks": { "PostToolUse": [ { "matcher": "Write|Edit",
    "hooks": [ { "type": "command", "command": "./scripts/post_write_figma_parity_check.sh" } ] } ] } }
```

**`commands/`** — 6 directories, 24 files. Two shapes ship: 12 plain `*.md` and
12 `*.md.tmpl` twins (Vercel, Zoom, Figma, Expo), plus `_`-prefixed shared
fragments (`_conventions.md` in both Vercel and Zoom). Frontmatter is
`description` only:

```markdown
---
description: Deploy the current project to Vercel. Pass "prod" or "production" as argument to deploy to production. Default is preview deployment.
---
```

and the body interpolates `$ARGUMENTS`.

**`agents/`** — 12 plugin-level directories, 514 directories including
skill-local ones. A plugin-level `agents/*.md` is a subagent definition with
`name` + `description` frontmatter; `agents/openai.yaml` is an interface
override:

```yaml
interface:
  display_name: "Figma"
  short_description: "Use Figma for design-to-code work"
  icon_small: "./assets/figma-small.svg"
  icon_large: "./assets/figma.png"
  default_prompt: "Use Figma to inspect the target design and translate it into implementable UI decisions."
```

**`skills/`** — 535 `SKILL.md` files. Frontmatter keys: `name` 535,
`description` 535, `metadata` 103, `version` 33, `license` 33,
`compatibility` 32, `disable-model-invocation` 15, `allowed-tools` 15.

---
## 3. What was missing, and what the substrate now reaches

### 3.1 Before

| Standard | Reachable | Unreachable |
|---|---|---|
| DSH | `tools/*.json`, `commands/*.md`, `skills/*/SKILL.md`, `agents/*.md`, `providers/*.json` (all BOS-owned; **no shipped DSH package uses any of them**) | `apply(ctx, config)` — the entire payload of every real package |
| Codex | skills, commands, prompts, interface metadata (partly) | MCP servers, app connectors |

The measured consequence, from the two packages actually installed here:

```
Codex  remotion/0722921d      -> 12 skills, 0 refusals
DSH    @opencode2dsh/dsh-plugin -> 1 panel, 0 providers, 3 refusals
```

The DSH package exists to supply free models and supplied none, because its
whole value is `ctx.llm.registerAdapter([PROVIDER_ID], adapter)`.

### 3.2 The choice

Three options were evaluated.

**(a) Node host bridge — CHOSEN.** A subprocess runs the plugin's entry against a
shim of the DSH `ctx` surface and streams registrations back over line-delimited
JSON. Node is already present (this project uses npm/pnpm; the DeepSeek Harness
app ships its own runtime at `Contents/Resources/runtime/bin/node`), so the
bridge adds **no Rust dependency at all** — the whole mechanism is `std::process`
plus `serde_json`, both already in the agent crate.

**(b) Embedded JS engine — REJECTED.** `Cargo.lock` was checked first, as
instructed. There is no `boa`, `quickjs`, `rquickjs`, `deno`, `v8`, `jsc`,
`swc` or `oxc` anywhere in the 13,080-line lock file. Adding `deno_core` or
`rquickjs` would be a new, heavy, platform-sensitive dependency (V8 alone is a
multi-hundred-megabyte build) bought solely to run plugin scripts, and it would
still not give a real sandbox. The project already depends on a Node toolchain
for its own builds; reusing it is cheaper and honest.

**(c) Declarative-only upgrade — KEPT AS THE FALLBACK, not the primary.** Static
analysis of `index.js` can extract a literal `ctx.tools.register({name, ...})`,
but it cannot evaluate `config`, a computed name, a schema built by a helper, or
a catalog fetched at load time. It stays in place for the case it is actually
good for: packages with no entry, and untrusted packages, which are read this
way today.

### 3.3 What now runs

`crates/agent/src/extensions/bridge/dsh-host-shim.js` (24 KB, embedded with
`include_str!` and written to a scratch directory per load) implements:

| Seam | Behaviour |
|---|---|
| `ctx.tools.register` / `registerTool` | Real. Registers a callable tool; `execute(args)` is invoked over the protocol. |
| `ctx.commands.register` | Real. Name/description/input validated as the reference does. |
| `ctx.skills.register` | Recorded, then refused with a named reason (see gaps). |
| `ctx.on` / `once` / `off` / `emit` / `cascade` | Real for the five event names that map onto a BOS hook stage; every other event is recorded and refused by name. |
| `ctx.effect` | Real: runs the callback, honours a returned disposer. |
| `ctx.inject` | Runs the callback for the services present; refuses each name it does not provide. |
| `ctx.get` | Answers for `tools`/`commands`/`skills`/`logger`; refuses anything else by name. |
| `ctx.logger` | Forwards all levels. |
| `ctx.config` | The config object the manifest/patch supplies. |
| `ctx.llm`, `ctx.fs`, `ctx.subprocess`, `ctx.settings`, `ctx.credentials`, `ctx.slots`, `ctx.loader`, `ctx.fiber`, `ctx.reflect`, `ctx.ptcRuntime`, `ctx.session(s)`, `ctx.agents`, `ctx.server`, `ctx.provide` | **Refusing seams.** Reading one THROWS a named `DshBridgeUnsupportedService` error and emits a `refuse` frame. |

A refusing seam is the deliberate design. `@opencode2dsh/dsh-plugin` against this
bridge, measured:

```
LOAD RESULT: ok=false
error: dsh-bridge: ctx.llm is the DeepSeek Harness model service; this bridge
       publishes model providers declaratively (providers/*.json) instead of
       proxying a live LLM adapter
REGISTRATIONS: []
REFUSALS: ["service/llm"]
```

The plugin fails **loudly, at load, with a sentence naming the seam** — instead
of loading, silently contributing nothing, and leaving a row that claims
`models` while no model can be selected. `ctx.llm` is not stubbed because a stub
would be a lie.

### 3.4 The mapping table: contribution to substrate seam

One table, both paths. A bridged registration and a declarative file land in
the SAME `Contribution` variant, so the registry, the agent install, and the
refusal list have one implementation and cannot drift.

| Source | Contribution | Installed by |
|---|---|---|
| bridged `ctx.tools.register` | `Contribution::Tool(Arc<BridgedTool>)` | `apply_to_agent` → `Agent::try_add_tool` |
| `tools/*.json` | `Contribution::Tool(Arc<DeclarativeTool>)` | same |
| `commands/*.md` | `Contribution::Tool(Arc<PromptTool>)` | same |
| bridged `ctx.commands.register` | `Contribution::Command` | `install_host_items` → `PluginRegistry::register_command` |
| `commands/*.md` | `Contribution::Command` + `Contribution::Prompt` | same + `registry.prompts()` |
| `skills/*/SKILL.md` | `Contribution::Skill` | `apply_to_agent` → `Agent::register_skill_manifest` |
| bridged `ctx.skills.register` | *refused by name* (no in-memory carrier) | — |
| bridged `ctx.on("llm/request"\|"llm/response"\|"llm/stream-token"\|"tool/call"\|"tool/result")` | `BridgeKind::Hook` → *refused by name* (the BOS hook seam takes an in-process `AgentPlugin`) | — |
| `agents/*.md` | `Contribution::Prompt` | `registry.prompts()` |
| `providers/*.json` | `Contribution::Provider` | `registry.providers()` |
| `dsh.client` | `Contribution::Panel(UiPanel)` | `install_host_items` → `PluginRegistry::register_panel` |

### 3.5 What each standard can now do

Two questions, two different answers, and conflating them is how a substrate ends
up advertising a capability it does not deliver:

- **Bridge-capable**: could `crates/agent/src/extensions/bridge.rs` do this if a
  host called it?
- **Wired**: does the host that ships here (bsh) actually reach it on the turn
  path? A bridge-capable feature with no caller is not a feature.

**DeepSeek Harness**

| Capability | Untrusted | Trusted, bridge-capable | Trusted, WIRED in bsh |
|---|---|---|---|
| Manifest, locale, icon, bundle patch read | yes | yes | yes |
| `dsh.client` → panel row | yes | yes | yes |
| Declarative `tools/`, `commands/`, `skills/`, `agents/`, `providers/` | yes | yes | yes |
| `apply(ctx, config)` executed | **no — named refusal, no process** | yes | **yes** |
| Bridged tool CALLABLE by the agent | no | yes | **yes** — asserted by calling it inside a real `build_agent` |
| Bridged command registered | no | yes | yes (host `PluginRegistry`) |
| Bridged skill registered | no | **no — refused by name** | no |
| Bridged event listener dispatched | no | **no — refused by name** | no |
| `ctx.llm` provider adapter | no | **no — refused by name** | no |
| Client module (`ctx.slots`) | no | **no — refused by name** | no |
| Bundle patch rows mounted | no | **no — refused by name** | no |

The gap between the last two columns is the point of the table. Until this round
the bridge existed and nothing in bsh called it: `caps.rs` loaded every package
with `ExtensionRegistry::load`, which hardcodes `Trust::Untrusted`, so a trusted
package's entry was refused on the turn path while `installed.rs` happily showed
it as trusted. The bridge ran only in tests.

**How it is wired.** `build_agent_for_session_inner` now reads each record's own
verdict (`entry.is_trusted()`, which requires approval AND an unchanged path) and
calls `load_with_trust`. A load that produced a session hands it to a process-wide
`BridgePool` keyed by `(id, root)`:

- a bridged tool is only callable while its process is alive, and a build re-reads
  the package on every turn, so the session cannot live in the build;
- the pool is REUSED, so turn two does not spawn a second child;
- every build reconciles it against the set of packages that build actually asked
  for, so disabling, uninstalling, distrusting or repointing a package stops its
  child instead of leaving an invisible process behind;
- `run()` calls `caps::stop_bridge_pool()` when the window closes, so nothing
  outlives the app.

**Codex**

| Capability | Before | Now |
|---|---|---|
| `skills/` | yes | yes |
| `commands/*.md` | yes | yes |
| `commands/*.md.tmpl` | **silently dropped** | **read** (same command name) |
| `commands/_fragment.md` | **registered as a command** | **skipped** (a private helper) |
| `agents/*.md` (frontmatter name) | yes | yes |
| `interface.*` metadata | displayName, shortDescription, logo/en composerIcon only | **all 17 fields captured** into `CodexInterface` |
| `mcpServers` / `.mcp.json` | refused, reason did not name the reachable path | refused, reason names it: a host with an MCP pool starts the SAME declaration, and only for a TRUSTED package |
| `apps` / `.app.json` | refused, reason was generic | refused, reason explains an app is an OAuth grant to a hosted service no substrate can supply |
| `hooks.json` | not read | not read (see gaps) |
| `plugin.lock.json` | not read | not read (see gaps) |

---

## 4. The trust and sandbox model

### 4.1 What protects the user

**A Node subprocess is not a security boundary.** The child runs with the
calling user's privileges. It can read and write their files, open network
connections, and spawn its own children. The refusing seams stop a plugin from
reaching the *host's* services through `ctx`, but a plugin that writes
`require("node:fs")` directly bypasses every one of them.

What actually protects the user is four things, in order of importance:

1. **The trust gate.** Only a package the reader has explicitly trusted is
   executed. `Trust` is a parameter of the load path
   (`ExtensionRegistry::load_with_trust`), and an untrusted package's entry is
   refused with a named `Refusal` while **no process is created at all**. The
   test `an_untrusted_dsh_entry_is_refused_and_starts_no_process` proves this
   with a fixture that writes a marker file on the first line of its module
   body: the marker is absent, and `package.session` is `None`.
2. **Path confinement.** The entry must canonicalise inside the canonical
   package root. A manifest cannot point the loader at `/bin/sh`. (This check
   compares canonical with canonical: on macOS `/var` symlinks to
   `/private/var`, and a mixed comparison rejects every scratch directory.)
3. **Bounded resources.** Output is capped (`MAX_FRAME_BYTES` 4 MiB per frame,
   `MAX_OUTPUT_BYTES` 4 KiB of stderr kept); `load` has a 20 s deadline and
   `invoke` a 30 s one; the child is killed on every error path and on `Drop`.
4. **No host secret on the wire.** The protocol carries contribution metadata
   and tool arguments only. The host never sends a credential, and no
   `ctx.credentials` seam exists to read one.

The host's own stored decision is what bsh uses: `InstalledExtension::is_trusted`
is true only when the reader approved **and** the approved path still equals the
installed path, so a reinstalled or moved package is untrusted again. The parse
cache keys on trust as well as on file mtime, so trusting a package re-reads it
instead of serving the untrusted parse.

### 4.2 What the sandbox does NOT contain

- Filesystem access. A trusted plugin can read `~/.ssh`.
- Network access. A trusted plugin can exfiltrate anything it can read.
- Process spawning. A trusted plugin can start its own children.
- CPU or memory. There is a wall-clock deadline, not a resource limit.

A reader who wants a real boundary needs an OS sandbox — a container, a
`sandbox-exec` profile, or a separate uid. This crate does not build one, and
saying so is the point: an implied boundary is worse than a stated absence.

### 4.3 Lifecycle

- **Load.** `BridgeSession::start` spawns, handshakes the protocol revision,
  sends `load`, and collects frames until the matching `result`. Every
  registration it announces during that window is collected; a failure inside
  the window discards all of them, so the load is all-or-nothing.
- **Call.** `BridgedTool::run` takes the session mutex, sends one `invoke`, and
  reads until the matching `result`. Two concurrent calls serialise rather than
  interleaving frames on the shared pipe.
- **Pool.** A load's session goes to `BridgePool`, keyed by `(id, root)` and
  reused across builds, because a bridged tool is only callable while its process
  is alive and a build re-reads the package every turn.
- **Reconcile.** Every build calls `BridgePool::retain` with the packages it
  actually asked for, so disabling, uninstalling, distrusting or repointing a
  package stops its child instead of leaving an invisible process running.
- **Unload.** `BridgeSession::shutdown` sends `shutdown`, waits briefly, then
  kills and reaps. `Drop` kills unconditionally, so an abandoned session cannot
  leak a child. `BridgeRegistry::unload`/`unload_all` and `BridgePool::stop` are
  the explicit paths, and `caps::stop_bridge_pool()` runs when the window closes.
- **Nothing runs at import time.** The shim is a file; the entry is only
  imported inside a `load` frame, and only after the caller supplied
  `Trust::Trusted`.

---
## 5. Tests, and what each proves

`crates/agent/tests/extensions.rs` — 34 tests (33 integration + 4 unit tests in
`bridge.rs`). The bridge ones **skip on a
machine with no Node** (that configuration is supported and separately asserted);
everything else always runs.

| Test | What it proves |
|---|---|
| `a_trusted_dsh_entry_registers_a_tool_that_the_agent_can_call` | The bridged tool is **CALLED**, not merely listed: `tool.run({"value":"hi"})` returns `{echoed:"hi", plugin:"fixture"}` through the agent's own `ToolRegistry`. The declared JSON Schema survives the round trip. A second call still works after the caller's session handle drops; a call after the session is killed FAILS rather than hanging. |
| `an_untrusted_dsh_entry_is_refused_and_starts_no_process` | The fixture writes a marker on its module's first line. Untrusted: the marker is **absent**, `package.session` is `None`, and the tool name is absent from the contributions. Three independent witnesses of "nothing ran". |
| `the_same_entry_runs_once_the_reader_trusts_it` | The discriminating half: same fixture, `Trust::Trusted`, the marker **exists** and the tool registers. Without this, "the marker is absent" would pass for a fixture that never worked. |
| `a_trusted_entry_that_throws_leaves_a_named_refusal_and_no_partial_load` | The fixture registers a tool and THEN throws. `dsh_before_throw` is absent, `package.session` is `None`, and the refusal quotes the plugin's own message and ends "Nothing it would have registered was loaded". |
| `a_hanging_entry_times_out_and_leaves_no_child_process` | A plugin that returns a never-settling Promise is killed at a 1.5 s deadline; the load returns in under 10 s with `"did not answer"` + `"killed"`. |
| `a_bridged_session_reports_a_pid_and_reaps_it_on_unload` | `BridgeRegistry::load` yields a live pid (`kill(pid, 0)` succeeds), `unload` reaps it (`kill(pid, 0)` fails afterwards). No leak. |
| `an_unimplemented_host_service_is_refused_by_name_and_loads_nothing` | `ctx.llm.registerAdapter` fails the load with a sentence naming `ctx.llm` and what this host offers instead. The shape of `@opencode2dsh/dsh-plugin`. |
| `an_entry_without_an_interpreter_is_refused_by_name` | No Node → the refusal says so and says the script did not run. |
| `a_codex_interface_block_is_read_into_the_substrate` | All 17 `interface` fields are captured, `capabilities` and `defaultPrompt` as lists. |
| `a_codex_agent_prompt_carries_its_frontmatter_name` | `agents/*.md` → a prompt named from its frontmatter, not its file stem. |
| `a_codex_mcp_refusal_names_the_path_that_can_start_it` | The MCP refusal names the trust gate as the way to start it, and leaks no secret. |
| `a_codex_app_connector_is_refused_with_its_own_reason` | An app does not borrow the MCP sentence. |
| `the_codex_path_loads_declarative_parts_and_refuses_the_rest` | Tools, commands, skills and prompts load while MCP + app stay on the refusal list. |
| `a_packed_codex_command_template_is_read_and_a_fragment_is_not_a_command` | `deploy.md` + `deploy.md.tmpl` reduce to ONE `deploy` command; `_conventions.md` is not a command. |
| `a_packed_codex_agent_file_is_a_prompt_not_a_command` | `agents/*.md` is a prompt named from its frontmatter, never a host command. |

And the tests that prove the bridge is **WIRED INTO THE HOST**, not merely capable
(`crates/bsh/src/caps.rs`, `crates/bsh/src/installed.rs`):

| Test | What it proves |
|---|---|
| `a_trusted_dsh_entry_reaches_the_turn_through_the_bridge` | A TRUSTED package's entry runs during a real `build_agent`, and `dsh_bridge_echo` is CALLED from that agent returning `{echoed:"turn", plugin:"fixture"}` — through the same code path the shell uses. The config came from the package's bundle patch. A second call after the build returned still works, so the process outlives the build. A build that does not list the package offers no such tool. |
| `an_untrusted_dsh_entry_contributes_no_tool_to_the_turn` | The same build path with an untrusted record: no tool, and no marker file, so no process. Discrimination-checked — forcing `Trust::Trusted` makes THIS test fail while the trusted one still passes. |
| `the_bridge_pool_stops_a_package_the_build_no_longer_asks_for` | The pool owns the process, not the build: dropping the built agent leaves the child alive, and a later build that does not ask for the package reaps it (`kill(pid, 0)` succeeds then fails). |
| `trusting_a_package_runs_its_entry_and_the_cache_follows` | The same record, through the same parse cache: untrusted leaves no marker, then trusting it runs the entry. This is what proves trust is part of the cache key. |

Unit tests in `bridge.rs` cover the entry-containment check (including a
symlink escape), the missing file, the protocol pin, and the byte-budget trimmer
never splitting a multi-byte character.

---

## 6. Remaining gaps, stated honestly

These are the things this substrate still cannot do, with the reason and what
it would take.

1. **`ctx.llm.registerAdapter` is refused.** This is the single highest-value
   gap: it is what `@opencode2dsh/dsh-plugin` exists for. An adapter is a live
   JavaScript object with its own streaming and catalog behaviour, so it cannot
   cross a JSON boundary as data. Bridging it needs either a host-side
   `LlmClient` that forwards each request *into* the subprocess and streams the
   response back, or a declarative carrier that can express a live catalog
   (`providers/*.json` has a fixed `models` array, which cannot). Today the
   seam refuses and the user is told exactly why.
2. **`ctx.skills.register` is refused.** The registry has no carrier for an
   in-memory skill body; `Contribution::Skill` points at a `SKILL.md` path. The
   content IS received over the protocol. The clean fix is a
   `SkillContribution` variant that carries a body, which is a change to the
   skills seam rather than to the bridge.
3. **Bridged event listeners are recorded, not dispatched.** `ctx.on`
   registrations are captured, but the BOS hook seam takes an in-process
   `AgentPlugin` (a Rust trait), and a bridged listener is a JavaScript
   closure. Dispatching it means a synchronous round trip into the subprocess
   on the hot path of every token, which is a performance decision this round
   did not make.
4. **`ctx.slots.*` is refused.** The DSH client plane is browser JavaScript for
   a web renderer; bsh draws native GPUI. There is no renderer to inject into,
   so this is a platform fact rather than a missing implementation.
5. **`dsh.bundle.patch` rows are parsed but not mounted.** Mounting them needs
   the Cordis loader. The row ids and their `name` fields are kept in
   `ExtensionDetails::bundle_rows`, and an MCP-client row is separately
   refused as `McpServer`.
6. **Codex `hooks.json` is not read.** One plugin ships one (Figma, a
   `PostToolUse` command hook). It is the Claude-Code hook shape, not
   `AgentPlugin`, so reading it is a new seam rather than a field.
7. **Codex `plugin.lock.json` is not read.** Three plugins ship it and it is a
   pack-time provenance record (source repo, ref, integrity hash) rather than a
   contribution. It would serve an integrity check, which nothing performs yet.
8. **`ctx.fs`, `ctx.subprocess`, `ctx.settings`, `ctx.credentials` are refused.**
   These are host services a plugin could reasonably want. Each is a deliberate
   decision (no host paths, no child processes, no settings document, no
   credential) rather than an oversight, and each is named in a refusal frame.
9. **No OS-level sandbox.** See §4.2.
10. **A fixture that writes evidence can contaminate its own test.** The untrusted
    fixture proves "nothing ran" by NOT writing a marker, so a marker left in the
    fixture directory by an earlier run inverts every assertion about it. This bit
    this round: three tests failed at once and the cause was a stale marker in
    `crates/agent/tests/fixtures/extensions/dsh-untrusted/`, not a code defect.
    Both suites now copy to a scratch root AND assert the source is clean first,
    so the failure mode is a named message rather than a silent inversion.

---

## Appendix: reproduction

Everything in §1 and §2 is a file read, not a network call.

**DSH asar.** Parse the header with `raw_decode`, walk the index accepting both
a list and a dict for `files`, and read each entry at `8 + pickle + offset`.
`docs/ui-mirror/reference/extract-asar.py` already does the extraction for the
bundle corpus; the package survey above used a variant that keeps the index in
memory so `ctx.<member>` counts can be taken per package without unpacking 12 MB.

**The `ctx` member counts.** For each of the 301 packages with a resolvable
`main`, read that file and match `\bctx\.([A-Za-z_$][A-Za-z0-9_$]*)`. Counting
the number of PACKAGES that use a member, not call sites, is what makes the list
usable: `ctx.effect` has 155 call sites across 93 packages, but one package
repeating a member 40 times is still one dependency.

**Codex.** `~/.codex/.tmp/plugins/plugins/` (read-only) is the unpacked plugin
tree: 64 `.codex-plugin/plugin.json`, 31 `.mcp.json`, 36 `.app.json`, 3
`plugin.lock.json`, 1 `hooks.json`, 535 `SKILL.md`. Count field frequency by
parsing every manifest and tabulating keys.

**The bridge end to end.** `cargo test -p agent --test extensions`. To drive the
shim by hand, spawn `node crates/agent/src/extensions/bridge/dsh-host-shim.js`
with the package root as cwd and write
`{"id":1,"method":"load","params":{"entry":"index.js","root":"<abs>","config":{}}}`
followed by a newline; registrations arrive as `register` frames and the load
finishes with the matching `result`.
