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
      "egress_allowlist": ["127.0.0.1", "10.0.0.0/8"],
      "egress_hosts": [],
      "egress_ports": [],
      "egress_unrestricted": false
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

The egress rules are a file, not a settings screen:

    <workspace>/.bsh/egress-allowlist

One rule per line; blank lines and `#` comments are ignored. A line is
classified by what it can be:

- a bare integer is a destination **port** (`443`). No port lines means any
  port; one or more means only those, and a request whose port is unknown is
  refused rather than waved through;
- something that parses as an address or a CIDR range is an **address** rule
  (`203.0.113.7`, `10.0.0.0/8`). This is the strong door: the decision is made
  on the resolved address, so a DNS rebind cannot ride it;
- anything else is a **hostname** rule (`api.example.com`, `*.crates.io`,
  lower-cased). This is the weaker door - the name is what the policy saw, so a
  rebind survives it - and it exists for a policy that names a service rather
  than an address.

### The file is tri-state: absent, empty, or rules

The file's own existence is a policy input, not a detail:

| state | data | reach |
|---|---|---|
| **ABSENT** - no file | `egress_unrestricted = true` | **UNRESTRICTED**: any address, hostname and port. This is the fresh-workspace state; the network works out of the box, and Standard/PTC inherit it. |
| **PRESENT, no rule** - only comments/blanks | all lists empty, `egress_unrestricted = false` | **DENY ALL**: the reader created the file and left it empty, which is a deliberate lockdown. |
| **PRESENT with rules** | the classified lists | **ALLOWLIST**: exactly the doors the rules name. |

Creating the file therefore OPTS INTO CONTROL. Nothing seeds it: registering or
opening a workspace writes nothing, because auto-writing the starter would flip
a fresh, unrestricted workspace into a deny-all its owner never chose. The
Policy pane offers **Create starter allowlist** as an explicit action, and its
copy states that creating the file and leaving it empty denies all egress.
Deleting the file restores the unrestricted default.

Allow-all has no hostname spelling (a bare `*` is a hostname rule that matches
no host); the honest equivalent is an address rule covering every address,
`0.0.0.0/0` and/or `::/0`, which the starter documents as a commented example
beside a worked allow example.

The security consequence is stated rather than implied: a fresh workspace is
permissive, and the reader opts into control by creating the file. A workspace
that wants the old deny-by-default floor creates the file and leaves it empty
(or writes `#` comments); a workspace that wants one destination writes that
rule.

Every network path is decided by the policy, at two points:

1. **Before a process exists.** The shell tool reads the endpoints a command
   line literally names (`curl`, `wget`, `ssh`, `scp`, `sftp`, `rsync`, `nc`,
   `ping`, and `git` with a fetching subcommand), resolves each name, and asks
   the policy about every resolved address. A refusal names its rule and no
   process runs.
2. **On every connection.** With heel on, the shell runs under heel's proxy,
   which asks the same policy per request with the host, the resolved address
   AND the port. An ABSENT file leaves the reach unrestricted, so the proxy runs
   heel's allow-all policy (heel 0.5 has no pass-through mode, so a proxy still
   exists and the kernel still refuses ICMP and host-local destinations); a
   PRESENT file runs the policy-backed proxy; a present-but-empty file starts
   no proxy at all and the kernel refuses outbound traffic outright.

The shell tool's result reports which of these applies as `network`:
`unrestricted` (no file), `deny-all` (present-but-empty, or a preset's `[]`),
or `allowlist` (rules constrain the reach).

**`unrestricted` is a policy state, not a pass-through.** The generated profile
still denies every direct socket and grants only the localhost proxy, so a client
that ignores the proxy environment - ICMP/`ping`, raw sockets, a TCP client with
no proxy support - fails with the kernel's EPERM/EACCES even though the policy
allows the network. Proxy-aware clients (`curl`, `git`, `npm`) work: they use
the proxy heel points them at. To tell the two apart, the result carries
`network_diagnostic: "unproxied-direct-socket"` on exactly that failure (a
non-zero exit, a confined heel shell, `network == "unrestricted"`, and a network
syscall beside EPERM/EACCES). The shell renders that id through
`tool.bash.unproxiedNetwork` (English and Chinese) in both the transcript step
body and the audit row, so the reader learns the kernel refused a path the
sandbox does not proxy - not that the policy refused the command.

## Composition is intersection

The baseline and the workspace file must **both** allow. A workspace policy may
narrow the baseline, never widen it: saying `allow` for a path the baseline
denies still denies, because the baseline's `verdict` is evaluated too. Deny
wins; the first denying module in source order supplies the rule and reason.

The egress file is where the workspace declares its reach: an ABSENT file
leaves it open, and a PRESENT file constrains it to what it names. Once the file
exists, a workspace policy cannot add an address, host or port the file does not
name; edit `.bsh/egress-allowlist` to change the reach.

## Preset authority

A preset names the complete configuration a session runs. Every field is
optional; an absent field inherits the workspace/default behaviour, and an
explicit empty list mounts nothing:

    [presets.scoped]
    level = "docs"                    # inspect | review | research | docs | code | admin
    permission = "workspace-write"    # preset ceiling: read-only | workspace-write | danger-full-access
    sandbox = "heel"                  # heel | none; absent follows the permission
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
- sandbox is the shell confinement toggle, and it is a per-preset POLICY choice,
  not a build flag: heel is still the one sandbox implementation and a required
  dependency. Absent inherits today's behaviour (heel whenever the effective
  permission confines). `heel` is the narrowing arm and confines even a
  full-access session. `none` is the widening arm and is permitted only when
  the preset declares `permission = "danger-full-access"` AND the session is
  full access; under any narrower ceiling the preset is REFUSED with rule
  `preset-sandbox-widens` and the shell stays confined. A malformed value is a
  load error naming `sandbox`, never a silent default.
- read and write are workspace-relative roots. None inherits the workspace;
  an empty list allows nothing. deny always wins and is unioned with the
  baseline .git/.bsh rules.
- egress is a second allowlist of ADDRESSES. None inherits the workspace
  .bsh/egress-allowlist - which is UNRESTRICTED when that file is absent, so an
  inherited reach can be open; an empty list denies all egress even then;
  entries are intersected with the workspace list, never unioned. Because the
  preset's list is address based, a hostname rule in the workspace file is only
  reachable while the preset's egress scope is absent (unrestricted).
  web_search overrides the hosted web-search setting.
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
- The shell network path is gated by heel: a derived allowlist runs a proxy
  that asks the policy per connection, a present-but-empty file derives no proxy
  (the kernel denies), and an absent file runs heel's allow-all policy. A
  command endpoint named literally is also refused at the command tier before a
  process exists.
- A preset write scope narrower than the workspace disables the shell rather
  than generating a custom profile. The code preset keeps the workspace-wide
  sandbox.

### What turning the sandbox off means

`sandbox = "none"` is honest about its cost, because the cost is real:

- **No filesystem confinement.** The kernel boundary heel installs is gone, so
  a shell command can write outside the workspace (a path assembled in a
  variable, an interpreter, a build tool). The tool-boundary policy still
  checks the write targets the command line literally names, and a policy
  denial still refuses the call before a process exists; a target the tool
  cannot see is not checked.
- **No egress enforcement for shell commands.** The workspace
  `.bsh/egress-allowlist` is enforced by heel's proxy, which asks the Rego
  policy per connection. No heel means no proxy and no per-connection check, so
  a command can reach a host the file does not name. The command-tier scan still
  refuses endpoints the command line literally names; a name or address built
  at runtime is not seen.
- **The tool-boundary policy still applies.** `Read`, `write`, `egress` and
  `spawn` decisions are still made for what the host can see; turning the
  sandbox off removes the kernel tier, not the policy tier.
- **The toggle governs the shell and the terminal, not the file tools.** The
  file tools keep their own permission tier and their own Rego checks; `none`
  does not release them, and `heel` does not add a second confinement to them.

The toggle therefore exists to let a deliberately full-access preset say
"unconfined" out loud, never to let a narrower preset escape its ceiling. The
guard is the reason the field cannot widen: `none` under a read-only or
workspace-write ceiling (or with no declared ceiling) is refused with
`preset-sandbox-widens`; the selection is refused, the turn is not sendable,
and the shell stays under heel. If a build is reached another way, the preset's
other limits still apply and the sandbox resolves to heel, so the refusal can
never leave a quietly unconfined shell.

Two readings are worth stating explicitly. On a session that is already full
access, `none` is the explicit spelling of today's release and changes
nothing; the behavioural arm is `heel`, which narrows a full-access session.
And every unconfined bash result is attributed: a preset's choice records the
reason `a preset turned the sandbox off`, while a permission-released run
records `the full-access permission released the sandbox`. For a preset's off,
the audit detail, the terminal status line (and its `BSH_SANDBOX=none`) and the
transcript's command note all show the consequence; a permission release is
attributed in the audit detail and the raw result, because it is the ordinary
full-access state rather than a policy choice.

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
- **Egress** is tri-state. An absent allowlist file
  (`data.egress_unrestricted`) allows any address, hostname and port. A present
  file allows only what it names: the resolved address in
  `data.egress_allowlist` (exact address or CIDR) or a hostname in
  `data.egress_hosts`, at a port in `data.egress_ports` when that list is
  non-empty. A present file that declares no rule denies all egress.
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
picks the change up. The pane distinguishes the two empty states in words: with
no file, egress is UNRESTRICTED and it offers **Create starter allowlist**
(writes the commented template); with a file that declares no rule, egress is
deny-all and it says that uncommenting a line allows one, or deleting the file
restores the unrestricted default. Creating the starter therefore opts into
control: it declares no rule, so it denies all egress. **Open file** reveals the
path in the platform browser (a missing file reveals its directory).

## Bash confinement

heel is the one sandbox implementation and a required dependency, on macOS,
Linux and Windows. In **Workspace Write**, the shell runs under it with the
grants the policy derived: writes are allowed under the workspace and the paths
the policy allows, and denied everywhere else. Writes are not executable except
in a policy-granted build cache, so a payload dropped into the workspace cannot
be run. The kernel enforces this per write syscall, so a path reached through a
shell variable or an interpreter is still blocked.

heel has no deny rule inside a granted tree: a write to the workspace's .git or
.bsh reached through a shell variable is refused by the tool-boundary policy
(which names the rule) rather than by the kernel, and a command line that names
the path is refused before it spawns. **Full access** is not sandboxed by
default. A preset can still narrow it with `sandbox = "heel"`, which builds the
Workspace Write grants even though the session permission is full access; and
`sandbox = "none"` can only widen where the preset and the session are both
full access. Tools that insist on writing their own caches outside the
workspace (for example ~/.cargo) need Full access or a cache relocated into the
workspace.

## What is not gated

- MCP **stdio** server spawns are visible to the policy as a spawn with tool
  mcp; the extension trust gate still decides which packages may contribute a
  server, and a workspace policy can deny the spawn. MCP HTTP requests are gated
  at resolution.
- Network from **inside bash**: the shell's child is routed through heel's
  egress proxy. A derived allowlist asks the policy per connection; a
  present-but-empty file starts no proxy and the kernel denies outbound traffic;
  an absent file runs heel's allow-all policy, which still refuses ICMP and
  host-local destinations (heel has no pass-through network mode). A command
  that resolves a name itself can still reach an address through a hostname
  rule's rebind window; address rules are the strong door.
- The kernel tier: no eBPF on macOS and no EndpointSecurity entitlement in this
  slice. `PolicySet::check` is the documented seam a syscall broker would call.
- Secrets: the audit records paths, hosts and rule ids; never environment
  values or file contents.
