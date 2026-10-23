---
module: bsh
tags: [extensions, codex, dsh, providers, gap-analysis]
problem_type: analysis
---

# Extension adaptation to Codex and DSH: what is missing, and why it does not work yet

Reviewed 2026-10-23 against the two packages actually installed on this machine, not
against the fixtures. The fixtures are self-consistent with the shim, so they pass
while the real packages do not work - that is the core of the problem.

## The evidence

The two real packages on this machine:

| Package | Standard | Loaded |
|---|---|---|
| `~/.codex/plugins/cache/openai-api-curated/remotion/0722921d` | Codex | 12 skills, 0 refusals |
| `~/.bos/bsh/extensions/opencode2dsh_dsh_plugin/node_modules/@opencode2dsh/dsh-plugin` | DSH | 1 panel, **0 providers**, 3 refusals |

Probe output for the DSH package (real package, real loader):

```
declared capabilities: [Hooks, Panels, Models]
REAL contributions -> providers=0 tools=0 hooks=0 panels=1
models capability declared=true but provider contributions=0
hooks capability declared=true but hook contributions=0
```

So the DSH package that the user installed to get **OpenCode Zen free models**
delivers **no models**. Its whole value - the LLM adapter - lives in
`lib/index.js` behind `ctx.llm.registerAdapter([PROVIDER_ID], adapter)`, and the
shim refuses to execute JavaScript.

## Why it does not work

### 1. The DSH host entry is where every real DSH package puts its value

The shipped DSH manifest declares no tools, commands or skills. The reference's
own manifest format has exactly three public fields: `dsh.manifestVersion`,
`dsh.bundle.patch`, `dsh.client`. Everything a package delivers is registered
imperatively from its host entry: `ctx.tools.register`, `ctx.commands.register`,
`ctx.skills.register`, `ctx.on`, `ctx.llm.registerAdapter`.

The shim reads those calls as **text markers** to report a capability, then
refuses the file. The marker scan is a grep, not an execution: `scan_markers`
does `source.contains(needle)`. So a package gets `Capability::Models` because
its source mentions `ctx.llm.registerAdapter`, and then contributes zero
providers.

**The capability list is therefore a claim about the package's source, not a fact
about what the host loaded.** The installed-plugin row shows "models" under
Capabilities while no model can be selected. That is exactly the "control that
only looks wired" the crate rules forbid.

### 2. The declarative carrier the shim reads is invented, and no real package ships it

`tools/*.json`, `commands/*.md`, `agents/*.md` and `providers/*.json` are a
BOS-owned surface (the module doc says so: "the same explicitly BOS-owned
declarative surface"). The opencode2dsh package ships **none** of them - no
`providers/` directory, no `tools/` directory. It ships `lib/index.js`,
`lib/client.js`, `cordis.patch.yml` and `package.json`.

The fixtures (`dsh-echo`, `dsh-provider`) were built to the invented surface,
so the integration suite is green against a shape no upstream package has.

### 3. The provider path can never work for this package even in principle

The plugin's model list is not static. `lib/catalog-*.js` fetches it live from
`https://opencode.ai/inference` and rotates it (the `-free` model ids are
discovered at runtime, e.g. `deepseek-v4.1-flash-free`, `exo-free`,
`muse-spark-1.3-contributor-free`). A `providers/*.json` file with a fixed
`models` array cannot express it, and the adapter itself is a JS object with a
`pi-ai` runtime behind it.

### 4. The bundle patch refusal is correct but incomplete

`dsh.bundle.patch` is parsed for row ids and then refused with
`RefusedFeature::BundleMount`. Correct - mounting a Cordis row needs the DSH
loader. But the row ids are parsed out of the patch and then **dropped**: no
consumer of `details.bundle_rows` exists outside the test that asserts it. The
MCP client row the patch declares is exactly what would give the package its
tools, and it is refused at the patch level AND at the `mcpServers` level, so
nothing reads it twice.

### 5. The Codex side works, and its gaps are different

Remotion loads clean: 12 skills, no refusals, nested reference directories
correctly ignored (they carry `REFERENCE.md`, not `SKILL.md`, so the
one-level read is right).

The Codex gaps against the real `plugin.json` are metadata, not function:

- `interface.defaultPrompt` - the reference's suggested prompts. Not captured.
- `interface.brandColor` / `brandColorDark` - the accent the row draws with.
  Not captured.
- `interface.category`, `developerName`, `supportURL`, `websiteURL`,
  `privacyPolicyURL`, `termsOfServiceURL`, `longDescription` - read by the
  reference's plugin manager, absent from `ExtensionDetails`.
- `author` - the real manifest has it as an **object**
  (`{name, email, url}`), not a string. The shim's doc comment lists it as a
  string field and never reads it at all.
- `commands/*.md` - read, but the resulting `Contribution::Command` goes into
  `PluginRegistry`, and **no host surface renders that registry's commands**:
  bsh's composer palette is a hardcoded `entries(streaming)` list, and
  `registry.commands()` has no caller outside tests. The same `PromptTool` is
  registered as an agent tool, so the command *is* callable by the model - but a
  reader typing `/` never sees it.

### 6. Two contribution kinds are inert everywhere

- `Contribution::Panel` - `dsh.client` produces a `UiPanel { id, title }` and
  bsh never calls `registry.panels()`. The panel is a row with no body.
- `Contribution::Prompt` - no consumer outside the registry.

## The gap list, by what it costs the reader

| # | Gap | Effect | Fix |
|---|---|---|---|
| 1 | Capability claims come from a source-text grep, not from what loaded | The row says "models" and delivers none | Derive capabilities from contributions; keep the source scan in a separate `declared` field |
| 2 | No JS runtime for `apply(ctx, config)` | Every real DSH package delivers nothing | Either embed a JS runtime, or ship a declarative DSH manifest extension upstream |
| 3 | `providers/*.json` cannot express a live catalog | The one provider case the user has is unreachable | Add a "provider with a live model list" carrier (base URL + discovery endpoint) |
| 4 | `bundle_rows` parsed then dropped | The patch's MCP row is lost twice over | Route the rows to the MCP pool, or stop parsing what cannot be mounted |
| 5 | `Contribution::Command` / `Panel` / `Prompt` have no host surface | Commands and panels exist only in a registry | Wire the command palette and a panel host to the registry |
| 6 | Codex `interface.*` metadata beyond displayName/shortDescription/logo is unread | The plugin row cannot draw the reference's card | Capture the fields into `ExtensionDetails` |
| 7 | `author` typed as a string in the docs, is an object in reality | A metadata field is wrong before it is missing | Type it as the object the reference ships |
| 8 | No MCP launch from a DSH package's bundle patch | A DSH package can never start a server | Same as #4 |

## What is NOT the problem

- The Codex shim is correct and the real Codex package loads.
- The MCP refusal is not a bug: bsh owns the MCP pool and starts the declared
  servers itself (`mcp::declared_servers`), so refusing them in the shim and
  filtering the refusal at the UI is the right split.
- The `dsh.client` platform is `web`; a native desktop host has no client
  runtime, so the client entry was always going to be refused.

## The honest summary

The shim is well built for **declarative** packages, and it handles the real
Codex package correctly. It does not work for the real DSH package because the
DSH standard is **imperative**: the manifest declares almost nothing and the
host entry does the work, and this build has no JavaScript runtime.

The two honest paths forward:

1. **Embed a JS runtime** (deno_core / boa / quickjs) and run `apply(ctx, config)`
   against a host-API shim that maps `ctx.tools/commands/skills/llm.on` onto the
   existing `Agent`, `PluginRegistry` and provider seams. This makes the
   ecosystem work as published, including the live-model catalog.
2. **Ship a declarative DSH manifest extension** (`dsh.declarative`) upstream,
   where a package can declare its provider, models and tools without script.
   That is a DSH-side change, not a BOS one.

Until one of those lands, the correct behaviour is to say so plainly in the UI:
the package row should show what the package **asked for** separately from what
the host **loaded**, and a package whose entire payload was refused should read
as unsupported rather than partial.
