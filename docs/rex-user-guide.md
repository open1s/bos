# rex User Guide

`rex` is the unified resource explorer and node runtime for BOS. It blurs the
line between **local** and **remote**: the same command works on a file on your
disk, a resource on another host, or a resource reached indirectly through a
relay. Location is just part of the URI.

```text
rex list
rex tree folder:///tmp/demo
rex peek file:///tmp/demo/a.txt --bytes 64
rex status folder:///tmp/b
rex watch folder:///tmp/a              # stream change events until Ctrl-C
rex serve --bind 127.0.0.1:4433 --register folder:///data
```

This guide covers installation, concepts, every subcommand, the URI model,
running a node, multi-node discovery, security (mTLS + policy), and a full
troubleshooting section.

---

## Table of Contents

1. [Installation](#installation)
2. [Quick start: 5 minutes to a working node](#quick-start-5-minutes-to-a-working-node)
3. [The big idea: URIs, not machines](#the-big-idea-uris-not-machines)
4. [Usage cases](#usage-cases)
5. [Resource kinds](#resource-kinds)
6. [Global flags](#global-flags)
7. [Subcommands](#subcommands)
8. [Running a node (`serve`)](#running-a-node-serve)
9. [Multi-node discovery & vnodes](#multi-node-discovery--vnodes)
10. [Security: certificates & mTLS](#security-certificates--mtls)
11. [Authorization: policy](#authorization-policy)
12. [Configuration file](#configuration-file)
13. [Data-plane streaming](#data-plane-streaming)
14. [Process management & supervision](#process-management--supervision)
15. [Multi-hop relay](#multi-hop-relay)
16. [Deep usage](#deep-usage)
17. [Troubleshooting](#troubleshooting)

---

## Installation

Build the CLI binary:

```bash
cargo build -p resource --features cli --bin rex --release
```

The binary is at `target/release/rex`.

> **macOS note:** large unstripped binaries can be killed by the kernel with
> `SIGKILL` at startup. The Cargo workspace is already configured with
> `strip = true` for release builds, so a plain `--release` build is fine. If
> you ever build without that flag and hit an instant `SIGKILL`, strip first:
>
> ```bash
> strip target/release/rex -o /tmp/rex && /tmp/rex ...
> ```

Run with `rex --help` or `rex <subcommand> --help` for the full flag reference.

---

## Quick start: 5 minutes to a working node

> **How to read this guide.** Learn rex the way it was designed to be used: a
> tiny thing first, then add one dimension at a time — *locality* (one host),
> then *distance* (two hosts), then *scale* (a mesh). Each numbered block ends
> with **✓ what you should see**, so you can tell instantly whether you're on
> the right track. Run the commands yourself; don't just read them.

### Step 1 — Build and check it exists

```bash
cargo build -p resource --features cli --bin rex --release
./target/release/rex --help
```

**✓ What you should see:** the full flag reference. If the binary dies with an
instant `SIGKILL` instead, see the macOS note in [Installation](#installation).

### Step 2 — Browse your *own* files (no server, no config)

rex's storage resources **auto-bind** to anything already on disk. You don't
register anything and don't run a server — just point a URI at a path:

```bash
mkdir -p /tmp/demo && echo "hello from rex" > /tmp/demo/a.txt

rex tree folder:///tmp/demo          # recursive view
rex status folder:///tmp/demo        # is it there? what kind/state?
rex peek file:///tmp/demo/a.txt --bytes 64
```

**✓ What you should see:** `tree` prints a box-drawing listing of `a.txt`;
`status` says `state: Closed` (the folder exists but was never "opened");
`peek` prints `hello from rex`.

**✏️ What this step teaches:** the core model — a resource is just a URI, and
local files are resources you get for free. Everything after this is the *same*
commands pointed at a *different* host.

### Step 3 — Run a node and reach it over the network

Now the same folder, served over QUIC with mutual TLS. First make the
certificates, then serve, then connect from a second terminal.

```bash
# terminal 1: generate a CA + server + client identity
rex ca --out certs --server-cn server --client-cn me

# terminal 1: serve the folder (leave this running)
rex serve --bind 127.0.0.1:4433 --register folder:///tmp/demo
```

```bash
# terminal 2: talk to the node. Note: --the *same* file URI, but the
# "--node" tells rex to route it to the server instead of resolving it locally.
rex --node 127.0.0.1:4433 --server server \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    list

rex --node 127.0.0.1:4433 --server server \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    peek file:///tmp/demo/a.txt --bytes 64
```

**✓ What you should see** (from `list`):

```text
⚙  compute   Open      node       proc://
📁  storage   Closed    node       folder:///tmp/demo
```

`peek` returns `hello from rex`.

**✏️ What this step teaches:**
- `serve` auto-hosts a `proc://` manager — that's the first `list` row.
- The **same URI**, `file:///tmp/demo/a.txt`, means *local* without `--node` and
  *remote* with it. Nothing in the command changes except *where* it points.
- mTLS is already working: the client proved its identity with its cert CN
  (`me`), and the server verified it. `--server server` must match the cert CN
  from Step 3's `--server-cn server`.

### Step 4 — Two nodes find each other (zero config)

Servers can discover each other and merge into a shared namespace with no flag
mangling. Start a bus router, then two nodes:

```bash
# terminal 0: the bus router (one per cluster)
zenohd

# terminal 1
mkdir -p /tmp/a && rex serve --node-id nodeA --bind 127.0.0.1:4433 --register folder:///tmp/a

# terminal 2
mkdir -p /tmp/b && rex serve --node-id nodeB --bind 127.0.0.1:4434 --register folder:///tmp/b
```

**✓ What you should see:** nodeA's log prints

```text
discovered peer nodeB (caps=[stream, proc, supervisor, relay, vnode]) with 1 resources (procs=0, uptime=1s)
```

and any client (or `rex list` run from a third shell against nodeA) shows a
`vnode://nodeB` entry — a live, aggregated view of the other node.

**✏️ What this step teaches:** discovery is automatic and self-healing. Kill
nodeB (`Ctrl-C`) and `vnode://nodeB` disappears from nodeA within ~15s; restart
it and it re-joins. No certificates were passed here because `serve` generated
them into `./certs/` when it couldn't find any.

### You're done. Where to go next

- [Usage cases](#usage-cases) — concrete "why would I do that?" workflows.
- [Subcommands](#subcommands) — precise reference for every command and flag.
- [Deep usage](#deep-usage) — streaming, processes, supervisors, relays at the
  library level.

---

## The big idea: URIs, not machines

Every resource is addressed by a URI with the shape `scheme://path`. A
`scheme://host/...` URI *(authority present)* selects a remote node; a
`scheme://...` URI without a host is local. From the caller's
perspective both look identical:

```text
file:///tmp/notes.txt          local file
folder:///srv/data             local directory
file://my-node/tmp/notes.txt   file on the machine named "my-node"
mem://cache                    abstract in-memory store
proc://1234                    a running process (local)
```

This means one command, no branching on location. `rex list` shows everything
it can see — its own resources plus each connected node's — with a colored
kind/state per row.

---

## Usage cases

The reference above tells you *what each command does*; these walk through
*why you'd chain them together*. Every case is a self-contained, copy-paste
sequence. They assume you've built `rex` and put it on your `PATH`.

### 1. Local exploration, zero setup

`file://` and `folder://` auto-bind to anything already on disk, so `tree`,
`status`, `peek`, and `watch` work on paths without registering or running a
server. (`list` is the exception: it enumerates *registered* resources, not
the filesystem — with a bare client and no `--register` it prints nothing.)

```bash
# Recursively inspect a tree up to 3 levels deep.
rex tree folder:///home/me/projects --depth 3

# Is that path really a directory, registered or not?
rex status folder:///home/me/projects

# Read the first 200 bytes of a config file without a text editor.
rex peek file:///home/me/.zshrc --bytes 200

# Watch a directory and react to what changes in real time.
rex watch folder:///home/me/projects
```

Use case: you're about to `git clean` a directory and want a quick, colored
inventory of what's there before you delete anything.

### 2. Two hosts, one shared filesystem view

You have an archive box in the basement that can't run heavy tooling — only a
thin `rex` node — while your laptop runs the clients.

```bash
# On the archive box (hosts the files)
rex ca --out certs --server-cn archive --client-cn laptop
rex serve --bind 0.0.0.0:4433 --register folder:///mnt/archive \
    --ca certs/ca.pem --cert certs/server.pem --key certs/server.key

# On your laptop (points at the box)
rex --node 10.0.0.7:4433 --server archive \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    list

rex --node 10.0.0.7:4433 --server archive \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    tree folder:///mnt/archive --depth 4
```

Use case: a headless NAS/file server exposes its whole tree; every client
browses and peeks at it with exactly the same commands they use locally — no
NFS, no Samba, no SSH.

### 3. A fleet you inspect from one machine

You run a dozen worker nodes. You want a single pane of glass to see them all,
and to have them *find each other* so you don't curate a host list by hand.

```bash
# Start zenohd once on a host every node can reach.
zenohd   # tcp/127.0.0.1:7447

# On each worker (distinct node ids; identical CAs or shared discovery)
rex serve --bind 127.0.0.1:4433 --node-id worker-03 --register folder:///data

# From your ops laptop, connect to any single node...
rex --node 127.0.0.1:4433 --server server --ca certs/ca.pem \
    --cert certs/client.pem --key certs/client.key list

# ...and once one node sees the fleet, you see it too:
#   vnode://worker-01
#   vnode://worker-02
#   → walk into any of them:
rex tree vnode://worker-03 --depth 2
```

Use case: dynamic, zero-config clustering — bring a node up and it joins the
mesh automatically (and disappears from it when it dies, ~15s later). No
inventory file, no DNS, no load balancer to update.

### 4. Pinning down "what changed?"

A service started misbehaving at 15:42. You want to know which file in an
agent's working directory changed around then.

```bash
# Leave this running in a terminal while someone reproduces the issue.
rex watch folder:///srv/agent/state modify create
```

Use case: filesystem watch as a poor-man's audit trail — you get a timestamped
stream of create/modify/remove events without instrumenting the app.

### 5. Bootstrapping a new peer's trust (no shared CA)

You don't want every node signed by one central CA; you want each node to carry
its own CA and still trust the mesh.

```bash
# Generate a per-node CA (separate dir per node).
mkdir -p nodeA-certs && rex ca --out nodeA-certs --server-cn nodeA --client-cn alice
mkdir -p nodeB-certs && rex ca --out nodeB-certs --server-cn nodeB --client-cn bob

# Both nodes run with discovery on; they exchange CAs over the bus.
rex serve --bind 127.0.0.1:4433 --node-id nodeA --register folder:///a \
    --ca nodeA-certs/ca.pem --cert nodeA-certs/server.pem --key nodeA-certs/server.key
rex serve --bind 127.0.0.1:4434 --node-id nodeB --register folder:///b \
    --ca nodeB-certs/ca.pem --cert nodeB-certs/server.pem --key nodeB-certs/server.key

# alice talks to nodeB without ever sharing a CA file out-of-band:
rex --node 127.0.0.1:4434 --server nodeB --cert nodeA-certs/client.pem \
    --key nodeA-certs/client.key list    # --ca auto-discovered from the bus
```

Use case: per-tenant isolation — each tenant's node is its own PKI root, yet
they interoperate through discovery without manual certificate exchange.

### 6. Locked-down data with a policy

You want `folder:///data` readable by anyone, but only `admin` may mutate it.

```json
// policy.json
{
  "admins": ["admin"],
  "rules": [
    { "agents": ["*"],      "uris": ["folder:///data"], "actions": ["read", "list", "stat"], "effect": "allow" },
    { "agents": ["admin"],  "uris": ["folder:///data"], "actions": ["*"], "effect": "allow" }
  ]
}
```

Run the reader node and the writer node with identities that map to those
agent names:

```bash
rex serve --bind 0.0.0.0:4433 --register folder:///data --policy policy.json \
    --ca certs/ca.pem --cert certs/server.pem --key certs/server.key
```

Use case: enforcement at the transport boundary — clients with a
`client-cn reader` cert can `peek`/`list` but their `write`/`remove`/`rename`
get `policy denied`. Identity from the mTLS cert CN, not from any argument the
client claims.

### 7. Troubleshooting a remote host without SSH

A box is up but painful to reach. You only need to *see* files and check that
resources are alive.

```bash
rex --node 10.0.0.7:4433 --server box --ca certs/ca.pem \
    --cert certs/client.pem --key certs/client.key \
    status folder:///var/log

rex --node 10.0.0.7:4433 --server box --ca certs/ca.pem \
    --cert certs/client.pem --key certs/client.key \
    peek file:///var/log/app.log --bytes 400
```

Use case: least-privilege diagnostic access — no shell, no agent, just a
filtered read-only view of the specific paths the policy allows.

### 8. Verifying the whole feature surface in one shot

The advanced subsystems (streaming, full-POSIX ops, proc manager, supervisor,
relay) are exercised end-to-end by the test suite — run it as a smoke check
before relying on them:

```bash
cargo test -p resource --all-features
```

Use case: CI or a release gate — 65 tests proving every transport and resource
kind works, local and cross-node, in ~10s.

---

## Resource kinds

| Kind | Schemes | Meaning |
|------|---------|---------|
| storage | `file://`, `folder://` | files and directories on the host |
| abstract | `mem://` | in-memory / application-defined resources |
| compute | `proc://`, `sup://` | processes and supervised process groups |
| network | `sock://`, `relay://` | sockets and relay endpoints |
| combine | `vnode://`, `combine://` | aggregations of other resources |

`file://` and `folder://` are *auto-bound*: addressing a path that exists on
disk is enough — no explicit registration needed. `proc://<pid>` resources are
created dynamically when you spawn a process.

---

## Global flags

These apply to any subcommand and may appear before or after it.

| Flag | Meaning |
|------|---------|
| `--debug` | verbose routing/transport/dispatch logs on stderr |
| `--no-color` | disable ANSI color (useful when piping output) |
| `--register <uri>` | register a local resource (repeatable) |
| `--node <addr>` | connect to one remote node as the *default* transport |
| `--server <name>` | SNI / server-cert CN for `--node` |
| `--ca <path>` | CA bundle (PEM) to verify the remote node |
| `--cert <path>` | your client certificate (PEM) |
| `--key <path>` | your client key (PEM) |
| `--config <path>` | load defaults from a TOML file (see [Config](#configuration-file)) |

Example — list the resources of a remote node:

```bash
rex --node 127.0.0.1:4434 --server server \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    list
```

When `--ca` is omitted the client attempts to auto-discover the server's CA
from the bus (see [Multi-node discovery](#multi-node-discovery--vnodes)); use
`--ca` when discovery isn't running.

---

## Subcommands

### `list`

Discover resources across the local node and every connected node.

```bash
rex list                    # all
rex list --kind storage     # only storage resources
rex list --kind compute     # only compute (proc etc.)
```

Output columns: icon, kind, state, owner, URI. States are color-coded
(green = open, gray = closed, red = error).

### `tree`

Recursive view of a resource and its children.

```bash
rex tree folder:///tmp/demo
rex tree folder:///tmp/demo --depth 2
rex tree vnode://nodeB      # the union of every remote folder's contents
```

Box-drawing output; `├──`/`└──` indent nested entries.

### `peek`

Read-only first-`N`-bytes of a file (or next-`N` of a socket).

```bash
rex peek file:///tmp/demo/README.md --bytes 128
```

ASCII output is printed as text; binary as hex.

### `status`

Short description of one resource.

```bash
rex status folder:///tmp/b
# uri, kind, state, owner
```

### `watch`

Live stream of a resource's push events until Ctrl-C. For `folder://` and
`file://` resources this is the filesystem watcher.

```bash
rex watch folder:///tmp/a          # create/modify/remove/access events
rex watch folder:///tmp/a create modify
```

Events print as `Custom("<tag>", <path bytes>)` (watch tags: `create`,
`modify`, `remove`, `access`, `other`).

### `ca`

Generate a CA + server + client certificate set so one node can run and
others can connect to it.

```bash
rex ca --out certs --server-cn myserver --client-cn agent1
```

Writes `certs/ca.pem`, `certs/server.pem`, `certs/server.key`,
`certs/client.pem`, `certs/client.key`. The server CN (`--server-cn`) is what
clients pass as `--server`; the client CN becomes the agent *identity* that
policy matches against.

### `serve`

Start a resource node (see the next section).

---

## Running a node (`serve`)

`serve` binds a QUIC server, hosts `--register` resources, and accepts
connections until interrupted (Ctrl-C).

```bash
rex serve \
  --bind 127.0.0.1:4433 \
  --register folder:///srv/data \
  --register mem://cache \
  --ca certs/ca.pem --cert certs/server.pem --key certs/server.key
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--bind` | `127.0.0.1:4433` | address to bind (`0.0.0.0:4433` to expose) |
| `--register` | — | resource URI to host (repeatable) |
| `--policy` | permissive | Rego/JSON policy file |
| `--ca` | `certs/ca.pem` | CA bundle that issued server/client certs |
| `--cert` / `--key` | `certs/server.{pem,key}` | this node's server identity |
| `--node-id` | hostname | node identity for bus discovery |

On startup `serve` auto-registers a `proc://` manager (so any node can spawn
processes) and, if `--node-id` is set, joins the discovery bus.

---

## Multi-node discovery & vnodes

When two or more nodes pass `--node-id`, they find each other automatically
through a bus (Zenoh) and aggregate into a **super virtual node**
(`vnode://<node_id>`) on every peer.

**Prerequisite:** a Zenoh router. Run `zenohd` on the default port
(`tcp/127.0.0.1:7447`) on some host both nodes can reach.

```bash
# node A
rex serve --bind 127.0.0.1:4433 --node-id nodeA --register folder:///srv/a

# node B
rex serve --bind 127.0.0.1:4434 --node-id nodeB --register folder:///srv/b
```

Each node now auto-discovers the other. Node A's log shows the rich directory
line:

```text
discovered peer nodeB (caps=[stream, proc, supervisor, relay, vnode]) with 3 resources (procs=0, uptime=12s)
```

On either node, `rex list` shows `vnode://nodeB`, and:

```bash
rex tree vnode://nodeB --depth 3    # union of nodeB's folder contents
```

Notes:

- Peers re-announce every 5s (that is also how the server CA is auto-discovered).
- **Peer loss** is detected after ~15s of silence: the `vnode://` entry is
  unregistered and the pooled transport is dropped. When the peer returns it
  is re-registered automatically.
- Announcements can be **HMAC-signed** to reject spoofed peers — set
  `[discovery] shared_secret` in the config; unsigned/tampered announcements
  are then rejected.

---

## Security: certificates & mTLS

`rex` transport is QUIC with **mutual TLS**.

1. **Generate** a cert set once:

   ```bash
   rex ca --out certs --server-cn server --client-cn agent1
   ```

2. **Server** presents `server.pem` and verifies clients against `ca.pem`.

3. **Client** presents `client.pem`, whose CN (`agent1` in the example) becomes
   its authenticated identity for policy decisions.

**Cross-node trust:** when two nodes each run `rex ca` with their *own* CA,
they exchange CAs over the discovery bus — a node collects peer CAs and trusts
them for client verification, and a connecting client auto-discovers the
server's CA. This is what makes `--ca` optional on the client when discovery
is active.

**Matching names:** `--server` on the client must match the server cert's CN
(the SNI check). If you generated the peer's server cert with a custom CN, pass
that same name via `--server`.

---

## Authorization: policy

`serve` enforces `(agent, uri, action)` allow/deny rules. Without `--policy` it
defaults to permissive (allow-all).

Provide a JSON `PolicyDoc` or a Rego file (`.rego`):

```bash
rex serve --policy policy.json ...
```

Example JSON policy (allow agent `agent1` everything under `folder://*`,
deny others):

```json
{
  "admins": ["admin"],
  "rules": [
    { "agents": ["agent1"], "uris": ["folder://*"], "actions": ["*"], "effect": "allow" }
  ]
}
```

`actions` are named: `open`, `close`, `read`, `write`, `list`, `mkdir`,
`remove`, `truncate`, `rename`, `stat`, `lock`, `unlock`, `spawn`, `kill`,
`wait`, `status`, and others — each resource kind supports a subset.

---

## Configuration file

Instead of repeating flags, put defaults in a TOML file and pass `--config`
(or name it `./rex.toml` / `./config.toml` to load automatically). CLI flags
always win.

```toml
# flat (legacy)
node = "127.0.0.1:4433"
server = "myserver"
ca = "certs/ca.pem"
cert = "certs/client.pem"
key = "certs/client.key"
register = ["folder:///tmp/data"]

# nested sections (preferred)
[node_section]
id = "nodeA"
bind = "127.0.0.1:4433"
server = "myserver"

[certs]
ca = "certs/ca.pem"
server = "certs/server.pem"
server_key = "certs/server.key"
auto_generate = true

[discovery]
shared_secret = "my-secret"
```

---

## Data-plane streaming

`rex` has true streaming transport: reads and writes move bytes incrementally
over QUIC in ~64 KiB chunks — a multi-gigabyte file is never buffered whole on
either side. This is what makes `tree`/`peek` cheap against remote folders, and
it is exposed in the Rust API via `read_stream`/`write_stream`. (There is no
`rex cp` subcommand yet; streaming is currently exercised by
`cargo test -p resource --all-features --test stream`.)

---

## Process management & supervision

Every `serve` node hosts a `proc://` manager. Spawn a process; it appears as
`proc://<pid>` and is reachable **cross-node** exactly like a file.

Processes and supervisors are not yet exposed as `rex` subcommands; they are
covered by the Rust library and integration tests:

- `cargo test -p resource --all-features --test proc_mgr` — spawn/kill/wait/list + exit events
- `cargo test -p resource --all-features --test supervisor` — restart policies, budgets, escalation
- `cargo test -p resource --all-features --test relay` — A → B → C multi-hop
- `cargo test -p resource --all-features --test stream` — streaming read/write
- `cargo test -p resource --all-features --test fs_ops` — truncate/rename/stat/lock/watch

---

## Multi-hop relay

A `relay://<node>` resource forwards actions to a downstream node the origin
cannot reach directly. Relays compose (`A → B → C → D`) with no special-casing.
Declarative route configuration for `serve` is not wired into the CLI yet; the
primitives (`RelayTransport` / `RelayResource`) are in the library and covered
by `tests/relay.rs`.

---

## Deep usage

Here is the key mental shift: rex is a **thin client/server over a library**.
Everything the CLI does, the `resource` crate does; but the crate does more than
the CLI exposes. This section teaches you to use the *deeper* surface.

Learn in order — each subsection assumes the one before it:

1. [What the CLI already gives you](#what-the-cli-already-gives-you)
2. [Embedding: write a 20-line program](#embedding-write-a-20-line-program)
3. [Streaming data-plane](#streaming-data-plane)
4. [Process manager](#process-manager)
5. [Supervision](#supervision)
6. [Multi-hop relay](#multi-hop-relay)
7. [Debugging the transport](#debugging-the-transport)

### What the CLI already gives you

Three of the advanced subsystems are reachable **today** through plain `rex`,
because they're wired into `serve`:

| Subsystem | How you use it through the CLI |
|-----------|-------------------------------|
| Streaming | `peek` (and `tree`/`list`) read data incrementally over QUIC — pull any file, any size, no buffering |
| Process manager | every `serve` node hosts `proc://`; see it in `list`, `status` it |
| Rich directory | `--node-id` makes peers announce `caps=[…]` + load; watch the `discovered peer …` log line |

The remaining three — direct stream copy, supervisors, and relays — are library
APIs. That's what the rest of this section teaches.

### Embedding: write a 20-line program

The crate is designed to be embedded. Here's a minimal, compilable client that
connects to a running `rex serve` node and peeks a file — the *library*
equivalent of `rex peek`:

```rust
use std::sync::Arc;
use resource::prelude::*;
use resource::transport::QuicTransport;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Arc::new(QuicTransport::new(
        "127.0.0.1:4433".parse()?,               // node address
        "server".into(),                         // server CN / SNI
        &std::fs::read("certs/ca.pem")?,         // trust
        std::fs::read("certs/client.pem")?.into(), // client identity
        std::fs::read("certs/client.key")?.into(),
    )?);

    let client = ResourceClient::new("me", None, Some(transport));
    let out = client
        .invoke("file:///tmp/demo/a.txt", ResourceAction::Read { offset: 0, len: 64 })
        .await?;
    if let ResourceOutput::ReadOk { data } = out {
        println!("{}", String::from_utf8_lossy(&data));
    }
    Ok(())
}
```

Add `resource = { path = "crates/resource", features = ["cli"] }` (or `default`
if you don't need the binary) to your crate's `Cargo.toml`.

**✏️ Lesson:** `ResourceClient::invoke(uri, action)` is the universal verb. The
CLI's `list`/`tree`/`peek`/`status`/`watch` are thin wrappers over `invoke`
(resp. `list`/`resolve`/`subscribe`). Once you can `invoke`, you can do
anything the CLI can — and everything it can't.

### Streaming data-plane

`invoke(Read/Writto)` buffers; the streaming API doesn't. Copy a file across
the network without ever holding it whole in memory:

```rust
use futures::StreamExt;
use resource::prelude::*;

let mut src = client.read_stream("file://remote/big.bin", 0, None).await?;
let mut dst = client.write_stream("file://local/big.bin", 0).await?;
while let Some(chunk) = src.next().await {
    dst.write_chunk(&chunk?).await?;
}
let written = dst.finish().await?;   // flushes + returns total bytes
```

**✏️ Lesson:** `read_stream(uri, offset, len)` yields ~64 KiB chunks; a `len` of
`None` reads to end-of-data. `write_stream(uri, offset)` returns a writer whose
`finish()` flushes and reports bytes written. This is what makes multi-GiB
transfers cheap against a remote node. There is no `rex cp` yet — but you can
write it in 8 lines, as above.

### Process manager

`rex serve` already hosts `proc://`. From any client:

```rust
// Spawn a job on the node (returns its pid).
let ResourceOutput::Spawned { pid } = client.invoke("proc://", ResourceAction::Spawn {
    args: vec!["/bin/sh".into(), "-c".into(), "run-the-job".into()],
    env: vec![],
}).await? else { panic!("spawn") };

// Wait for it — from the same process, another one, or another host. The URI
// is stable, so the caller never needs to know where the process actually runs.
let ResourceOutput::Exited { code } =
    client.invoke(&format!("proc://{pid}"), ResourceAction::Wait).await?
    else { panic!("wait") };

// Kill instead of wait:
// client.invoke(&format!("proc://{pid}"), ResourceAction::Kill { signal: 9 }).await?;
```

**✏️ Lesson:** `proc://<pid>` is just another URI, reachable cross-node exactly
like a file. The manager (`proc://`) creates children; each child is a resource
you can `status`/`kill`/`wait`/`subscribe` to.

### Supervision

A supervisor keeps a flaky process alive and kills the group when it keeps
dying. Register one on the node that should run the process:

```rust
use resource::{Supervisor, SupPolicy, RestartPolicy, ChildSpec};

let sup = Supervisor::new("api", mgr, vec![ChildSpec {
        args: vec!["/bin/sh".into(), "-c".into(), "./server".into()],
        env: vec![],
    }],
    SupPolicy { restart: RestartPolicy::OnFailure, max_restarts: 3,
                window: Duration::from_secs(60) });
mgr.register(Box::new(sup), "admin".into()).await?;

// Start the group:
client.invoke("sup://api", ResourceAction::Open).await?;
// Inspect:
match client.invoke("sup://api", ResourceAction::List { pattern: None }).await? {
    ResourceOutput::Listed { entries } => println!("{entries:?}"),
    _ => {}
}
```

**✏️ Lesson:** three policies — `Always`, `OnFailure`, `Never`. A restart counts
against `max_restarts` in a rolling `window`; exceeding it *escalates*: the
whole group is killed and the supervisor reports `Closed`. Exit status *is* the
health signal — no separate probes.

### Multi-hop relay

A node you can't reach directly can still be reached *through* a node you can.
`RelayResource` (on the middle node) and `RelayTransport` (on the origin) are
inverses:

```rust
use resource::{RelayResource, RelayTransport};

// On B: register a relay to C.
b_mgr.register(Box::new(RelayResource::new("c", transport_to_c)), "admin".into()).await?;

// On A: a transport that wraps every action and forwards it through B.
let relay: Arc<dyn resource::Transport> =
    Arc::new(RelayTransport::new(transport_to_b, "c"));
let client = ResourceClient::new("me", None, Some(relay));
// client.invoke("proc://", Spawn{..}) now transparently runs on C.
```

**✏️ Lesson:** because a `RelayTransport` is just another `Transport`, relays
compose — `A → B → C → D` needs no special case. You can build a private
overlay where nodes reach each other only through designated gateways.

### Debugging the transport

When something fails "over the wire", the tracing layer is your best tool:

```bash
rex --debug --node 127.0.0.1:4433 --server server \
    --ca certs/ca.pem --cert certs/client.pem --key certs/client.key \
    tree folder:///tmp/demo
```

You'll see every decision: `route <uri> -> local|remote`, QUIC connect and
stream events, policy verdicts, and dispatch results. This is the same signal
the integration tests assert on, so you can reason about a failure the same way
the test suite would.

**Verify everything at once:** `cargo test -p resource --all-features` runs 65
tests across all ten subsystems in ~10s — the authoritative smoke check after
any change.

---

## Troubleshooting

| Symptom | Likely cause / fix |
|---------|--------------------|
| Instant `SIGKILL` at startup (macOS) | binary too large for the kernel; rebuild with `strip = true` or `strip` the binary |
| `could not discover server CA for <addr>` | start the server first (it re-announces its CA every 5s), or pass `--ca` explicitly |
| `policy denied: agent=… uri=… action=…` | the peer's client-cert CN has no allow rule; check `--policy` |
| `resource is closed` on a file `Read`/`Write` | files must be `Open`ed first for buffered reads/writes; streaming (`read_stream`) does not |
| `no known scheme for <uri>` | scheme unsupported on that build; use `file://`, `folder://`, `mem://`, `proc://` |
| server won't accept client certs from another node | each node signed by its own CA → start discovery so peer CAs are exchanged, or merge the CAs into one root |
| peer `vnode://X` never appears | is `zenohd` running? is `--node-id` set on both nodes? (defaults to hostname, so two shells on one host collide — use distinct ids) |
| `vnode://X` vanishes after a while | peer lost: it stopped re-announcing; it will re-register when it returns |

For deeper diagnosis run with `--debug` (tracing shows routing, transport, and
dispatch decisions) or set `RUST_LOG=debug`.