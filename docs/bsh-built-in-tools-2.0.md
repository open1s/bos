# BSH Built-in Tools 2.0

## Goals

Built-in Tools 2.0 optimizes for reliable model execution, not tool count:

1. Make tool arguments unambiguous through descriptive, bounded schemas.
2. Reduce model/tool round-trips for common multi-file work.
3. Keep every operation inside the existing workspace and policy boundary.
4. Bound tool-result bytes before they expand the model context or transcript.
5. Measure model behavior instead of assuming a schema change improved it.

## File tool surface

| Tool | Intended use | Model-efficiency rule |
|---|---|---|
| `glob` | Discover paths | Path matching only; use `grep` for contents |
| `grep` | Search file contents | Use `read_files` for several known matches |
| `read_file` | Read one file | Use line windows for large files |
| `read_files` | Read up to 10 files | One call; shared offset/limit; 1 MiB aggregate content budget |
| `write_file` | Create or replace a file | Prefer targeted edits when replacing existing content |
| `edit_file` | One exact-text edit | Old text must be unique unless replace-all is explicit |
| `apply_patch` | Grouped exact-text edits across up to 10 files | Up to 20 edits per file; one atomic write per file |

### `read_files` contract

- Accepts 1–10 distinct paths; path aliases are deduplicated after resolution.
- Each path is independently resolved and checked with the existing read policy.
- A failure for one path is returned in that entry's `error`; other files continue.
- `offset` and `limit` apply to every requested file.
- Combined returned file content is capped at 1 MiB. Each entry reports `bytes` and `truncated`; the top-level result reports total bytes and truncation.

### `apply_patch` contract

- Input is structured exact-text edits, not a unified-diff string.
- Preflight resolves all paths, rejects duplicate targets, validates edit shape and bounds, and checks write permission for every target before reading file contents.
- Each file is locked, read once, edited in memory, and written atomically only after every edit for that file succeeds.
- A missing or ambiguous match leaves that file unchanged.
- The operation is **not a cross-file transaction**: another file may already have been committed if a later file encounters an I/O or edit error. Inspect every per-file result.
- Combined old/new patch text is capped at 1 MiB; resulting files remain under the existing edit ceiling.

## Schema validation contract

The runtime validator enforces the subset used by built-in tools:

- object properties and required fields;
- primitive types, including strict JSON-number handling;
- `enum`;
- `additionalProperties: false`;
- string `minLength` / `maxLength`;
- numeric `minimum` / `maximum`;
- array `minItems` / `maxItems`;
- recursive validation of array items and nested objects.

This is not a complete JSON Schema implementation. Do not add unsupported keywords to a tool schema and assume the runtime enforces them. Extend the validator and its tests first.

## Result budgets

- BSH's configured tool-output cap is measured in UTF-8 bytes in both the model-context path and transcript display path.
- A zero cap delegates to the engine's default budget.
- File batch tools have their own tighter output budgets so a single call cannot flood the request context.
- Truncation must be explicit. A model must not interpret a partial result as a complete file or search.

## Security invariants

- Keep `resolve_path` and `check_path` on every file operation.
- Check write permission before reading a file for an edit.
- Do not bypass the file policy through `bash` when a dedicated file tool can perform the operation.
- Keep per-file atomicity and same-path locking.
- Do not claim multi-file transactionality.
- Reject unknown arguments rather than silently ignoring model typos.

## Benchmark plan

Run a fixed task set against representative models and providers. Compare the pre-2.0 baseline with the same tasks on 2.0:

| Task | Baseline path | 2.0 path |
|---|---|---|
| Inspect 5 known source files | 5 `read_file` calls | 1 `read_files` call |
| Change 3 files with 2 edits each | Up to 6 `edit_file` calls | 1 `apply_patch` call |
| Search then inspect matches | `grep` plus repeated reads | `grep` plus one `read_files` |
| Invalid parameter recovery | Provider/tool error retry | Schema rejection with actionable field name |

Record:

- task completion and correctness;
- invalid tool-argument rate;
- tool calls and model round-trips per task;
- input/output tokens, including cached tokens;
- wall-clock latency;
- repeated identical tool failures;
- truncation frequency and whether the model correctly requests narrower output;
- policy denials and any unauthorized file access (must remain zero).

Do not publish percentage-improvement claims until the same model versions, prompts, tasks, and workspace fixtures have been measured on both baselines.

## Verification status

The implementation and regression tests are committed in the repository. This document does not imply that the Rust workspace has been built or that tests have passed: those checks must run in CI or a local BOS checkout before merge/release.
