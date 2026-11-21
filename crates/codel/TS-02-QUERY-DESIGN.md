# TS-02 — Tree-sitter Query Registry and Execution Requirements

Status: Implemented (TS-02A and TS-02B)  
Crate: `crates/codel`  
Architecture prerequisite: TS-01 native `ParsedDocument`  
Scope: Native Tree-sitter query registration and bounded query execution  
Non-goal: A new syntax tree, AST, language-specific declaration model, or semantic compiler

## 1. Summary

TS-02 introduces a small, reusable query layer over the native Tree-sitter trees owned by `ParsedDocument`.

It has two implementation slices:

- **TS-02A — Query Registry:** compile, identify, version, and retrieve native Tree-sitter `Query` values against the exact grammar supplied by a `LanguageProvider`.
- **TS-02B — Query Execution:** execute a registered query against a compatible `ParsedDocument` using a fresh per-execution `QueryCursor`, shared cancellation/deadline control, and explicit result limits.

The query layer is a syntax access mechanism, not a second parser representation. Consumers continue to work with native Tree-sitter `Tree`, `Node`, `Query`, and `QueryCursor`. Query matches are consumed as native Tree-sitter results. Any later persisted/queryable evidence must remain generic, source-backed, and must not recreate a syntax hierarchy.

## 2. Goals

1. Compile query source against the actual grammar used by a provider.
2. Give every query a stable identifier and explicit version.
3. Prevent a query compiled for one provider/grammar version from being run against an incompatible parsed document.
4. Share compiled queries safely while keeping each mutable query cursor local to one execution.
5. Cooperate with the existing workspace cancellation/deadline budget.
6. Bound query work and retained output independently.
7. Preserve Tree-sitter's native match/capture semantics and source byte ranges.
8. Make failures, cancellation, partial output, and match-limit exhaustion explicit.
9. Keep the public API small and independent of BSH, Ferris, GPUI, LSP, compiler execution, and external services.

## 3. Non-goals

TS-02 does not:

- define `RustModule`, `RustImport`, `RustSymbol`, namespace, scope-tree, declaration, or other language-specific AST DTOs;
- translate Tree-sitter nodes into a parallel syntax model;
- perform compiler-equivalent name resolution, type checking, macro expansion, or Cargo target selection;
- execute project code, build scripts, compilers, package managers, or tests;
- introduce a graph database, vector database, LLM, LSP, SCIP, WASM runtime, or network service;
- persist native `Node` references or serialize a Tree-sitter tree;
- make a query result automatically mean that a cross-file relationship is semantically resolved.

## 4. Normative architecture constraints

### 4.1 Tree-sitter remains the only syntax tree

The native `tree_sitter::Tree` held by `ParsedDocument` is the syntax tree. A `Node` is a borrowed view into that tree. A `Query` and `QueryCursor` operate directly on those native nodes.

Do not add an intermediate AST, node wrapper hierarchy, or language-specific structure mirroring grammar productions. In particular, do not add custom Rust module/import/symbol structures as a consequence of query execution.

### 4.2 Generic query metadata is allowed

Registry metadata may include:

- stable query ID;
- query definition version;
- `LanguageId`;
- provider ID and exact provider version;
- the native compiled `Query`.

This is registry metadata, not syntax data.

A future generic query-evidence record may include query ID/version, capture name/index, workspace ID, relative file path, byte range, and coverage state. It must not own or emulate a Tree-sitter node tree. Evidence persistence is not required by TS-02A and should be a separate follow-up if a concrete retrieval use case needs it.

### 4.3 Provider grammar is authoritative

A query must be compiled with the same Tree-sitter `Language` used to parse the document. `LanguageId` alone is insufficient to establish grammar compatibility.

Extend `LanguageProvider` with a backward-compatible optional grammar accessor:

```rust
fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
    None
}
```

The Rust provider returns its native grammar:

```rust
fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
    Some(tree_sitter_rust::LANGUAGE.into())
}
```

The default `None` preserves source compatibility for existing third-party `LanguageProvider` implementations. Such providers remain usable for parsing but cannot register Tree-sitter queries until they expose their grammar.

The registry must obtain the grammar from the provider; callers must not pass an unrelated `Language` while separately claiming provider metadata.

### 4.4 Compatibility identity

A registered query is bound to this tuple:

`(LanguageId, provider_id, provider_version, query_id, query_version)`

At execution time, the document's language, provider ID, and provider version must exactly match the registered query's compatibility metadata. A mismatch is an error, not a best-effort execution.

The provider version must change whenever its grammar or query-relevant parser configuration changes. Query definition changes must increment the query version. Query versions are opaque caller-managed identifiers; TS-02 does not prescribe semantic-version parsing.

## 5. Proposed API shape

The following is a design target, not a claim that these APIs have already been implemented. Match crate naming and error conventions during implementation.

### 5.1 Registry

Suggested module: `crates/codel/src/syntax/query.rs`.

Conceptual API:

```rust
pub struct QueryRegistry {
    // Stable query IDs mapped to registered native queries.
}

pub struct RegisteredQuery {
    // Stable ID and version.
    // Language/provider compatibility metadata.
    // Native tree_sitter::Query, shared through Arc if needed.
}

impl QueryRegistry {
    pub fn new() -> Self;

    pub fn register(
        &mut self,
        provider: &dyn LanguageProvider,
        query_id: impl Into<String>,
        query_version: impl Into<String>,
        source: &str,
    ) -> Result<(), QueryRegistrationError>;

    pub fn get(&self, query_id: &str) -> Option<&RegisteredQuery>;
}
```

Requirements:

- Reject empty/invalid IDs and versions using a documented, deterministic validation rule.
- Reject duplicate query IDs. Replacement must be an explicit future API, not an accidental overwrite.
- Reject providers that do not expose a Tree-sitter grammar.
- Compile with `tree_sitter::Query::new(&language, source)`.
- Do not retain query source by default; the compiled native query and identity metadata are sufficient for execution. If diagnostics require source location, preserve only the compiler-reported location/offset, not source excerpts.
- Preserve useful compile-error metadata (for example, error kind and source position where provided by the pinned Tree-sitter binding) without echoing query source into error messages or logs.
- Do not store a `QueryCursor` in the registry.
- Registry mutation is setup-time work. Concurrent execution may share immutable registered queries; no mutable global cursor or global execution lock is allowed.
- Keep the native `Query` accessible to the execution module without introducing a wrapper around syntax nodes.

### 5.2 Query execution

TS-02B adds execution against `ParsedDocument`. Each invocation creates a new `tree_sitter::QueryCursor` and runs it against `document.root_node()`.

Conceptual API:

```rust
pub struct QueryExecutionLimits {
    pub max_matches: usize,
    pub max_captures: usize,
    pub match_limit: u32,
}

pub fn execute_query<'tree>(
    registered: &RegisteredQuery,
    document: &'tree ParsedDocument,
    control: &QueryControl<'_>,
    limits: &QueryExecutionLimits,
) -> Result<QueryExecution<'tree>, QueryExecutionError>;
```

The exact return shape must preserve native Tree-sitter query-match/capture access and lifetimes. Do not collect matches into a custom AST-like tree. If bounded collection is required, store only generic match/capture metadata and byte ranges; do not store independent `Node` objects beyond the lifetime of the parsed tree.

Execution requirements:

- Validate document/query compatibility before creating or running the cursor.
- Create one fresh cursor per invocation. Never share a cursor between threads or executions.
- Set Tree-sitter's match limit using `QueryCursor::set_match_limit`; reject invalid values outside the binding's supported range.
- Consume the streaming match/capture iterator incrementally. Enforce total match and capture caps while consuming; Tree-sitter's match limit is **not** a total-output limit.
- Observe cancellation/deadline through `QueryCursorOptions.progress_callback` and the existing shared budget.
- Stop retaining output immediately when a configured output cap is reached; return an explicit limited/partial state rather than silently truncating and reporting success.
- Report `QueryCursor::did_exceed_match_limit()` as a distinct condition.
- Keep all source ranges in the original document's byte coordinate space.
- Do not decode arbitrary capture bytes as UTF-8 unless the caller explicitly requests validated text from the document snapshot.
- No arbitrary query execution timeout claim: the pinned Tree-sitter API exposes cooperative progress callbacks, not a hard wall-clock preemption guarantee. The budget is cooperative, and query complexity/native allocation are not a hard RSS sandbox.

### 5.3 Range semantics

If query execution supports range restriction, expose the distinction explicitly:

- `set_byte_range`: restricts query execution to the specified byte range according to Tree-sitter's range semantics.
- `set_containing_byte_range`: requires matched nodes to be fully contained in the specified range.

Do not present these as interchangeable. Range boundaries must be validated against the document source length. Tests must cover matches crossing the requested boundary.

### 5.4 Result and error states

Execution must distinguish at least:

- complete execution;
- cancelled execution;
- shared deadline/budget exhaustion;
- output limit reached;
- Tree-sitter match limit exceeded;
- incompatible document/provider/query;
- invalid execution limits;
- query execution failure, if the binding exposes one.

Partial results must carry an explicit non-complete state and must never be labeled complete. Cancellation and limits must not be converted into empty successful results.

Registration errors must distinguish invalid identity, duplicate ID, missing grammar, and Tree-sitter query compilation failure. Error text must not include the complete query source.

## 6. Resource and security requirements

1. Query execution accepts only a `ParsedDocument` created from the existing bounded source pipeline.
2. It must not open files, traverse directories, or invoke project tools.
3. The shared workspace budget remains the authority for cancellation/deadline.
4. Each execution has independent match, capture, and in-progress match limits.
5. Enforce output limits during streaming; do not first collect an unbounded `Vec`.
6. A query can be computationally expensive even when its output is small. Cooperative cancellation is required, but do not claim a hard CPU, memory, or RSS limit that Tree-sitter does not provide.
7. Query compilation is setup-time work. Do not compile arbitrary model-generated query text in a hot path. Query definitions should be reviewed, versioned, and registered by trusted crate/application code.
8. Query IDs and versions are metadata, not filesystem paths or shell fragments.
9. Avoid logging source text or full query definitions by default.

## 7. Test requirements and acceptance criteria

### TS-02A — Registry

- [x] A Rust query compiles against the grammar exposed by `RustProvider`.
- [x] A syntactically invalid query returns a structured compilation error with useful location metadata when available.
- [x] A provider with the default `None` grammar accessor is rejected for query registration.
- [x] Empty/invalid query IDs and versions are rejected.
- [x] Duplicate IDs are rejected without replacing the original registration.
- [x] Query ID, query version, language, provider ID, and provider version are observable through documented metadata accessors.
- [x] The registry contains no `QueryCursor`.
- [x] Multiple registered queries can coexist.
- [x] Existing `LanguageProvider` implementations that do not override the new optional method continue to compile.

### TS-02B — Execution

- [x] A registered query matches expected Rust syntax using the native Tree-sitter tree.
- [x] Captures expose the original capture names and exact byte ranges.
- [x] A query registered for another provider/version is rejected before execution.
- [x] Each execution owns a fresh cursor; independent executions can run concurrently against compatible immutable documents/queries.
- [x] Match and capture caps are enforced incrementally and produce an explicit limited state.
- [x] Match-limit exhaustion is distinguishable from total output-cap exhaustion.
- [x] An already-cancelled budget prevents execution.
- [x] Cancellation during a sufficiently large query is observed through the progress callback.
- [x] Deadline/budget exhaustion is not reported as complete.
- [x] Byte-range and containing-byte-range behavior are tested separately, including boundary-crossing nodes.
- [x] Recovered documents remain queryable, with their parse coverage status retained and not silently upgraded to complete.
- [x] Query execution does not access the filesystem or execute project code.

### Crate quality

- [x] Public APIs have documentation and satisfy the crate's missing-docs policy.
- [x] Tests are deterministic and do not rely on timing-only assertions where a controlled large input can be used.
- [x] No new AST, language-specific syntax DTO, or persisted Tree-sitter node representation is introduced.
- [x] Existing scanner, parser, and `ParsedDocument` tests remain unchanged and pass.
- [x] Formatting, crate tests, clippy, and declared MSRV checks pass in CI.

## 8. Implementation plan

Keep TS-02 split into reviewable changes:

| Slice | Scope | Deliverable |
|---|---|---|
| TS-02A | Optional provider grammar accessor + native QueryRegistry | Query registration and compile-time validation only |
| TS-02B | QueryCursor execution + QueryControl + execution limits | Bounded, cancellable native query execution |
| TS-03 | Generic query evidence and retrieval integration | Only after a concrete consumer/use case is defined |

TS-02A should not add query-result DTOs or migrate the old `ParseResult` projection. TS-02B should not introduce cross-file semantic resolution. TS-03 decides whether generic evidence needs persistence, based on actual retrieval workflows.

## 9. Dependencies and compatibility

- Use the versions already pinned in the workspace lockfile: `tree-sitter = 0.26.13` and `tree-sitter-rust = 0.24.2` at the time of this design.
- Do not upgrade Tree-sitter as part of TS-02.
- Adding a defaulted method to the public `LanguageProvider` trait is source-compatible for existing implementers; adding a required method is not.
- The query registry is an additive crate capability. Do not add dependencies on BSH, Ferris, GPUI, or agent runtime crates.
- Re-evaluate exact method signatures against the pinned Tree-sitter crate source during implementation; do not substitute APIs from another Tree-sitter version.

## 10. Open decisions to resolve during implementation

1. **Registry mutability:** initial design uses exclusive mutable registration and immutable lookup. Add synchronization only if a real runtime registration use case requires it.
2. **Query ID convention:** recommend namespaced stable IDs such as `rust.items.v1`; keep validation simple and avoid imposing a URI framework.
3. **Output representation:** prefer streaming native matches for internal consumers. Add a generic bounded evidence representation only when a caller needs ownership beyond the query iterator's borrow.
4. **Query source retention:** default to not retaining source; if diagnostics or reproducible query packs require it, store it explicitly as a separate opt-in.
5. **Native tree memory:** query limits do not cap the memory already occupied by `ParsedDocument`'s Tree-sitter tree. This remains governed by source-size limits and the existing parser resource design.

## 11. Definition of done

TS-02 is complete only when TS-02A and TS-02B meet their acceptance criteria, CI validates the declared MSRV and core workspace, and the public API documentation makes provider compatibility, query lifetime, cancellation, range semantics, and partial-result behavior unambiguous.

A successful query match is syntax evidence only. It must never be represented as compiler-resolved semantics without an independent resolution mechanism and explicit supporting evidence.

## 12. Implementation notes

TS-02A and TS-02B are implemented in `src/syntax/query.rs`.

- `LanguageProvider::tree_sitter_language` is the backward-compatible
  grammar accessor; `RustProvider` returns `tree_sitter_rust::LANGUAGE`.
- `QueryRegistry` compiles queries eagerly against the provider grammar and
  stores only identity metadata plus an `Arc<tree_sitter::Query>`.
  Registration requires exclusive mutation; lookup and execution share
  immutable queries.
- `RegisteredQuery` exposes the query ID/version, language, provider
  ID/version, capture names, pattern count, and the native `Query`.
- `execute_query` validates language, provider ID, and provider version
  before creating a fresh `QueryCursor`, applies `QueryExecutionOptions`,
  and returns a bounded `QueryExecution`.
- `QueryExecutionStatus` distinguishes complete, cancelled, deadline expiry,
  retained-output caps (`Matches` and `Captures`), and Tree-sitter
  match-limit exhaustion. `QueryExecution::document_status` preserves a
  recovered document's parse coverage.
- `QueryRangeMode` exposes `set_byte_range` and
  `set_containing_byte_range` as distinct modes. Ranges are validated
  against the document snapshot before the cursor runs.
- Registration errors distinguish invalid identity, duplicate ID, missing
  grammar, and Tree-sitter compilation failures with kind, row, column, and
  offset. Query source is neither retained nor echoed.

The open decisions in section 10 are resolved as follows: registration stays
exclusively mutable; IDs use the simple namespaced convention; execution
returns bounded generic match/capture records because a native query iterator
cannot outlive the local cursor it borrows; query source is not retained; and
query limits do not change the memory already owned by the parsed tree.
