# The tool-boundary policy

Every file, shell and network action a turn performs is decided at the tool
boundary by one Rego policy set. This file is the contract a workspace author
writes against, with worked examples to copy.

The policy is a file, not a settings screen:

    <workspace>/.bsh/policy.rego

It is read when the turn is built (once per turn, never per tool call). A
missing file means the shipped baseline is the only policy. A file that exists
and does not parse puts the set in its **failure mode**: writes and network
egress are denied, and a read that resolves inside the workspace is allowed.

## One engine, one decision

The host compiles the baseline and the workspace file into **one** regorus
engine per turn, installs the data document once, and lowers each module once
to regorus bytecode. The bytecode VM is reused for every decision; the
interpreter remains the fallback if lowering fails. Regorus has no JIT, and no
policy is compiled on the tool-call path.

## The verdict

A workspace module declares a package and a `verdict` rule:

    package policy.workspace

    verdict := {"allow": false, "rule": "deny-env", "reason": "env files are secrets"} if {
        input.action == "read"
        contains(input.target.path, "/.env")
    } else := {"allow": true, "rule": "workspace-allow"}

`verdict` may be a boolean or an object. Object fields:

| field    | type   | meaning                                                    |
|----------|--------|------------------------------------------------------------|
| `allow`  | bool   | whether the action may proceed (missing reads as false)    |
| `rule`   | string | the id a refusal names; pair it with a `# rule: <id>` comment |
| `reason` | string | why; shown in the tool's refusal                          |

The baseline in `crates/agent/src/policy/baseline.rs` is the reference example.
It also shows the `# rule: <id>` convention: the loader extracts every comment
into `PolicySet::rule_ids`, and the verdict repeats the id that fired. A denial
without a `rule` still denies; it simply has no id to report.

## The input

Every decision gets one of these documents. The principal is the session, the
active preset and the tool; the target is the path or the resolved host.

    {
      "session": "<session id>",
      "preset": "<agent preset id, empty when none>",
      "tool": "write_file",
      "action": "read" | "write" | "egress" | "spawn" | "capability",
      "target": {
        "kind": "path",
        "path": "/workspace/src/main.rs"
      },
      "workspace_root": "/workspace"
    }

A network target carries both the name and the address the policy decides on:

    "target": {"kind": "host", "host": "example.com", "ip": "203.0.113.9"}

The unit is the **resolved IP**. The hostname is a hint. That is what defeats a
DNS rebind: the name can change, the address the policy approved cannot.

For a spawn, `target.path` is the working directory. For a write, it is the
file to be created or replaced; for an edit, the file to be changed.

A **capability** question is asked once per turn, while the agent is built, for
each tool the host can withhold. The tool name is the subject and the target is
the workspace root; the answer decides whether the tool is offered at all.

    {"session": "…", "preset": "…", "tool": "job_output",
     "action": "capability", "target": {"kind": "path", "path": "/workspace"}}

The gated names are `run_in_background` (the shell's detached-run flag),
`job_output`, `job_kill` and `subagent`. `bash` itself is governed by the
permission matrix and the preset's `tools` allowlist as well as by its spawn
check at call time.

## The data

The host installs one data document:

    {
      "workspace_root": "/workspace",
      "workspace_prefix": "/workspace/",
      "preset": "<agent preset id>",
      "permission": "workspace-write",
      "egress_allowlist": ["127.0.0.1", "10.0.0.0/8"]
    }

`permission` is the **effective** permission: the session's mode capped by the
preset's ceiling (`read-only` | `workspace-write` | `danger-full-access`).
The baseline's capability rule reads it, so a read-only turn refuses
`run_in_background`, `job_output`, `job_kill` and `subagent` in Rego as well
as in the host's registration matrix. `preset_permission` remains the preset's
own ceiling, which is what the generated read-only module reads.

`workspace_prefix` is the root with a trailing separator (a root of `/` stays
`/`), so containment is one comparison:

    startswith(concat("", [path, "/"]), data.workspace_prefix)

The egress allowlist is a file, not a settings screen:

    <workspace>/.bsh/egress-allowlist

One exact address or CIDR range per line; blank lines and `#` comments are
ignored. A missing file is an empty list, which denies all egress.

## Composition is intersection

The baseline and the workspace file must **both** allow. A workspace policy may
narrow the baseline, never widen it: saying `allow` for a path the baseline
denies still denies, because the baseline's `verdict` is evaluated too. Deny
wins; the first denying module in source order supplies the rule and reason.

The allowlist is the one widening control: a workspace policy cannot add an
address the allowlist does not name. Edit `.bsh/egress-allowlist`.

## Preset authority

A preset names the complete configuration a session runs. Every field is
optional; an absent field inherits the workspace/default behaviour, and an
explicit empty list mounts nothing:

    [presets.scoped]
    level = "docs"                    # inspect | review | research | docs | code | admin
    permission = "workspace-write"    # preset ceiling: read-only | workspace-write | danger-full-access
    read = ["."]                      # workspace-relative read roots; [] = none
    write = ["docs", "README.md"]     # workspace-relative write roots; [] = none
    deny = [".git", ".bsh", ".env", "secrets", ".ssh", ".aws"]
    egress = []                       # None inherits; [] denies; entries intersect
    web_search = false                # hosted web-search override
    plugins = ["files"]              # bundle names: shell, files, plan
    tools = ["read_file", "bash", "job_output", "job_kill", "subagent"]
    skills = ["alpha"]               # explicit skill allowlist
    mcps = ["github"]                # MCP server namespaces
    mcp_tools = ["github_search"]    # optional MCP tool allowlist
    extensions = ["dsh-echo"]        # installed-extension settings ids
    system_prompt = "You are terse." # the LLM persona for this preset
    builtin_policy = ["deny-secrets", "docs-only", "no-shell", "no-egress"]
    policy_mode = "required"         # required | advisory
    trusted = true                   # required for the custom policy below
    policy = """
    package policy.scoped
    # rule: no-secret-reads
    verdict := {"allow": false, "rule": "no-secret-reads"} if {
        input.action == "read"
        contains(input.target.path, "secret")
    } else := {"allow": true}
    """

- permission is a ceiling, not a grant. The effective permission is
  min(session permission, preset permission), so a read-only preset keeps a
  Full-access session read-only.
- read and write are workspace-relative roots. None inherits the workspace;
  an empty list allows nothing. deny always wins and is unioned with the
  baseline .git/.bsh rules.
- egress is a second allowlist. None inherits the workspace .bsh/egress-allowlist;
  an empty list denies all egress; entries are intersected with the workspace
  list. web_search overrides the hosted web-search setting.
- plugins gates the core bundles; a selected preset with an empty list mounts
  none.
- tools, skills, mcps, mcp_tools and extensions are allowlists. Absent means
  every tool/skill/server/tool/extension the build would otherwise carry; []
  means none. MCP entries are server namespaces (the name the settings entry
  or extension server registers under).
- The tool vocabulary is `bash`, `read_file`, `write_file`, `edit_file`,
  `list_dir`, `update_plan`, `job_output`, `job_kill` and `subagent`.
  `run_in_background` is the shell's own flag, so the `bash` entry gates it;
  it is also its own capability name for a Rego policy. A preset cannot use
  `tools` to widen the permission matrix: the allowlist is intersected with
  the effective permission, so `tools = ["subagent"]` under a read-only
  ceiling still registers no subagent tool.
- system_prompt replaces the workspace prompt for the chats that select the
  preset; absent keeps the workspace prompt. It is the persona the LLM call
  receives.
- builtin_policy names binary-shipped Rego modules and is trusted by
  construction. policy is the custom module. With policy_mode = required, an
  untrusted custom policy makes the preset unusable (the send gate refuses
  and the pane disables Use in this chat); advisory keeps the old
  ignore-with-a-warning behaviour.
- policy is intersected with the baseline, the built-in modules and the
  workspace policy. No source can widen another.

Shipped presets:

- standard: the default everyday coding mode; shell, files and plan, inherit
  the session permission and workspace policy, deny secrets and Git internals.
- ptc: all Standard capabilities plus parallel_tool_calls and a batch-call
  persona (filter, organize, deduplicate, count, summarize).
- creator: all Standard capabilities plus an extension-authoring persona; writes
  extension packages, skills and presets inside the workspace.
- read-only: read the workspace and search the web; no writes, shell, MCP or
  extensions.

PTC and Creator are bsh behaviours, not host-plane runtimes: PTC uses parallel
tool calls and a batch persona, Creator uses workspace writes and an
extension-authoring persona.

The Agent presets pane is DSH-shaped: a two-column Built-in grid, a Custom
section below, and a compact no-preset row above it. Each card shows its name,
Built-in/Default/In this chat tags, level, id and description; the footer has
Mode details, trust, Use in this chat and a document icon. Mode details shows
access, paths, network, parallel calls, bundles, tools, skills, MCPs,
extensions, persona and security. Use in this chat writes Session.agent_preset;
the next turn builds against it. The hero/header chip shows the applied preset.

A custom card has Edit and Delete. Edit opens a TOML editor for the [preset]
table inside the pane; Save preset writes it back to the settings draft.
Delete removes it from the draft and clears the global-default and session
references. Settings Save is still what persists either change. The dashed
Create a custom preset block adds a safe read-only template.

The Security row lists the active policy sources instead of claiming whether a
custom module exists: baseline, generated scope, generated egress, generated
permission, binary-shipped modules and the custom module. Every preset is
evaluated through the Rego substrate; a read-only ceiling with no built-in
read-only module generates a preset-permission source that denies writes. The
Rust permission cap and tool registration are defense in depth, not a
replacement for the policy.

Known limits of this slice:

- MCP HTTP connections are pool-level; a preset's egress list is enforced by
  the turn policy, not by reconnecting the shared pool per session. Presets
  that must be network-free disable MCP servers and the shell.
- The shell network path is not egress-gated. L0-L2 presets disable the shell;
  code is sandboxed for files but not for network.
- A preset write scope narrower than the workspace disables the shell rather
  than generating a custom Seatbelt profile. The code preset keeps the
  workspace-wide sandbox.

## Failure is asymmetric

An evaluator error, a timeout or a missing verdict:

- **denies** writes and network egress;
- **allows** a read that resolves inside the workspace root.

A broken policy can never silently widen a write or an egress.

## The shipped baseline

In plain English:

- **Reads** are allowed inside the workspace root and refused outside it.
- **Writes** are allowed under the root, except anywhere below a `.bsh` or
  `.git` directory. A write outside the root is refused.
- **Egress** is allowed only when the resolved address is in
  `data.egress_allowlist` (exact address or CIDR). Empty means deny all.
- **Spawn** is allowed for `bash` with the workspace root as its working
  directory, and for `job_kill` (stopping a job the session owns) and
  `subagent` (delegating to a child agent). A workspace policy may narrow
  those; running any other tool as a process is refused.
- **Capability** questions allow `run_in_background`, `job_output`,
  `job_kill` and `subagent` only while `data.permission` is not
  `read-only`.
- Anything else falls to a default refusal.

## Worked examples

### Deny reading env files

    package policy.workspace

    # rule: deny-env-reads
    verdict := {"allow": false, "rule": "deny-env-reads", "reason": "env files are secrets"} if {
        input.action == "read"
        contains(concat("", [input.target.path, "/"]), "/.env")
    } else := {"allow": true, "rule": "workspace-allow"}

The baseline still applies, so the `.env` file outside the workspace stays
denied and an ordinary read still passes.

### Deny egress to one address

    package policy.workspace

    # rule: deny-metadata
    verdict := {"allow": false, "rule": "deny-metadata", "reason": "the metadata address is not reachable"} if {
        input.action == "egress"
        input.target.ip == "169.254.169.254"
    } else := {"allow": true, "rule": "workspace-allow"}

The baseline's allowlist still has the final say: a workspace policy can only
remove addresses, never add one.

### Allow egress to an address

Add it to `.bsh/egress-allowlist`:

    203.0.113.9
    10.0.0.0/8

The next turn's build picks the file up.

### Refuse every shell spawn

    package policy.workspace

    # rule: no-shell
    verdict := {"allow": false, "rule": "no-shell", "reason": "this workspace does not run commands"} if {
        input.action == "spawn"
    } else := {"allow": true, "rule": "workspace-allow"}

## The policy pane

Settings -> Built-in plugins -> Policy (or --settings=policy) shows the last
turn's audit - allowed counts per action, denials with their rule, tool and
target, the evaluator backend, and a failure message when the set is in its
failure mode. Its Refresh control re-reads the live ring and its Clear control
empties it. The same pane edits the workspace egress allowlist: one address or
CIDR range per row, Add writes the file, Remove takes it out, and the next turn
picks the change up.

## Bash confinement

In **Workspace Write**, on macOS, the shell runs under a Seatbelt profile
(sandbox-exec): writes are allowed under the workspace, the temp directories
and the device sinks, and denied everywhere else, including the workspace's
.git and .bsh internals. The kernel enforces this per write syscall, so a path
reached through a shell variable or an interpreter is still blocked. A host
without /usr/bin/sandbox-exec falls back to the lexical pre-spawn scan, which is
a guard rail rather than a sandbox. **Full access** is not sandboxed. Tools that
insist on writing their own caches outside the workspace (for example ~/.cargo)
need Full access or a cache relocated into the workspace.

## What is not gated

- MCP **stdio** server spawns are visible to the policy as a spawn with tool
  mcp; the extension trust gate still decides which packages may contribute a
  server, and a workspace policy can deny the spawn. MCP HTTP requests are gated
  at resolution.
- Network from **inside bash**: the shell's child is not routed through the
  egress gate. The policy gates the tool's own MCP HTTP path, and the Seatbelt
  profile allows process and network by default, so a shell command can still
  reach the network when the egress allowlist is empty. A shell network gate is
  a later kernel-tier item.
- The kernel tier: no eBPF on macOS and no EndpointSecurity entitlement in this
  slice. `PolicySet::check` is the documented seam a syscall broker would call.
- Secrets: the audit records paths, hosts and rule ids; never environment
  values or file contents.
