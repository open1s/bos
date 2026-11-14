# Change History

A first-party standalone BSH extension that lets the agent inspect and
manipulate a repository's **change history**, backed by **jj** when the
workspace is a jj repository (the BOS convention) and **git** otherwise.

It is a Cordis package: the payload is `apply(ctx, config)` in `index.js`,
executed by the sandboxed Node bridge. Every command runs through
`ctx.terminal.run`, the host's capability-gated terminal seam, so the
`process.execute` grant and the extension audit trail cover every effect.
The package opens no child processes of its own.

## Install

~~~text
bsh ext grant com.bos.bsh.change-history process.execute
bsh ext grant com.bos.bsh.change-history agent.tools.register
bsh ext install examples/extensions/change-history
bsh ext info com.bos.bsh.change-history
~~~

The tool registers as `change_history__change_history` (the extension's
namespace plus the contribution id). Like every extension tool it is deferred:
the model reaches it through `find_tool`, and the Tools pane lists it.

## The tool

One tool, `change_history`, with an `action` and the repository's absolute
`path` (usually the session workspace). `backend` forces `jj` or `git`;
`auto` prefers jj when both markers exist (a colocated repo).

| Action | jj | git |
|---|---|---|
| `status` | working-copy status | porcelain status + branch, parsed |
| `log` | change stack (`revset` override, default `ancestors(@, limit)`) | commit log, parsed |
| `diff` | working copy, `-r revision`, or `--from/--to` | `HEAD`, `revision^!`, or a range |
| `show` | one change (`revision`, default `@`) | one commit (default `HEAD`) |
| `new` | start a change on top (`revision` = parent, `message` optional) | commit the working tree (`message` required) |
| `describe` | set a change's message (`revision`, default `@`) | reword **HEAD only** |
| `edit` | move the working copy onto a change | checkout a revision (reports detached HEAD) |
| `squash` | fold a change into its parent (`message` optional, else `-u`) | fold the working tree into **HEAD** (`--amend`) |
| `rebase` | `-r revision -d destination` (both required) | one commit (`--onto`, merges refused) or the current branch |
| `restore` | recover files from `from` (default `@-`) | `git restore --source` (default `HEAD`) |
| `undo` | reverse the last jj operation | **refused**: git has no operation log |

Common parameters: `paths` (repository-relative filter for diff/restore),
`limit` (log, 1–200, default 20), `stat` (diff/show summary).

Results are one envelope: `{ok, backend, action, repo, output, stderr?,
data?, truncated}`. `data` carries parsed entries for `log` and `status`;
`output` is the command's own text. Failures are named refusals that quote
the backend's stderr — a refused action never runs half a command.

### Safety

- Every argument is quoted for the host shell's dialect (POSIX sh on Unix,
  PowerShell on Windows) before it reaches the command line; a message,
  revision or path cannot be reinterpreted as shell syntax. The integration
  tests round-trip a hostile message (`it's a "test"; $(echo pwned) @head`)
  verbatim through both backends.
- No `--force`, no history deletion: jj's own immutability rules apply, and
  `undo` reverses the last operation. On git, destructive combinations
  (rewording an old commit, squashing a non-HEAD revision, rebasing a merge)
  are refused with the reason instead of approximated.
- `jj squash` never opens an editor: `-m` with a message, `-u` without
  (the parent's description wins, mirroring git's `--amend --no-edit`).
- Calls are serialized (`max_concurrent_calls = 1`) because jj takes a
  working-copy lock per command; output is capped at 200 KB per stream.
- `undo` on jj reverses the **last operation**, whatever it was — including
  a working-copy snapshot taken by a read. Run it right after the mutation
  you want to reverse.

## The Git panel

The package also contributes a live **Git** panel (right sidebar, refreshed
every 5 s) that mirrors the reference Git view:

- **Changes** — a commit message field (Enter commits), the total +adds/−dels
  line stats, and one row per changed file with its A/M/D status and exact
  line counts. A row opens that file's diff with a Back button.
- **Graph** — an *All branches* filter (jj bookmarks / git branches) over the
  commit list, with the working copy marked @ (jj) and HEAD/branch badges
  (git). The footer names the backend and head, with * when dirty.

The card is a ctx.panels.register registration served through the Cordis
runtime's panel seam; its actions are ordinary tool calls (refresh, commit,
file), so every press passes the same per-call capability check and audit
line as a model call. The repository is the session workspace, handed to the
render by the host; an explicit path overrides it.

### Gap check against the reference

- **Backend** — real: jj/git through the capability-gated terminal seam; the
  commit action runs jj commit / git add+commit and the card re-reads.
- **Interaction** — message field (Cmd+Enter or Enter submits), branch
  dropdown, collapsible file tree, file rows and Back are wired and
  click-verified in the GPUI harness. The reference's **Session** subtab is
  present with an honest empty state: session change tracking has no seam an
  out-of-process extension can read, so it says so instead of faking data.
- **Tips/help** — the placeholder names the head the commit lands on, in the
  reference's own wording; an empty working copy says "No changes in the
  working copy"; a stale file row refuses by name instead of opening the
  wrong diff.
- **Errors** — a failed command surfaces the backend's stderr in the panel's
  error line; an empty message or clean tree refuses before any command runs.
- **Look** — the card mirrors the reference's structure: Git/Session tabs,
  the rounded message field, the +adds/−dels stats row with the file-count
  badge, a folder tree with file-class icons, status letters and per-file
  line stats, the All-branches dropdown, and a commit rail with dots and
  HEAD/branch/remote chips. The UI contract grew badge, fill, timeline and
  tree/list icon+badge fields to carry it; file-class icon colours are the
  renderer's decorative constants, and per-language icon colours (JS yellow
  vs Rust orange) collapse to one code colour. The sparkle AI-commit affordance
  is not drawn: the extension has no model seam, and a button that pretends
  to generate a message would be a control that only looks wired.

## Development

~~~text
node --test          # 32 tests: unit + real jj/git integration, tool and panel
~~~

The engine (`lib/vcs.js`) is terminal-injected: unit tests run it against a
recording mock, integration tests run the real command lines through the host
shell dialect against scratch repositories, and skip themselves when the
backend binary is absent. `crates/bsh/src/extension/e2e.rs` additionally
installs this package through the real store/host/policy and calls the tool
through the production agent path.

## Limits

- The git backend has no operation log, so `undo` is refused; use `restore`
  or `edit` to recover.
- Windows quoting targets PowerShell single-quoted literals; the host's
  terminal seam prefers pwsh and falls back to Windows PowerShell.
- `path` must be absolute so the capability scope the policy checks and the
  directory the command runs in are the same place.
