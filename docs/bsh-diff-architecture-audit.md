# BSH Diff / Review Architecture and Feature Audit

Status: 2026-11-10, after BSH 4.1.1.

This audit answers one question: **is any diff/review feature missing from the
shipped surface, and is the architecture behind it coherent?** It is deliberately
a matrix with evidence, not a roadmap restatement.

## 1. Architecture

The surface has four layers and one shared content store.

```
write_file / edit_file / apply_patch
        |
        v
ReviewRuntime  -- exact before/after snapshots -->  .bsh/review/objects
        |                                                 |
        v                                                 v
   changeset manifest                              ReviewStore::load
        |                                                 |
        v                                                 v
review/diff.rs  (FileDiff)  --------------------->  ui/review.rs
        |                                                 |
        | unified patch (build_patch)                     | "Open in Diff"
        v                                                 v
ui/diff_pane.rs (kit DiffState) <---------------- open_diff_text
        |
        +--> split/unified, syntax, wrapping, conflicts, save, turn preview
```

Decisions are appended to `.bsh/review/audit.jsonl`; the Audit tab reads every
event for the active session, so the decision history is not the latest review's
strip.

### What each layer owns

- **Capture** (`review/caps.rs`, `review/store.rs`): content-addressed blobs and
  exact `SnapshotState` before/after for the built-in file tools. Shell effects
  remain `Unattributed`; that is a scope limit, not a silent fallback.
- **Domain** (`review/domain.rs`): `ChangeSet -> Change -> Revision ->
  SnapshotState`; digest binds a decision to content; findings, evidence,
  verifications and decisions are first-class records.
- **Diff** (`review/diff.rs`): text, structural (tree-sitter), config and
  Markdown adapters; binary metadata; image dimensions. A full unified patch is
  generated only when the reader asks for the full Diff pane.
- **Review UI** (`ui/review.rs`): change list, compact detail, structural tree,
  findings, verification section, provenance, decision buttons.
- **Full Diff** (`ui/diff_pane.rs`): the kit's own comparison surface. It is the
  one place for split/unified, syntax, line wrapping, conflict resolution and
  saving; Review hands it the snapshots as a patch instead of reimplementing it.
- **Audit** (`ui/audit.rs`): the cross-changeset decision timeline, one row per
  decision.

## 2. Feature matrix

| Feature | Status | Evidence / limit |
|---|---|---|
| Exact before/after for write/edit/patch | Done | `caps.rs` capture tests; `SnapshotState` |
| File create/delete | Done | `ChangeKind`; all-added/all-removed tests |
| Rename / mode change | Recorded | change row shows the old path for renames |
| Text diff | Done | `similar`; line numbers, added/removed counts |
| Structural diff: Rust, TS, TSX, JS, Python | Done | tree-sitter adapters; method parent is now retained and indented |
| Structural diff: other languages | Missing | falls back to line diff; no adapter |
| Config diff: JSON/TOML/YAML | Done | flattened key paths; invalid config yields no structural nodes |
| Markdown heading diff | Done | exact heading text; duplicate titles collapse |
| Image review | Partial | dimensions and byte sizes; no visual/pixel diff |
| PDF / docx / archive review | Partial | metadata-level note only; no semantic content diff |
| Agent attribution | Done (visible) | changeset turn/task/attempt line; per-change tool names; finding evidence |
| Findings navigate to their change | Done | finding row opens its `change_id` |
| Verification record type | Done | `Verification` in `Review` |
| Verification producer | Missing | section renders "No verification recorded"; no command/test evidence is captured yet |
| Decision rationale input | Missing | model/audit carries `rationale`; Review buttons submit an empty one |
| Digest-bound approve / request changes | Done | `ReviewStore::decide`; stale digest refuses |
| Cross-changeset decision timeline | Done | `audit.jsonl` + `decisions_for_session` |
| Open selected change in full Diff | Done | unified patch -> `open_diff_text`; split/unified, syntax, wrapping |
| Diff pane: patch, conflict, unchanged | Done | `diff_pane.rs` loader + kit `DiffState` |
| Diff pane: save resolved merge | Done | `DiffSource::Merge` + resolved-text path |
| Review keyboard navigation / filtering | Missing | click navigation only; no search/filter |
| Snapshot GC / encryption / retention | Missing | content-addressed store is append-only |
| Tamper-evident audit chain | Missing | `audit.jsonl` is append-only, not hash-chained |
| Extension-host review service | Missing | review is still a host feature |

## 3. Gaps that are real, not cosmetic

1. **Verification is a view, not evidence yet.** The UI now has the measured
   section and rows, but nothing writes a `Verification` when a test command
   runs. Until a producer exists, "Intelligent Verification" is incomplete.
2. **Multi-format is metadata-only.** Images show dimensions, not pixels; PDF,
   docx and archives show a byte note. A real semantic adapter is a separate
   format project, not a missing switch.
3. **The Review decision is not explainable from the UI.** `rationale` is
   persisted and shown in Audit, but the Review buttons always write an empty
   string. A rationale field is the missing input.
4. **Shell effects are still unattributed.** Only the built-in file tools produce
   exact snapshots. A shell command that edits a file is listed as a command,
   not as an exact file revision.
5. **The store has no lifecycle.** No GC, encryption or retention; the review
   objects grow with use.
6. **The audit log is append-only, not tamper-evident.** A hash chain is still
   owed.
7. **Structural coverage stops at five languages**, and config parse failures
   degrade silently to no structural nodes (the line diff still works).
8. **The Review tab has no keyboard/filter surface.** The change list is a click
   target; a search/filter is not implemented.

## 4. Evidence

- `cargo test -p bsh --lib review::` — 13 tests, including patch headers for
  created files and a method's impl parent.
- `cargo test -p bsh --lib review_tab_renders_a_captured_changeset` — two
  changesets, two decisions, an Audit timeline, the verification/findings
  sections, and Review -> full Diff handoff.
- `cargo clippy -p bsh --all-targets -- -D warnings` — clean.
- `cargo fmt -p bsh -- --check` — clean.

## 5. Verdict

The **P1 diff/review surface is complete for its stated formats and integration
points**: exact capture, text/structural/config/Markdown diff, Review UI, full
Diff handoff, digest-bound decisions, task/audit links and the cross-changeset
timeline. No diff-pane capability is lost when Review opens a change; the patch
bridge is the one source for both surfaces.

The named P2 items remain real gaps, especially verification evidence and
multi-format semantics. They are listed here rather than described as done.
