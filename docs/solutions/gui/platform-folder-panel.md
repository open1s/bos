---
module: gui
tags: [ui, file-dialog, testability, mocking, workspace, platform]
problem_type: best-practice
---

# Ask the platform for the folder, and keep an in-app fallback

## Problem

"Add a workspace" has to answer one question before anything else: *which folder
does this workspace own?* The root is chosen once and then trusted for a long
time, so the gesture should be the one the user already knows: the system's
folder panel.

The obvious route — `tauri-plugin-dialog` or `rfd` — was not available: neither
crate is in this machine's offline cargo registry. The first version of this
feature treated that as the reason to build an in-app browser *instead*, and wrote
the trade-off down as a design decision. That was the wrong conclusion. A missing
dependency is a reason to reach for the panel the OS already ships, not a reason
to hand-roll a weaker one. `rfd` on macOS is, at bottom, a call to the same
AppKit panel that `osascript` can open.

Two hard constraints remain, and they shape the design:

1. **A modal cannot be automated.** `live_selftest.sh` is the only end-to-end loop
   reachable on macOS (tauri-driver has no macOS backend), and it drives the
   shipped page in headless Chrome. A panel on the user's screen is outside that
   page, and `window.prompt`/`window.confirm` already taught this repository that
   lesson.
2. **A panel may not exist.** A container, a headless CI runner, or a Linux box
   without zenity has none. A dead button there is a broken feature.

## Solution

`crates/gui/src/picker.rs` builds the platform's own command line and reads the
answer back:

| Platform | Program | The user's answer |
| --- | --- | --- |
| macOS | `osascript`, `choose folder` | `POSIX path of` the chosen alias |
| Linux | `zenity --file-selection --directory`, else `kdialog --getexistingdirectory` | the path on stdout |
| Windows | PowerShell + `System.Windows.Forms.FolderBrowserDialog` | `SelectedPath` on stdout |

The panel is asked for through the backend (`choose_folder`), because the answer
must be resolved before anyone trusts it, and because `spawn_blocking` keeps a
modal from holding an async worker while a human decides.

The answer is a **type**, not a string:

```rust
pub(crate) enum ChooseOutcome {
    Picked(String),          // a folder; still to be resolved
    Cancelled,               // a decision, not a failure
    Unavailable(String),     // no panel here, with a reason to show
}
```

Collapsing those into `Result<Option<String>, String>` is how "changed my mind"
becomes a red error message. The three answers lead to three different UIs: a path
lands, a dismissal changes nothing and writes no error, and an absent panel opens
the fallback **with the reason**.

## Testing a modal you cannot click

Only the panel call itself is out of reach, so only the panel call is swapped:

```rust
let (program, mut args) = command("Pick", None)?;   // the real argv
for arg in args.iter_mut() {
    if arg.starts_with("set f to choose folder") {
        *arg = format!("set f to POSIX file {}", quote_applescript(&dir));
    }
}
```

The resulting argv runs against the **real** `osascript`, which puts the `-e`
list, the AppleScript escaping, `POSIX path of` and the `on error number -128`
handler under test. Two rules make the swap honest:

- **assert the swap happened** — a statement that silently fails to match leaves
  the panel call in place and hangs the suite;
- **assert what a successful swap cannot exercise** — the `-128` handler is why a
  dismissal is not an error, and nothing else in that test would notice if its
  number were wrong.

Everything else about the panel is pure and unit-tested: `command`, `outcome`,
`quote_applescript`, `quote_powershell`. The escaping tests are written in the
direction that bites — the backslash is escaped **before** the quote (otherwise
`C:\tmp` gains a `t` escape), and control characters are dropped, because a
newline cannot live inside an AppleScript literal at all.

## Why the in-app browser is still here

As the fallback, and only the fallback. `list_folder` (see the previous revision
of this note for its rules: directories only, everything resolved, a reported cap,
and a deliberate read *outside* the current workspace) is opened when the panel is
unavailable, and it carries the reason.

One field, three doors — the panel, the browser, and the keyboard — and one rule:
whichever door produced the path, `folders::resolve` is the single place it is
canonicalized and checked, so a root cannot be trusted differently depending on how
it was chosen.

## Costs and limits

The panel is only as good as the platform's program being present (hence the
fallback). The start folder is passed only when it exists, because a panel opened
at a missing path reports an error instead of opening. There is no "recent
folders" memory yet. And the human click itself is the one step no guard can walk:
it is reported as manually verified, never as automated.
