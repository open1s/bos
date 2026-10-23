# The right-pane terminal across macOS, Linux and Windows

What the pane is, what each platform can do with it, and where the line is
between "the pane works" and "the sandbox can join it".

## The layers

| layer | where | portable? |
|---|---|---|
| cell emulator (VT parser, grid, scrollback, selection) | `bsh/src/terminal.rs` | yes: `vte` + pure Rust |
| pty + child | `portable-pty` | yes: `openpty`/login_tty on Unix, ConPTY on Windows |
| sandboxed shell | `agent::sandbox::SandboxSpec::spawn_terminal` | **macOS and Linux** (see below) |
| pane UI, keys, clipboard | `bsh/src/ui/right_panel.rs` | key mapping is platform-neutral: Cmd-C on macOS, Ctrl-Shift-C elsewhere |
| shell discovery | `terminal::shell_program_for` | per-platform table (zsh/bash/sh, pwsh/powershell/cmd) |

## Shell discovery

`shell_program_for(confined)` reads `$SHELL`, then `$COMSPEC` (Windows), and
falls back to a per-platform candidate list:

- macOS: `/bin/zsh`, `/bin/bash`, `/bin/sh`
- Linux: `/bin/bash`, `/bin/zsh`, `/bin/sh`
- Windows: `pwsh.exe`, `powershell.exe`, `cmd.exe` (resolved on PATH; a bare
  name is left for CreateProcess, never a Unix path)

A confined shell cannot be given a controlling terminal, and fish refuses to be
interactive without one (measured: "No TTY for interactive shell (tcgetpgrp
failed)"). A confined pane therefore replaces fish with zsh and prints one line
saying so. Linux and Windows panes are never confined, so the reader's own shell
is kept.

## The sandbox and the pty

heel is a required dependency: there is one sandbox implementation, with no
feature flag or in-tree fallback. `SandboxSpec::terminal_for(platform)` is the
table:

| platform | shell | why |
|---|---|---|
| macOS | confined, interactive | heel's SBPL profile can grant the pane's tty device (`allow_tty_write`), and the shell opens it by name |
| Linux | confined, interactive | heel's Landlock ruleset grants `/dev/tty`, `/dev/ptmx` and `/dev/pts` every filesystem right when `allow_tty_write` is set, so the same by-path door opens |
| Windows | interactive, not confined | heel launches through `CreateProcessW` with its own stdio pipes; a ConPTY is attached as a process attribute at creation, not a handle the sandbox could be given |

The Linux claim is heel's own code, quoted from
`src/platform/linux/landlock_rules.rs` (heel 0.5.1):

    let tty_access = if config.allow_tty_write() {
        AccessFs::from_all(abi)
    } else {
        AccessFs::from_read(abi)
    };
    for device in ["/dev/tty", "/dev/ptmx", "/dev/pts"] {
        add_system_path(ruleset, device, tty_access, abi)?;
    }

so `SandboxSpec::spawn_terminal` needs no Linux-specific branch: it starts
`/bin/sh -c 'exec "$0" -i </dev/pts/N >/dev/pts/N 2>/dev/pts/N'` and the
Landlock rule above is what lets the open succeed. The seccomp filter is
default-allow with explicit blocks and does not touch `ioctl`.

On every platform the pane itself works: it spawns its shell through
`portable-pty`, which is the only path that performs the terminal handshake.
Where the sandbox cannot join, the pane says so: the status line carries the
reason and the shell sees `BSH_SANDBOX=unsupported-pty`.

The sandboxed spawn path is POSIX by construction (a shell redirects its own
streams onto a device node). On Windows `SandboxSpec::spawn_terminal` returns a
typed refusal instead of running a `/bin/sh` that does not exist, and the pane
falls back to its own spawn because `tty_path` has no device to offer.

## What would make Windows sandboxed

heel's Windows launcher (quoted from `src/platform/windows/launch.rs`, heel
0.5.1) always builds its own pipes and never sets a pseudoconsole attribute:

    startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child[0];
    startup.StartupInfo.hStdOutput = child[1];
    startup.StartupInfo.hStdError = child[2];

    let mut attributes = AttributeList::new(2 + u32::from(!inherited.is_empty()))?;
    attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, ..)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, ..)?;

A ConPTY is attached with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` at process
creation and has no handle to inherit, so no caller-supplied console can be
joined. Two shapes would close it:

- heel accepting a pseudoconsole handle (or a pre-built attribute list) in its
  spawn request, or
- a helper process that owns the ConPTY inside the sandbox (created by heel with
  pipes) and relays bytes for the pane — the same trick one layer up.

Until then the honest state is: the shell tool is confined on all three
platforms, the terminal is confined on macOS and Linux, and the terminal is
functional (unconfined) on Windows.

## Known behaviour of a confined pane

- The shell has no controlling terminal, because only portable-pty's own spawn
  performs `login_tty`. bash therefore prints "cannot set terminal process
  group (-1): Inappropriate ioctl for device" and "no job control in this
  shell"; zsh does not. fish refuses to be interactive at all, so a confined
  pane replaces it and says which shell it used.
- `$HOME` is denied (heel's `protect_user_home` is on by default), so the shell
  starts without the reader's rc files. That is the sandbox's intent — reads are
  opt-in — and a confined zsh gets a minimal prompt and colour setup from the
  pane instead (see the last bullet).
- Tab stays in the pane. macOS hands an unbound Tab to AppKit's key-view loop,
  so focus would leave the terminal before the shell saw the byte; the pane
  carries the `terminal` key context and binds Tab there to an action that
  forwards it exactly as a keystroke. The shell's completion is bound to the
  pane's focus, not to the window's focus order.
- Ctrl-C is delivered by the pane, not only by the tty driver. The shell the
  sandbox starts opens the pane's tty by path and never performs `login_tty`,
  so the tty can end up with no foreground process group and the driver's
  `ISIG` signal has nowhere to go. `Terminal::send_key` asks the tty for its
  foreground group (falling back to the shell's own group) and signals it
  outright, never the application's group. The plain terminal spawn also
  starts the shell as a process-group leader; the heel backend's spawn builder
  exposes no process-group hook, so a confined shell's group is heel's choice
  there.
- A confined **zsh** gets the completion setup it would normally receive from
  the reader's zshrc. BSH writes one fixed bootstrap command to the PTY, loads
  the system's completion functions with `compinit -C -D` (no security audit,
  no dump), keeps completion to the two cheap passes, points history at
  `/dev/null`, and sets a coloured prompt; `CLICOLOR` and `LSCOLORS` ride in
  the pane's environment instead, so `ls` (and `git`, whose `color.ui` defaults
  to auto) draw colour even though no rc file runs. The shell echoes that one command at its first prompt; it does
  not read the reader's rc files, does not widen the sandbox's read grants, and
  writes no state. bash and sh keep their own line-editor completion defaults.

## Verification

- The per-platform decisions (`TerminalPlatform`, `shell_choice`) are pure
  functions with a platform parameter, so their tables are tested for all three
  platforms from any one of them (`the_shell_decision_table_covers_all_three_platforms`,
  `the_terminal_plan_is_a_per_platform_table`).
- The pane's own spawn (portable-pty) is covered by the interactive tests:
  echo, resize, exit, restart and a workspace write, plus the sandboxed variant
  on macOS. `a_confined_zsh_completes_a_shell_argument` drives a real confined
  zsh and proves that Tab completes a shell argument; the bootstrap command
  stays out of the reader's view.
- The sandboxed POSIX path is compiled out on non-Unix; on Windows the pane
  takes its own ConPTY spawn.
- Linux uses the same code path as macOS; what differs is the backend's rules,
  and those are heel's (quoted above). Run `cargo test -p agent -p bsh --lib`
  on a Linux host to exercise it (kernel 6.7+ for Landlock ABI v4).
