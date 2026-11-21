# codel-mcp

A stdio MCP server over the codel candidate graph. It is an adapter crate:
the engine stays independent of MCP and of any host runtime. It indexes Rust,
TypeScript, Python, JavaScript, Go, Java, C/C++, Ruby, C#, Kotlin, Scala, and PHP
through Tree-sitter and returns every graph answer with a token estimate and a
truncation flag. Java, JavaScript, C/C++, Go, C#, Ruby, PHP, Kotlin, and Scala
also have conservative file-level import resolvers.

## SCIP indexes

Generate the index with an external indexer, then import it:

```bash
scip-java index --output index.scip .
scip-typescript index --output index.scip
scip-python index --output index.scip
```

```json
{"index":"index.scip"}
```

`index` may also be a directory containing multiple `.scip` files; all are
merged. Relative paths resolve against the active workspace root. Local
ReadAccess/WriteAccess references become `accesses`/`writes` graph edges;
external symbols are returned with documentation and signature metadata. The
store records the SCIP index digest and `symbol_types`; a cached graph is reused
only when both match, so restart/refresh preserves enrichment.

## Tools

| Tool | Answers |
|---|---|
| `help` | **Call first.** Workspace rules, tool surface, and copy-paste workflows |
| `search` | Find declarations by name or path words |
| `ast_query` | Language-aware preset (`functions/classes/methods/calls/imports`) or custom Tree-sitter query |
| `inspect` | `view=explain` (edges/context), `tree`, `ast`, or `compare` |
| `analyze` | `analysis=affected\|path\|impact\|cycles\|dead_code\|god_nodes` |
| `workspace` | `action=status\|identity\|list\|set\|use\|refresh\|scip_import\|benchmark` |
| `export` | Markdown, JSON, wiki, Obsidian, HTML, SVG, GraphML, Neo4j Cypher, callflow |

Start with `help`; it returns the workspace rules, tool surface, and
copy-paste workflows. `workspace` also accepts `{"action":"help"}`.
The server also sends an `instructions` field in `initialize`. Legacy names
(`query`, `explain`, `path`, ...) are rejected; only the seven
tools above are accepted and advertised.

Every tool response carries a `workspace` object:

```json
{
  "workspaceId": "...",
  "label": "bos",
  "root": "/abs/path",
  "storeRoot": null,
  "source": "host",
  "rootSource": "host"
}
```

Always pass the absolute workspace root as `root` on `search`,
`inspect`, `analyze`, and `export`. Only
`workspace` actions `list`, `help`, and `identity` may omit it.
Use `workspace action=list` to discover roots, ids, and labels.
`search` also accepts `roots` or `all_roots: true` and tags every hit
with its own root/workspace id.

`ast_query` defaults to `preset:"functions"`; invalid custom queries report the
detected language and suggest presets. Every search hit includes a `selector` of the form `path#name`; pass it to
`inspect`/`analyze` when a bare name is ambiguous. A full symbol id and a
byte-range id with the `:kind` suffix omitted are also accepted.

Every edge is labelled `extracted`, `inferred`, or `ambiguous`, and keeps the
source range and resolver state that produced it. `search` and
`inspect` attach a bounded snippet, documentation block, and
line/byte/complexity metrics. `--watch` keeps answers current; an LLM can call
`workspace` with `action=refresh` for an immediate rebuild and deltas.

## Workspace identity

The server never derives a workspace identity from a path. A host supplies both
identity and root:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "_meta": {
      "codel": {
        "workspaceId": "f6b0f0e2-8f3a-4f1a-9e3f-2a6f0c9c1a11",
        "root": "/absolute/path/to/worktree",
        "storeRoot": "/absolute/path/to/codel-store"
      }
    }
  }
}
```

If `root` is omitted, the server asks for MCP `roots/list` and accepts exactly
one root. When `root` is present and roots are listed, the host root must be one
of them. Two worktrees therefore stay separate even when one MCP server template
is reused.

When `storeRoot` is present, startup loads `<storeRoot>/<workspaceId>/graph.json`
instead of re-indexing. A miss indexes once and saves the graph atomically. Each
snapshot carries a content-hash manifest; any source edit re-indexes the graph
automatically. Parsing is skipped when the digest matches. `--watch` uses the
platform filesystem watcher and refreshes on events, while `--hook` installs
post-commit/post-checkout/post-merge hooks that call `--refresh` with a stable
workspace id.

For standalone launches (no host metadata), set:

```bash
CODEL_WORKSPACE_ID=<uuid> \
CODEL_ROOT=/path/to/worktree \
CODEL_STORE_ROOT=/path/to/codel-store \
codel-mcp
```

To let the LLM discover the active workspace, call `workspace`; it
returns `workspaceId`, `root`, `storeRoot`, `source`, and `rootSource`.
`rootSource` is `host`, `tool`, `env`, `mcp-roots`, or `cwd`. If it says
`cwd`, the server had no host root or MCP roots and used its own launch
directory; set `CODEL_ROOT`, call `set_workspace`, or send a
root. `status` returns the same id after indexing.

To switch from the LLM:

```json
{"name":"set_workspace","arguments":{"root":"/abs/path/to/workspace"}}
```

The root is canonicalized, its `.codel/workspace-id` is read or created,
the previous graph is dropped, and the next tool call indexes the new root. The
root remains known, so `use_workspace` can switch back without
recreating the id. Repeated `set_workspace` calls build a workspace
list; each root keeps its own id and store.

The initialize host context overrides the environment. If neither is supplied,
the server creates `.codel/workspace-id` inside the workspace once and
reuses it across restarts, with the store under `.codel/store`. Explicit
host context always wins.

## Build and run

Codintelli owns its tree-sitter dependency set and is excluded from the BOS
workspace. Build it by manifest path, not with `-p` from the BOS root.

```bash
cargo build --manifest-path crates/codel-mcp/Cargo.toml
cargo test --manifest-path crates/codel/Cargo.toml
cargo test --manifest-path crates/codel-mcp/Cargo.toml
cargo run --manifest-path crates/codel-mcp/Cargo.toml
# HTTP JSON-RPC:
cargo run --manifest-path crates/codel-mcp/Cargo.toml -- --http 127.0.0.1:8765
# One-shot store refresh:
CODEL_WORKSPACE_ID=<uuid> CODEL_ROOT=. codel-mcp --refresh
# Filesystem watcher:
CODEL_WORKSPACE_ID=<uuid> CODEL_ROOT=. codel-mcp --watch
# Install git hooks:
codel-mcp --hook /path/to/repo
```

HTTP mode serves POST `/mcp` and GET `/health`, returns an
`Mcp-Session-Id`, emits SSE when the client accepts `text/event-stream`, and
can require a bearer token through `CODEL_HTTP_TOKEN`. It keeps one
process-wide session and does not push server-to-client `roots/list` requests;
supply `CODEL_WORKSPACE_ID` and `CODEL_ROOT` (or initialize
metadata) in HTTP mode.

The server speaks newline-delimited JSON-RPC on stdin/stdout. Diagnostics go to
stderr; stdout is reserved for protocol messages. The workspace is indexed
lazily on the first tool call and cached for the session. Roots changes
invalidate that cache.

The server does not sandbox itself. The launching host (BSH) must apply the OS
filesystem isolation required by ADR-009 and pass only the selected worktree
root; codel still confines every read through `WorkspaceFs`.

## Smoke test

```bash
tmp=$(mktemp -d); mkdir -p "$tmp/src"
printf 'fn alpha() { beta(); }\nfn beta() {}\n' > "$tmp/src/lib.rs"
ws=$(uuidgen | tr 'A-Z' 'a-z')
{
  printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"_meta":{"codel":{"workspaceId":"%s","root":"%s"}}}}\n' "$ws" "$tmp"
  printf '{"jsonrpc":"2.0","method":"notifications/initialized"}\n'
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query","arguments":{"query":"beta"}}}\n'
} | cargo run -q -p codel-mcp
```

The second response contains a `query` result whose first hit is
`beta`.
