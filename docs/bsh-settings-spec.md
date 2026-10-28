# BSH settings — three levels, one source of truth

The configuration model for `crates/bsh`. This is a **reimplementation**: the
previous flat `Settings` / `Profile` / `ModelCatalog` / `Workspace` structs
are not a constraint and are not migrated. `~/.bos/conf/config.toml` is not
read at all.

## Principle

> **Only information that crosses workspaces — or has no workspace relation —
> is global. Everything else is per workspace.**

A second rule follows from it:

> **Definitions are global; the workspace records which one it uses.**

Providers, presets and extensions all take that shape.

## Layout

    ~/.bos/bsh/settings.toml              global — crosses workspaces
    <root>/.bsh/settings/settings.toml    per workspace — how the agent runs here
    <root>/.bsh/sessions/<id>.json        per session — transcript and its model

```
crates/bsh/src/settings.rs           the model + both loaders; declares the two below
crates/bsh/src/settings/global.rs    ~/.bos/bsh/settings.toml
crates/bsh/src/settings/workspace.rs  <root>/.bsh/settings/settings.toml
```

The two submodules are declared as `pub(crate) mod global;` and
`pub(crate) mod workspace;` *inside* `settings.rs`. There is deliberately no
`settings/mod.rs`: `settings.rs` already exists, and a second path for one
module does not compile.

## Global — `~/.bos/bsh/settings.toml`

```toml
# Root keys come FIRST. TOML files a bare key into the most recent table
# header, so anything written below [general] would land inside it. The parser
# refuses that outright rather than letting the value vanish.
schema_version = 1
skills_dir = "~/.bos/skills"   # cross-workspace default; a workspace may override
mcp_servers = []               # merged with the workspace's list
default_preset = "standard"    # the preset a session takes when it names none

[general]
theme_mode = "system"          # light | dark | system
lang = "en"
content_font_size = 14
content_width = 1050           # absent = the reference clamp
sidebar_width = 320            # absent = the reference default
transcript_view = "standard"   # compact | standard | detailed | verbose
link_opening = "sidebar"       # sidebar | new-tab
composer_enter = "queue"       # queue | steer
performance_usage = "detailed" # compact | detailed
developer_tools = true
sandbox_enabled = false        # absent = heel applies; false = shell and terminal unconfined
session_sort = ""
active_workspace = "<id>"      # registry-level pointer
active_session = "<id>"        # the session last on screen (window state)

[providers.<id>]               # <id> is concrete: nvidia, openrouter, openai.
base_url = "https://…"         # One node per provider — a duplicate id is a
api_key = "…"                  # parse error, not a second entry.

  [providers.<id>."<nickname>"]  # bare keys with dots MUST be quoted
  model = "<full API id>"        # REQUIRED. Key is a nickname; this is the payload.
  api_mode = "chat"              # optional; absent = the engine's own default
  reasoning_effort = "high"      # optional
  input_types = ["text", "image"] # optional; ABSENT MEANS PERMISSIVE

[presets.<id>]                # an agent preset: a plugin COMPOSITION
plugins = ["shell", "files"]   # the complete mounted set; [] mounts nothing
name = "..."                   # optional; a shipped preset publishes none
description = "..."            # optional
order = 0                      # optional roster order
system_prompt = "…"            # optional persona; unset keeps the workspace prompt
complete = false               # true: that persona IS the whole system prompt

[[extentions]]                 # the installed inventory
id = "shell"

[[workspaces]]                 # the registry; overrides are NOT inline
id = "…"
name = "bsh"
root = "/Users/…/bos"
created_at = 1792158733
updated_at = 1792158733
```

### The persona and its `complete` flag

`system_prompt` fills the persona SLOT by default: the harness identity opens
the prompt, the workflow guidance closes it, and per-tool rules are appended for
the tools the build actually registers. `complete = true` is the reference's
`minimal`-preset arm (its persona row's `complete: true`): the prompt is the
persona text ALONE, with no identity, workflow or tool guidance. It is off by
default, it applies only while the persona is non-empty (an empty persona under
`complete` falls back to the default composition), and a preset's own complete
persona outranks the workspace one. The runtime-context message is NOT
suppressed: unlike the reference's `includeRuntimeContext: false`, BSH's
snapshot is the model's one source of sandbox, permission and preset truth.

### Providers

- The provider node owns the connection (`base_url`, `api_key`); the leaf owns
  the model. This is what stops one gateway being re-declared per model.
- An `[[extentions]]` id the binary does not ship is shown **unavailable**.
  A builtin the binary ships but the list omits is shown **installable** —
  installing is writing a name, because the code is already compiled in.

### Web search

The local `web_search` tool the model calls is configured here, beside the
machine facts it shares the file with. Its credential is NOT here: the tool
reads the vault entry `web_search:api_key`, written from Settings > Web search.

```toml
[web_search]
base_url = "https://api.deepseek.com/v1/tools"  # the endpoint base, no operation
results_per_search = 10                          # 1..=50
method = "post"                                  # post (JSON body) | get (query string)
path = "search"                                  # appended to the base
query_key = "query"                              # request field carrying the query
count_key = "count"                              # request field carrying the budget; "" omits it
results_key = "results"                          # response array holding the hits
url_key = "url"                                  # hit field carrying the URL (required)
title_key = "title"                              # hit field carrying a title; "" omits
snippet_key = "snippet"                          # hit field carrying a snippet; "" omits
date_key = "date"                                # hit field carrying a date; "" omits

[web_search.body]                                # static extra fields merged into every request
need_content = false
```

Every field's default is the DeepSeek Search API's wire, so a file that names
none of them speaks exactly the shipped shape. Two other providers as spelled
sections:

```toml
# Tavily: POST https://api.tavily.com/search, Bearer key, no count field,
# snippets under `content`, dates under `published_date`.
[web_search]
base_url = "https://api.tavily.com"
method = "post"
count_key = ""
snippet_key = "content"
date_key = "published_date"
results_per_search = 10

[web_search.body]
search_depth = "advanced"

# SearXNG (self-hosted): GET with the query in the URL.
[web_search]
base_url = "http://127.0.0.1:8888"
method = "get"
query_key = "q"
count_key = ""
snippet_key = "content"
date_key = "publishedDate"
```

The search is LOCAL network access: the workspace egress allowlist and the
preset's own `egress` decide whether the tool may reach the endpoint, and the
read-only preset declines search for exactly that reason.

**Implementation note, found while writing the parser.** A model leaf sits
*directly* under its provider, so the model map is `#[serde(flatten)]`ed rather
than a named `models` table. Without the flatten,
`[providers.nvidia."glm-5.3-flash"]` is **silently ignored** and the provider
ends up with **zero** models - the file looks right and nothing resolves. The
flatten has a second, welcome effect: a misspelled provider field lands in the
map as a leaf, where it is refused for carrying no model id, so
`base_urll = "..."` fails loudly instead of vanishing.

### Presets: measured, not invented

Read from the reference's own packages inside `app.asar`
(`@deepseek-ai/dsh-agent-preset` and `.../dsh-agent-preset-registry`; the asar's JSON header
starts at byte 16 and the file data begins at `8 + u32[4]`)

- The declaration's fields are exactly **`id`, `plugins`, `name`,
  `description`, `order`**. `plugins` is required and is a child plugin
  entry list; the registry's `default` is the deployment default (`standard`)
  and its `selectedDefault` is the user's.
- There is **no model, no system prompt and no step cap**: "the selected preset's
  `plugins` own the model-visible tools and prompt sections". So a preset picks
  *tools, prompt sections and skills* - not the model. That is why the redesign
  **drops `Preset.model`** rather than inventing a syntax for it.
- "Declare several presets and **let sessions select one**", and `config.id` is
  "the preset identity **saved by sessions**" - which is why `active_preset`
  moved off the workspace and onto the session.
- Four ship: `standard`, `ptc`, `minimal`, `cordis`. Only the first two
  ship here: `ptc` mounts a host-plane PTC runtime this deployment composes
  nowhere, and `cordis` is a Cordis plugin composition with no counterpart.
  Shipping either would be a name with nothing behind it.
- A shipped preset **publishes no `name`**; the dictionaries carry its copy. A
  declaration that names itself owns its copy and is never translated.
- "A user override replaces the complete child list and does not automatically
  merge" - hence replacement, not merge.

## Workspace — `<root>/.bsh/settings/settings.toml`

```toml
schema_version = 1

[agent]
system_prompt = "…"
complete = false               # true: system_prompt IS the whole system prompt
temperature = 0.3              # ABSENT means inherit. 0.0 is a real value.
reasoning_effort = "high"      # absent inherits; the model node may declare one
permission = "workspace-write" # read-only | workspace-write | danger-full-access
bash_enabled = true
file_tools_enabled = true
parallel_tool_calls = false
memory_enabled = true
max_tokens = 4096              # absent leaves it to the engine
context_budget = 32768
shell_timeout_secs = 300
tool_output_cap = 16384
skills_dir = "…"               # absent inherits the global one
memory_path = "…"              # absent uses the shared default
mcp_servers = []               # merged with the global list; this one wins

[disabled]
tools = []
skills = []
extentions = []                # subtracts from the global installed inventory
```

The file is AGENT CONFIGURATION ONLY. It carries no model and no session
identity: a model is a per-session choice, and which session is on screen is
a window fact (several sessions in one workspace can run at once). Both used
to sit under `[general]` and are still accepted as deserialize-only legacy
input (`#[serde(rename = "...", skip_serializing)]`, the same shape as
`Provider::api_key`) so a file written by an older build parses: the model
pair is read once and given to the session that is open, and the remembered
session id is read once into the machine preference `active_session`. A
saved file carries none of the three lines.

## Session — `<root>/.bsh/sessions/<id>.json`

```json
{
  "id": "…", "title": "…", "created_at": 0, "updated_at": 0,
  "workspace": "<id>",
  "provider": "<id>", "model": "<nickname>",
  "agent_preset": "<id>",
  "messages": [], "plan": [], "goal": null,
  "pinned": false, "archived_at": null, "archived": []
}
```

`provider` and `model` are the session's EXPLICIT choice, written when the
composer picks a model in THIS chat; `agent_preset` is the same for the preset.
They are **null** when the session names none, and null does not mean "take the
level below" - there is no level below. A session with no choice has no model:
the chip shows *Select model* and the send gate refuses with
`composer.chooseModel` instead of sending to an endpoint nobody chose. A
half-made reference (a provider with no model) counts as null rather than being
sent on to fail as an unknown name. A new session is never given a copy of
another session's choice.

The session stores a **reference**, never `base_url` or `api_key`. A session
whose reference no longer resolves is **unresolvable**: it shows the model it
wanted and refuses to send until a model is picked. It never falls back
silently — sending to a gateway nobody chose is worse than asking.

**Which session is on screen** is a machine preference, not workspace state:
`global.general.active_session`. Several chats in one workspace may be
streaming at once, so selecting one only changes what the window shows (and
writes the global document). A launch accepts the remembered id only while it
still belongs to the active workspace; otherwise the newest visible chat there
is opened. An older workspace file's `active_session` is migrated into this
preference once, and never overwrites one the reader has since recorded.

## File rules

TOML files a bare key into the **most recent table header**, so a root key
written below `[general]` lands *inside* `[general]`. Every struct except
`Provider` therefore carries `#[serde(deny_unknown_fields)]`: a misplaced or
misspelled key is **refused with the key named**, instead of being ignored while
its value silently disappears. This was not hypothetical - a fixture that put
`mcp_servers` under `[general]` made an MCP server vanish, and the skills
test passed only because the framework default happens to share the same suffix.
`Provider` cannot carry the attribute (serde forbids it together with
`flatten`), so its own misspellings are caught by the missing-model-id rule
instead. Write root keys above the first `[table]`.

**An absent file is not an error.** A fresh install ships no global file and a
fresh workspace ships no workspace file, so `load` returns the defaults and
**reading never creates the file it read**. `save` creates its directory. Any
other read failure is reported, because then the file exists and a silent
fallback would hide the reason.

**A save goes where the settings were loaded from** — the location is a field, not
a recomputed path — and a test points it at its own scratch store. A test that
changed a preference used to write the user's real configuration, which is how a
shell ended up confined to a temporary directory a test had made.

## Resolution

**Model:** `session.provider/model` → **refuse and say so**. There is no
workspace and no global term: there is no default model. A legacy workspace pair
is migrated onto the open session at launch, once, which is an upgrade step and
not a resolution level.

**Preset:** `session.agent_preset` → `default_preset` → none. The preset is a
**session** fact, not a workspace one, because it decides the tool schemas and
prompt sections the model sees: it has to be reconstructible from the session
log alone. An id nothing defines **refuses the turn** instead of falling back.

**Inheritable** (`temperature`, `reasoning_effort`, `skills_dir`, `memory_path`):
absent in the workspace inherits. Every one is `Option<T>`, so absence is
genuinely distinguishable from a value. The old `0.0`-and-`""` sentinel scheme
is gone.

**MCP servers** are LISTS, merged: the global list is the machine's, the
workspace's is the project's; a workspace entry with the same name wins. An
entry with no `name` cannot be matched, so it is appended rather than dropped.
The global order survives and a collision is replaced in place.

An entry is `{name, command, args}` for a stdio server or `{name, url}` for a
streamable HTTP one, plus an optional `enabled` (default `true`) so a server
can be parked without deleting it. The name is the tool namespace, so it is
limited to `[A-Za-z0-9._-]`: the registry exposes a server's tool as
`<name>_<tool>`.

**Connections are a pool, not a document read.** `src/mcp.rs` keeps one live
connection per enabled server, reconciled at launch, after the MCP pane saves,
and when the active workspace changes; an unchanged server keeps its process,
and a reconfigured one is dropped (its child is killed on drop) and reopened.
`caps::build_agent` registers the pool's tools on every turn, so a server the
pane configured is a tool the model can actually call. A connection failure is
kept as an error the pane shows; it never blocks a turn.

**Implemented in `src/settings/resolve.rs`**, which returns *data* and never
wording, so the view owns the copy:

- `ModelChoice` is three-valued - `Found { reference, connection }`, `Unchosen`,
  `Missing { reference }`. `refusal()` turns the two bad states into `NoModel` /
  `UnknownModel` for the composer.
- `Resolution::refusal()` checks the preset **first** — a mount that cannot be
  built is a harder stop than a model that cannot be reached — and
  `Settings::refusal_for()` maps the three variants onto the composer's own
  keys (`composer.chooseModel`, `composer.modelUnavailable`,
  `composer.presetUnavailable`).
  The resolution layer returns *data*, never wording.
- The preset supplies exactly one thing: `plugins`, the complete mounted set.
  It carries no model, no system prompt and no step cap, so it cannot collide
  with the workspace's agent configuration at all.
- `reasoning_effort` resolves model-node first, workspace second: the node
  describes the model, the workspace says what it wants *here*.
- `skills_dir` is workspace then global then `~/.bos/skills`; `memory_path`
  is workspace then `~/.bos/bsh/memory.jsonl`.
- A declaration **replaces** a shipped preset of the same id outright; it does
  not merge. Merging would make a shipped plugin impossible to remove.

## Not config

The **Account pane** renders derived facts — the version comes from the compiled
crate, the paths from `dir()`, counts from disk. There is no `[Account]` table in
any file. Diagnostics live on screen, not in a config.

## Fresh install

Ships **nothing**: no providers, no models, no preset the user did not write.
The one record a launch derives for itself is the **workspace**: a shell that
carries no registry has no root to confine its tools to and no session it can
create, so `ensure_workspace` names the directory it was launched from. That is
a fact about the launch, not a seeded preference — `[providers]` and
`[presets]` stay empty until somebody fills them.

The workspace picker's empty state offers *add a provider*, so the cold start is
one screen. `<root>/.bsh/settings/settings.toml` is written when a workspace is
added — before that there is no agent, so there is nothing to configure.

A turn a fresh install cannot answer is **refused, not faked**:
`Settings::refusal_for()` folds the resolution and hands the view a key
(`composer.chooseModel` / `composer.modelUnavailable` /
`composer.presetUnavailable`), and `Shell::send` shows it as a notice instead of
opening a turn against a model nobody chose.

## Not carried over

| removed | why |
|---|---|
| `~/.bos/conf/config.toml` as a read path | two sources for one truth |
| `discover_profiles()`, `apply_model_defaults` call | the reads themselves |
| `require_approval`, `web_tools_enabled` | no consumer anywhere |
| `catalogs` | absorbed into the provider's model leaves |
| `presets/` directory | `[presets.*]` is the only form |
| `active_provider` / `model` / `base_url` empty-string sentinels | replaced by absence |
| `model`/`base_url`/`api_key` as flat globals | the provider node owns them |

## Rust sketch

```rust
struct GlobalSettings {
    schema_version: u32,
    general: General,
    skills_dir: Option<String>,
    mcp_servers: Vec<McpServer>,
    providers: BTreeMap<ProviderId, Provider>,
    presets: BTreeMap<PresetId, Preset>,
    extensions: BTreeMap<ExtensionId, Extension>,
    workspaces: Vec<WorkspaceRef>,
}

struct Provider { base_url: String, api_key: String,
                  models: BTreeMap<ModelNickname, ModelEntry> }

struct ModelEntry { model: String,
                    api_mode: Option<ApiMode>,
                    reasoning_effort: Option<String>,
                    input_types: Option<Vec<InputType>> }

struct WorkspaceSettings {
    schema_version: u32,
    general: WorkspaceGeneral,          // LEGACY-ONLY: provider/model/active_session input
    agent: AgentSection,                // prompt, temperature: Option, permission, budgets
    disabled: Disabled,                 // tools, skills, extensions
}

struct Session { /* … */ model: Option<ModelRef> }   // reference only
```

## Invariants

1. A model leaf always carries a non-empty `model`. The nickname is display only.
2. No `api_key` is ever written outside the global provider tree.
3. Absent `input_types` is permissive — turning this strict would silently start
   refusing images for every existing model.
4. A duplicate provider id, preset id or extension id is a load error.
5. Each file carries its own `schema_version`.
