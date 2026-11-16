# codintelli Knowledge Graph Architecture

Status: Proposed  
Scope: Independent Rust code-intelligence engine in `crates/codintelli`  
Reference: Graphify-Labs/graphify (architecture and product concepts only; no implementation copied)

## 1. Purpose

Turn a workspace into a deterministic, explainable, incrementally maintainable code knowledge graph.

The graph is a first-class representation of a codebase, not a vector index and not a collection of LLM-generated summaries. It should let callers answer:

- What symbols are defined in this workspace, and where?
- Which modules import, call, implement, extend, or reference a symbol?
- What is the dependency/call path between two symbols?
- What may be affected by a proposed change?
- Which facts came directly from source syntax, and which were resolved or inferred?

The engine remains independent of BSH, Ferris, GPUI, MCP, and any particular agent runtime. Those systems may consume the engine through a future adapter.

## 2. Design principles

1. **Deterministic first.** Source-code structure is extracted locally from parsers. The same source snapshot and engine/parser versions should produce the same structural graph.
2. **Evidence on every fact.** Nodes and edges retain source provenance. A consumer must be able to navigate from a graph fact back to the source range that supports it.
3. **Extracted is not inferred.** Syntax directly present in source is distinct from cross-file resolution or heuristic inference. Never silently promote an unresolved guess to a fact.
4. **Incremental by source unit.** A changed file is re-extracted and its outgoing evidence replaced atomically. A failed extraction must preserve the last known-good records for that file.
5. **Partial is explicit.** A scan or graph build that hit a budget, cancellation, parser failure, or I/O error must not be reported as complete.
6. **Bounded and non-executing.** Indexing never runs project code, build scripts, tests, package-manager hooks, or compiler plugins. All filesystem reads pass through `WorkspaceFs` and shared scan budgets.
7. **Portable identity.** Persisted IDs and paths are workspace-relative. Never persist machine-specific absolute paths.
8. **Storage is replaceable.** Extraction, graph semantics, persistence, and query APIs must not be coupled to one database or serialization format.

## 3. Reference analysis: what to adopt from Graphify

Graphify demonstrates a useful pipeline:

`detect → extract → build → analyze → report/export`

It also demonstrates several valuable product-level ideas:

- AST-based structural extraction can be deterministic and local.
- A graph supports explain, neighborhood, and path queries without requiring embeddings.
- Explicit `EXTRACTED` versus `INFERRED` edge provenance makes graph claims auditable.
- A persisted schema version is necessary for compatibility checks.
- Per-file extraction and caching are the basis for incremental refresh.

codintelli should adopt these concepts, but not Graphify's implementation choices as requirements. In particular:

- Keep the core in Rust; do not port its Python module structure literally.
- Use typed Rust data contracts rather than loosely structured dictionaries.
- Keep parsers behind a language-provider seam; do not make a large fixed grammar set a prerequisite for the engine.
- Do not require an LLM for source-code extraction. Optional semantic enrichment for documentation can be a later provider.
- Do not make NetworkX, HTML visualization, MCP serving, clustering, or a vector database part of the core.
- Treat external tools and language servers as optional providers/adapters, never as mandatory runtime dependencies.

Reference repository: https://github.com/Graphify-Labs/graphify

## 4. Layered architecture

```text
WorkspaceFs + ScanBudget
        |
        v
Workspace Discovery ----> Ignore / File Classification
        |
        v
Source Snapshot + Fingerprints
        |
        v
Language Provider Registry
        |
        v
Deterministic AST Extraction
        |
        v
Local Facts (declarations, references, imports, scopes)
        |
        v
Cross-file Resolution
        |
        v
Typed Knowledge Graph + Evidence
        |
        +------> Incremental Graph Store
        |
        +------> Query / Explain / Path / Impact
        |
        +------> Export adapters (JSON, GraphML, visualization, MCP)
```

### 4.1 Workspace discovery

Responsibilities:

- Traverse only through `WorkspaceFs` no-follow APIs.
- Apply configurable ignore rules and built-in exclusions.
- Classify files by path, extension, and bounded content sniffing where necessary.
- Enforce one shared `ScanBudget` for the entire scan.
- Produce deterministic workspace-relative file records.

Initial built-in exclusions should include VCS internals and common generated/dependency directories (for example `.git`, `target`, `node_modules`, and common build output directories). Exclusions must be configurable; they must not silently hide user source directories.

The scanner must return a typed completion state: `Complete`, `Partial`, `Cancelled`, or `Failed`, with usage and diagnostics. A budget-exceeded scan is partial, never complete.

### 4.2 Source snapshot and fingerprints

Each file record should include:

- normalized workspace-relative path;
- detected language/provider;
- byte length;
- content fingerprint (cryptographic digest);
- last successful extraction version;
- extraction status and diagnostic summary.

Fingerprints are content-derived, not absolute-path-derived. File identity for graph nodes must remain stable across moving the same workspace to another machine.

### 4.3 Language provider registry

A language provider is responsible for parsing a supported language and producing language-neutral facts. The core should not depend on a particular language grammar.

Initial implementation order:

1. Rust — first-class target because BOS itself is Rust.
2. TypeScript / JavaScript.
3. JSON, TOML, YAML and common manifest/config formats as data sources, not executable inputs.
4. Additional languages through independently testable providers.

Tree-sitter is a strong candidate for the parser substrate, but the parser API and grammar dependency strategy must be evaluated before freezing the public provider contract. A provider must declare its language ID, supported file kinds, parser version, and extraction schema version.

### 4.4 Structural extraction

The first extraction tier is syntax-grounded and local:

- modules / namespaces;
- functions and methods;
- structs, classes, enums, traits, interfaces;
- fields and parameters where useful;
- imports and exports;
- calls and type references when syntax makes the target explicit;
- inheritance, implementation, and containment relations.

Extraction must not require package resolution, compiler execution, or build-script execution. When syntax does not identify a target unambiguously, preserve an unresolved reference fact rather than inventing a target node.

### 4.5 Cross-file resolution

Resolution is a separate stage over extracted facts and workspace metadata. It may resolve:

- module/import paths;
- qualified names;
- explicit type references;
- call targets where lexical and type information is sufficient.

Resolution output must retain its own provenance and diagnostics. Ambiguous candidates remain ambiguous; they are not collapsed to an arbitrary target.

### 4.6 Graph assembly and query

Graph assembly is a deterministic transformation from typed facts to a graph snapshot. It must not perform parsing or filesystem access.

Initial query operations:

- `symbol_search(query)`
- `node(node_id)`
- `neighbors(node_id, relation_filter, direction, limit)`
- `explain(node_id)`
- `path(source, target, max_hops)`
- `references(node_id)`
- `impact(node_id, relation_policy, depth)`

Query results should return evidence references and completeness/coverage metadata, not only labels and IDs.

## 5. Graph data model

The exact Rust types will be introduced in a later implementation change. This section defines the semantic contract.

### 5.1 Node

A node represents a source-backed entity or an explicitly modeled workspace artifact.

Required semantic fields:

- `id`: deterministic opaque ID;
- `kind`: module, function, method, type, field, constant, package, file, config, document, etc.;
- `name`: display name;
- `qualified_name`: optional language-qualified name;
- `source`: workspace-relative path and source range, when applicable;
- `language`: optional language identifier;
- `attributes`: versioned, typed extension data.

A node ID must not contain an absolute filesystem path. The initial stable-ID recipe should use workspace-relative path, language, entity kind, qualified name, and a deterministic disambiguator for same-name declarations. Line numbers alone must not be the identity because unrelated edits can shift them.

### 5.2 Edge

An edge represents a directed relationship between two nodes.

Initial relation vocabulary:

- `contains`
- `defines`
- `imports`
- `exports`
- `calls`
- `references`
- `type_uses`
- `implements`
- `extends`
- `depends_on`

Each edge must include:

- source and target node IDs;
- relation kind;
- provenance: `Extracted`, `Resolved`, or `Inferred`;
- confidence/ambiguity state;
- one or more evidence references;
- extractor/provider identity and version.

`Extracted` means the relation is directly supported by syntax or explicit metadata. `Resolved` means an explicit source reference was linked to a concrete target by deterministic workspace resolution. `Inferred` is reserved for heuristic or semantic derivations and must never be mixed with extracted facts in query results without an explicit filter.

### 5.3 Evidence

Evidence is a first-class record, not a free-form explanation string.

It should contain:

- workspace-relative source path;
- byte range (start inclusive, end exclusive);
- line/column range when available;
- evidence kind (declaration, import statement, call expression, type reference, manifest declaration, etc.);
- optional excerpt hash (not necessarily the source text);
- provider and provider version.

Source text should be fetched through the workspace capability when a consumer needs to display a snippet. Persisting arbitrary excerpts increases storage and secret-retention risk.

## 6. Incremental indexing and failure semantics

Per-file extraction is an independent unit of work.

For each source file:

1. Read through `WorkspaceFs` with a strict byte limit.
2. Verify the observed bytes do not exceed the reservation and configured limits.
3. Compute the content fingerprint.
4. Skip extraction only when the fingerprint and all relevant provider/schema versions match.
5. Parse and produce a complete file-local fact set.
6. Validate IDs, evidence ranges, and edge endpoints.
7. Commit the file's replacement facts atomically.

A failed or cancelled extraction must not delete the last known-good facts. A file that was successfully read and deliberately determined to contain no indexable entities may replace its previous facts with an empty set. These are different outcomes and must be represented separately.

Graph-level status must include:

- scan completion state;
- number of discovered, skipped, parsed, failed, and unchanged files;
- provider-level diagnostics;
- graph schema version;
- engine version;
- snapshot/fingerprint identity.

## 7. Persistence strategy

Do not select a database prematurely.

First define a storage-neutral graph snapshot and per-file extraction records. Then benchmark:

- append-only extraction journal plus compacted snapshot;
- embedded key/value storage;
- SQLite or another embedded relational store;
- JSON/JSONL export for debugging and interoperability.

The production store must support atomic per-file replacement, bounded memory use, schema migration, corruption detection, and deterministic rebuild. The public engine API must not expose storage-specific handles.

A persisted graph must carry at least:

- `schema_version`;
- `engine_version`;
- provider IDs and versions;
- workspace identity supplied by the host;
- graph snapshot fingerprint.

## 8. Security and resource boundaries

- The workspace root is selected and authorized by the host.
- After opening, all reads use the retained `WorkspaceFs` capability.
- No symlink traversal or path escape is permitted.
- Every directory entry, including ignored or unsupported entries, must count against the aggregate scan budget.
- File count and byte reservations happen before content reads.
- Parsing must be bounded by file size, aggregate bytes, depth, wall-clock deadline, and cancellation.
- Parser crashes or malformed input must be contained to the file/provider boundary.
- No shell, compiler, build system, package manager, test runner, or project-defined plugin is invoked.
- Logs and diagnostics must avoid leaking source contents or secrets by default.

## 9. API boundary

The engine should expose host-neutral operations similar to:

- create/open a workspace context from a host-provided `WorkspaceDescriptor`;
- start a bounded scan/index operation;
- request cooperative cancellation;
- inspect scan/index status and diagnostics;
- query graph snapshots and evidence;
- export a versioned snapshot.

The exact sync/async boundary is intentionally not frozen here. The core indexing pipeline should be usable from synchronous library callers while allowing hosts to run it on bounded background workers. A future Ferris/Builtin adapter can map these operations into host tools without making the engine depend on Ferris.

## 10. Delivery sequence

| Stage | Deliverable | Acceptance gate |
|---|---|---|
| CI-01 | Standalone crate skeleton | Workspace builds; no host dependencies |
| CI-02 | Workspace identity | Stable host-owned workspace identity contract |
| CI-03 | Confined filesystem and shared budgets | No-follow access; bounded enumeration; cross-platform tests |
| CI-04 | Recursive scanner | Global entry/depth/time/byte budgets; deterministic file inventory; explicit partial status |
| CI-05 | Ignore rules and file classification | Configurable exclusions; deterministic classification |
| CI-06 | Typed graph schema and validation | Versioned node/edge/evidence contracts; validation tests |
| CI-07 | Parser/provider seam + Rust provider | Deterministic source facts with source evidence |
| CI-08 | Symbol/import resolution | Resolved edges with ambiguity preserved |
| CI-09 | Incremental graph store | Atomic per-file replacement; last-known-good preservation |
| CI-10 | Query API | Search, explain, neighbors, path, references, impact |
| CI-11 | Optional export and host adapter | Versioned exports; integration remains outside engine |

Each stage should be independently reviewable and normally fit within a small PR. Do not implement graph persistence, semantic LLM extraction, clustering, or visualization before the graph schema and evidence contracts are stable.

## 11. Open decisions

1. Tree-sitter grammar distribution: statically linked per-language crates versus feature-gated provider crates.
2. Stable node ID canonicalization for overloaded functions and anonymous declarations.
3. Whether graph snapshots use a directed multigraph internally or normalized node/edge tables.
4. Which embedded persistence backend best meets write amplification, incremental replacement, and cross-platform requirements.
5. Whether document semantic enrichment belongs in codintelli or a separate optional provider crate.

These decisions should be resolved with focused prototypes and benchmarks, not by copying another project's dependency choices.
