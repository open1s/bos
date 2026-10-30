---
module: bsh
tags: [extensions, standard, manifest, capabilities, runtimes, ui, events, security]
problem_type: reference
---

# BSH Extension Standard 1.0

**Status:** normative for BSH 3.x, extension API 1.0
**Implemented by:** crates/bsh/src/extension
**Design background:** docs/BSH_Extension_Framework_Design.md

This document is the contract a package and a host implement. Where the design
document describes intent, this one describes the accepted bytes, the accepted
behaviour and the refusals. The implementation in crates/bsh is the second
authority; a disagreement between this document and the code is a bug in one of
them and must be fixed in the same change.

## 1. What an extension is

An extension is a directory with an extension.toml manifest. It may contribute:

| Contribution | Surface | Runtime |
|---|---|---|
| Tool | Agent tool registry | built-in, declarative, process |
| Command | Host plugin registry (composer and command palette) | declarative, process, built-in |
| Panel | Host plugin registry plus a declarative UI document | declarative, process, built-in |
| Prompt | Agent system prompt section | declarative, process, built-in |
| Context | Agent system prompt section | declarative, process, built-in |
| Workflow | Recorded; no executor in this build | declarative |
| Theme | Recorded; no applier in this build | declarative |

Registering a contribution and being allowed to perform its operation are two
different decisions. See section 6.

## 2. Identity and versions

### 2.1 Extension id

An extension id is a reverse-domain identifier:

- at least two dot-separated segments;
- 1 to 128 bytes total, each segment 1 to 63 bytes;
- lowercase ASCII letters, digits, hyphen and underscore;
- each segment starts with a letter or digit and ends with a letter or digit.

Examples: com.example.git-assistant, com.bos.bsh.extensions.

The id is also the on-disk directory name and a TOML key, so the grammar is
narrower than DNS on purpose.

### 2.2 Contribution id

A contribution id is dotted and lowercase, 1 to 96 bytes, each segment 1 to 63
bytes of letters, digits, hyphen or underscore. It is unique within one
extension across every contribution kind. A tool named git.status and a command
named git.status in one package is a validation error.

### 2.3 Model-facing tool name

Tool names are a flat namespace in every LLM API. The host qualifies a tool id
with the extension's last id segment:

~~~text
com.example.git-assistant + git.status -> git-assistant__git_status
~~~

Hyphens are preserved because provider function-name grammars accept them. The
result is capped at 64 characters. A collision across two packages is reported
and the later tool is not registered; it is never allowed to replace a built-in.

### 2.4 Versions and compatibility

Package versions and ranges use SemVer 1.0. Both fields are required:

~~~toml
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
~~~

- bsh is checked against the host crate version (3.0.2 at the time of writing).
- extension_api is checked against EXTENSION_API_VERSION (1.0.0).
- ui_api is optional; when present it is checked against the same API version
  and a mismatch is a warning, because this host validates the UI document
  shape before rendering.
- An incompatible package is listed with the failure but never activated.

## 3. Package layout

~~~text
my-extension/
  extension.toml          required
  README.md               recommended
  LICENSE                 recommended
  tools/<id>.json         one per declared tool
  commands/<id>.md        one per declared command
  prompts/<id>.md         one per declared prompt
  contexts/<id>.md        one per declared context
  panels/<id>.json        one per declared panel
  workflows/<id>.yaml     recorded only
  themes/<id>.json        recorded only
  schemas/                configuration schemas
  assets/                 icons and images referenced as asset/...
~~~

The file name is the FULL contribution id, so git.status is
tools/git.status.json. Using the full id rather than its last segment prevents
two contributions with the same leaf name from colliding on disk.

Every declared contribution must have its file. A package that declares a tool
and ships no descriptor is refused; it does not silently contribute less than
its manifest says.

## 4. Manifest reference

The parser rejects unknown fields. A misspelled mandatory field that disappears
silently is how a package asks for nothing and receives nothing. Optional
additions use serde defaults, which is the forward-compatibility rule.

### 4.1 Top level

| Field | Type | Required | Meaning |
|---|---|---|---|
| schema_version | string | yes | exactly "1.0" |
| extension | table | yes | identity |
| compatibility | table | yes | bsh and extension_api ranges |
| runtime | table | yes | execution mode |
| capabilities | table | no | required and optional requests |
| contributes | table | no | declared contributions |
| configuration | table | no | JSON Schema path and defaults |
| resources | table | no | requested ceilings |
| dependencies | table | no | id -> version range, at most 64 |

### 4.2 extension

~~~toml
[extension]
id = "com.example.git-assistant"
name = "Git Assistant"
version = "1.0.0"
description = "Git tools for the BSH agent"
authors = ["Example Developer"]
license = "MIT"
repository = "https://example.com/git-assistant"
homepage = "https://example.com"
~~~

name is 1 to 128 characters, description 1 to 4096 bytes. authors has at most
32 entries.

### 4.3 runtime

~~~toml
[runtime]
kind = "declarative"        # builtin | declarative | process | wasm-component
entry = "bin/server"        # process and wasm only
args = ["--stdio"]          # process only
startup = "on-demand"       # on-startup | on-demand | on-workspace-open | manual
protocol = "jsonrpc-2.0"    # process only
[runtime.env]
LOG_LEVEL = "info"
~~~

- builtin: first-party Rust linked into the host. A manifest read from disk may
  not claim this runtime; the built-in registry owns those ids.
- declarative: reads the declared files. entry is forbidden.
- process: entry is required. It is either a package-relative executable or one
  of the allowlisted launchers: python3, python, node, bun, deno, sh, bash.
  Package-relative paths may not be absolute and may not contain "..".
- wasm-component: entry must be a package-relative .wasm file. The contract is
  published in crates/bsh/wit. This build refuses activation with
  UnsupportedRuntime; it does not pretend to run wasm.

startup defaults to on-demand. manual packages are never applied automatically.

Environment policy: the host clears the environment and forwards only PATH,
HOME, TMPDIR, LANG and LC_ALL, plus the manifest's own variables. A package may
not set PATH, LD_PRELOAD, LD_LIBRARY_PATH, any DYLD_* variable, PYTHONPATH or
NODE_OPTIONS.

### 4.4 Capabilities

~~~toml
[capabilities]
required = ["agent.tools.register", "workspace.read"]
optional = ["workspace.write", "network.http"]
~~~

A capability is a request, never a grant. Required capabilities are checked at
activation; optional ones are checked when a tool asks for them.

### 4.5 Contributions

~~~toml
[contributes]
tools = ["git.status", "git.diff"]
commands = ["git.refresh"]
panels = ["git.panel"]
prompts = ["git.prompt"]
contexts = ["git.context"]
workflows = ["git.release"]
themes = ["git.dark"]
events = ["workspace.opened", "tool.*"]
publishes = ["git.changed"]
~~~

events are subscription topics; a trailing .* is allowed. publishes are topics
the package may emit; a wildcard is not allowed there. Topic segments are
lowercase letters, digits, hyphen and underscore, separated by dots.

### 4.6 Resources

~~~toml
[resources]
max_memory_mb = 128          # 1..=4096, default 256
max_execution_ms = 5000      # 1..=600000, default 30000
max_concurrent_calls = 4     # 1..=64, default 4
max_output_bytes = 1048576   # 1..=16777216, default 1048576
~~~

A manifest asking above a ceiling is invalid rather than silently clamped. The
host applies max_execution_ms and max_output_bytes today. max_memory_mb is for
the wasm runtime, which is not implemented. max_concurrent_calls is a declared
allowance: the process runtime serializes calls through one child, so its
effective concurrency is one until a pooled transport lands.

### 4.7 Dependencies

~~~toml
[dependencies]
"com.example.base" = "^1.0"
~~~

There is exactly one selected version per id in a resolution. A later range
that excludes the selected version is a conflict; the host does not install two
versions of one id, because they would share a storage namespace and a tool
namespace. Resolution is depth-first, dependencies before dependents, highest
matching version. Cycles and missing ids are named in the error. A disabled
package is not a candidate, even as a dependency; the dependent then fails to
resolve rather than running code the reader switched off.

## 5. Tool descriptors

Every declared tool has a descriptor file:

~~~json
{
  "description": "Read the status of one Git path.",
  "input_schema": {
    "type": "object",
    "properties": { "path": { "type": "string" } },
    "required": ["path"]
  },
  "capabilities": ["workspace.read"],
  "implementation": { "kind": "echo_arguments" }
}
~~~

- description: 1 to 4096 bytes; defaults to the package name and id.
- input_schema: a JSON object. Defaults to an object with no properties.
- capabilities: checked per call, in addition to the activation check. This is
  the least-privilege seam: a package may register ten tools but only one of
  them may ask for workspace.write.
- implementation: required for declarative tools, ignored by process tools.

Implementation variants:

| kind | fields | result |
|---|---|---|
| echo_arguments | none | the arguments unchanged |
| constant | value | a fixed JSON value |
| template | text | a string with top-level {{name}} placeholders |

Template placeholders read top-level argument names. A string renders as its
text, any other JSON value renders as compact JSON, and a missing argument
renders as empty. Tool output is capped by max_output_bytes.

## 6. Capability model

| Capability | Risk | Built-in default | Verified default | Community / local default |
|---|---|---|---|---|
| workspace.read | low | allow | allow | allow, scoped to the workspace |
| extension.storage | low | allow | allow | allow |
| agent.tools.register | low | allow | allow | allow |
| agent.context.provide | low | allow | allow | allow |
| agent.prompts.provide | low | allow | allow | allow |
| events.publish | low | allow | allow | allow |
| events.subscribe | low | allow | allow | allow |
| ui.panels | low | allow | allow | allow |
| ui.commands | low | allow | allow | allow |
| ui.theme | low | allow | allow | allow |
| agent.models.provide | medium | allow | allow | explicit grant |
| agent.agents.provide | medium | allow | allow | explicit grant |
| agent.workflows.provide | medium | allow | allow | explicit grant |
| workspace.write | high | allow | explicit grant | explicit grant |
| network.http | high | allow | explicit grant | explicit grant |
| process.execute | critical | allow | explicit grant | explicit grant |
| computer.use | critical | allow | explicit grant | explicit grant |
| secrets.read | critical | allow | explicit grant | explicit grant |

The default for a package read from disk is local. A publisher verification
path does not exist yet, so "verified" is a recorded level, not a claim this
build can prove.

### 6.1 Grants

Grants are stored in extension-state/grants.toml:

~~~toml
[grants."com.example.git-assistant"."workspace.write"]
scope = "workspace"
granted_at = 1790000000
note = "needed for the fix command"
~~~

Scopes:

| Scope | Covers |
|---|---|
| any | every call |
| workspace | a call with an open workspace |
| path:/prefix | a call whose path (or workspace) starts with the prefix |
| domain:example.com | that host and its subdomains |
| secret:NAME | that one named secret |

A grant scoped to one path does not cover another path. A call that does not
match its grant's scope is denied with the scope in the reason, not silently
widened.

### 6.2 Audit

Every evaluation is appended to logs/extensions/audit.jsonl as one JSON object
per line with timestamp, extension, capability, allowed, scope, trust and
reason. The log rotates at 1 MiB to audit.jsonl.1. Secret values are never
logged.

### 6.3 Per-call scope extraction

The host derives a call scope from the arguments before authorization:

- path: the argument named path, resolved against the workspace when relative;
- domain: the host of an argument named url;
- secret: an argument named secret.

A tool that reads a path must name it path for a path-scoped grant to match. A
tool that reaches a host must name the URL url.

## 7. Lifecycle

~~~text
Discovered -> Validating -> Resolved -> Installing -> Installed
Installed -> Loading -> Starting -> Active
Active -> Suspending -> Suspended -> Starting
Active -> Stopping -> Unloaded
any non-Unloaded -> Failed
Failed -> Loading (retry)
Unloaded -> Loading (reinstall)
~~~

A transition the table does not define is an error, not a silent rewrite.
Failure is always legal, because the point is to stop a broken package.

Persistence: enabled flags, the last state, the last error and the last start
time live in extension-state/state.toml. They survive a package upgrade.
Uninstall removes the state record and keeps extension-data/<id> unless
--purge-data is passed.

Startup policies: on-startup and on-demand apply at build time; on-workspace-open
requires an open workspace; manual is never applied automatically. A package can
be enabled and still not applied (for example manual, disabled, missing a
required grant, or failed dependency resolution), and the status row says which.

Runtime activation is lazy. The build registers descriptors synchronously; a
process is spawned on its first call and a wasm instance would compile on its
first call. The lifecycle state Active means the host authorized and loaded the
package, not that a process is necessarily running.

## 8. Process protocol

Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout. One JSON object per
line, UTF-8. stderr is diagnostic only and is logged with the extension id.

### 8.1 Handshake

Host sends:

~~~json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocolVersion":"1.0",
  "extensionId":"com.example.server",
  "host":{"name":"bsh","version":"3.0.2"}
}}
~~~

Extension answers:

~~~json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}
~~~

The host kills the process and refuses the package if the result's
protocolVersion does not start with "1.".

### 8.2 Calls

~~~json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
  "name":"git.status",
  "arguments":{"path":"."},
  "correlationId":"6f1c..."
}}
~~~

The result is the tool's JSON value:

~~~json
{"jsonrpc":"2.0","id":2,"result":{"branch":"main","clean":true}}
~~~

An error uses the standard JSON-RPC error object and becomes a tool error:

~~~json
{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"not a git repository"}}
~~~

Responses with an id other than the request's are treated as notifications or
late answers and skipped. A process that closes stdout before answering fails
the call. A timed-out call kills the session; the next call starts a fresh
process rather than reading a stale response.

### 8.3 Shutdown

The host sends a shutdown request with a 1 second deadline, then kills the
process. Dropping the runtime also kills the child.

### 8.4 Limits

- one line: 4 MiB;
- one result: max_output_bytes;
- one call: min(caller deadline, max_execution_ms);
- handshake: 10 seconds;
- calls are serialized through one child; max_concurrent_calls describes the
  package's allowance, not a promise that one process is parallel.

## 9. Events

Envelope:

~~~json
{
  "id": "uuid",
  "source": "com.example.package",
  "event_type": "tool.completed",
  "schema_version": "1.0",
  "timestamp": 1790000000000,
  "correlation_id": "turn-1",
  "payload": {}
}
~~~

Reserved topics: app.started, app.shutdown, workspace.opened,
workspace.closed, agent.started, agent.completed, agent.failed, tool.invoked,
tool.completed, tool.failed, extension.loaded, extension.unloaded,
extension.failed.

Controls:

- subscriber filters are exact topics or a trailing namespace wildcard;
- each subscriber has a bounded queue (default 64, maximum 4096);
- a full queue drops the event and increments a counter; it never blocks the
  publisher and never grows without bound;
- an event over 256 KiB is refused before publication;
- subscription drop removes it from the bus.

The host publishes extension.loaded and extension.failed today, and every
extension tool publishes tool.invoked, tool.completed and tool.failed on its own
source with the call's correlation id.

## 10. Declarative UI

Panels carry a schema_version and a root node:

~~~json
{
  "title": "Git Status",
  "icon": "assets/git.svg",
  "view": {
    "schema_version": "1.0",
    "root": {
      "type": "column",
      "children": [
        { "type": "heading", "text": "Git Status", "level": 2 },
        { "type": "text", "text": "Working tree clean" },
        { "type": "button", "label": "Refresh", "action": "git.refresh", "style": "primary" }
      ]
    }
  }
}
~~~

Components: container, row, column, text, heading, markdown, button, input,
select, checkbox, list, table, tree, tabs, progress, code, image, notification,
divider, spacer.

Limits: depth 32, nodes 1000, one text leaf 16 KiB, all text 256 KiB, list 500
items, table 500 rows and 32 columns, cell 2048 characters, tabs 16, serialized
document 1 MiB.

Rules:

- no HTML, script, CSS or executable content is accepted;
- an action is a contribution id owned by the package;
- an image is an https URL or an asset/ package-relative path; data:, file:,
  javascript: and http: are refused;
- every update carries a revision; a patch at or below the current revision is
  stale and refused;
- an action carries the revision it was rendered from; a mismatched revision is
  refused, so a control from an old view cannot act on a new one.

The host owns GPUI objects. A panel contribution is registered as an id and a
title today; the declarative renderer consumes the document next.

## 11. Storage

Each extension owns extension-data/<id>/. Keys are relative paths; absolute
paths, "..", "." components and backslashes are refused. The canonical parent
must stay inside the namespace, which catches a symlinked directory planted
inside it. A stored value cannot be a symlink. One value is at most 8 MiB and a
namespace holds at most 10,000 files. Writes are atomic (temp file, fsync,
rename).

The namespace is created and purged by the host. The containment API is
implemented and tested; no runtime host binding is exposed yet, so a package
cannot call it until the host-function surface lands. No runtime will be able to
read another namespace through that API when it does.

## 12. Store, integrity and locking

~~~text
~/.bos/bsh/extensions/          default root, override with BSH_EXTENSION_ROOT
  installed/<id>/<version>/     immutable package code and assets
  staging/                      an install in progress
  cache/                        downloaded packages
  quarantine/                   disabled or failed packages awaiting review
  extension-data/<id>/          business data
  extension-state/
    registry.toml               installed versions, hashes and sources
    state.toml                  enabled flags and last errors
    grants.toml                 capability grants
    lock.toml                   the resolved dependency set
  logs/extensions/
    audit.jsonl                 capability decisions
    <id>/                       package logs
~~~

The package hash is SHA-256 over each file's relative path, byte length and
content digest, in sorted path order. Directory metadata, timestamps and modes
are excluded. .git, .jj, target and node_modules are excluded from both the hash
and the copy. Symlinks are refused: a package must be self-contained.

Install is staged, hashed, then renamed into place. A failed install leaves the
staging directory, never a half-populated version. An existing id/version is
refused; an upgrade is a new version directory or an explicit uninstall/install.

The lockfile records format, host, extension API, and for each package its id,
exact version, hash, source and resolved dependency versions. It is sorted by id
and is deterministic for a given resolution.

Signatures: the registry record has a signature field and doctor warns when one
is present, because this build records signature metadata but does not verify
it. A hash proves integrity after install; it does not prove who produced the
package.

## 13. Diagnostics and CLI

Per extension the host exposes: id, name, version, runtime, trust, source, root,
hash, lifecycle state, health, configured vs applied, qualified tool, command and
panel names, capability decisions with scopes and reasons, the last error,
startup duration, install time and dependency ids.

~~~text
bsh ext new <id> [--dir <path>] [--force]
bsh ext validate <path>
bsh ext hash <path>
bsh ext install <path>
bsh ext uninstall <id> [--purge-data]
bsh ext list [--json]
bsh ext info <id> [--json]
bsh ext enable <id>
bsh ext disable <id>
bsh ext grant <id> <capability> [--scope <scope>] [--note <text>]
bsh ext revoke <id> <capability>
bsh ext capabilities
bsh ext logs <id>
bsh ext doctor [--json]
~~~

Global options: --root <path> and --json. Exit codes: 0 success, 1 failure,
2 usage. validate returns 1 on an invalid manifest or a missing/unparseable
contribution file. doctor returns 1 when an error-severity issue exists.

## 14. Trust and security checklist

A host that implements this standard must enforce:

- path containment for every package-relative path and every storage key;
- manifest schema and semantic validation before any code runs;
- capability authorization at activation and per call, with scope matching;
- process environment filtering, package-root working directory, executable
  allowlist and an explicit process.execute grant;
- timeouts, line and output ceilings, and queue bounds;
- no JS, HTML or executable UI content;
- atomic state writes and an audit record for every decision;
- hash verification on load and a doctor check for post-install tampering;
- no silent fallback when a declared contribution cannot load.

## 15. Deliberate limits of this implementation

These are stated so a package author does not design around a promise that is
not kept:

1. The wasm-component runtime is specified in WIT but not implemented; a wasm
   package validates and then refuses activation with a clear error.
2. Signatures are recorded, not verified.
3. Workflows and themes are recorded in status but have no executor or applier.
4. The declarative UI is validated and exposed as a contract; the GPUI renderer
   is the next consumer, so a panel currently appears as a registered id/title.
5. The built-in runtime is for first-party code linked into bsh; an installed
   package cannot claim it.
6. The extension root is ~/.bos/bsh/extensions (BSH's existing home) rather than
   ~/.bsh as the design sketched; BSH_EXTENSION_ROOT overrides both paths.
7. The event bus is implemented and tested, but no host service subscribes yet;
   extension tools and lifecycle transitions already publish on it.
8. Command contributions register in the host plugin registry and appear in
   status, but the composer's command palette is still a hardcoded list, so a
   reader typing / does not see them yet. Panel contributions are registered and
   validated but wait on the declarative renderer (item 4).

## 16. Example

A minimal declarative package:

~~~toml
schema_version = "1.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "One echo tool and one command"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["echo"]
commands = ["greet"]
~~~

tools/echo.json:

~~~json
{
  "description": "Echo the arguments back to the model",
  "input_schema": { "type": "object", "properties": {} },
  "implementation": { "kind": "echo_arguments" }
}
~~~

commands/greet.md:

~~~markdown
---
description: Greet the reader
---
Say hello and name the open workspace.
~~~

Run:

~~~text
bsh ext validate examples/extensions/hello
bsh ext install examples/extensions/hello
bsh ext list
bsh ext doctor
~~~
