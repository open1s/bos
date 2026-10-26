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
      "sandbox_enabled": true,
      "sandbox_confined": true,
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

Two keys describe the shell's confinement, and they answer different questions:

- `sandbox_enabled` is the **settings master switch**: true while the reader
  left it on, false once they turned it off. It says nothing about the
  permission tier.
- `sandbox_confined` is the **resolved answer**: true only while heel actually
  confines this turn. It is false with the switch off AND at Full access (where
  the permission releases the shell with the switch still on), and true for a
  preset's `sandbox = "heel"` even at Full access.

**Yes - an unconfined shell can be DENIED by policy.** Read
`data.sandbox_confined`; the spawn question is still asked when no sandbox
exists, so a workspace that requires confinement refuses the call instead of
letting it run unconfined:

    package policy.workspace

    # rule: require-sandbox
    verdict := {"allow": false, "rule": "require-sandbox", "reason": "this workspace requires the sandbox"} if {
        not data.sandbox_confined
        input.action == "spawn"
    } else := {"allow": true, "rule": "workspace-allow"}

The refusal happens before a process exists, so the shell never starts. This one
rule covers every unconfined arm, including the full-access release a
switch-only rule would miss, and it does not refuse a preset-forced heel. The
interactive terminal is a separate seam with its own
`.bsh/terminal-policy.rego` gate; a workspace that wants both denied adds a
`start` refusal there too.

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

The shell tool's result reports which of these applies as `network`. There are
FOUR values, because "no sandbox" is not a policy state:

| value | when | what it means |
|---|---|---|
| `deny-all` | a present-but-empty workspace file, or a preset's `egress = []` | the policy denies every destination |
| `allowlist` | a present file with rules, or a preset list | the reach is what the rules name |
| `unrestricted` | a CONFINED heel shell and no egress file | the proxy runs heel's allow-all policy; ICMP and host-local destinations are still refused by the kernel |
| `unconfined` | no sandbox proxy existed (the settings switch is off, a preset's `sandbox = "none"`, or the full-access permission release) | the workspace `.bsh/egress-allowlist` was **not consulted**; no allowlist answer may be inferred |

`unconfined` is deliberately distinct from `unrestricted`. Reporting the first
as the second let a reader conclude the allowlist had been read and passed, when
nothing enforced it. It is not "no policy at all" either: the command-tier scan
still refuses an endpoint the command line literally names before the process
exists. The audit detail localizes `unconfined` including that caveat (English
and Chinese), the Markdown export carries it in its confinement column, and the
JSON export keeps the stored value verbatim.

**`unrestricted` is a policy state, not a pass-through.** The generated profile
still denies every direct socket and grants only the localhost proxy, so a client
that ignores the proxy environment - ICMP/`ping`, raw sockets, a TCP client with
no proxy support - fails with the kernel's EPERM/EACCES even though the policy
allows the network. Proxy-aware clients (`curl`, `git`, `npm`) work: they use
the proxy heel points them at. To tell the two apart, the result carries
`network_diagnostic: "unproxied-direct-socket"` on exactly that failure (a
non-zero exit, a confined heel shell, `network == "unrestricted"`, and a network
syscall beside EPERM/EACCES). A run reported as `unconfined` never gets this
diagnostic: there is no sandbox profile to refuse its direct socket, so a
failure there is the command's own. The shell renders that id through
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
  load error naming `sandbox`, never a silent default. The settings master
  switch (`sandbox_enabled`, see below) OUTRANKS this field: with it off, heel
  is not applied for any preset, and a `heel` declaration is shown as
  overridden on its card rather than silently ignored.
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
- The tool vocabulary is `bash`, `glob`, `grep`, `read_file`, `write_file`,
  `edit_file`, `list_dir`, `update_plan`, `job_output`, `job_kill` and
  `subagent`.
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

TWO controls can leave the shell and the terminal unconfined: the settings
master switch (`sandbox_enabled`, absent means on) and a preset's
`sandbox = "none"`. Both are honest about their cost, because the cost is real:

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
And every unconfined bash result is attributed to exactly one of THREE causes:
the settings master switch (`the sandbox is turned off in settings`), a preset's
choice (`a preset turned the sandbox off`), or the permission's own release
(`the full-access permission released the sandbox`). For either policy release
the audit detail, the terminal status line (and its `BSH_SANDBOX=none`) and the
transcript's command note all show the consequence and which policy did it; a
permission release is attributed in the audit detail and the raw result,
because it is the ordinary full-access state rather than a policy choice.

### The settings master switch, and the three reasons

The settings file carries `sandbox_enabled` (General -> Use the sandbox). It is
absent when the sandbox applies and written as `false` only when it is off, so an
enabled file mirrors a fresh one and a restart restores the reader's choice.

The switch turns HEEL off for the shell and the terminal, for EVERY preset. It
does not turn presets off: nothing about the preset machinery is inert, and the
shell's unconfined state is the only thing that changes.

It WINS over a preset's `sandbox = "heel"`: a preset may keep declaring heel and
the declaration is simply not applied. A preset's `sandbox = "none"` is
unaffected, because it was already unconfined. At full access the switch changes
nothing, so it does not claim the release.

The precedence table is the whole rule:

| settings switch | preset `sandbox` | effective permission | heel applied | attributed to |
|---|---|---|---|---|
| on / absent | absent | read-only or workspace-write | yes | - |
| on / absent | `heel` | any | yes | - |
| on / absent | `none` | full access, preset ceiling full access | no | the preset |
| on / absent | absent | full access | no | the permission release |
| off | absent | read-only or workspace-write | no | the settings switch |
| off | `heel` | any, including full access | no; the declaration is overridden | the settings switch |
| off | `none` | full access, preset ceiling full access | no | the preset (already unconfined) |
| off | absent | full access | no | the permission release (the switch changed nothing) |

Every unconfined shell result carries exactly ONE of three reasons, and the
three name three different controls:

| reason | when | reversed by |
|---|---|---|
| `the sandbox is turned off in settings` | the master switch is off and heel would otherwise have applied | General -> Use the sandbox |
| `a preset turned the sandbox off` | the preset declares `sandbox = "none"` at full access | selecting another preset |
| `the full-access permission released the sandbox` | the session is full access and no preset asks for heel | lowering the permission mode |

The same three are distinguishable in the audit record and its JSON export
(the stored reason is the constant, and the row localizes it), and the
transcript's command note names which one applies.

The switch overrides the ARM, never the widening GUARD. A preset declaring
`sandbox = "none"` under a ceiling narrower than full access is still refused
with `preset-sandbox-widens` while the switch is off: the rule is a statement
about the declaration (and about the preset being unusable), and letting the
switch make it sendable would silently change what a declaration means.

Turning the switch OFF is a widening, so it takes the same confirmation the
other widenings take. The sentence names both losses - no kernel filesystem
confinement, and no sandbox egress proxy or allowlist for shell and terminal
commands - and, when any preset on the roster declares `heel`, that those
declarations are being overridden. Cancel changes nothing; Confirm saves and
rebakes the live policy in the same frame. An affected preset's card states the
override, the terminal status line says the same while its shell runs, and each
tool result and audit row names it for the call that saw it - the shell itself
reports `BSH_SANDBOX=none` from a note passed BEFORE the spawn, so the process
and the status line cannot disagree. The composer's permission menu deliberately
does not carry the state: its three rows are the mode label and a short
qualifier, so the sandbox copy stays on those surfaces rather than becoming a
standing note in the menu.

A spawn spec is SNAPSHOTTED. A turn that is already streaming and a terminal
that is already open keep the spec they were spawned with: neither turning the
switch off nor raising the session to Full access rewrites a running process's
confinement. The terminal status line says so explicitly while the pane's
spawn-time confinement class differs from the one a terminal opened now would
get - a policy release (the switch or a preset) OR a permission release ("this
terminal keeps the sandbox it started with; a new terminal uses the current
setting") - and the switch's own notice appends that the running turn keeps its
current build. A terminal opened afterwards resolves the spec again.

What still applies while the switch is off (this is why it is not a second
"presets off"):

- the preset resolves and applies its permission ceiling, read/write/deny scopes,
  bundles, tools, skills, MCP servers, extensions, persona and policy modules;
- the Rego tool-boundary policy still refuses the paths and endpoints a command
  line names, for every tool; the file tools keep their own permission tier and
  their own checks;
- the browser's own loopback gate is untouched, because it is not heel.

What no longer applies: heel's kernel filesystem confinement and heel's egress
proxy with its allowlist, for the shell and the terminal. A command can write
outside the workspace and can reach a host the allowlist does not name - the
cost the confirmation states.

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
