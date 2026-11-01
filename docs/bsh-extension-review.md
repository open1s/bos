---
module: bsh
tags: [extensions, review, product, architecture, templates]
problem_type: review
---

# BSH Extension Framework Review

## 1. Product review (PM)

### Users

- **Package author**: wants a manifest, a runnable local template, a fast mock,
  and a clear contract per runtime.
- **Reader**: wants to discover a package, install it, see what it contributes,
  switch it off without uninstalling, and understand a failure.
- **Operator**: wants hashes, signatures, capability grants, audit, quotas and a
  reproducible lock.

### Journeys

1. Author copies a starter, runs `bsh ext validate` and `bsh ext mock`.
2. Reader opens **Settings → Extensions → Market**, refreshes, installs.
3. Reader sees the package under **Installed**, enables/disables it, and sees
   contributed panels, commands, tools, skills, languages and integrations.
4. A denied capability is shown inline with the exact grant command.
5. Upgrades and rollbacks preserve data and the lockfile.

### PM findings and resolutions

| Finding | Resolution |
|---|---|
| Extension page was the legacy Codex/DSH plugin list | Rail now opens the BSH extension platform |
| No way to discover/install from the UI | Market tab with Git provider, install, enable/disable |
| Installed items had no control | Per-extension enable/disable in the inventory |
| MCP and Skills settings would be lost | Kept as tabs of Extensions |
| Language/integration were descriptor-only | Real diagnostics and action dispatch |
| Process integrations could not run commands | `commands/execute` added to the process protocol |

Remaining product gap: **audio capture** has no host producer of its own;
`audio.captured` fires when an audio attachment is captured, and there is no
voice-input subsystem to plug into.

## 2. Architecture review

### Runtime matrix

| Runtime | Tools | Commands | Events | Panels | Language | Integration |
|---|---|---|---|---|---|---|
| Declarative | yes | yes | no | yes | descriptor + tool | descriptor + tool/command |
| Process | yes | yes | yes | yes | descriptor + tool | descriptor + tool/command |
| WASM component | yes | yes | yes | yes | descriptor + tool | descriptor + tool/command |
| Extism | yes | yes | yes | yes | descriptor + tool | descriptor + tool/command |
| MCP | yes | no | yes | no | via mapped tools | via mapped tools |
| Built-in | yes | yes | yes | yes | descriptor + tool | descriptor + tool/command |

### Control flow

```text
manifest -> package hash -> store -> resolver/lock
  -> lifecycle -> capability policy -> runtime
  -> tools/commands/panels/prompts/contexts/skills/languages/integrations
  -> agent, composer, settings, event bus
```

### Architecture findings and resolutions

- **Descriptors must execute**: language and integration descriptors now require
  a declared tool/action; `ExtensionHost::language_check` and
  `run_integration` dispatch it through the runtime.
- **Process command parity**: the process runtime now sends
  `commands/execute` like Extism/WASM/declarative, so an integration command is
  not a dead end.
- **No leak**: host shutdown drains runtimes and subscriptions; app exit calls
  it; the event pump exits on generation/weak handles; process/MCP children are
  killed; an end-to-end test asserts subscriber count returns to zero.
- **Windows**: a `windows-latest` job runs the extension suite; the TOML lock
  fixture was fixed for backslashes.
- **Market trust**: installed Git packages use community trust; signatures can
  elevate; grants are still required before activation.

Remaining architecture gap: language diagnostics are callable through the host
and agent but are not yet wired into an editor gutter; integration actions are
callable but not a generic outbound proxy. Both are consumers, not missing
contracts.

## 3. End-user review

The **Extensions** rail cell now shows:

- **Installed**: name, id, version, runtime, trust, state, tools,
  skills/languages/integrations, error, enable/disable.
- **MCP**: the existing server settings.
- **Skills**: the existing sources and toggles.
- **Market**: cached Git index, refresh, available/installed state, install,
  enable and disable.

Failure text is inline. A package denied a capability stays installed but
inactive, with the grant command in its error.

## 4. Starter templates

| Type | Starter | What is complete |
|---|---|---|
| Declarative | `examples/extensions/hello` | tool, command, prompt, context, panel, theme, preset, model, agent |
| Process | `examples/extensions/process-echo` | handshake, token, tool, events, commands |
| WASM Component | `examples/extensions/wasm-echo` | WIT exports, progress |
| Extism | `examples/extensions/extism-echo` | PDK exports, host bridge |
| MCP | `examples/extensions/mcp-echo` | tools, resources, prompts, progress |
| Hooks/UI | `examples/extensions/hook-demo` | every hook topic, skills, languages, integrations, UI, theme, presets |
| Language | `examples/extensions/language-python` | real Python diagnostics, GitHub integration |
| Computer Use | `examples/extensions/computer-use` | process runtime, 9 tools, panel, command, prompt, skill, 2 integrations, workflow, hooks, macOS permissions |
| Built-in | `examples/extensions/builtin-template` | Rust trait + manifest snippet |

## 5. Verdict

The contracts are implemented and the main journeys work end to end: validate,
mock, install, grant, activate, call, subscribe, dispatch language/integration
actions, disable, re-enable, market install, and shut down without leaked
subscriptions. The remaining gaps are named above and are not hidden behind
descriptor-only fields.
