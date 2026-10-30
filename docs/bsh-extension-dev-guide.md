---
module: bsh
tags: [extensions, guide, developer, declarative, process, capabilities]
problem_type: guide
---

# BSH Extension Developer Guide

This guide takes a package from an empty directory to an installed tool that the
agent can call. The normative contract is docs/bsh-extension-standard.md; read
it before shipping a package.

## 1. Quick start

~~~text
bsh ext new com.example.hello --dir hello
bsh ext validate hello
bsh ext install hello
bsh ext list
bsh ext info com.example.hello
bsh ext doctor
~~~

new writes a declarative package with one tool and one command. validate parses
the manifest, checks compatibility, hashes the tree and prints every error it
finds in one run. install copies the package into the managed store only after
those checks pass.

## 2. A declarative tool

A declarative package executes no code. Its tools are pure transformations, so
it needs no process and no sandbox runtime.

extension.toml:

~~~toml
schema_version = "1.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "One echo tool"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["echo"]
~~~

tools/echo.json:

~~~json
{
  "description": "Echo the arguments back to the model",
  "input_schema": {
    "type": "object",
    "properties": {
      "text": { "type": "string", "description": "Text to echo" }
    }
  },
  "implementation": { "kind": "echo_arguments" }
}
~~~

The model calls hello__echo (the extension's last id segment, then the tool id).
The result is the argument object unchanged.

Three bodies are available:

~~~json
{ "kind": "echo_arguments" }
{ "kind": "constant", "value": { "answer": 42 } }
{ "kind": "template", "text": "hello {{name}}" }
~~~

## 3. A command and a prompt

commands/greet.md becomes a host command and a prompt contribution:

~~~markdown
---
description: Greet the reader
---
Say hello and name the open workspace.
~~~

The command is registered as hello__greet in the host plugin registry and is
also exposed to the model as prompt material when the agent is built.

prompts/system.md and contexts/workspace.md work the same way, but contribute
only prompt text, not commands.

## 4. A panel

panels/status.json carries a validated declarative view:

~~~json
{
  "title": "Hello",
  "view": {
    "schema_version": "1.0",
    "root": {
      "type": "column",
      "children": [
        { "type": "heading", "text": "Hello", "level": 2 },
        { "type": "text", "text": "The extension is active." },
        { "type": "button", "label": "Refresh", "action": "refresh" }
      ]
    }
  }
}
~~~

The action is a contribution id owned by the package. The current host registers
the panel id and title; the declarative renderer is the next consumer. Invalid
actions, over-deep trees, an oversized text leaf and a non-https image are
refused at load time.

## 5. A process extension

Use the process runtime when the tool needs a real language runtime, a service
or dependencies that do not fit in wasm.

extension.toml:

~~~toml
schema_version = "1.0"

[extension]
id = "com.example.process-echo"
name = "Process Echo"
version = "1.0.0"
description = "An echo tool served by a Python process"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "process"
entry = "python3"
args = ["server.py"]
protocol = "jsonrpc-2.0"
startup = "on-demand"

[capabilities]
required = ["process.execute", "agent.tools.register"]

[contributes]
tools = ["echo"]
~~~

server.py reads one JSON object per line, answers initialize, tools/call and
shutdown, and writes one JSON object per line to stdout:

~~~python
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "1.0"}
    elif method == "tools/call":
        result = request["params"]["arguments"]
    elif method == "shutdown":
        result = {}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
~~~

The process starts with a cleared environment, a package-root working directory
and piped stdio. It receives only PATH, HOME, TMPDIR, LANG, LC_ALL and the
manifest's own runtime.env variables. It is killed when the call times out, when
the runtime is unloaded and when the child is dropped.

process.execute is critical, so a local package is denied until the reader
grants it:

~~~text
bsh ext grant com.example.process-echo process.execute --scope any --note "local dev server"
bsh ext install examples/extensions/process-echo
~~~

## 6. Capabilities

A manifest declaration is a request. A grant is the permission. The defaults
for a package read from disk are in the standard's table: low-risk registration
and read capabilities are allowed, write, network, process, computer use and
secrets need an explicit grant.

~~~text
bsh ext capabilities
bsh ext grant com.example.tool workspace.write --scope workspace
bsh ext grant com.example.tool network.http --scope domain:api.example.com
bsh ext grant com.example.tool secrets.read --scope secret:GITHUB_TOKEN
bsh ext revoke com.example.tool workspace.write
~~~

A tool lists the capabilities its body needs in its descriptor. The host checks
them on every call, so a tool can be registered and still be denied when it
asks for something its package only listed as optional.

The call scope is derived from the arguments: path, url and secret. Name them
exactly if a scoped grant must match.

## 7. Testing and debugging

~~~text
bsh ext validate <path>       # every manifest error in one run
bsh ext hash <path>           # canonical hash; changes whenever content changes
bsh ext doctor                # hash drift, dependency failures, policy gaps
bsh ext logs <id>             # audit decisions and per-extension logs
bsh ext info <id> --json      # the full status row for a script
~~~

Common refusals and what they mean:

| Message | Cause |
|---|---|
| unsupported schema version | schema_version is not "1.0" |
| requires bsh ... | compatibility.bsh does not include the host version |
| runtime.entry | a process entry is absolute, traverses, or is missing |
| every declared tool must have tools/<id>.json | contributes.tools names a file that is not there |
| declarative tool X needs an implementation | tools/X.json has no implementation object |
| requires an explicit grant | the capability is above the trust level's default |
| grant is scoped to ..., which does not cover this call | the argument's scope does not match the grant |
| dependency resolution failed | a dependency is missing, conflicting or cyclic |
| process closed stdout before answering | the server exited or wrote nothing for the request |
| process line exceeds ... | the server wrote a line over the 4 MiB ceiling |

## 8. Lifecycle in practice

The build registers descriptors and authorizes required capabilities. The
process or wasm work starts on the first call. A package is enabled by default
when installed; disable it per workspace with the existing settings inventory
toggle, or with:

~~~text
bsh ext disable com.example.hello
bsh ext enable com.example.hello
~~~

A state change is picked up on the next agent build; the host does not re-read
manifests per frame.

The host ships one built-in extension, com.bos.bsh.extensions. It offers the
read-only tool extensions__list, which reports the status rows a reader sees in
bsh ext list. It is a package like any other for gating: a preset whose
extensions allowlist is empty hides it, and bsh ext disable can switch it off.

Uninstall keeps extension-data/<id> by default. Pass --purge-data only when the
reader has agreed to lose it.

## 9. Where the implementation lives

| File | Responsibility |
|---|---|
| types.rs | ids, versions, lifecycle vocabulary, errors |
| manifest.rs | parse and validate extension.toml |
| package.rs | store, hash, staging, atomic install, state |
| resolver.rs | version and dependency resolution, lockfile |
| capability.rs | capability vocabulary, grants, policy, audit |
| lifecycle.rs | state machine and health |
| runtime/mod.rs | runtime contract, tool files, lazy activation |
| runtime/builtin.rs | first-party Rust |
| runtime/declarative.rs | pure file contributions |
| runtime/process.rs | JSON-RPC process runtime |
| events.rs | bounded event bus |
| storage.rs | contained per-extension storage |
| ui.rs | declarative UI validation and revisions |
| agent.rs | tools, commands, panels, prompts and per-call authorization |
| host.rs | discovery, resolution, activation, status, doctor |
| cli.rs | bsh ext |
