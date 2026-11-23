# Changelog

## v4.1.24 (2026-11-23)

- **`/` and `@` pick fuzzily.** Both composer cards rank their rows with
  skim's own subsequence matcher (word-start bonuses, smart case) and highlight
  the matched characters, so a needle like `ab` finds `a…b` in order instead
  of demanding a prefix. skim 5.9.0 is used as a library; the picker stays
  GPUI, and the highlight indices are character-based, so a CJK needle colours
  the right glyph.

- **The `@` card reads an index, not the filesystem.** Candidates come from a
  flat, ignore-aware workspace index built on a worker thread and swapped in by
  generation: the old path walked the tree from the render function, from every
  arrow press and again from the Enter handler (9,732 files outside the build
  trees in this workspace). The walk reuses the file tools' own walker and
  rules - `.gitignore`, `.ignore`, `.rgignore`, `.bsh/.ignore`, the hidden
  and no-symlink policies - holds folders as well as files, is capped at
  200,000 entries with an honest truncated marker, and is rebuilt only when the
  workspace changes, the card opens stale, or the workspace watcher reports a
  change.

- **An agent build scans each skill source once.** Registering N enabled skills
  re-discovered the same source directory N times; 121 skills cost about 2.2 s
  on the UI thread, which blocked the first frame after Enter.
  `register_scanned_skills` groups by source and discovers once - 866 ms ->
  100 ms measured (debug, warm cache), with the same registered set.

- **The model reads a projection of a tool result, not the whole payload.**
  Built-in tools declare the fields a result may show the model; host state
  (cwd, timeouts, empty stderr, status echoes, the derivable summary) stops
  riding along, and a failure carries the error class plus the two recovery
  flags. `bash "echo hi"` measured 124 bytes unprojected against 45 projected.
  Extensions, MCP and federation keep their own payloads.

## v4.1.23 (2026-11-22)

- **A fresh workspace samples for coding.** With no `temperature` and no reply
  cap configured, every request now sends BSH's coding defaults - temperature
  `0.2` and `max_tokens: 16800` - instead of the framework's chat `0.7` (which
  let two runs of one edit differ) and the vendor's implicit 12.8k (which
  reserved more of the window than the code being read could spare; 16,800 keeps
  about 46% of the shipped 32k window for the prompt). Precedence is
  unchanged: the reader's values win, then a model leaf's declared cap, then the
  coding default. Both pane hints and the settings spec now name those numbers,
  and a build test pins the config a fresh workspace produces.

- **A folder that holds a change still collapses (Files pane).** The tree opens
  a directory that contains a change so the reader can see it, and every watcher
  refresh re-ran that expansion from the status snapshot - so a folder with
  changes reopened on the next event and could not be closed at all in a busy
  workspace. Directories the reader closes are now remembered and skipped by the
  automatic expansion; a fresh press of the Changed filter still reveals them
  (that press is an explicit request for the rows), and reopening a branch by
  hand clears its mark. A regression test drives a real git working copy, closes
  the changed folder, refreshes the way the watcher does, and pairs the
  assertion with the Changed filter reopening it, so the test cannot pass by
  expansion being dead.

- **The sampling settings reach the wire, and only with values a provider
  accepts.** `temperature` and `max_tokens` are resolved once and carried on
  every request, but three paths could send something else. A model leaf that
  declared its own `max_tokens` was stored and never used, so the request fell
  back to the engine's 12,800 default (a package's "this model emits at most N"
  was dead data); the workspace's value now wins, the leaf's value fills an
  absent workspace cap, and the engine's default is last. A stored `0` was sent
  verbatim as `max_tokens: 0`, which every endpoint rejects, and was also
  invisible to the truncation guard; `0` now reads as "unset" at the settings
  field, in resolution, and in all three adapters, so the request carries the
  default the engine compares against. A non-finite `temperature` (`nan`
  parses as a float) could not be serialized at all and failed the whole
  request; it now resolves as "no opinion", and an out-of-range value is bounded
  where the wire requires it (2.0 for OpenAI-compatible, 1.0 for Anthropic
  Messages, which omits it entirely while extended thinking is on). The pane
  follows the same grouping: `Max output tokens` now sits beside `Temperature`
  in the Agent loop pane (it was under Context), and the complete-persona
  switch keeps the slot above the retry budget so it stays reachable at the
  reference window size. Tests pin the settings -> agent config -> typed
  request -> wire chain, and the field's pane.

- **The sampling pane fields take effect, or say why not.** Save now commits the
  typed `Temperature` and `Max output tokens` into the settings the next turn is
  built from, and a test drives that whole path (field -> committed document ->
  the agent's config). Two ways a value could silently fail to take effect are
  closed. A value the field cannot read - a word, a temperature outside 0 to 2,
  a count too large for the wire - now refuses the save with the reason shown in
  the dialog, instead of keeping the stored number while the field displayed
  the new text. And a save from a panel control (the diff-mode toggle, a
  launcher switch) no longer reads the dialog's fields, so text abandoned by
  closing the dialog can never be committed later. Counts accept digit grouping
  (`12,800`), matching the docs' own spelling.

- **A large MCP roster stays discoverable through `find_tool`.** Above the
  gateway threshold (12 tools) the build registered only the `mcp` gateway, so
  the per-server tools were in no catalogue at all: a search for the server or
  the tool name answered `{"tools":[],"other_names":[]}`, and the model
  concluded the capability was missing (measured on a live turn with 36
  connected tools, `query: "llmbroker"`). The gateway is now a browsing
  shortcut beside the tools: every MCP tool is registered and carries its MCP
  mark, which withholds it from the advertised array anyway, so discovery can
  report it and the model can call it by its namespaced name without adding a
  request byte. A regression test pins registration, the MCP mark the search
  classifies by, and the empty advertised array.

## v4.1.22 (2026-11-22)

- Release v4.1.22.

## v4.1.21 (2026-11-19)

- Release v4.1.21.

## v4.1.20 (2026-11-19)

- **One result envelope for every tool call (#447).** The engine now records
  each tool result as `{status, summary, data, error, continuation, truncated}`
  instead of stringifying the payload and prefixing failures with "Error: ".
  `status` is one of `success`, `partial`, `pending`, `error`; `error` carries a
  stable class (`invalid_input`, `permission_denied`, `timeout`, `cancelled`,
  `unavailable`, …) with `retryable` and `indeterminate` advice. The tool's own
  payload stays unmodified under `data`, so existing payload consumers keep
  reading the same keys. `ToolError` gained field-level validation errors and
  permission/cancellation/conflict/unavailable classes, and the capability
  boundary now preserves the class instead of flattening it into a message.
  Truncated results stay valid JSON and say so. `read_file` and `read_files`
  now return an explicit `next_offset` (per file for a batch) as their single
  continuation (no duplicate `next` object and no envelope `continuation`
  repeating it): the line to pass as `offset` next, or `null` when
  the file is fully read (every per-file entry carries the field, a failed
  entry as null), so the model never has to infer completion from an absent
  field. The hint says to pass `next_offset` as `offset`; the input
  schema keeps the simple `offset`/`limit` pair. See
  `docs/rfc/tools-2.0.md`.

- **Tool failures stop stacking their own labels.** The agent's tool adapter no
  longer rewrites every failure as `Failed(error.to_string())`, so a class
  (`permission_denied`, `timeout`, `unavailable`, …) survives to the envelope
  and the message no longer reads "Tool execution failed: Tool error: …".
  `grep` with `context` now defaults to the full block plus the match reserve
  (2×context+2), and only before-context rows reserve a slot, so one complete
  hit is `ok` even at `limit: 2×context+1`, not a false `partial`; `count`
  counts matching lines and `rows` adds the context framing. Quiet
  `p` filters say so: `sed` returns `quiet: true` and a hint that the deleted
  diff lines are the filter. `read_files`/`apply_patch` supply count-based
  summaries ("read 3 of 4 files; 1 failed"), and a missing file in
  `apply_patch` is a per-file error, never a batch refusal - even when every
  target is missing, the batch is `partial` with per-file errors, not a
  call-level `error` with a synthetic `internal` code.

- **Reasoning is turn-local; the thinking-summary channel is removed.**
  `AssistantReasoning` blocks may be replayed only inside the turn that
  produced them: a tool loop sends them back (DeepSeek `reasoning_content`,
  OpenRouter `reasoning`/`reasoning_details`, Anthropic signed
  thinking/redacted thinking, Responses reasoning items with content,
  summary, signature and encrypted content). Every turn starts by clearing the
  previous turn's reasoning and provider state, and BSH no longer seeds
  reasoning from the transcript, so no chain of thought crosses a turn. The
  `thinking_context` setting, `ThinkingSnapshot`, summary extraction and the
  `thinking-summary` context block are gone. Anthropic now requests extended
  thinking from the reader's effort level, and Responses asks for the summary
  and encrypted reasoning item where the endpoint implements those extensions.

- **"Off" is an explicit provider switch.** DeepSeek and Anthropic receive
  `thinking: {"type": "disabled"}`, OpenRouter receives
  `reasoning: {"enabled": false}`, and a Responses endpoint that accepts an off
  value receives `effort: "none"`; OpenAI's own Responses API omits the block
  because it has no portable off value. Previously the composer's Off level was
  filtered to "no effort field", which left DeepSeek thinking on.

## v4.1.19 (2026-11-18)

- **An ask_user answer steers the running turn instead of queueing.** Clicking
  a question option, or typing the answer, now pushes the choice into the live
  turn's steer mailbox when the turn is still streaming. It reaches the next
  step immediately and the queue dock no longer holds it behind the turn. A
  session with no live mailbox (or a turn whose settings changed too far to
  steer) keeps the ordinary queue/send fallback.

- **Completed thinking intents are retired before injection (#441).**
  Conversational "let me first..." narration is no longer extracted as a durable
  intent. When a tool call succeeds, the live intent whose statement matches the
  action stored with that call is superseded and never injected; an unrelated
  intent stays live, and a failed or interrupted call never takes the completion
  path. The injection duplicate guard now also recognizes paraphrases and simple
  inflections already present in recent history, not only exact substrings.

- **Harness instructions get a real developer role.** `LlmMessage::Developer`
  carries application-level instructions at `Precedence::High`, below system
  policy and distinct from the reader's request. Chat Completions and the
  Responses API serialize the `developer` role; Anthropic has no such role, so
  its adapter folds the text into the system channel with a `[developer]`
  marker. BSH now injects its runtime snapshot, time context, workspace
  instructions and extension instructions as developer rows instead of system
  rows.

- **Thinking summaries and retrieved memory leave the system prompt (#439,
  #440).** They now ride a typed `ProviderRequest::context_blocks` channel that
  adapters serialize as existing roles immediately before the current input:
  the bounded thinking summary as a framed non-authoritative `assistant` turn,
  retrieved memory as a framed untrusted-data `user` turn. Neither is part of
  the system prompt. Source, trust class and provenance survive into the
  envelope and audit; a memory block cannot close its own envelope even when it
  contains instruction-like text. `ProviderRequest::wire_messages` is the one
  placement path for Chat Completions, Anthropic and Responses. Context now
  carries the shared four-level `Precedence` (`Highest`/`High`/`Medium`/`Low`)
  independently of role and trust: the summary and retrieved memory are Medium,
  quoted/untrusted content can be Low, and provider-native reasoning/tool
  traffic stays Low.

- **Reasoning is preserved as its own message (#438).** `LlmMessage` gained
  `AssistantReasoning { content, provider_state }`, kept separate from
  `Assistant` so the model's semantic boundary survives. The engine records
  non-streamed and streamed reasoning once per step, before the visible text
  and tool calls. Chat Completions never replays it; Anthropic replays signed
  `thinking`/`redacted_thinking` blocks and merges them with their `tool_use`
  blocks; the Responses API replays a reasoning item only when its own opaque
  state (`id`/`encrypted_content`) is present. A provider switch therefore
  cannot replay another protocol's state. BSH persists the opaque states in
  `MessageMeta.reasoning_states` and `Runner::seed_session` pairs each timeline
  thinking block with its own state, ahead of the text. Thinking summaries remain a separate, opt-in
  context channel. See `docs/reasoning-messages.md`.

- **Thinking summaries feed later steps without rewriting history (#438).** A
  reasoning step's claims, intents, rationale, open questions and next action are
  extracted into a typed, bounded summary (`react::thinking`). A tool result
  verifies a claim only when its output actually supports that claim; execution
  success alone never promotes a decision, and an intent is never verified by
  its own success. A failure retires the judgment it contradicts and withdraws a
  verification that rested on the failed tool. Superseded and completed items
  are never injected. The note rides `HostGuidance` with an explicit
  `thinking-summary` source, composes into the system prompt of one call, is
  filtered against the history, and is never appended as a user or assistant
  message. `ThinkingSnapshot` persists beside the messages in `AgentSession`
  (session schema 6, snapshot schema 3). Effective polarity resolves negation
  around a claim's predicate, so "does not fail" is positive and "does not
  pass" is negative. Output polarity is scored over predicate occurrences with
  their negation context: "did not fail" and "no failures" are positive,
  "failed to pass" is negative, "not present" is an absence, and absence
  reports cannot confirm a failure claim. Resume-versus-new-task is decided from
  a bounded request signature, not the phase alone: a different request after an
  interruption clears stale pending state, while a continuation or the same
  task preserves the in-flight call as interrupted/unknown instead of success. Reasoning is
  folded in batches and at every recoverable stream termination (provider error,
  stop, dropped stream after a batch) without ever marking a stopped turn
  complete. The feature is on by default and can be disabled for an Agent
  (`AgentConfig::thinking_context(false)` or `thinking_context = false` in
  TOML), Python (`with_thinking_context(False)`), JavaScript
  (`thinkingContext(false)`), BSH's **Settings → Agent loop → Thinking context**
  row, or a direct engine (`ReActEngineBuilder::thinking_context(false)`);
  disabled means no extraction,
  no snapshot write and no `thinking-summary` guidance, while a previously
  stored snapshot is left intact. (The feature was removed in Unreleased; see
  the entry above.)

## v4.1.18 (2026-11-17)

- **Release-gate fixes.** Federation capability tests now pass the required
  `read_file` offset; the stale `codel` query-limit doc links no longer
  fail the docs build; and Extension prompt activation no longer fires merely
  because an extension tool is admitted - the task must name the capability, so
  an unrelated research turn does not carry the Extension DevKit workflow.
  Extension injection rows also name the contribution id, so two contributions
  of one package are distinguishable.

- **Extension instructions activate per task (#436).** Prompt and context
  contributions are no longer appended to the standing system prompt at
  install. They are stored with extension id, version, trust, contribution id,
  declared terms and any diagnostic, and the request assembler selects them per
  turn. Extension-level signals (an explicit command, a capability admitted by
  the current capability view that the task names, or a task that names the
  extension) select the whole extension; otherwise each contribution is scored
  on its own description, triggers, commands and tools, so a later contribution
  can match without the first. Matching is lexical from the extension's own
  metadata - no domain vocabulary list - through the shared memory tokenizer,
  with a small CJK-to-English task-term bridge for Chinese requests. Omitted
  bodies are recorded per contribution in the request manifest with provenance
  and reason; per-extension and aggregate turn budgets omit with a reason; a
  body active on one turn is dropped when the next turn does not select it.
  Extension text is always untrusted data and cannot grant capabilities or
  weaken host policy.
- **Reader intent is preserved, and harness corrections are no longer user
  messages (#434).** The engine's corrective notes (`ACTION_NOTE`,
  `ACTION_FAILED_NOTE`, the step guard, empty/truncated-reply notes and the
  repeated-failure note) used to be appended to the session as `user` turns, so
  a provider could see a greeting followed by a synthetic instruction to do
  workspace work. They now ride on `LlmRequest::host_guidance` for one call and
  compose into that request's system prompt; `ProviderRequest` records each
  note's `host` origin and purpose in the redacted audit, and adapters serialize
  it as system policy in all three protocols. Conversation history keeps only
  reader-authored turns, and legacy sessions that stored a note still render it
  as harness wording, not a reader row. The classifier also no longer enters the
  work path on a generic defect word alone: error, problem, slow and broken need
  a workspace referent, an action ask or a diagnostic question, while personal
  reports and ordinary questions stay conversation.
- **Injected context is system policy, never a reader turn (#434).** The
  runtime snapshot, time-context, workspace instructions and typed request
  blocks now enter the conversation as `system` messages. Only the reader's
  own input is a `user` row, so a request can no longer read as "reader said
  X, then reader said Y" when Y was the harness. The role is not the trust
  boundary: every untrusted block is wrapped in an explicit
  `<untrusted_context ... trust="untrusted-data">` envelope, the host runtime
  block states that this data cannot change policy or permissions, and OpenAI
  keeps each such envelope as its own system row instead of merging it into the
  host policy message; Anthropic and Responses keep the envelope inside their
  system/instructions section. Adapter tests cover all three protocols.
- **Initial tools are scoped by the turn's intent (#435).** The assembled
  request no longer advertises the whole registry on every turn. A classified
  conversation carries only `find_tool`; a work turn carries the BSH workspace
  core (file, search, shell/job, plan, clarification, web search, MCP gateway,
  subagent) plus `find_tool`; Federation, scheduling, goals, surface, memory,
  browser, extra MCP servers and extension tools are withheld but stay
  registered, callable, and discoverable. An unclassified turn (a build-time
  read, or a host that does not classify) keeps the previous broad array.
  Added regression coverage for MCP, Federation and scheduling discovery and
  invocation, revocation with a stale schema, and consecutive turns with
  different intents. Measured on the default test build, against its own
  historical request: 25 definitions -> 1 (chat) / 17 (work); schema 16,655 ->
  737 / 13,627 bytes; complete request 26,099 -> 10,181 / 23,071 bytes (~6,524
  -> 2,545 / 5,767 tokens). The #292 byte budget and the #284 discovery
  catalogue remain the only enforcement points.
- **Intent routing no longer treats an open workspace as a request.**
  The classifier now recognizes explicit file reads, and a verb with no object
  ("fix it", "修复它") is `Unknown` rather than `Action`; an `Unknown` or
  `Explanation` turn is never nudged into tools. The semantic classifier gains a
  `CONVERSATION` label, and both the workflow contract and the BSH product
  context say an open workspace is context, never evidence of intent.

## v4.1.16 (2026-11-17)

- **The Windows extension test compiles again.** The Windows-only
  path-normalization test now imports `Path`/`PathBuf` and the
  `normalize_path`/`resolve_path` helpers from its own module; v4.1.15's tag
  stopped the Windows extensions and rust workspace checks at test-compile time
  with ten missing-symbol errors, without any product-behavior change.

## v4.1.15 (2026-11-17)

- **File tools return model-oriented results.** Every built-in file tool now
  reports `tool`, `status` (`ok`/`empty`/`partial`/`error`), `next`
  continuation arguments, `count`, and a recovery `hint` instead of
  `true`/`false` flags. `read_file`/`read_files` report exact line ranges and
  totals, a partial window carries `next.offset`, and a batch cut in the middle
  of a line re-reads that line rather than skipping it.
- **`read_file` requires a 1-based offset.** The first line is `1`, not `0`;
  `limit` remains optional, and there is no implicit whole-file read.
- **Search returns a block, not only a hit.** `grep` accepts up to 2,000 context
  rows per side, sizes the default row budget to the first window, and reserves
  a row for the match so before-context cannot hide it. `glob` and `list_dir`
  report counts and continuation hints.
- **Mutations return local context.** `write_file`, `edit_file`, `apply_patch`
  and `sed` report an outcome word and a bounded changed-line diff (20 lines)
  with `diff_status`; a successful edit with a cut diff is never mislabeled as
  a partial edit. `sed` adds `mode` and `content_status` for dry runs.
- **The Windows file path is real now.** Overwrites use `MoveFileExW` with
  `MOVEFILE_REPLACE_EXISTING` (std `rename` fails on an existing Windows
  target); drive and UNC prefixes survive normalization; drive-relative and
  rooted-without-drive paths are refused; write locks fold case. CRLF is kept
  and an edit miss that used bare LF in a CRLF file says so.
- **Context and task reliability.** Typed request context with versioned
  fingerprints, checkpoint and archive digests, structured critical facts,
  retry as an authorized deduplicated compensating protocol, runtime task
  verification, session restore, and a long-horizon/restart/crash corpus
  (#416-#433).
- **codel resource limits.** Query and native parse paths now enforce the
  shared scan budget and cancellation contract (#398-#403).

## v4.1.14 (2026-11-16)

- **MCP stdio servers start from the program you installed.** A server
  configured as `npx -y …` was spawned as a literal name against a
  Finder-launch `PATH` and failed with `No such file or directory (os error
  2)`. The stdio transport now resolves the configured command through the
  same cross-platform program search as every other runtime, and gives the
  child the extended `PATH` so `npx`'s `#!/usr/bin/env node` resolves too.
- **MCP two-way exchange is correct on both transports.** The client no longer
  advertises `roots`/`sampling` capabilities it never implements; a
  server-initiated request is answered (`ping` with `{}`, anything else with
  JSON-RPC `-32601`) instead of being dropped; a server that closes its stdout
  is reported immediately instead of being polled. Over HTTP the status is
  checked before the content type, the SSE response is selected by request id
  (notifications and multi-line events included), and `MCP-Protocol-Version`
  is sent.
- **The workspace resolves again.** codel's Tree-sitter pin follows BSH's
  (`0.26.13`), removing the `links = "tree-sitter"` conflict that blocked a
  whole-workspace build.

## v4.1.13 (2026-11-16)

- **Extension runtimes resolve the interpreter you actually installed.** Every
  runtime that spawns an external program - `process`, `mcp` and the
  `cordis` Node bridge - now uses one cross-platform program search instead of
  a per-language table: `$BOS_<PROGRAM>`, an activated `VIRTUAL_ENV`, the
  manager's own choice (`mise`, `asdf`, `pyenv`, `volta`), the managers'
  installed versions and data directories, `PATH`, then the system prefixes. A
  Finder- or Explorer-launched BSH therefore still honours mise/asdf/volta/nvm
  configuration, and the macOS `/usr/bin/python3` stub no longer shadows the
  reader's installed interpreter. The manifest name remains the capability
  subject (`program:python3`); only the executable that answers to it changes.
- **The extended PATH is handed to every child.** The host terminal's shell,
  `process`/`mcp` children and the Cordis bridge inherit the PATH BSH built,
  so a package that runs `jj`, a Homebrew `git` or a version-managed tool
  finds it even when BSH itself was started from Finder.
- **codel, the host-independent code-intelligence engine, lands as a
  compiling, documented workspace crate.** It confines every read to a
  host-supplied workspace, bounds scans with one shared budget, parses Rust with
  upstream Tree-sitter, and exposes native `ParsedDocument`s with a versioned
  Tree-sitter query registry and bounded, cancellable query execution (TS-02).
  It stores no custom AST and never executes project code; the architecture and
  TS-02 design documents live in `crates/codel/`.

## v4.1.12 (2026-11-16)

- **The extensions Grant button works for Cordis packages, and a grant
  written from the CLI reaches the running app.** A Cordis runtime has no process
  executable to scope `process.execute` to — its entry is the JavaScript
  module it loads, and the commands it runs are chosen per `terminal.run`
  call — so the button refused with "process extension has no declared
  executable" and wrote nothing; reported against `com.open1s.vcs` 1.4.0,
  whose required `process.execute` could not be granted from the UI at all.
  It now grants `process.execute` as `any` for Cordis, the same scope
  `bsh ext grant` already derives, and the runtime keeps gating every command
  at call time. Grants and revokes written by `bsh ext grant` (another
  process, its own policy engine) are re-read at every rebuild, so following
  the error row's own hint no longer leaves the pane failing until a restart.
  MCP capability rows now carry the declared program too, matching the scope
  the activation check already used.

## v4.1.11 (2026-11-16)

- **A slow model is waited out, and the wait is a setting.** The vendor read
  guard is an hour (was ten minutes) and the connect guard a minute, so a
  reasoning model or a buffering gateway is no longer reported as
  `Request timed out` while it is still working. One `llm_timeout_secs` now
  bounds both the engine's per-call ceiling and the wire; the shipped 60s
  framework default no longer cuts a turn.
- **The retry budget is configurable.** `max_retries` is the engine's own
  budget (counted after the first attempt, 0 disables retries) and reaches
  Rust, the TOML surface, Python (`with_max_retries`) and JavaScript
  (`maxRetries`); BSH configures no rate limiter, so the number the reader
  picks is the number the loop obeys.
- **The Agent loop is a first-class settings surface.** A rail cell carries
  the request wait, sampling temperature and retry rows, the memory-capture
  opt-in, the dispatch switch on the shared preference-row anatomy, and the
  persona contract - which now names the document it edits and lives only
  here, not in General as well.
- **The complete-persona flag follows the persona's owner.** A preset that
  ships a persona owns the flag even when the workspace flag says otherwise;
  a blank preset persona is no persona, so it no longer blanks the workspace
  prompt or steals the flag.
- **Federation and the market.** The Federation pane's Work tab edits the
  three remote-task caps (steps, tokens, deadline) with the same
  refuse-not-clamp admission semantics as the file, and the extension
  market's filter now sits above the list it narrows rather than at the top
  of the pane.
## v4.1.10 (2026-11-15)

- **Reliability kernel** (issues #347-#356): a transition is real only at the
  durability level it promises, and anything that can no longer make progress
  is stopped and accounted for rather than silently abandoned.
  `hostkit::durable` gains `CommitLevel::{Accepted, Written, Durable}`, a
  streaming `commit_stream()` and `IoStage::SyncDir`, so a segment or
  manifest commit syncs the file AND the parent directory (the weaker Windows
  guarantee is documented). `hostkit::coord` gains
  `FileLock::acquire_bounded()`; every history mutation takes the cross-process
  lock before the in-process guard (one order) with a bounded budget, so a busy
  directory is a typed error instead of a stalled UI thread, and segment
  ids/temp files are collision-safe across processes.
  History carries `HistoryError` (Read, Decode, Schema, Oversized, Locked) and
  `IndexState` (Valid, Rebuilt, Unavailable); the schema version is recorded
  per segment and in the manifest; a present but malformed column names
  segment, row and column while an absent legacy column keeps its documented
  default; a manifest that disagrees with the footers is rebuilt from them and
  one that references unavailable data is reported, not shortened; `diagnose()`
  is the offline audit. The page cache is bounded by bytes as well as pages, a
  row above the inline limit is refused before anything is written, and window
  materialization is bounded with a page-walking stream.
  Store journals carry a format version and checksum; recovery never deletes
  what it cannot read and quarantines corrupt, truncated, unknown-version or
  stale journals with a counts-only report surfaced as a startup notice; a
  failed page read is a typed `PageFailure` instead of a valid-looking empty
  page. Federation gains `Open.attempt` with an idempotent `Cancel` that stops
  the exact provider producer, every source read is bounded by one absolute
  deadline and the cancellation signal, a checkpoint survives cancellation, and
  an unloadable replay store fails closed with an audited
  `reset_replay_store()` operator recovery.
  Design: `docs/reliability-kernel.md`; solution notes:
  `docs/solutions/durability/reliability-kernel.md`.

- **Launch provider refresh is atomic**: `refresh_plugin_providers` opened the
  process-global extension host and then read the contributed provider and
  preset caches in separate lock holds, so a concurrent host open (a workspace
  switch, or another test in the same process) could swap the host in between
  and cache the other host's - usually empty - surfaces. The new
  `extension::launch_surfaces` reads statuses, providers and presets in ONE
  lock hold.

- **CI**: the rust workflow caches the workspace crate artifacts (not only the
  dependencies), the release-mode extension budgets are opt-in via
  `workflow_dispatch` instead of a fat-LTO release build on every push, and the
  gold-standard gate no longer re-runs the fmt/clippy/extension suite the
  workspace gate already ran. Together these cut a warm "Test and lint" run
  from 45 minutes to roughly 10-12.

## v4.1.9 (2026-11-15)

- **Durable Record Kernel** (issues #338-#346): one durable write path for the
  whole system. `hostkit::durable` stages every commit (create dir, temp file,
  write, sync, harden, replace) behind typed errors and a `CommitGate` fault
  seam; `hostkit::coord` gives a canonical identity and a cross-process file
  lock; `load` fails closed. Memory is rebuilt on it: `FileMemory` stores
  schema-2 `MemoryRecord`s (kind, scope, provenance, confidence, lifecycle,
  relations), migrates legacy JSON-lines on read, and every handle to one
  canonical path shares one coordinated store. The fallible surface is
  `MemoryRepository` / `MemoryStore::try_*` / `Agent::try_remember`; the
  legacy infallible methods stay best-effort adapters. Recall keeps the
  fixture-pinned legacy `search` and adds hybrid `recall` (BM25 + CJK n-grams,
  scope filtering before ranking, token budget, explanations). BSH resolves
  storage identity and access scope through `MemoryScopeResolver`, captures
  knowledge at completed turns (the `memory_capture` setting), and derives
  context diagnostics from the typed `AssemblySnapshot`, never from rendered
  strings.

- **Ferris memory contract** (`docs/ferris-memory-records.md`): extensions reach
  the kernel through `ferris_sdk::memory` - schema-2 DTOs, a `MemoryRecords`
  service, `dispatch_memory_json` for out-of-process runtimes, and the
  `memory.read` / `memory.write` capabilities. BSH provides the service per
  activation (declaration- and grant-gated, bound to the session scope, revoked
  with the activation); writes are host-bound to the viewer scope and
  attributed `extension:<id>`. `com.bos.bsh.memory-tools` is the first-party
  worked example (`memory_recall` / `memory_remember` / `memory_forget`).

- **Memory pane**: a new right-panel tab (deep link `--panel memory`) over the
  same coordinated stores - list, hybrid search with score explanations, kind
  filters, history toggle, add, edit, supersede and delete, with one activity
  log reporting the kernel's typed error labels and the last capture summary.

- **Memory pane design pass**: gradient hero with the live count and actions,
  per-kind colour identity (accent bar, tinted glyph tile, kind/scope/source
  pills), readable scope labels instead of raw UUIDs, and a card stack on the
  soft page tint. Fixed the scroll layout that hid the hero at launch and a
  zero timestamp that rendered as "20772d".

- **CI and portability**: `ferris-runtime` imports the Windows `CommandExt`
  trait where it builds suspended children (the MCP and bridge runtimes), so the
  Windows lane compiles again; the Chrome tool-runner browser case skips on CI
  hosts, where the shared Linux runner's Chrome under Xvfb is load-flaky and a
  failed launch could abort the GPUI tests running beside it.

- **Extension panel**: a periodic refresh is a quiet re-read, so the card no
  longer flashes its loading row, and the input/row layout polish landed.

- **Panels and context**: the Git panel gained a launcher tile, dedicated tabs,
  workspace follow and a drill-down that survives refresh, plus a launch-crash
  fix; the change-history list respects `.gitignore` and treats a clean commit
  as a gentle notice; context injection deduplicates repeated injections.

## v4.1.8 (2026-11-12)

- **Action-first hardening** (issue #293 residuals): the last limits are closed.
  An UNCLASSIFIED turn gets a semantic second opinion from the configured model
  through `INTENT_CLASSIFIER_PROMPT` (one words-only completion, eight tokens,
  8s timeout, cached process-wide by message and model, skipped for slash
  commands). Transient provider failures are retried twice with backoff (plus
  any registered fallback provider), the classifier's usage is drained into the
  turn's usage report, and when the provider stays unavailable the deterministic
  LOCAL feature classifier routes the message (workspace/code-object references,
  statement vs question, definition guard) instead of dropping to the keyword
  gate. The engine still spends the bounded action retry when a promise replaces
  an action (an explicit explanation turn is never nudged), and a new
  `ACTION_FAILED_NOTE` retry fires when every tool call of an action turn
  failed. A failed call of a tool that DECLARES read-only side effects is retried
  once when the failure is transient; a write, process, network or spend tool is
  never replayed, and a persistent failure is refused as a completion, logged
  through `TurnOutcome`, and surfaced in the status bar by
  `AgentEvent::ActionMissed` with its call and failure counts and a one-click
  **Retry** that resends the reader's own message. The three shipped preset
  personas (PTC, Creator, Read-only) end with an `ACTION_FIRST_PERSONA` stance.
  `complete: true` is a real switch again, drawn ALWAYS in General's persona row
  and in the composer's permission menu (on/off, disabled while no persona is
  written): it sends the persona in place of the identity, workflow and
  per-tool sections, while a compact `COMPLETE_PERSONA_CONTRACT` footer keeps
  the work-first/evidence rules in the prompt. The flag is written where the
  persona lives (workspace `[agent] complete`, or the preset's own field), so a
  shipped preset can be switched without an inert workspace flag; the composer
  badge names the mode and returns to the slot composition in one click. Docs:
  `docs/bsh-prompts.md` section 9, `docs/bsh-settings-spec.md`.

- **Action-first coding turns** (issue #293): the workflow contract now
  separates the requested deliverable from the method that produces it - a
  repository-specific "why does X fail?" gathers evidence with tools before it
  explains - and carries an explicit route for diagnose, fix/implement, review
  and explain/teach, with the first move for concrete coding work being a tool
  call rather than a narration. The runtime enforces the contract: BSH
  classifies each turn's ask (`crates/bsh/src/intent.rs`, bilingual, lexical,
  leading 400 characters) into action/explanation/unknown, the engine reads it
  from `LlmRequest::intent` and spends ONE corrective retry (`ACTION_NOTE`)
  when an action turn is answered with prose and no tool ran - skipped when no
  acting tool is available or the reply asks the reader a question, and never
  applied to explanation turns. Each turn closes with
  `TelemetryEvent::TurnOutcome` (intent, tools available, tool calls, first
  tool step, requests, whether the retry was spent, answer bytes) and a
  `turn outcome:` log line. A `complete: true` persona now logs a warning and
  publishes `persona_complete`/`workflow_contract` on `instruction.build`,
  so a talk-only turn caused by the configuration is identifiable. Behaviour is
  covered by scripted-provider tests for the blocking and streaming paths,
  including the negative cases (explanation turns, no tools, a question reply,
  one-retry bound). Documented in `docs/bsh-prompts.md` sections 4 and 9.

- **Bounded initial tool schemas** (issue #292): the tool array a request
  starts with is now accounted under one explicit per-turn budget instead of
  being assembled separately from the `find_tool` discovery budget. A new
  `react::tool::admission` module admits definitions by a policy - a
  cumulative byte ceiling, a per-tool ceiling, a reserve held back for
  discovery, and names that must never be withheld - and the engine prices the
  admitted array (`SchemaBudget::account_initial`) before each turn, so the
  initial definitions and every later reveal spend the same 64 KiB. A definition
  over budget is withheld, never dropped: it stays registered and callable, the
  search is admitted and priced in its place, and the same decision is written
  into the catalogue `find_tool` reads. `Agent::require_tool` names the
  definitions that must stay (BSH marks every tool its prompt guidance names),
  `Agent::tool_admission` reports what was withheld with bytes and reason, and
  admission is deterministic and name-sorted so the cached prompt prefix is
  stable. Requests now report their composition per step: system prompt, tool
  definitions (count and largest), skill catalogue, history and input bytes,
  plus the turn's schema spend, through `TelemetryEvent::RequestComposition`
  and the `build tools=... schema_budget=...` line. Documented in
  `docs/bsh-prompts.md`.

## v4.1.6 (2026-11-11)

- **sed tool**: the files bundle now carries a first-class `sed` tool built on
  uutils sed 0.3.0 (the `sed` crate). One program processes a private copy of a
  workspace file and replaces the file by rename only when the bytes change, so
  a failed or no-op script leaves the original untouched and the engine's output
  never reaches the GUI process's stdout; `dry_run` returns that copy's text
  instead of replacing anything. Output follows GNU's in-place semantics: `p`,
  `P`, `=`, `l`, `F` and `s///p` write to the edited file, `quiet` mirrors `-n`
  (so `quiet` plus `/pattern/p` filters the file), `line_length` mirrors `-l`,
  `backup_suffix` mirrors `-iSUFFIX`, and the result's `exit_code` is the status
  `q`/`Q` set; `F` names the target rather than the private copy. The result
  carries a bounded line diff, so the transcript's changed-files card and Diff
  pane preview a sed step's real changed lines instead of omitting it. The
  engine's compile-time sandbox is always on, refusing `e`, `r`, `R`, `w` and
  `W`. The tool honors the workspace ignore file, the permission matrix and the
  per-file lock, declares `fs.write:workspace` capability metadata, and records
  its edit in Intelligent Review.

- **Unified intelligence runtimes**: goals now carry a durable identity and
  revision that fence late round settlements, refuse overlength objectives
  instead of truncating them, and localize the round-limit blocker while keeping
  its stable code. Skills gain validated metadata (version, tags, dependencies,
  conflicts, provenance, digest), a metadata-only catalogue with hybrid
  retrieval and deterministic dependency resolution, trust policy, a task-scoped
  runtime with context budgets, and governed federated metadata merge. Tool
  discovery gains category/tag/intent metadata, metadata-aware ranking and a
  cumulative per-turn schema budget that reports what it refused. The production
  `load_skill` path now runs through the task-scoped runtime
  (dependency-first activation, trust and budget checks, digest-refused stale
  manifests, composed instructions injected as one request-scoped block and
  cleared when the task ends), discovery reports capability id/version/provider,
  and both retrieval corpora assert P@3/R@3/MRR. Added an offline
  with/without-skill activation benchmark, governed federated metadata
  discovery (policy-scoped peers, withdrawal, trust-gated remote loads), and
  turn-correlated discovery/selection telemetry with an effectiveness
  accumulator.

- **Binding parity for live runtime updates**: Python and JavaScript can now
  add/remove tools and skills while a turn is running through the shared live
  overlay (`add_live_tool`/`addLiveTool`, `add_live_skill`/`addLiveSkill` and
  their directory/removal counterparts). `list_tools` and the new
  `tool_definitions`/`toolDefinitions` include live and MCP tools, and skill
  snapshots now carry version and tags.
- **Model-written compaction**: `Agent.compact_now`/`compactNow` exposes the
  model checkpoint path alongside the existing mechanical session compaction.
- **Full plugin lifecycle**: `on_stream_token`, `on_turn_start` and
  `on_turn_end` are available in both bindings; Python plugins that omit a
  handler no longer veto that hook.
- **MCP prompts and health**: `get_prompt`/`getPrompt`,
  `get_capabilities`/`getCapabilities`, plus `restart` and `health_status` on
  the JavaScript client.
- **Tool capability metadata**: `ToolDef` carries `version`, `risk`,
  `side_effects`, `permissions`, `timeout_ms` and `max_output_bytes`, and the
  full tool definitions report them.

## v4.1.3 (2026-11-11)

- **Declarative extension panels**: a `panel` contribution ships
  `panels/<id>.json` - title, location, default visibility, a light and a dark
  theme, a refresh interval, and a view tree the host renders. The document is
  validated at load with explicit ceilings (1000 nodes, depth 32, 16 KiB per
  text leaf, 256 KiB of text, 500 table rows, 2048 characters per cell, 1 MiB
  serialized), and a component the host cannot honour is a load error rather
  than a silent fallback. The extension pane mounts these documents in the
  right-hand pane.
- **Wasm-component extensions**: a `wasm-component` guest now gets WASI preview
  2's clock and random interfaces, so `std::time` and `getrandom` work without
  a host bridge; stdout and stderr are inherited, while the filesystem,
  environment, sockets and stdin stay behind the `bsh:extension/host` imports
  and their capability checks. A wasm component can also observe hooks, which
  the extism runtime already could, under the same `hooks.observe` capability.
- **Hook points are explicit**: the host dispatches eight of them -
  `model.request`, `model.response`, `tool.before`, `tool.after`,
  `http.exchange`, `turn.start`, `turn.end` and `skill.load` - and a manifest
  naming anything else is rejected as "not a hook point this host dispatches"
  instead of silently never firing.
- **An abandoned stream still reports its exchange**: a response body the caller
  drops now reports through a drop guard that fires exactly once, so the view
  behind the vendor tap shows a truncated stream as truncated instead of
  showing nothing.
- **Right-pane deep links**: `bsh --panel=<files|terminal|diff|browser|extensions|federation|tasks>`
  opens that pane on launch, and `--browser=<url>` opens the Browser pane on a
  URL.

## v4.1.2 (2026-11-10)

- **Linux ships one AppImage**: the `.deb`, `.rpm` and portable tarball are
  gone, replaced by `bsh-<version>-linux-<arch>.AppImage`. The deb could not be
  installed on Ubuntu 24.04 at all: its `Depends` came from the release
  binary's full `ldd` closure, which included WebKitGTK's transitive
  `libicu70`/`libunistring2` because the package was built on 22.04, and apt on
  24.04 has neither. Packaging the app instead of declaring its dependencies
  removes that whole class of failure.
- **What the image carries, and why not WebKit**: bsh plus the libraries the
  host's own WebKitGTK does not already load, computed at package time from
  `ldd` - so the set maintains itself. WebKitGTK is a host prerequisite on
  purpose: it resolves its helper processes and injected bundle at paths
  compiled into the library, which no environment variable relocates, so a
  bundled copy cannot be found without a mount namespace over `/usr`.
- **Readable start-up failures**: `AppRun` checks glibc and every unresolved
  library before the exec, and prints the one command the detected distribution
  needs (`apt`, `dnf`, `pacman`, `zypper`) instead of a loader error. FUSE 2
  is optional via `--appimage-extract-and-run`.
- **Desktop integration without a package manager**: `--integrate` writes a
  menu entry pointing at the image where it sits and `--uninstall` removes it,
  compensating for the deleted deb's `.desktop` file and uninstall path.
- **Verified on clean systems**: the release job runs the image in
  `ubuntu:22.04`, `ubuntu:24.04`, `debian:12` and `fedora:41` containers with
  WebKitGTK installed, and in one without it to prove the preflight fires.

## v4.1.1 (2026-11-10)

- **Tool and skill discovery in the prompt**: the system prompt now opens with
  a discovery section derived from the request's own tool array. Its heading
  names `find_tool` and/or `find_skill` only when the request carries them, so
  a withheld tool family is never hidden and a search the request does not carry
  is never promised. The section still leads the prompt, and a build that
  withholds nothing says nothing about a search it does not have.
- **Workspace ignore file**: `.bsh/.ignore` (gitignore syntax) is honoured by
  every file tool - `list_dir`, `read_file`, `read_files`, `write_file`,
  `edit_file`, `apply_patch`, `glob` and `grep` - with the harness's own
  `.bsh` and `.bos` trees always excluded. A fresh workspace gets the file with
  every rule commented out, so nothing changes until its reader uncomments a
  line; an existing file is never overwritten.
- **File preview**: a Markdown tab opens on its rendered document with a
  Source/Preview switch, an image tab adds a `W x H - size` caption, and SVG is
  rasterized instead of painted as a tinted mask. The source buffer remains the
  editable truth for save, undo and comments.
- **Extensions pane**: tools, skills and installed extensions are searchable
  from the pane, and a declarative extension panel picks which installed
  extension it mounts.
- **Transcript scrolling**: a long, scrolled transcript keeps only its visible
  rows built and bounded instead of laying out the whole history.

## v4.1.0 (2026-11-09)

- **Intelligent Review (MVP)**: file tools now capture exact before/after
  snapshots into a content-addressed store under .bsh/review; text and
  tree-sitter structural diffs are derived from those snapshots, a new Review
  tab renders changes, findings, verification placeholders, tool provenance and
  digest-bound decisions, task attempts plus the Audit tab link to the captured
  changeset, and Audit shows every review decision across changesets as a
  timeline. Any changed file can open the full Diff pane (split/unified,
  syntax, wrapping) from its snapshots, structural rows keep their parent
  indentation, and findings open the change they name. Binary changes report
  byte counts, and images also report their dimensions. Shell output remains
  explicitly unattributed, verification evidence has no producer yet, and
  PDF/document/archive support is metadata-level in this cut.
- **Pane fit**: the Trajectory, Audit and Review tabs now fit the centre column
  at any width. Their side columns take a share of a narrowed pane instead of
  holding their designed 220/330/380/390, the step and event detail cards float
  over the body (keeping their own close) when the body would fall under a
  readable width, and the run header, filter rows and decision buttons truncate
  or wrap instead of painting past the pane's edge. An expanded right panel used
  to squeeze the run timeline to 44px, clip the session line under the header
  buttons, and put Audit's export control outside the pane.
- **Trajectory liveness**: a step now reads 进行中 only while it is still
  producing data. A tool call whose result has landed is 已完成 even while the
  rest of its turn streams, the model-inference row hands off to a call in
  flight, and a historical run no longer reports running because its session is
  still streaming.

## v4.0.3 (2026-11-09)

- **BSH interface refresh**: the settings shell now uses the macOS-style
  blue-white design system across Account, General, Models, Web Search,
  Extensions and Agent Presets, with a fixed navigation rail, runtime status,
  recent activity and the unified account/data paths.
- **Chat and trajectory**: conversation rows, composer and message actions use
  the new visual language. Trajectory gained a run navigator, step timeline,
  progress header, step inspector and rich Markdown/code/diff/image previews.
  The measured timeline remains available from the same tab.
- **Audit**: a flat event table with overview statistics, event/result/time
  filters, inline event inspector and a direct jump to the related Run.
- **Tasks**: the right dock now draws the execution tree as real work units
  with Main Agent, Subagent, Peer and Task nodes, distinct run states, a
  selected-unit inspector, a work-unit List and the durable task board.
- **Tool discovery**: the model-facing `call_tool` dispatcher was removed.
  `find_tool` now returns each tool's name, description and full JSON Schema,
  and those definitions are added to the next model step so discovered tools
  are called directly by name.
- **Federation and runtime**: additional federation work, extension runtime
  fixes, scheduling and agent-loop corrections, and test coverage for deferred
  tools and the new BSH surfaces.

## v4.0.2 (2026-11-05)

- **Ferris SDK bindings**: `nbos` and `jsbos` expose `FerrisManifest`,
  `FerrisHarness` and `FerrisHost`. A Python or JavaScript object can
  implement the Ferris SDK `Extension` interface (manifest +
  start/invoke/shutdown callbacks) and run in process through the Rust
  `ExtensionRuntime`. `FerrisHost` adds the full package host: install, list,
  versions, enable/disable, rollback, quarantine, dependency-ordered plan,
  activation of the declarative/process/MCP/Cordis/Extism/WASM runtimes,
  contribution collection, invocation, policy grants and audit. The host also
  accepts contribution/revocation callbacks, exposes event subscription and
  publication, and can register an in-process language extension as a linked
  package. A chainable `FerrisManifestBuilder` covers the full schema-2.0
  builder. The host is enabled by default through the `ferris-host` Cargo
  feature; build with `--no-default-features` for the authoring-only addon.
  Hardened after review: unknown grant scopes are refused instead of silently
  becoming `any`, contribution leases are token-scoped so a stale lease cannot
  delete a replacement's entry, a rejected contribution callback leaves no
  phantom entry, replacing a linked extension tears the old runtime down, and
  event subscription ids are no longer truncated. `registerAgentTools` /
  `register_agent_tools` registers applied tool contributions on an agent, and
  MCP/Cordis runtime coverage was added. Host assembly now lives in the shared
  `ferris-bindings` crate so Python and JavaScript cannot drift. See
  `docs/ferris-bindings.md`.
- **Python/JS bindings**: catch up with the agent-runtime surface that landed
  with builtin tools 2.0. Both bindings now expose the working plan
  (`planItems`/`setPlan`/`clearPlan`/`addPlanTool`), durable goals
  (`Goal`, `GoalStore`, goal tools), mid-turn steering, deferred tools
  (`deferTool`/`deferToolPrefix`), live prompt sections, inline skills, and
  background jobs (`JobRegistry`, job tools, job-aware bash). Agent config
  gains `personaComplete`, `compactionInputTokens`, `toolOutputCap`,
  `parallelToolCalls`, `webSearch`, `contextBudget`, `skillCatalogue` and
  `fallbacks`.
- **Python bindings**: a run now writes its state back to the stored agent, so
  conversation history survives across calls and steering reaches the running
  turn. Inline skills declared with `skill(name, content)` are registered
  instead of silently ignored, and `with_bash` works (the config object
  accepts dynamic attributes).
- **Templates**: both bindings expose `renderTemplate`/`render_template`,
  `withMemory`/`with_memory` and `estimateTokens`/`estimate_tokens` from
  `react::template` and the agent context estimator.

- **Windows**: the bridge resolves `node.exe` on `PATH`, so the Cordis runtime
  and its end-to-end coverage run on Windows CI; the Cordis entry tests that
  still hit the drive-letter path bug are skipped there with that reason.
- **Release automation**: every pipeline releases only from a `vX.Y.Z` tag
  whose version matches Cargo, jsbos, nbos and the Python `__version__`, and
  skips when it does not; the macOS dmg is built through an explicitly sized
  image, linux-arm64 builds with thin LTO so it fits a free arm64 runner, and
  a dispatch publishes with the gh CLI instead of retagging.

## v4.0.1 (2026-11-04)

**Breaking**: extension manifest 2.0 is now the only schema the host accepts, so
1.x packages must be migrated with `bsh ext migrate`. That break is why the
platform moves to 4.x.

- **Extensions**: a host-neutral `ExtensionManager` now owns install,
  activation, rollback, retry and quarantine; packages load through the
  manifest 2.0 envelope with legacy migration; `bsh ext plan` previews the
  activation plan; versioned service contracts resolve into the activation
  graph with generation-scoped leases, drain handles and fiber-owned
  revocation.
- **Ferris SDK**: the standalone `bos-ext` crates (core, runtime, SDK, smoke)
  land with a manifest surface, an author API and examples; BSH runs on the
  SDK host facade.
- **Windows**: Process and MCP children launch in an AppContainer and their
  memory is capped with Job Objects; a Windows lane covers the extension
  runtimes.
- **Agent tools**: `find_tool`/`find_skill` expose full schemas, tool-call
  errors are schema-aware, batched file reads are bounded, and a multi-file
  patch tool applies atomically; tool arguments are enforced against their
  JSON schemas.
- **BSH workspace**: installed extensions can be searched and uninstalled,
  installed themes render as cards, extension commands appear in the slash
  menu, and image paste is fixed.
- **Runtime safety**: rkyv archives arriving over the network are validated,
  session ids stay out of logs, MCP discovery and writes are bounded and run
  off executor workers, WASM/Extism cancellation interrupts pure compute,
  child process groups are terminated, and per-id lifecycle work is serialized
  so failed reloads stay retryable.
- **Release**: a tagged pipeline builds and publishes installers for macOS
  (dmg/app zip), Linux (deb/rpm/tar.gz) and Windows (msi/zip), with signing and
  notarization hooks, a tagged-tree test gate and artifact verification.

## v3.0.2 (2026-10-07)

- **Fix**: rate-limiter and retry waits are now recorded on the live
  LLM path — `rate_limit_waits`, `total_rate_limit_wait` and
  `total_resilience_time` no longer stay at zero in production runs
- **Fix**: `react()`/`stream()` record per-run token, tool-call and
  tool-time deltas instead of engine-lifetime cumulative totals; the
  `total_tokens` hook reports the run's real token sum and
  `last_stream_*` values reset at the start of each stream
- **Fix**: jsbos publish workflow serializes concurrent publishes,
  refuses mismatched tags, repairs a stranded `latest` dist-tag, and
  stays idempotent for the root and per-platform packages
- **Docs**: root, nbos and jsbos READMEs refreshed for the 3.0.2
  baseline release

## v3.0.1 (2026-10-07)

- **Memory**: in-memory and file-backed memory store with metadata
  filters and a capacity cap; `Agent::remember`/`recall`/`forget`
  ergonomics; a `{{memory}}` placeholder places recalled context in
  the system prompt
- **Templates**: `react::template` renders `{{name}}` prompt
  placeholders with typed errors
- **Bindings**: Python and JS gain a `Memory` class (shared ranking
  fixture, JSON-lines persistence) and recall-on-run
- **Tools**: `AsyncFunctionTool::from_fn` for async closures that
  own their arguments
- **Resilience**: retry classification by error type
  (`LlmError::is_retryable`, `execute_with`); the rate limiter now
  honours `retry_backoff` and `auto_wait`
- **Observability**: per-call minimum/maximum LLM wall time in
  metrics, surfaced by both bindings
- **Cleanup**: removed dead collectors and unused module code

## v2.3.7 (2026-07-13)

- **Fix**: MCP tool namespaces now use `_` separator instead of `/` to comply with OpenAI function naming restrictions (only `[a-zA-Z0-9_-]` allowed)

## v2.1.1 (2026-05-13)

- **Discoverability**: Rewrote README with problem-first hook, badge row, "Why BOS?" comparison table, hero image
- **Discoverability**: GitHub Discussions enabled, topics updated (8 tags added)
- **Discoverability**: Wiki published (12 pages — Python, JS, Rust guides, architecture, config, MCP, skills, API ref)
- **Discoverability**: PyPI metadata updated (keywords, author, classifiers, project URLs)
- **Discoverability**: npm keywords updated (agent-framework, multi-agent, llm, mcp, brainos, tools, skills)
- **Feature**: Custom OG social card image for GitHub sharing
- **Release**: v2.1.0 release notes published

## v2.1.0 (2026-05-10)

- **Python rename**: `pybos` → `nbos` published on PyPI (`pip install nbos`)
- **Python**: Resilience config API (circuit breaker + rate limiter)
- **JS**: Hooks, plugins, session management in fluent BrainOS API
- **JS**: Performance metrics (`getPerfMetrics()` / `resetPerfMetrics()`)
- **JS**: Enhanced MCP client (`listPrompts()`, `listResources()`, `readResource()`)
- **JS**: TypeScript definitions expanded
- **Docs**: Updated user guides and API references for all languages
