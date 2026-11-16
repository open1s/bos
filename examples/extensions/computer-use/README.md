# Computer Use (BSH example)

A real macOS screen, pointer and keyboard extension. It exists to verify the BSH
extension framework end to end, not to be the product's built-in computer-use
service.

| Surface | Implemented? |
|---|---|
| Process runtime + JSON-RPC handshake | yes |
| Tool descriptors and calls | 9 tools |
| Panel with actions | right panel, `refresh` command |
| Command contribution | `/refresh` prompt |
| Prompt contribution | safety rules |
| Skill contribution | `computer-safety` |
| Integration (tool action) | `desktop.control` → `screen_size` |
| Integration (command action) | `desktop.refresh` → `refresh` |
| Workflow | `inspect` = size + cursor + screenshot |
| Event subscription | `session.created`, `tool.completed` |
| Capability gate | `process.execute`, `computer.use` |

## Permissions

macOS requires two independent grants, both to the **BSH app** (or the terminal
that launched the debug binary):

1. **Screen Recording** — System Settings → Privacy & Security → Screen Recording.
2. **Accessibility** — System Settings → Privacy & Security → Accessibility.

Without Screen Recording, `screenshot` fails with a clear error. Without
Accessibility, pointer/keyboard events can be silently ignored by the OS; the
tool may return `ok` even though nothing moved. This is an OS behaviour, not a
framework bug.

## Install and verify

```text
bsh ext validate examples/extensions/computer-use
bsh ext install examples/extensions/computer-use
bsh ext grant com.example.computer-use process.execute --scope program:python3
bsh ext grant com.example.computer-use computer.use
bsh ext grant com.example.computer-use agent.tools.register
bsh ext grant com.example.computer-use agent.prompts.provide
bsh ext grant com.example.computer-use agent.skills.provide
bsh ext grant com.example.computer-use agent.workflows.provide
bsh ext grant com.example.computer-use ui.panels
bsh ext grant com.example.computer-use ui.commands
bsh ext grant com.example.computer-use events.subscribe
bsh ext info com.example.computer-use
```

Expected info: `active`, `healthy`, tools `computer-use__screen_size` …
`computer-use__inspect`, panel `computer-use__desktop`, command
`computer-use__refresh`.

Safe smoke tests, in order:

```text
bsh ext mock examples/extensions/computer-use screen_size '{}'
bsh ext mock examples/extensions/computer-use cursor_position '{}'
bsh ext mock examples/extensions/computer-use screenshot '{}'
bsh ext integrate com.example.computer-use desktop.control '{}'
bsh ext integrate com.example.computer-use desktop.refresh '{}'
```

The last command proves the command seam and returns a revision-2 panel
document. The panel's Refresh button uses the same path.

Do not run mouse/keyboard tools against an app you do not control. To test the
panel, install, grant, then open Settings → Extensions → Installed and the right
panel's Computer Use tab. Clicking `Screenshot` routes the action to the tool and
shows its summarized result; `Refresh` returns the document.

## Safety and the framework finding

`computer.use` is enforced as an **activation-time capability** and audited.
It is not a per-call OS sandbox for a process runtime: once the Python child is
alive, it can call CoreGraphics directly. This is the same trust model as any
process extension and is why the package declares `process.execute` and
`computer.use` explicitly, and why the README tells the reader to review the
code before installing.

A production computer-use service needs a host-owned API (screen/input) with
per-call consent and platform-specific isolation. This example deliberately
stays a process extension so it can verify the framework without pretending the
capability is a sandbox.

## Coordinates

- `screen_size` returns **points** (mouse coordinate space) and **pixels**
  (screenshot dimensions).
- Retina screenshots are 2x their point size; do not pass screenshot pixel
  coordinates directly to `mouse_click`.
- Multi-display capture is not implemented; the main display is used.

## Files

```text
server.py              JSON-RPC process, CoreGraphics via ctypes
panels/desktop.json    initial panel document
tools/*.json           9 tool descriptors
commands/refresh.md    composer prompt
prompts/system.md      safety instructions
skills/computer-safety/SKILL.md
integrations/*.json    tool action + command action
workflows/inspect.json 3-step same-package workflow
```
