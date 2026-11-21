# codel vs Graphify: gap analysis

Status: working document, 2026-11-21  
Reference: Graphify CLI 0.9.100 / Graphify-Labs/graphify  
Scope: the codel engine, not the hosted Graphify product

Graphify is a deterministic, local code knowledge graph. Its published CLI
combines Tree-sitter extraction, cross-file resolution, Leiden communities, a
query/path/affected surface, a persistent `graphify-out/` directory, incremental
updates, an MCP server, and optional LLM extraction for docs and media.

codel does not need to copy that product shape. It needs to be a better
foundation for agents: every fact source-bound and checkable, every workspace
isolated, every partial result explicit.

## Where codel already targets superiority

| Axis | Graphify | codel |
|---|---|---|
| Evidence | Edge labels `EXTRACTED`, `INFERRED`, `AMBIGUOUS` | Every record binds workspace, relative path, source fingerprint, provider version, query version, capture, and byte range |
| Isolation | `graphify-out/` per scanned folder; identity is the path/repository | Host-supplied `WorkspaceId`; one store root holds one directory per worktree; a snapshot with the wrong identity is rejected |
| Durability | `graph.json`, manifest, cache; freshness by re-extraction | Checksummed atomic generation plus `snapshot.prev.json` fallback; schema and workspace identity checked before any record is accepted |
| Source verification | Graph nodes/edges reference files | Context assembly re-reads source and verifies the fingerprint; a changed or missing source is an explicit omission, never a quote |
| Partial results | Confidence labels | Explicit parse/query coverage and stale marks; budget exhaustion is observable |
| Execution safety | Tree-sitter locally; LLM only for non-code | No project code, build script, compiler, package manager, or test runner is executed for indexing |
| Dependencies | NetworkX, Leiden, optional model | No graph database, vector database, LLM, or language server is required for the core |

## Parity gaps, in priority order

| # | Capability | Graphify | codel today | Plan |
|---|---|---|---|---|
| 1 | Graph query surface | `query`, `explain`, `path`, `affected`, `god-nodes` with a token budget | **Landed** query/explain/path/affected/god-nodes with edge classes and `AnswerBudget` trimming | Later: richer ranking than degree |
| 2 | MCP interface | `graphify-mcp` over stdio and HTTP | **Landed** stdio + HTTP POST `/mcp`, `Mcp-Session-Id`, SSE, optional bearer token, store-backed startup; OAuth/multi-tenant absent | Later: OAuth and multi-tenant Streamable HTTP |
| 3 | Incremental operation | `update`, `watch`, git hooks, manifest/cache | **Landed** content-hash refresh, debounced notify `--watch`, `--refresh`, and post-commit/post-checkout/post-merge hooks | Later: cross-platform event filtering refinements |
| 4 | Language coverage | 13 languages through Tree-sitter | **13 languages**: Rust/TypeScript/Python specific providers + generic providers; Java, JavaScript, C/C++, Go, C#, Ruby, PHP, Kotlin, and Scala now have conventional import resolvers | Later: true scope/type binding for the generic languages |
| 5 | Call and inheritance edges | Per-language resolvers, `AMBIGUOUS` when not unique | **Landed (candidate)** calls/imports/inherits/implements; Rust/TS type annotations; Java/JS/Kotlin/Scala/PHP/Go/C#/Ruby/C/C++ imports bind to symbols; Java/TypeScript/C#/Kotlin/Scala/Go receiver tables bind calls; Java/TypeScript/C# bind field writes; Java adds array elements, user/collection generics, functional-interface returns, arity/argument-type overloads, cast and conditional receivers | P1: lambda-literal target typing |
| 6 | Reports and exports | graph.html, wiki, SVG, GraphML, Neo4j, Obsidian, callflow | **Landed** Markdown, JSON, wiki, Obsidian, HTML, SVG, GraphML, Neo4j Cypher, callflow, benchmark | Later: richer interactive HTML |
| 7 | Communities | Leiden clustering and labels | Dedicated `leiden.rs`: queue local moving, well-connected refinement, aggregation, best partition by modularity; deterministic, no randomized restarts | Later: randomized restart strategy if needed |
| 8 | Non-code extraction | Docs, PDFs, images, video through a model | Out of scope | P2 or never in the core; a separate provider crate may produce evidence records |
| 9 | Token benchmark | `graphify benchmark` reports 127x on its own repo | **Landed** `GraphBenchmark`: raw source bytes vs serialized graph, 4 bytes/token | Later: corpus-level benchmark page |
| 10 | Work memory / PR impact / global graph | Present in the hosted and CLI products | Retrieval has memory units; no PR or multi-repo layer | P2: host-level features over workspaces, not core index behavior |

## Fixes landed in this pass

1. **CI-07 durable store** (`src/store.rs`): workspace-scoped directory, checksummed
   envelope, atomic generation, previous-generation fallback, schema/identity/range
   validation, and no source text on disk. It composes with `plan`/`apply` for
   incremental updates.
2. **Native query extraction** (`src/syntax/rust.rs`): declarations, imports, and
   calls are classified by a Tree-sitter query executed with `QueryCursor`; the
   Rust provider no longer walks node kinds by hand and parses once into
   `ParsedDocument`.
3. **Source-verified context** (`src/context.rs`): byte/segment budget, fingerprint
   verification, explicit omissions, and UTF-8-safe clipping.
4. **Provenance on retrieval** (`src/retrieve.rs`): every source-backed candidate
   carries its document, fingerprint, range, and provider; remote-only candidates
   report none.
5. **Candidate graph** (`src/graph.rs`): deterministic file/declaration nodes,
   `contains`/`calls`/`imports`/`inherits`/`implements` edges, explicit evidence
   classes, coverage, and bounded `query`/`explain`/`path`/`affected` operations.
6. **MCP adapter** (`crates/codel-mcp`): stdio JSON-RPC, five tools, host
   context plus MCP roots, one workspace per server, lazy index invalidation on
   roots changes.
7. **Token-budgeted graph answers** (`AnswerBudget`): query/explain/path/affected
   trim to a deterministic estimate and report `estimated_tokens`/`truncated`.
8. **Store-backed MCP startup** (`GraphStore`): a workspace-scoped, checksummed,
   atomically replaced graph snapshot; MCP loads it instead of re-indexing and
   saves a graph on the first successful index.
9. **TypeScript provider** (`src/syntax/typescript.rs`): native Tree-sitter
   parsing, one query for declarations/methods/calls/imports, and shared
   facts/evidence contracts. MCP indexes Rust and TypeScript together.
10. **Inheritance and type edges** (`RelationFact`): Rust `impl Trait for Type`
    and TypeScript `extends`/`implements` become candidate `inherits` and
    `implements` edges, matched by name and labelled inferred or ambiguous.
11. **God nodes** (`CodeGraph::god_nodes`): deterministic degree ranking, exposed
    through MCP with a token budget.
12. **HTTP MCP transport**: `codel-mcp --http ADDR` serves JSON-RPC over
    POST `/mcp` and `GET /health` with the same server state.
13. **Metadata refresh**: `scan_manifest_digest` plus `GraphStore` digests
    re-index when the file inventory or sizes change; same-size edits are a known
    limitation until content hashes land.
14. **Python provider**: native Tree-sitter parsing, declarations/methods/calls/
    imports/type annotations, and `inherits` relations.
15. **Type references**: TypeScript `type_annotation` facts produce
    `references_type` candidate edges.
16. **Reports, wiki, communities, benchmark**: Markdown/JSON/wiki projections,
    connected candidate communities, and raw-vs-graph token comparison.
17. **Remaining grammars**: JavaScript, Go, Java, C, C++/headers, Ruby, C#, Kotlin,
    Scala, and PHP use one generic structural provider over their native trees.
18. **Content-hash refresh**: `content_manifest_digest` reads source bytes, so
    same-size edits invalidate a store snapshot; parsing is still skipped on a
    match.
19. **Watch and hooks**: `--watch` uses the notify filesystem watcher; `--hook
    install` adds post-commit, post-checkout, and post-merge hooks with a stable
    workspace id.
20. **Modularity communities**: local moving plus connected refinement, community
    labels, and a modularity value; not full Leiden.
21. **HTTP sessions**: `Mcp-Session-Id`, SSE responses when requested, optional
    bearer-token auth via `CODEL_HTTP_TOKEN`.
22. **All text exports**: HTML, SVG, GraphML, Neo4j Cypher, Obsidian, callflow.
23. **Rust type references**: field/parameter/return/static/const/type-alias
    annotations produce `references_type` facts and edges.
24. **Leiden phases**: dedicated module with queue local moving, well-connected
    refinement, aggregation, and best-partition tracking. It is deterministic and
    omits randomized restarts.
25. **Watch debounce**: event bursts are coalesced for 250ms before one refresh.
26. **LanguageResolver + import edges**: Java, JavaScript, C/C++, Go, C#, Ruby,
    PHP, Kotlin, and Scala imports resolve to workspace files and become inferred
    file-level import edges.
27. **Scoped symbol binding**: Java/JS/Kotlin/Scala/PHP/Go/C#/Ruby/C/C++ imported
    names bind to imported symbols; type references and calls become
    symbol-to-symbol edges instead of only file-level import edges.
28. **Member index**: declaration ranges produce owner-to-member containment edges,
    the first layer under method/field resolution.
29. **Java receiver table**: typed local/field/parameter facts plus receiver calls
    bind obj.method(), this.field.method(), generic base types, arity-selected
    overloads, and field reads/writes to field symbols. Overloads match arity plus
    known argument types; method-return and array-element receivers resolve
    chained field access; write spelling retains the operator. TypeScript, C#,
    Kotlin, Scala, and Go have receiver tables for typed variables/methods/calls.
    The shared projection and Java/C#/Kotlin/Scala/Go semantic facts all run as
    Tree-sitter QueryCursor passes. TypeScript and C# mark field writes and
    operators. SCIP indexes can be consumed through src/scip_semantics.rs and
    the codel_scip_import tool. Full type inference, generic substitution, and
    casts are not implemented.

## What "surpass" means here

Do not claim feature parity. Claim a stronger correctness contract on the axes
above. Gaps 1, 2, 3, 5, 6, 7, and 9 now have bounded, evidence-labelled
implementations; 13 grammars share one contract. The remaining work is depth, not
breadth: full Leiden refinement, language-specific precision for the generic
providers, OAuth/multi-tenant HTTP, and full Leiden bisection. A smaller graph that
can prove every edge, isolate every worktree, and survive a crash is more useful
to an agent than a larger graph that cannot.
