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

A node may advertise a federation membership in its settings:

```toml
[federation]
federation_id = "team:research"
```

The id and a policy revision travel in the signed discovery record, and a peer
record can carry the federation it was enrolled under. Membership is metadata,
never authority: the peer still needs explicit enrollment and a grant, and a
matching federation id does not widen one.

### Key custody

`federation::custody` defines where a node's seed lives. `FileCustody` writes
one 0600 file per name (names are sanitized so they cannot escape the
directory); `PassphraseCustody` wraps each secret in Argon2id (64 MiB, 3
passes) + XChaCha20-Poly1305 with a per-file random salt and nonce, so a copied
file is useless without the passphrase. `migrate` moves a key between backends
(the target stores first; the source is deleted only after that succeeds). An
encrypted custody file is also the backup: copy it and open it with the same
passphrase on another host.

`KeychainCustody` uses the platform store (macOS `security`, Linux
`secret-tool`) and reports `available() == false` elsewhere. BSH selects it
with `[federation] identity_custody = "keychain"`: the node loads or creates
its seed there, and falls back to the identity file only when no backend
exists — a keychain read error is reported, never silently replaced by a new
identity. The default remains `"file"`.

### Ledger operations

`AuditLedger::rotate` archives the current segment and continues the chain in a
fresh one; a `.head` sidecar carries the archived tail so a restart before the
first append still chains correctly. `verify_dir` orders segments by the hashes
themselves (not by file name) and refuses a gap or a tampered archived segment.
`checkpoint_to` writes the last hash to a 0600 file and
`verify_with_checkpoint_file` refuses a truncated ledger. Ledger and checkpoint
files are created 0600.

### Audit correlation

Invoke rows on both sides share the trace/span, the task, the last delegation
hop and the original principal. A delegation request carries its own
`trace_id`/`span_id` (a sub-delegation inherits the parent's unless it names
its own), the receiver's row correlates from the nested `request`, and the
reply adds the accepted delegation id. Those rows land in the hash-chained
ledger beside the invoke that started them.

### Acceptance matrix

`crates/federation/tests/acceptance.rs` runs the cross-node promises on the
deterministic wire: three nodes publish their catalogue, a caller resolves a
remote provider and invokes through it, a restart keeps the queued task, an
unreachable peer fails closed instead of hanging, a delegation correlates its
trace and accepted id across both nodes, a workspace-scoped grant is enforced on
the caller and the provider, and a revocation stops the next call immediately.
`task_tcp_e2e.rs` covers the task and delegation flow on a real Zenoh loopback.

### Optional coordinator

A node can run a coordinator (`FederationNode::with_coordinator` or
`with_coordinator_persistence`). It records membership — status, advertised
catalogue revision, policy revision, endpoint — and answers
`coordinator_topology` and `coordinator_register` from trusted peers. Only the
configured administrator Node may register or suspend members; any trusted peer
may read the topology. Membership is metadata, never authority: a member the
coordinator marks active still needs the provider's local grants to invoke
anything, and nodes keep operating peer-to-peer while the coordinator is down.

The coordinator also carries the current policy revision: `set_policy_revision`
is admin-gated and monotonic, `members_behind()` lists members whose advertised
revision is older, and the topology reports both, so a member can detect
staleness without the coordinator pushing anything. The membership document is
saved atomically and reloaded on start.

### Resumable transfers

With `FederationNode::with_transfer_persistence` a node checkpoints each
resource transfer after every chunk (lease, uri, peer, next byte). A later
`open_resource` with offset 0 resumes from the checkpoint automatically; an
explicit nonzero offset still wins. A completed or cancelled stream removes its
checkpoint, and a missing checkpoint file is just a fresh store.

### Protocol compatibility

The envelope speaks `bsh.fed/<major>` and a schema generation. A receiver
checks the major (the compatibility unit: `bsh.fed/1` and `bsh.fed/1.x` are
accepted, `bsh.fed/2` is a breaking change) and a supported schema window, and
refuses a mismatch with a typed error naming what it found and what the build
accepts. Unknown envelope fields are ignored, so an additive change does not
break a signed message. Discovery records advertise the publisher's protocol,
schema and optional features (`invoke`, `delegate`, `resource`, `knowledge`,
`tasks`, `coordinator`), and a record outside the window is refused before its
catalogue is used; a record written before these fields existed reads as v1.
`tests/conformance.rs` pins one envelope's exact signature, so a change to the
canonical form is caught as a wire-format break.

### Membership lifecycle

A peer record moves through explicit states, and a transition that does not fit
the current state is refused:

`invited` → `pending` → `trusted` (active) → `suspended` → `trusted`;
`revoked` is terminal.

The store exposes `invite`, `approve`, `suspend`, `resume` and `revoke`;
`enroll` (the CLI's default action) is invite+approve in one step. Suspending a
peer stops new calls but keeps its grants for a resume; `FederationNode::revoke_peer`
also cancels the delegations that peer submitted and persists the cancelled
records. An enrollment past `expires_at_ms` is inactive without a state change. A signed RotationRecord lets the old
key authorize a successor and moves the peer's grants. After `rotate`, the next
BSH launch reads `rotation.json` and delivers it to every enrolled peer; the
same record can be applied by hand with `accept-rotation`, and delivery is
idempotent for a peer that was offline.

## Several nodes on one machine

One BSH process runs one federation node. A **profile** is that node's name on
this machine: its own key, peer store, delegations, artifacts and audit log.
The default profile keeps today's layout (files directly under
`~/.bos/bsh/federation/`); any other profile lives in
`~/.bos/bsh/federation/<profile>/`.

Run a second node by giving the second instance a profile:

```sh
BSH_FEDERATION_PROFILE=work bsh
bsh --federation-profile=work        # the same thing as a launch flag
```

Both instances read one `settings.toml`, so the launch flag (or the
environment) is what makes the second one a different node;
`[federation] profile = "work"` is only the persistent default for a machine
that always runs that profile.

See what is running:

```sh
$ bsh federation profiles
PROFILE  STATE    PEERS  NODE                                  NAME
default  running  0      1UIH2hlJd9z0atv+wrwudbUtWopCGE/t/cAAJPDj6No=  studio
work     idle     0      kQ3...                                 studio (work)
profiles 2
```

`STATE` is the directory lock: `running` means a process owns that profile's
files right now, which is what makes the column live rather than stored. The
node name carries the profile, so peers can tell two nodes on one machine
apart.

Manage one profile's files while its app is closed:

```sh
bsh federation --profile work status
bsh federation --profile work enroll <node-id>
bsh federation --profile work grant <node-id> builtin/read_file
```

The first node on the machine listens on `tcp/[::]:7447`; when that port is
already taken, the next one binds a loopback ephemeral port and links to
`tcp/127.0.0.1:7447` itself, so local nodes connect with no endpoint
configuration at all. (Zenoh scouting runs on top of that but is not relied on
for the local case.) A node with its own `listen` keeps it and is never
silently moved.

Each profile's files are locked while its node runs: a second process on the
same profile is refused with the directory named, and `profiles` reports that
profile as `running`.

Run nodes without a window - as many as the machine needs:

```sh
bsh federation serve                     # the default profile, foreground
bsh federation serve --profile work      # a second node, same machine
bsh federation serve --profile a --profile b   # several nodes, ONE process
```

Each node prints its id, identity path and status, and a status line whenever
its peer picture changes; Ctrl-C stops them all and releases every lock. A
single process may host as many profiles as named - each keeps its own
identity, peer store, directory lock and transport session. The exception is
TLS: its terminator is process-wide and every profile shares the settings that
carry the certificates, so a second TLS node is refused instead of silently
replacing the first one's listener. In that line
`peers` is presence - nodes whose announcement is live - while `catalogues`
counts how many of them published capabilities, so a linked pair with nothing
exposed reads `peers=1 catalogues=0`, not zero peers. The Federation row in
Settings > General uses the same presence count and names a non-default
profile, so two windows on one machine are tellable apart without a terminal.

## Verifying a host

`scripts/verify-federation-local.sh` runs the whole local story against a
scratch `BSH_HOME`: profile creation, pairing, the live roster, two processes
finding each other over TCP, the same profile refused twice, and several nodes
in one process. It prints `PASS`/`FAIL` per check and exits non-zero on a
failure, so it can gate a release.

The protocol itself is covered by `cargo test -p federation` (identity,
envelope, discovery, peers/grants, delegation, resources, knowledge, rotation,
audit), and the host path by the `federation` tests in `bsh` - including
`two_local_profiles_federate_over_the_real_transport`, which runs two profiles
over a real Zenoh TCP link and exercises presence, the granted catalogue, a
remote read, delegation accept/status/cancel, the audit log, revocation on both
planes, and lock release.

## Managing peers

Management is headless, so it works before any window exists and on a machine
with no GUI:

```sh
bsh federation enroll <peer-node-id>
bsh federation invite <peer-node-id>
bsh federation approve <peer-node-id>
bsh federation suspend <peer-node-id>
bsh federation resume <peer-node-id>
bsh federation grant <peer-node-id> builtin/read_file
bsh federation revoke-grant <peer-node-id> builtin/read_file
bsh federation revoke <peer-node-id>
bsh federation rotate [reason]
bsh federation accept-rotation <record.json>
```

`enroll` is invite+approve in one step; `invite` records a peer without
trusting it, `approve` activates it, and `suspend`/`resume` stop and restart its
calls while keeping its grants. `enroll` and the four verbs take the data
directory's writer lock, so they refuse to write behind a running app.
`grant` names one capability.
`revoke` drops the peer and every grant it held. Enrollments and grants are
persisted in `peers.json` beside the identity with an atomic write.

### Pairing two local profiles

Linking is automatic; trusting is one command. `pair` enrolls both profiles
with each other and takes both directory locks while it writes:

```sh
bsh federation pair work --grant builtin/read_file
# paired <id> (default) <-> <id> (work)
#   grant: work may invoke builtin/read_file on default
```

Grants are directional: `--grant` means "the OTHER profile may invoke this
capability here", so run it once per direction. Both nodes must be stopped
(the locks are held for a running node's lifetime), a refused pair leaves both
stores untouched, and the capability must also be listed in this node's
`[federation] expose` for the node to serve it.

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

### Production profile

Set `security = "production"` to make transport security a startup
requirement. In production a node refuses to start unless the TLS material is
complete (`tls_ca`, `tls_cert`, `tls_key`), certificate-name verification is on
(`tls_verify_name` not false), mutual TLS is on (`tls_mtls = true`), and every
`endpoints`/`listen` entry is a `tls/` locator. `security = "development"` (the
default) keeps plaintext loopback testing permissive.

A node that cannot start is logged and the app runs without federation: a
broken peer configuration must never keep the window from opening.

## Running a delegation

A delegation can ask the target to run one capability. Put the request in
`result_contract` as JSON:

```json
{ "capability": "builtin/read_file", "input": { "path": "notes.txt" } }
```

Only the peer that submitted a delegation may read or cancel it: the record
stores the authenticated submitter, a sub-delegation inherits the parent's
owner, and any other trusted peer is refused (a revoked submitter is refused
before ownership is even considered).

After it accepts, the target transitions the record to `running`, invokes the
capability on its own workspace runtime, and settles it as `succeeded` (the
serialized output is the record's result) or `failed` (the error is the
record's error). Three limits keep this safe:

- the capability must be named in the delegation's `capabilities`; a contract
  outside that list fails instead of escalating;
- the node must actually serve it (configure `[federation] expose`), otherwise
  the record fails with `this Node serves no capabilities`;
- a cancel that wins the race before execution starts means the task does not
  run.

An objective with no structured contract is recorded and left `accepted`: bsh
has no language-model executor, so a prose task is queued, not pretended.

## Delegations survive a restart

The delegations a node accepts are written to `delegations.json` beside
`peers.json` in the node's data directory, after each inbound change, with one
writer at a time. A restarted node reloads them, so a caller asking for status
still gets the accepted record instead of `delegation_not_found`. The file is
written 0600. A malformed file is logged and the node starts with an empty store
rather than refusing to run.

Resource leases are live stream state, not records: a restart ends the streams
and the peer re-opens them. They are deliberately not written to disk, since a
lease naming a stream that no longer exists would be a lie.

## Ingress bounds

The transport buffers at most 1024 inbound frames (and 256 announcements)
ahead of the router. A full queue drops the frame and logs the first drop and
every 1024th after it: a peer that sends faster than the node routes cannot grow
the process without bound. Announcements are periodic presence, so a dropped one
is replaced by the next tick; a dropped invoke fails like any unreachable frame.

## Stream checksums and limits

The producer hashes each chunk into an incremental SHA-256 and finalizes it when
the stream closes, so memory stays bounded by the chunk rather than the resource
length; the hex digest is byte-for-byte the old wire value. At most 128 streams
run concurrently per Node and one stream may last ten minutes; both limits end
the stream with an `error` rather than growing without bound. Stream frames are
routed inline (not through handler admission) so a `close` can never overtake a
chunk and a chunk is never dropped to satisfy a handler budget.

## Replay protection

Every inbound frame except a result is admitted through a replay cache keyed by
`(sender, message_id)` and, when the frame carries one, `(sender, kind,
idempotency_key)`: a caller-supplied key is never trusted as globally unique. A
duplicate of a request gets a typed `duplicate` refusal without running the
provider again; a duplicate resource message is dropped. Each rejection is
audited as `replay/duplicate`.

An entry lives until the envelope that carried it expires, so the cache never
forgets a frame while it is still replayable. Storage is bounded at 4096 entries
with the oldest evicted. A node started with replay persistence (BSH uses
`federation/replay.json`) loads the unexpired entries and writes the cache after
each admitted request, so a restart does not reopen the duplicate window. A write
failure is logged and the frame is still processed: replay protection degrades
rather than an authenticated request being silently dropped, and the failure is
visible in the log. Expired entries are pruned at load.

## Discovery freshness

A discovery record is current for five minutes (`DISCOVERY_TTL_MS`). A consumer
checks the signature, the expiry, and the issue time (a record more than
60 seconds ahead of the local clock is future-dated). It also remembers the
newest publication it accepted from each peer: an older record is a replay and
is refused, while an equal timestamp is the same catalogue and stays
idempotent. A failed check surfaces as `CapabilityError::StaleDiscovery` from
`discover`, and an announcement that fails it is dropped by the pane's
discovery cache. bsh keeps no offline catalogue for composition: a peer whose
announcements stop disappears after the freshness window instead of being used
as if it were current.

## Resource streams belong to one peer

An inbound stream records the peer it was opened with, the lease, the next
expected chunk sequence, and whether it is still open. Every stream message is
checked against that state before it reaches the consumer:

- another trusted peer that knows the lease id is rejected and audited;
- a duplicate or out-of-order chunk is rejected without advancing the stream;
- `close` and `error` are accepted once, then the stream is closed to further
  messages (a later chunk is rejected);
- `open`/`credit` are not valid inbound data for a caller-owned stream.

The queue between routing and the consumer is bounded at 32 messages and
applies backpressure, and the stream's entry is removed exactly once when it
settles or the reader drops it. On the provider side, a lease already open for
one peer cannot be taken over by another, and credit from a peer that does not
own the stream is refused.

## Deadlines and cancellation

Every invoke carries the caller's absolute deadline (`deadline_ms`) inside the
signed request. The target converts it to a local monotonic deadline, so the
effective budget can only shrink across hops, and refuses immediately when the
deadline has already passed. Each in-flight invocation is registered with a
cancellation token; when the caller times out or its future is dropped, it
sends a signed, idempotent `invoke_cancel` for that correlation. The target
cancels the matching token only when the cancellation comes from the peer that
owns the invocation, and a duplicate or late cancel is acknowledged without
effect. A cooperative provider sees `context.cancelled()`.

## Handler admission

A bounded inbox is not enough: every accepted invoke, delegation, discovery,
and resource message used to spawn its own task. Dispatch now takes an admission
permit first: 64 handlers in flight per Node and 8 per peer. A request beyond
either bound is refused with a retryable `rate_limited` outcome (50 ms) instead
of spawning; a permit is released on success, failure, panic, or abort. The
constants are `MAX_IN_FLIGHT_HANDLERS` and `MAX_IN_FLIGHT_PER_PEER`.

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
design; `BSH_HOME` gives every instance its own settings, sessions, identity,
peer store, lock, and log. Give each node a distinct listen port and point the
others at it:

```toml
# ~/bsh-a/settings.toml, launched with BSH_HOME=~/bsh-a
[federation]
enabled = true
listen = ["tcp/127.0.0.1:7447"]
```

```toml
# ~/bsh-b/settings.toml, launched with BSH_HOME=~/bsh-b
[federation]
enabled = true
listen = ["tcp/127.0.0.1:7448"]
endpoints = ["tcp/127.0.0.1:7447"]
```

`BSH_HOME` is also what the `bsh federation` CLI uses, so managing instance B is
`BSH_HOME=~/bsh-b bsh federation status`. Without it every instance shares
`~/.bos/bsh`, and the second one is refused by the directory lock.

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

## Knowledge federation

Knowledge is a separate opt-in protocol over the same signed envelope. A node
installs a `KnowledgeService` (the authoritative local store) and a
`KnowledgeAuthorizer` (the host decision) with `FederationNode::with_knowledge`;
without both, a `knowledge_query` or `knowledge_publication` is refused and
never silently ignored.

```rust
let service = Arc::new(RwLock::new(KnowledgeService::new()));
let node = FederationNode::new(identity, transport)
    .with_knowledge(service, Arc::new(my_authorizer));
```

The contract is versioned:

- A query names its purpose, classification ceiling, optional workspace scope,
  terms, item budget, and absolute deadline.
- A publication carries items with an origin, source reference, issue/expiry
  time, hex SHA-256 of the canonical payload, classification, and optional
  retention ceiling.
- A response is always marked `KnowledgeOrigin::Remote`; it is never merged into
  local memory. Expired, revoked, and over-class items are dropped; a denied
  query is a typed `Denied`, and a query past its deadline is a `Timeout`.
- Revocation is publisher-scoped and idempotent, and a revoked id cannot be
  replayed into the store. The authorizer sees the authenticated peer, class,
  purpose, and scope, so revoking a grant takes effect on the next query.

The authorizer is the source of ACLs; the protocol deliberately does not embed a
second permission language. Results are bounded excerpts, not a replicated index,
and knowledge is returned as data that a consumer evaluates rather than as
instructions.

## Collaboration graphs

Above delegation, `federation::collaboration` provides a durable task graph:
tasks declare dependencies, the runner executes only ready tasks with bounded
concurrency, a failure cancels only its dependents, and every transition is
persisted atomically (0600). Loading a graph marks a task left Running by a
restart as Failed with an indeterminate reason, then cancels its pending
dependents. Tasks carry an attempt ceiling, an acceptance criterion, and artifact
references; a retryable failure returns the task to Pending until the ceiling,
a successful outcome records its artifacts, and a final failure calls the
executor's compensation hook (recorded on the task). A failed remote
compensation is queued durably so `DelegationExecutor::reconcile_compensations`
can retry it; BSH runs that queue every 30 seconds with exponential backoff.
A receipt is stored on the task after success; the Work tab's Audit receipts
action and the node's 30-second auditor compare a task-graph receipt batch with
each target's live records. The graph is a library API: the
right panel lists delegations but does not draw the graph, and a host supplies
the `TaskExecutor`; `DelegationExecutor` is the delegation-backed one.

```rust
let mut runner = Orchestrator::new(graph).with_persistence(path);
let progress = runner.run(executor, 4).await?;
```

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
- Delegations and leases persist in `federation/delegations.json` and
  `federation/leases.json` (atomic 0600 writes). An interrupted delegation is
  recovered as `Failed` with an indeterminate-outcome reason; an expired lease is
  never revived. A lease whose stream ended still requires the peer to re-open.
- The Federation pane is task-shaped: tabs for Peers, Work, Resources and
  Knowledge, grant chips on a peer card, and one Ask field for knowledge. Under
  Advanced, **Exposed capabilities** is a searchable checkbox dropdown over
  the safe read-only set - file reads, glob/grep, workspace context and enabled
  skills - plus any capability already configured, with a one-click clear. It
  replaces the always-on chips. In Work, each finished card has its own
  **Clear** action; **Clear all** removes every work record, including active
  ones, so it is the explicit bulk reset.
  Raw node ids, capability ids, lease ids, URIs and JSON payloads live under an
  Advanced disclosure rather than in the primary path. The local artifact store
  is listed with size and Remove; an executor that supports cleanup deletes
  expired bytes at the end of a task run.
- TLS deployments disable Zenoh scouting so a scouted plaintext link cannot
  bypass the configured TLS endpoints. BSH terminates TLS itself with a
  reloadable rustls resolver and forwards plaintext only to a loopback Zenoh
  listener, so Reload TLS swaps the server certificate and the client CA for new
  handshakes without dropping established connections. Outbound TLS is the same
  shape: BSH keeps Zenoh on a loopback client proxy that owns the client
  certificate, so Reload TLS also replaces the client cert and its CA.
The model has the same surface as `federation_status`, `federation_peers`,
`federation_delegate`, `federation_cancel`, `federation_query` and
`federation_cleanup` tools; it reads peer ids from `federation_peers` instead of
being asked to type them.

- Knowledge federation is opt-in: a host must install a `KnowledgeService` and a
  `KnowledgeAuthorizer`. BSH installs both when
  `[federation] knowledge_enabled = true`; a peer still needs a
  `builtin/knowledge_query` or `builtin/knowledge_publish` grant. The default is
  deny, and a node without both refuses explicitly. BSH persists the store to
  `knowledge.json` after a publication or revocation, and the pane lists local
  items with a Revoke action.
- Federation membership is advertised (signed `federation` and `policy_revision`
  on discovery records) and recorded on a peer at enrollment, but it grants
  nothing by itself: trust and grants stay per peer. There is no shared membership
  authority; a signed rotation record moves grants from the old key to its
  successor and revokes the old identity.

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
