# The terminal policy

bsh's terminal runs the user's own shell on a real PTY. One Rego module decides
what may cross three points of that terminal: the shell's **start**, each
submitted **input** line, and each **output** chunk read from the PTY. This file
is the contract a policy author writes against, with worked examples to copy.

The policy is a file, not a settings screen:

    <workspace>/.bsh/terminal-policy.rego

bsh reads it when the terminal starts, and re-reads it when the file's
modification time or length changes (once per terminal-clock tick, never per
frame). There is no settings UI for policies: the file, its reload and the audit
ring are the management surface for now.

The file is not authored from nothing. Every workspace carries
`<workspace>/.bsh/terminal-policy.rego.tpl`, written by bsh beside the other two
authoring templates (`policy.rego.tpl` and `egress-allowlist.tpl`): it states
the tri-state, documents the three input shapes and the answer shapes, and
carries worked examples behind one switch. Opening the terminal writes the same
policy body to `.bsh/terminal-policy.rego` when that file is missing, so the
gate always has a starter to edit; the `.tpl` is the documented copy. A `.tpl`
file is never read and never enforced.

## The decision

The gate reads exactly one rule, `data.terminal.verdict`. A policy may define
either of two rules and the shim builds `verdict` from it:

- `decision` - a **boolean**, or an **object**:

      {"allow": false, "reason": "destructive command"}
      {"allow": true,  "redact": "REDACTED"}
      {"allow": true}

- `allow` - a plain boolean (the original shape; older policies keep working).

`decision` wins when it is defined for an action. When neither is defined the
gate refuses, so an empty or unrelated module can never open the door.

Object fields:

| field    | type   | meaning                                                        |
|----------|--------|----------------------------------------------------------------|
| `allow`  | bool   | whether the action may proceed (missing reads as false)        |
| `reason` | string | optional; shown in the pane's status line when refused         |
| `redact` | string | output only: feed this text **instead of** the chunk           |

`redact` applies to output. A redacted chunk is still an `allow: true` decision,
and the audit ring records it as redacted.

## The input objects

Every action is evaluated with `input` set to one of these objects. `deny` is
bsh's own escape hatch and is always `false`; a policy may ignore it.

`cwd` is the directory the terminal was started in (the session workspace).
bsh keeps no mirror of the shell's live directory, so a `cd` the shell made
is invisible here: a policy that must confine paths should resolve them
itself rather than trust this field.

**start** - asked before any process exists:

    {"action": "start", "command": "<shell path>", "shell": "<shell path>",
     "cwd": "<workspace>", "deny": false}

**input** - asked on Enter, before the submitting carriage return is written.
`command` is the line exactly as this pane mirrored it from the keystrokes:

    {"action": "input", "command": "rm -rf build", "cwd": "<workspace>",
     "deny": false}

**output** - asked once per chunk read from the PTY master, before the chunk
reaches the grid. `text` is the chunk as text (lossy for a split multibyte
sequence), truncated to 4096 characters:

    {"action": "output", "text": "<chunk>", "deny": false}

Example rules use the action field to tell the three apart:

    input.action == "start"
    input.action == "input"
    input.action == "output"

## Worked examples

A workspace with no policy file is the documented default: everything is
allowed. To enable one, copy `<workspace>/.bsh/terminal-policy.rego.tpl` to
`<workspace>/.bsh/terminal-policy.rego` and edit it there. The template's three
worked examples - a destructive-input refusal, a secret redaction and a refused
shell - sit behind one `examples := false` switch over lists that start EMPTY,
so flipping the switch alone changes nothing until a list names what to refuse.

The modules below are the same rules, written out one at a time.

### Deny a destructive command list

    package terminal

    denied := ["rm -rf", "mkfs", "dd of=/dev/"]

    decision := {"allow": false, "reason": "destructive command"} if {
        input.action == "input"
        some needle in denied
        contains(input.command, needle)
    }

Everything the module does not mention falls back to the shipped `allow :=
true`, so `ls` still runs. The refusal's reason is shown in the pane.

### Deny input that names a path outside the workspace

    package terminal

    decision := {"allow": false, "reason": "path outside the workspace"} if {
        input.action == "input"
        some token in split(input.command, " ")
        startswith(token, "/")
    }

(A path check is only as good as the shell's own quoting and expansion; this
rule is a guard rail, not a sandbox. The start gate, and the permission mode
around the agent's tools, are the hard boundaries.)

### Redact a secret in output instead of dropping the chunk

    package terminal

    decision := {"allow": true, "redact": "[redacted]"} if {
        input.action == "output"
        regex.match("sk-[A-Za-z0-9]+", input.text)
    }

A redaction replaces the **whole chunk** with the replacement text. Escape
sequences in that chunk are lost with it, which is why redaction is meant for
short, secret-bearing output rather than for a screen's worth of drawing.

### Deny output entirely

    package terminal

    allow := false if { input.action == "output" }

Each refused chunk is dropped and one marker line ("Output blocked by policy")
is written in its place. That marker is bsh's own text and is never re-gated.

### The default (no file at all)

bsh ships this module; a missing policy file means it is the only one:

    package terminal

    default allow := true

    allow := false if { input.deny }

    verdict := answer if {
        answer := data.terminal.decision
    } else := allow

## Fail-closed, reload, and the audit

- A policy that **exists but does not parse** closes all three gates: start,
  input and output. An unusable rule must never widen the gate it was meant to
  narrow. On a first load the terminal reports the refusal and spawns nothing.
- A **broken reload keeps the last good policy**. The parse error is recorded
  in the audit ring and shown in the status line; a later edit that parses
  replaces the gate again. Deleting the file restores the allow-all default.
- Every decision is appended to a bounded ring of 256 rows
  (`Terminal::audit`), each carrying the decision time, the gate point, the
  subject (the command, the policy path, or the truncated chunk), whether it
  was allowed, whether it was redacted, the policy's reason, and whether the
  deciding rules came from the file or from the shipped default.
- The pane shows a compact count in its status line once at least one decision
  was refused or redacted. Nothing else is added to the window.

## What is deliberately not gated

Individual keystrokes (printable input, arrows, control chords, backspace),
cursor movement, resizing and scrollback browsing are not gated: they are line
editing and window management, not commands. Because the terminal echoes as you
type, the line already exists in the shell's line editor before Enter, so the
input gate sees the line this pane mirrored. A shell-side edit (history recall,
a mid-line cursor jump) can differ from that mirror. The start gate is the hard
boundary: it decides whether a shell exists at all.
