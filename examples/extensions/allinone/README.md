# All-in-One Exam Demo

One BSH extension package that exercises **every contribution kind** and the
process protocol end to end. It is a teaching artefact and a self-checking exam:
the package itself is the subject, and `scripts/exam.py` is the grader.

```text
examples/extensions/allinone/
  extension.toml              manifest: identity, compatibility, runtime, capabilities,
                              contributions, configuration, resources
  server.py                   process runtime: JSON-RPC, tools, commands, events
  tools/*.json                7 tools
  commands/*.md               2 composer commands (also panel actions)
  panels/dashboard.json       declarative UI document
  prompts/guide.md            agent instruction
  contexts/workspace.md       path-scoped context
  skills/allinone/SKILL.md    agent skill
  languages/demo.json         language descriptor
  integrations/*.json         GitHub and command integration descriptors
  themes/aurora.json          extension theme
  presets/safe.json           read-only agent preset
  providers/models/demo.json  model provider descriptor
  providers/agents/helper.json agent role descriptor
  workflows/inspect.json      3-step workflow over the package's own tools
  schemas/config.schema.json  configuration schema
  scripts/exam.py             the exam
```

## Quick start

```text
bsh ext validate examples/extensions/allinone
bsh ext test     examples/extensions/allinone
bsh ext mock     examples/extensions/allinone describe '{}'
bsh ext mock     examples/extensions/allinone sum '{"a":2,"b":3}'
```

Install with the one capability a process package cannot avoid granting:

```text
bsh ext grant com.example.allinone process.execute --scope program:python3 --note "all-in-one demo"
bsh ext install examples/extensions/allinone
bsh ext list
bsh ext info com.example.allinone
bsh ext providers
bsh ext languages
bsh ext integrations
bsh ext doctor
```

The other declared capabilities are optional and low risk. The package activates
with the process grant alone; the model and agent providers, panels, commands,
skills, workflows and theme are additional surfaces the host offers when the
reader has not disabled them.

## Run the exam

```text
python3 examples/extensions/allinone/scripts/exam.py
# or: BSH_BIN=./target/debug/bsh python3 examples/extensions/allinone/scripts/exam.py
# or: python3 examples/extensions/allinone/scripts/exam.py --quick
```

The exam needs Python 3.11+ for `tomllib` (the extension runtime itself supports
3.8+). It checks the manifest, every contribution file, every JSON document, the
process handshake, typed arguments, panel actions, events, the deliberate
failure, the developer CLI, and an install into a throwaway store.

`bsh ext mock` constructs the inner process runtime only, so the workflow
wrapper (`allinone__inspect`) is exercised by an agent build rather than by
mock; the exam therefore checks every workflow step against the declared tool
list, and `bsh ext test` counts the workflow. It prints a
`PASS`/`FAIL` line per check and a final score, and exits non-zero on any
failure.

## What each surface proves

| Surface | Where | How to observe |
|---|---|---|
| Tool (7) | `tools/*.json` + `server.py` | `bsh ext mock <pkg> <tool> '<json>'` |
| Command | `commands/hello.md`, `refresh.md` | Composer `/` palette; panel buttons |
| Panel | `panels/dashboard.json` | Settings → Extensions → panel, or the right panel |
| Prompt | `prompts/guide.md` | Injected into the built agent |
| Context | `contexts/workspace.md` | Injected only when `crates/**/*.rs` or `src/**/*.rs` matches |
| Skill | `skills/allinone/SKILL.md` | Skill inventory |
| Language | `languages/demo.json` | `bsh ext languages` |
| Integration | `integrations/*.json` | `bsh ext integrations` |
| Theme | `themes/aurora.json` | Settings → Extensions theme selector |
| Preset | `presets/safe.json` | Agent presets |
| Model provider | `providers/models/demo.json` | `bsh ext providers` |
| Agent provider | `providers/agents/helper.json` | `bsh ext providers` |
| Workflow | `workflows/inspect.json` | The `inspect` tool; chains describe → config → events |
| Events | `extension.toml` `events = [...]` | `events` tool after host activity |
| Configuration | `schemas/config.schema.json` + defaults | `config` tool |

## The tools

| Tool | Proves |
|---|---|
| `describe` | identity, runtime, capabilities and contribution inventory |
| `echo` | JSON arguments survive the process bridge unchanged |
| `sum` | typed arguments and input validation |
| `progress` | live `$/progress` notifications (2–10 steps) |
| `fail` | a JSON-RPC error becomes a tool error, not a result |
| `events` | host events arrive as `events/event` notifications |
| `config` | the configuration from the initialize handshake |

`progress` sleeps briefly between notifications so the host can deliver them;
`fail` always fails, which is why the exam expects a non-zero exit for it.

## Panel actions

The **Refresh panel** button (and its list row) dispatches the `refresh`
command back into the process. `server.py` returns a full declarative
`UiDocument`; the host validates it and applies it at revision 2. The returned
document shows the live event count, the configuration and a timestamp, and
carries the same two actions again, so the panel is repeatable. **Say hello**
returns a plain value, which the panel summarizes.

## Configuration

Defaults live under `[configuration.defaults]` and the schema is
`schemas/config.schema.json`:

```toml
[configuration.defaults]
greeting = "hello from the all-in-one demo"
verbose = true
```

The host passes the resolved configuration to the process in the initialize
handshake; `config` and `describe` report it, and the environment report shows
only the variables the runtime policy forwards (`PATH`, `HOME`, `TMPDIR`,
`LANG`, `LC_ALL` plus this package's own `DEMO_GREETING`).

## Why a process runtime

A package has exactly one runtime. This demo chooses `process` because it is
the only one that can demonstrate **all** of the following in one package:
JSON-RPC tool calls, live progress, deliberate failures, panel command actions,
event subscriptions, configuration handshakes and arbitrary validation. The
declarative tool bodies (`echo_arguments`, `constant`, `template`) need a
`declarative` runtime, and the WASM/Extism/MCP variants are separate packages
under `examples/extensions/`; the contribution kinds above are the same across
runtimes.

## Exam questions (with answers)

1. Which contribution is the only one that can return a new UI document?
   **A panel action handled by the package's runtime**
   (`commands/execute` → `UiDocument`).
2. Why does `fail` exit non-zero under `bsh ext mock`?
   **The server returns a JSON-RPC error, and the host maps it to a tool failure.**
3. Where does the per-call capability check happen?
   **At the tool boundary**, from `tools/*.json.capabilities` and the package's
   grants; this package asks for none per tool.
4. Why is `contexts/workspace.md` not always injected?
   **Its `paths:` globs must match a file in the open workspace.**
5. What makes the workflow unable to reach another package?
   **A workflow can call only tools declared by the same package.**
6. What is the package's model-facing tool name for `describe`?
   **`allinone__describe`** (last id segment + `__` + contribution id).
7. Which file is excluded from the package hash, and why? **`package.sig`**, because
   including it would make the signature impossible to verify after signing.
8. What happens to a declared contribution with no file?
   **Validation refuses the package; it never silently contributes less.**

## Limits

- The model provider points at `https://example.invalid/v1`; it exists to prove
  the descriptor and key-bearing surface, not to serve traffic.
- The panel is a right-rail card; the action path is the demonstration, not a
  dashboard product.
- The workflow is sequential by design (the executor is a bounded chain).
- The package records events in memory only; restarting the process clears the
  log. The exam starts a fresh process, which is why its event count is zero.

## License

MIT.
