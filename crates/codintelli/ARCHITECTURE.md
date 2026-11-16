# codintelli Code Intelligence Architecture

Status: Proposed  
Scope: Independent Rust code-intelligence engine in `crates/codintelli`  
Reference: Graphify-Labs/graphify (one optional reference; not a requirements baseline)

## 1. Purpose

Build an independent, resource-bounded code intelligence engine that turns a workspace into useful, verifiable context for tools and AI agents.

The primary product is not a graph. It is a set of composable capabilities that help answer practical codebase questions:

- Find relevant files, symbols, declarations, and references.
- Understand source structure and relationships when a language provider can establish them.
- Retrieve bounded, source-grounded context for a task.
- Explain where a result came from and how certain the relationship is.
- Identify likely change impact when enough dependency information exists.
- Update only the affected parts of an index as files change.

The engine may use several complementary representations: file inventory, text/search indexes, symbol tables, syntax facts, dependency records, and graph-shaped relationship views. No single representation is mandatory for every language or query.

The engine remains independent of BSH, Ferris, GPUI, MCP, and any particular agent runtime. Those systems may consume it through a future adapter.

## 2. Design principles

1. **Use-case driven.** Add an index or representation only when it improves a defined code-intelligence query or workflow.
2. **Layered fidelity.** Start with safe file discovery and text retrieval; add language-aware structure and resolution progressively. Unsupported languages must still have useful baseline behavior.
3. **Evidence first.** Every structural result should be traceable to a workspace-relative source location. Do not present a heuristic as a parser-established fact.
4. **Incremental by source unit.** Recompute only affected records where practical. A failed update must not destroy the last known-good index.
5. **Partial is explicit.** Budget exhaustion, cancellation, unsupported syntax, parser errors, and I/O failures must be observable. Partial results must never be labeled complete.
6. **Bounded and non-executing.** Indexing never runs project code, build scripts, tests, package-manager hooks, or compiler plugins.
7. **Portable identity.** Persisted paths are workspace-relative. Machine-specific absolute paths are not part of index identity.
8. **Replaceable internals.** Extraction, retrieval, relationship analysis, persistence, and host integration have explicit seams.
9. **No premature infrastructure.** Do not require a graph database, vector database, LLM, language server, or external service for the core MVP.

## 3. Reference use: Graphify

Graphify is useful as a reference for selected ideas, especially AST-grounded extraction, traceable relationships, incremental processing, and explainable graph queries. It is not the target architecture and does not define the codintelli feature set.

We should evaluate each idea against codintelli's own goals:

- Adopt a concept only when it solves a concrete code-context problem.
- Prefer Rust-native, typed contracts and existing BOS workspace conventions.
- Keep source extraction independent from relationship presentation.
- Keep graph traversal as one possible query strategy, not the universal storage model.
- Do not require Python, NetworkX, Agent Skills, report generation, clustering, visualization, or an LLM.
- Do not copy Graphify's command structure, data schema, or dependency choices without independent evaluation.

Reference: https://github.com/Graphify-Labs/graphify

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
- Classify files deterministically from workspace-relative names and extensions in the inventory stage. Content sniffing, when needed for ambiguous formats, belongs to a later bounded source-reading stage.
- Enforce one shared `ScanBudget` for the entire scan.
- Produce deterministic workspace-relative file records.

Initial built-in exclusions include VCS internals and common generated/dependency directories such as `.git/.hg/.svn`, `target`, `node_modules`, `dist`, `build`, `.next`, `.turbo`, common Python virtual environments, and language-tool caches. Exclusions are configurable through ordered path-prefix, directory-name, and file-extension rules; the last matching rule wins. Built-ins are the initial exclusion layer and can be overridden by later explicit include rules. When an include targets content beneath an ignored directory, the scanner traverses that directory without returning it as an indexed entry. Ignored entries still count toward the global directory-entry budget, while excluded files do not consume the accepted-file budget. The initial policy is deliberately not a `.gitignore` parser; nested `.gitignore` discovery and full Git ignore semantics require a separate design.

The scanner must return a typed completion state: `Complete`, `Partial`, `Cancelled`, or `Failed`, with usage and diagnostics. A budget-exceeded scan is partial, never complete.

### 4.2 Bounded source loading and syntax extraction

The initial source pipeline is implemented as a synchronous, sequential operation intended to run on a bounded host worker:

1. Run the inventory scanner with the same shared ScanBudget.
2. Classify accepted files using workspace-relative names and extensions.
3. Select a registered LanguageProvider for source files.
4. Reserve the no-follow metadata size against the aggregate byte budget before reading.
5. Read through WorkspaceFs with a per-file limit equal to the reserved size.
6. Parse only the bounded in-memory bytes and retain workspace identity, relative path, provider version, schema version, facts, and diagnostics.

A file that grows between enumeration and reading is rejected by the bounded read. Byte reservations are conservative and are not refunded after read or parse failure. Unsupported languages remain visible as unsupported results; they are not silently dropped from coverage accounting. Documentation, configuration, data, markup, style, and asset files remain in inventory but are not sent to source parsers by the initial pipeline.

The Rust provider observes the shared cancellation flag and deadline through Tree-sitter's parse-progress callback, then checks them again during iterative syntax-tree projection. If Tree-sitter aborts, its parser state is reset before reuse. Other providers that rely on the default trait method are checked before and after parsing; they must override the controlled-parse method to provide mid-parse cancellation.

Per-file ParseLimits bound visited syntax nodes (1,000,000 by default), retained facts (100,000), and retained diagnostics (10,000). A limit produces an explicit Limited parse status, preserves facts already extracted, and records the exact limit reason. The iterative traversal also bounds its pending-node stack. The input byte limit remains the first bound on Tree-sitter's native syntax-tree allocation; the node/fact limits bound codintelli's projection and retained output. No project commands, compilers, build scripts, package managers, or tests are executed.

### 4.3 Language provider registry

A language provider is responsible for parsing a supported language and producing language-neutral facts. The core should not depend on a particular language grammar.

Initial implementation order:

1. Rust — first-class target because BOS itself is Rust.
2. TypeScript / JavaScript.
3. JSON, TOML, YAML and common manifest/config formats as data sources, not executable inputs.
4. Additional languages through independently testable providers.

Tree-sitter is the selected parser substrate for the first Rust provider. Additional language grammars should be added behind the same provider contract and evaluated independently for build portability, grammar maintenance, and resource behavior. A provider must declare its language ID, supported file kinds, parser version, and extraction schema version.

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

### 4.5 Symbol candidate indexing (initial CI-09B slice)

The initial resolver builds a deterministic declaration catalog from source-backed syntax facts and associates function calls, method calls, and a narrow subset of simple Rust imports with same-name declaration candidates. Candidate outcomes are explicit: no candidates, one candidate, multiple candidates, or unsupported import syntax. A single candidate is not labeled a resolved edge.

This is intentionally conservative. The current Rust facts do not yet carry full lexical scope, module path, visibility, receiver type, trait-in-scope, or macro-expansion information. Consequently, same-name matching is useful for navigation and retrieval but is not semantic Rust name resolution. Grouped, glob, and aliased imports remain unsupported until the Tree-sitter provider emits structured import-path facts. Mixed workspace identities are rejected.

### 4.6 Rust module declaration facts (CI-09D)

The Rust provider emits typed module declarations in addition to generic syntax facts. Each declaration records its lexical inline-module scope, declared name, inline-versus-external form, and source range. This separates `mod child { ... }` from `mod child;`, which is required before attempting filesystem-based module lookup. These facts do not claim that an external module file exists, and do not yet account for `#[path]`, conditional compilation, macro-generated declarations, or Cargo target selection. Those conditions must remain explicit uncertainty in the later resolver rather than being guessed.

### 4.7 Rust module path candidates (CI-09E)

The module candidate mapper consumes typed module declarations and the caller-provided workspace inventory. It checks both conventional layouts (`name.rs` and `name/mod.rs`) using the source-file directory rules. Crate roots are explicit host input; codintelli does not guess Cargo targets or execute Cargo metadata commands. Inline module scopes are reflected in candidate paths. If both conventional files exist, ambiguity is preserved. If neither exists, the result is `NoCandidate`, not a compiler error. Declarations with module-resolution-affecting `cfg`, `cfg_attr`, or `path` attributes are skipped as uncertain; unrelated documentation or lint attributes do not suppress conventional lookup. A unique filesystem candidate is still only a path candidate, not a semantic or compiler-resolved module edge.

### 4.8 Crate-root-anchored module graph (CI-09F)

The graph builder starts only from crate roots supplied by the host. Each root has an independent logical module namespace, even when multiple roots reference the same source file. Inline module declarations create in-file module nodes; external declarations are followed only when exactly one conventional candidate exists and that candidate has a Rust parse result. Ambiguous, missing, attribute-dependent, and unparsed targets remain explicit declaration edges. Recovered, limited, failed, or missing target parses make graph coverage partial. Inline modules guarded by unresolved `cfg`/`path` attributes are not materialized, and declarations nested beneath those scopes are not traversed. Configurable node, edge, and logical-depth limits bound output. This is a navigation/retrieval graph, not compiler-equivalent module resolution: Cargo target discovery, `cfg` evaluation, macro-generated modules, and `#[path]` remain outside this stage.

### 4.9 Rust import-to-module context (CI-09G)

The import mapper consumes structured `ImportFact` records and the crate-root module graph. It supports explicit `crate::`, `self::`, and repeated `super::` roots, resolves only the longest known module prefix, and returns remaining path segments as member-path candidates. It preserves aliases and glob markers. Bare/unqualified paths are left unresolved because they may refer to the extern prelude or language-specific lexical rules. Multiple module nodes with the same logical path remain ambiguous. The mapper also propagates unresolved graph-edge states: attribute uncertainty, unparsed targets, depth limits, and ambiguous declarations are not collapsed into `NoModuleCandidate`. A module-prefix match does not resolve the imported symbol; symbol visibility, re-exports, macro expansion, dependency crates, and compiler configuration remain outside this stage.

### 4.10 Crate-root-aware scoped Rust symbol catalog (CI-09H)

The Rust provider now attaches lexical inline-module scope to each syntax fact and
marks whether the fact is in module scope or inside a function/block-local
context. The scoped symbol catalog combines those facts with crate-root-specific
module graph nodes. Its identity includes the crate root, logical module path,
source path, source range, and declaration kind. A source file shared by two
crate roots therefore yields distinct symbols in the two logical namespaces.

The first slice indexes module-scope functions, methods, types, traits, aliases,
constants, and statics. Function/block-local declarations are excluded rather
than incorrectly promoted to module scope. The catalog retains recovered or
limited source paths as coverage metadata. It is still a structural catalog,
not Rust name resolution: visibility, cfg selection, re-exports, macro-generated
items, extern-prelude lookup, trait scope, and receiver/type-based method
resolution remain unresolved. The next slice should connect structured imports
to these scoped declarations while preserving candidate and uncertainty states.

### 4.11 Rust import-to-scoped-symbol candidates (CI-09I)

The import-symbol layer joins `RustImportResolutionReport` with
`RustScopedSymbolReport`. For a module-prefix import with exactly one remaining
member segment, it matches the declaration name inside the exact crate-root and
module-path namespace. Explicit aliases become the candidate's local import
name. Duplicate declarations remain ambiguous.

Glob imports return direct module-scope declarations as a candidate set; they
do not claim those declarations are actually imported because visibility and
re-export rules are not evaluated. Impl methods are excluded from module-level
matching because the current syntax facts do not yet preserve their containing
type/impl namespace. Multi-segment member paths remain unsupported rather than
being guessed as associated-item resolution.

Every result retains the underlying module-resolution status. A missing symbol
is reported as conclusive only when the target source and graph are complete;
otherwise the result is marked incomplete. A unique candidate is not a
compiler-resolved symbol. Visibility, cfg selection, re-exports, macro
expansion, extern-prelude lookup, trait scope, enum-variant imports, and
type-dependent associated-item resolution remain future work.

### 4.12 Declared Rust visibility facts (CI-09J)

The Tree-sitter provider records the visibility modifier attached to Rust
module-level declarations and `use` declarations: implicit private visibility,
`pub`, `pub(crate)`, `pub(super)`, `pub(self)`, and `pub(in path)`. Unknown
modifier spellings are retained as unknown evidence rather than silently
converted to private. The provider schema version is bumped when this fact
contract changes.

The scoped symbol catalog carries the declaration's written visibility. This
is deliberately not an effective-accessibility result: a public declaration
can still be unreachable through a private ancestor module, and `cfg`,
conditional compilation, re-exports, and the importing module's lexical
position still affect accessibility. The import candidate resolver therefore
continues to report source-backed candidates, not valid or compiler-resolved
imports. Visibility-aware accessibility is a separate stage and must account
for every ancestor module and the complete path from the crate root.

Trait members have no independent visibility modifier; their visibility fact
remains absent rather than being mislabeled as private. The first visibility
slice does not yet model field visibility, enum-variant visibility, macro
visibility, or effective privacy across re-export chains.
Those require explicit namespace and containment facts rather than inference
from declaration names alone.

### 4.13 Cross-file resolution

Resolution is a separate stage over extracted facts and workspace metadata. It may resolve:

- module/import paths;
- qualified names;
- explicit type references;
- call targets where lexical and type information is sufficient.

Resolution output must retain its own provenance and diagnostics. Ambiguous candidates remain ambiguous; they are not collapsed to an arbitrary target.

### 4.14 Relationship analysis and query

Relationship analysis is a deterministic transformation over extracted facts. A graph-shaped view may be built when it helps a query, but the underlying index must not require all records to be represented as graph nodes and edges. This stage must not perform parsing or filesystem access.

Initial query operations:

- `symbol_search(query)`
- `node(node_id)`
- `neighbors(node_id, relation_filter, direction, limit)`
- `explain(node_id)`
- `path(source, target, max_hops)`
- `references(node_id)`
- `impact(node_id, relation_policy, depth)`

Query results should return evidence references and completeness/coverage metadata, not only labels and IDs.

## 4.7 First syntax provider: Rust (Tree-sitter)

The first provider uses the upstream Tree-sitter runtime and upstream Rust
grammar directly. Tree-sitter owns tokenization, parsing, syntax-tree
construction, and error recovery; codintelli does not implement a Rust parser.
The initial locked versions are `tree-sitter 0.26.13` and
`tree-sitter-rust 0.24.2`, selected because both are already represented in
the workspace lockfile.

The provider contract is host-neutral and consumes only already-bounded UTF-8
bytes. The caller must use `WorkspaceFs` and reserve file/aggregate bytes with
`ScanBudget` before parsing. The provider does not read files, inspect Cargo
metadata, invoke rustc, or execute workspace-controlled code.

Tree-sitter error recovery is preserved: syntax facts from recoverable regions
remain available and the result is marked `Recovered` with source-ranged
diagnostics. Facts cover modules, free functions, methods, structs, enums,
unions, traits, impl blocks, type aliases, constants, statics, imports, direct
function-call expressions, and method-call expressions. Calls are syntax
observations only; they are not resolved to declarations. Each fact carries a
half-open byte range into the original UTF-8 source. Provider ID, parser and
grammar versions, schema version, workspace ID, and relative path are retained
so persisted results can be invalidated or partitioned correctly.

Tree-sitter's Rust runtime and generated grammar include native C build
components. This is an explicit dependency trade-off for using the requested
parser rather than maintaining parser logic in codintelli; CI must validate
supported Windows, macOS, Linux, and MSRV targets.

## 5. Structural facts and optional graph view

The exact Rust types will be introduced in a later implementation change. The following is a candidate contract for structural facts and an optional graph projection, not a mandate to model every indexed item as a graph entity.

### 5.1 Node

In the optional graph projection, a node represents a source-backed entity or an explicitly modeled workspace artifact.

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

In the optional graph projection, an edge represents a directed relationship between two nodes.

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

Do not select a database prematurely, and do not assume the graph projection is the primary persisted format.

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

The source pipeline is synchronous and sequential today. Hosts must run it on bounded background workers; a future worker-pool API must preserve one shared budget, deterministic result ordering, cooperative cancellation, and explicit partial coverage. A future Ferris/Builtin adapter can map these operations into host tools without making the engine depend on Ferris.

## 10. Delivery sequence

| Stage | Deliverable | Acceptance gate |
|---|---|---|
| CI-01 | Standalone crate skeleton | Workspace builds; no host dependencies |
| CI-02 | Workspace identity | Stable host-owned workspace identity contract |
| CI-03 | Confined filesystem and shared budgets | No-follow access; bounded enumeration; cross-platform tests |
| CI-04 | Recursive scanner | Global entry/depth/time/byte budgets; deterministic file inventory; explicit partial status |
| CI-05 | Ignore rules and file classification | Configurable exclusions; deterministic classification |
| CI-06 | Typed source facts and relationship contracts | Versioned declarations/references/evidence; optional graph projection; validation tests |
| CI-07 | Parser/provider seam + Rust provider | Deterministic source facts with source evidence |
| CI-08 | Bounded source-to-facts pipeline | Inventory, classification, reserved reads, provider dispatch, explicit coverage |
| CI-09A | Cooperative parser cancellation and output budgets | Tree-sitter progress cancellation; bounded nodes/facts/diagnostics; explicit limit reasons |
| CI-09B | Symbol candidate catalog and conservative references | Deterministic declaration IDs; unique/ambiguous/unresolved candidates; no false resolved edges 
| CI-09C | Structured Rust import facts | Tree-sitter import paths, grouped expansion, aliases, globs, lexical scope and source evidence |
| CI-09D | Rust module declaration facts | Distinguish inline and external declarations, retain lexical scope and source evidence; no false file-resolution claim |
| CI-09E | Conservative Rust module path candidates | Per-root conventional file candidates; preserve ambiguity and cfg/path uncertainty |
| CI-09F | Crate-root-anchored module graph | Separate crate namespaces; bounded traversal of unique parsed candidates; explicit partial coverage |
| CI-09G | Rust import-to-module context | Resolve explicit crate/self/super module prefixes; propagate unresolved graph-edge states; no false symbol resolution |
| CI-10 | Incremental extraction store | Atomic per-file replacement; last-known-good preservation |
| CI-11 | Query API | File/symbol search, source retrieval, explain, references, optional neighbors/path/impact |
| CI-12 | Optional export and host adapter | Versioned exports; integration remains outside engine |

Each stage should be independently reviewable and normally fit within a small PR. Do not implement graph persistence, semantic LLM extraction, clustering, or visualization before concrete query requirements justify them. Stabilize source-fact and evidence contracts first.

## 11. Open decisions

1. Which code-context queries should be optimized first, and which indexes are justified by them?
2. Tree-sitter grammar distribution: statically linked per-language crates versus feature-gated provider crates.
2. Stable node ID canonicalization for overloaded functions and anonymous declarations.
3. Whether graph snapshots use a directed multigraph internally or normalized node/edge tables.
4. Which embedded persistence backend best meets write amplification, incremental replacement, and cross-platform requirements.
5. Whether document semantic enrichment belongs in codintelli or a separate optional provider crate.

These decisions should be resolved with focused prototypes and benchmarks, not by copying another project's dependency choices.
