# BSH Federation

Status: implemented foundation. The node runs by default; every grant and every
remote tool is opt-in. This guide describes what
a BSH node does today, how to run one, and where the boundaries are. The design
record is [bsh-2.0-architecture-rfc.md](bsh-2.0-architecture-rfc.md) (ADR-008 and
ADR-009); the source-level analysis is
[bsh-2.0-adapter-research.md](bsh-2.0-adapter-research.md).

## What federation is

A BSH node is one running app with its own identity, policies, secrets, and
audit record. Federation lets two or more nodes use each other's capabilities
without merging those boundaries:

- a **capability** is a versioned, described unit of work;
- a **binding** says where it runs: here, or on a named peer;
- a **grant** says which capability a peer may invoke;
- a **delegation** is a bounded task handed to another node;
- a **lease** is a time- and byte-bounded grant to move resource bytes.

Every remote call is authorized twice: by the caller's kernel and again by the
provider's kernel. Neither decision substitutes for the other, and each denial
names which side refused.

## Identity

A node's identity is an Ed25519 key. Its `NodeId` is the base64 public key, so
it is not a hostname, a display name, or a transport address: it survives a
machine rename or a transport change.

```sh
bsh federation status
# node 1UIH2hlJd9z0atv+wrwudbUtWopCGE/t/cAAJPDj6No=
# identity /Users/me/.bos/bsh/federation/node.key
# peers 0
```

The seed file is created on first use, readable by its owner only on Unix. It
*is* the identity: losing it loses every grant the peers recorded for you.

## Managing peers

Management is headless, so it works before any window exists and on a machine
with no GUI:

```sh
bsh federation enroll <peer-node-id>
bsh federation grant <peer-node-id> builtin/read_file
bsh federation revoke-grant <peer-node-id> builtin/read_file
bsh federation revoke <peer-node-id>
```

`enroll` trusts the identity; it grants nothing. `grant` names one capability.
`revoke` drops the peer and every grant it held. Enrollments and grants are
persisted in `peers.json` beside the identity with an atomic write.

A peer id is the value the other node prints as `node ...` in its own
`bsh federation status`.

## Running a node

The node starts at launch, on its own thread, and keeps running for the life of
the process. It is started beside the window rather than before it, so opening a
Zenoh session never delays the first frame; the right panel's Refresh re-reads
the node after it is up. Set `enabled = false` to run the app without one; a fresh install
binds Zenoh's default peer endpoint, but nothing is shared until a peer is
enrolled and granted. A settings file written by an earlier version that already
contains `enabled = false` stays off: the written value is the reader's choice,
so set it to `true` (or delete the line) to turn the node back on.

```toml
[federation]
enabled = true
# Omit the paths to use ~/.bos/bsh/federation/{node.key,peers.json}.
# identity_path = "/path/to/node.key"
# peers_path = "/path/to/peers.json"

# Zenoh endpoints: where to reach peers, and where to accept them.
endpoints = ["tcp/192.168.1.10:7447"]
listen = ["tcp/0.0.0.0:7447"]
mode = "peer"

# Offer trusted peers' capabilities to the model as tools.
include_remote_tools = true

# Transport TLS: the same identity is presented in both directions.
tls_ca = "/etc/bsh/ca.pem"
tls_cert = "/etc/bsh/node.pem"
tls_key = "/etc/bsh/node-key.pem"
tls_mtls = true            # require a client certificate from peers
tls_verify_name = true     # default; set false only for lab endpoints
```

A node that asks for TLS and cannot use it does not start: the generated Zenoh
document is validated, a listener that cannot read its material is an error, and
the failure is logged like any other startup failure. It never falls back to an
unencrypted session.

A node that cannot start is logged and the app runs without federation: a
broken peer configuration must never keep the window from opening.

## Ingress bounds

The transport buffers at most 1024 inbound frames (and 256 announcements)
ahead of the router. A full queue drops the frame and logs the first drop and
every 1024th after it: a peer that sends faster than the node routes cannot grow
the process without bound. Announcements are periodic presence, so a dropped one
is replaced by the next tick; a dropped invoke fails like any unreachable frame.

## How a node is named

A node shows a display name instead of the first ten characters of its key. The
default is the machine's host name (`devbook.local` becomes `devbook`); set your
own with the **Node name** field in the Federation pane or in settings:

```toml
[federation]
node_name = "studio-mac"
```

`bsh federation status` prints the same name above the node id. The name travels inside the signed discovery record, so a peer displays the name
the node itself asserted, and a tampered name fails verification like any other
signed field. The pane's status line, the peer rows and the revoke/enroll notices
all use the same name; a peer enrolled before it had a name keeps showing its
short id until it is enrolled again.

## Several nodes on one host

Each node needs its own data directory and its own transport endpoint. The data
directory lock is per directory, so two nodes that share one are refused by
design; a per-node `HOME` gives every node its own settings, identity, peer
store, lock, and audit log. Give each node a distinct listen port and point the
others at it:

```toml
# Instance A — ~/bsh-a/.bos/bsh/settings.toml, launched with HOME=~/bsh-a
[federation]
enabled = true
listen = ["tcp/127.0.0.1:7447"]
```

```toml
# Instance B — HOME=~/bsh-b
[federation]
enabled = true
listen = ["tcp/127.0.0.1:7448"]
endpoints = ["tcp/127.0.0.1:7447"]
```

Both nodes publish on the shared announce key (`bsh/v1/announce`) and, once
their transports are connected, each lists the other under **Discovered peers**
in the right panel, excluding itself. The list shows the capabilities the peer
advertises; select the ones to grant and press **Enroll & grant**. No enrollment
is implied by being discovered: a discovered peer cannot invoke anything until
both sides hold a grant.

Enrolling and granting are menus, not free-text fields: **Enroll a peer** lists
the nodes discovery heard (the manual node-id field below it remains for a peer
on another network), **Grant a capability** lists what the selected peer
announced, and expiry is picked from **No expiry / 1 hour / 24 hours / 7 days /
30 days**. A capability id can still be typed when a peer holds a granted-only
capability it did not announce.

A node advertises presence plus the read-only capabilities configured under
`[federation] expose`, confined to `expose_root`:

```toml
[federation]
enabled = true
# Empty (the default) advertises and serves nothing.
expose = ["builtin/read_file", "builtin/read_files", "builtin/glob", "builtin/grep"]
expose_root = "/srv/shared-notes"
```

The same choice is in the Federation pane: **Exposed capabilities** carries one
checkbox per read-only tool and a root field, and writes these settings directly.
Supported ids are the four read-only file tools; any other id is refused when
the node starts and the refusal is logged, so a typo cannot quietly expose the
shell. An exposed capability is announced as a Public descriptor and is served
from a read-only workspace confined to `expose_root`. Being discovered grants
nothing: a peer must be enrolled and granted on both sides before it can call.
Changing the set applies when the node restarts (the pane says so).

The `bsh federation` CLI uses the same per-node directory, so run it with the
same `HOME` (or set `identity_path`/`peers_path` in that node's settings).

## What a running node does

1. It opens the configured Zenoh session and subscribes to its inbox
   (`bsh/v1/node/<hex-node-key>/inbox`).
2. It discovers every enrolled peer's published capabilities (signed record,
   public descriptors only) at startup and every 30 seconds.
3. It answers `invoke`, `delegate`, `delegation_status`,
   `delegation_cancel`, `discover`, and `resource` messages from enrolled
   peers, authorizing each against its own peer store.
4. With `include_remote_tools`, each turn's build attaches discovered remote
   capabilities to the agent as tools. A name the local registry already holds
   is skipped: the local tool wins.

`bsh federation status` prints `discovered_peers` and the composed
`capabilities` / `remote_capabilities` counts when the app is running; the
headless command reads the files, so it reports identity and grants only.

## Right panel

The right panel's **Federation** tab is the management surface: it lists enrolled
peers with a shortened id, their trust state, and their grants, and offers the
four actions the store supports: enroll a peer, grant a capability to the
selected peer (with an optional expiry in minutes), revoke one grant, and revoke
a peer. Revoking a peer takes two presses: the first asks, the second acts, and
Cancel leaves the store untouched. The status line carries **Start** or **Stop** for the node itself. Start opens
the transport on a worker thread, like the launch path, so a frame never waits
for Zenoh; Stop aborts the router and discovery tasks and releases the data
directory lock immediately (the peer store is on disk, so nothing is lost). The
`enabled` switch still decides whether the node starts at launch.

Two switches sit at the top: **Run a federation node** (the `enabled` setting;
takes effect at the next launch) and **Offer peers' capabilities to the model**
(`include_remote_tools`; takes effect at the next turn, and the node must be
running). Both persist through the settings file. The pane validates ids and
the expiry before they reach the store and reports the refusal in place. Refresh re-reads the node
and the store, and re-announces this node, which is how a node that finished
starting after the pane opened (or a peer that started late) appears without
reopening it.

With a running node the change is applied to the live peer store and saved; with
no running node the peer store file is edited and the change applies at the next
launch, which the pane states in its header. The header tells the two reasons
apart: federation switched off in settings (`[federation] enabled = false`) and
federation enabled but the node failed to start (the launch log has the reason,
for example another instance holding the data directory lock). Revoking is a state change, not a
deletion: the peer stays listed as revoked so the pane shows what happened.

## Security model

- **Transport is not authority.** A Zenoh key is an address. A peer that can
  publish to a key is still refused by the receiving kernel. Link TLS is
  hop-by-hop; the signed envelope and the receiving kernel are what bind
  authority.
- **Discovery is not trust.** A discovered capability is a claim; invoking it
  still requires a grant on both sides.
- **Signatures are not permissions.** The envelope proves who sent a message.
  It never proves the sender may invoke what it asks for.
- **Delegation attenuates.** A child delegation may only carry a subset of its
  parent's capabilities, the same target, and no later deadline. Forwarding a
  task cannot manufacture authority.
- **Leases bound bytes.** A resource stream is charged against a lease per
  chunk; an expired or over-quota lease stops the stream with an error, not a
  truncated success.

## Audit

Every cross-node decision leaves a record on the side that made it: the caller
records that it sent a call, and the provider records that it authorized and
answered it. A record carries the peer, the operation, the capability, the
outcome class (`ok`, `denied`, `transport`, `unknown_outcome`, ...), an
optional reason, and the duration. It carries **no arguments, no result, and no
secret** — the event type has no field for them.

Records cover `invoke`, `delegate`, `delegation_status`,
`delegation_cancel`, `discover`, and resource opens and endings. A coordination record names the
delegation in `subject`; a resource record names the lease. A lease the provider
did not grant is recorded as `not_found` on the provider side, even though the
caller's open had already been sent — which is why both sides are recorded
rather than one. When the stream ends, one more provider record names the lease,
the outcome (`ok`, `quota_exceeded`, `provider_failed`), the bytes moved, and
why it stopped. The caller records its own ending too — `ok` on a close,
`error` with the provider's message, or `cancelled` when the reader drops the
stream — always with the bytes it collected. Records are per stream, not per
chunk.

Each process writes its own file, `audit-<pid>.jsonl`, beside the identity and
the peer store. Two bsh instances sharing a data directory therefore never write
the same inode, so one process compacting its log cannot swallow a line from the
other. Each file is bounded like the composition log (compacted past 256 KiB to
the newest 200 lines), and a write failure is logged, never surfaced to the
caller. Readers — the export, the settings row, `bsh federation status` — merge
the per-process files with the legacy `audit.jsonl` and order them by event
time. A launch, and `bsh federation status`, remove per-process files older than
seven days (never the current process's own file): a crashed instance cannot
leave them accumulating forever. Age, not pid liveness, is the signal, because
a recycled pid would otherwise keep an unrelated file alive. `MemoryAudit` is the same seam for tests and for a host that wants a
tail instead of a file. The audit export reads the newest 50 records into its
JSON as a `federation` array, so an exported audit explains a cross-node call
without access to the peer. `bsh federation status` prints the five most recent
decisions under the peer list, so the log is readable without the UI. While the
app runs, a successful write also broadcasts the record in-process, and the
shell drains that channel while rendering (a cheap receive, never a file read),
so the settings row follows new decisions without a relaunch.

## Failure behaviour

- **A frame that fails verification is dropped.** A mutated payload, an expired
  message, and a schema this build does not speak all fail before a handler
  runs: no reply, no state change, and the node keeps serving peers that are
  entitled to an answer. Version handling is reject-unknown, not downgrade.
- **An unreachable peer is a failure, not a fallback.** A remote tool whose peer
  leaves fails; the local registry has no substitute for that name, and the
  peer's execution counter does not move.
- **A sent request with no reply is an unknown outcome.** It is never retried
  automatically; only an idempotency key makes a retry safe, and the caller
  decides.

## Current limits

- The composed capability registry (`general.capability_registry`) is opt-in.
  Its plan, snapshot, and shim registry are cached by tool surface and turn
  principal, so a warm build does not rebuild them; flipping the default still
  needs the tests that assert the synchronous registry updated first.
- Remote capabilities are re-discovered on a timer, not pushed. A peer that
  publishes a new capability is visible within 30 seconds.
- A remote capability whose id a local tool already owns is not shadowed; the
  local binding wins for that id.
- General settings shows a read-only federation line (off, or the node's id,
  discovered peers, remote capability count, how many delegations are active,
  and the newest recorded decision). Under it, up to five enrolled peers are
  listed with a shortened id, their trust state, and their grant count, read
  once at launch. It does not offer controls:
  enrollment and grants stay the CLI and the settings file, because a switch in
  a pane would be a permission decision dressed as a preference.
- Resource streams and delegations are implemented at the protocol layer; the
  UI does not yet surface a delegation list or a resource browser.

## Acceptance scenario

One test in `crates/bsh/src/federation_node.rs` runs the whole path over two
in-process nodes and is the acceptance evidence for this foundation:

1. **Discovery** — the provider publishes one public capability; the caller's
   node discovers it and caches it.
2. **Composition** — the caller's workspace holds one local tool and the
   peer's capability; a remote invocation runs on the peer (the peer's counter
   proves exactly one execution).
3. **Delegation** — a task is submitted, the authoritative state reads back
   `Accepted`, and cancelling it returns `Cancelled`.
4. **Resource stream** — a lease with a four-byte quota stops a ten-byte
   resource after four bytes with an error, not a truncated success.
5. **Revocation** — revoking the caller makes both the capability call and a
   new delegation fail with `Denied`, naming the refusal.

## Where the code lives

| Piece | Crate |
|---|---|
| Identity, envelope, peers, discovery, delegation, leases, transport | `crates/federation` |
| Transport seam, descriptors, composition resolver, workspace runtime | `crates/capability` |
| Tool adapter, shim, native provider | `crates/agent` (`capabilities`) |
| Runtime node, CLI, composition cache, audit export | `crates/bsh` |

Tests: `cargo test -p federation`, `cargo test -p capability`, and the
`federation*` and `caps::tests::*composition*` cases in `cargo test -p bsh --lib`.
