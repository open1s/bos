# dsh compaction: /compact, the automatic trigger, and the marker

Measured from the shipped DeepSeek Harness asar, not from memory. The corpus
was missing when this round started, so it was re-extracted with
docs/ui-mirror/reference/extract-asar.py (data base = 8 + pickle size) into
docs/ui-mirror/reference/pkgs:

- dsh-command-compact (lib/index.js, locale/{en,zh}.json, README)
- dsh-compaction (the contract and checkpoint predicate)
- dsh-compaction-basic (the shipped backend, 49 KB lib/index.js)
- dsh-compaction-image-offload, dsh-compaction-tool-result-pruner
- dsh-client-ui-chat (CompactionItem + GenericCommandCard + dictionaries)
- dsh-client-ui-commands / dsh-commands (the catalog row and the command plane)
- dsh-agent-loop, dsh-token-meter (the trigger point and the meter)

## The command (dsh-command-compact/lib/index.js)

Registration: name compact, description 'Compact older conversation history',
definition id '@deepseek-ai/dsh-command-compact'. The handler receives
rawInput and parses nothing:

    if (invocation.rawInput.trim().length > 0) return {
      kind: 'error', text: USAGE
    };
    const USAGE = 'Usage: /compact (no arguments)';

So the grammar is exactly '/compact' with no arguments or flags; any
non-empty trimmed suffix is an error result carrying the usage line. The
command takes no range, no count, no force flag.

Outcomes and their exact English strings (the package ships no zh dictionary
for these; zh.json only carries meta.title = '上下文压缩命令'):

| Result | kind | text |
|---|---|---|
| commit | success | Compacted {N} history items (~{T} tokens). |
| nothing safe | success | No compactable history yet. |
| busy | error | Compaction is unavailable because this process has an active compaction, or the agent is not idle. |
| cancelled | error | Compaction cancelled. |
| changed | error | The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log. |
| summary | error | Compaction could not produce a useful summary. The attempt is recorded in the session log. |
| commit | error | Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying. |
| persistence | error | Compaction finished, but the session could not be saved. |

The success text is built with result.shadowedSeqs.length and
result.shadowedTokenCount, i.e. the number of replaced surface events and
their estimated tokens - not the net difference after the checkpoint lands.

## Manual vs automatic

Both use the same engine, the same transaction and the same marker; the
manual path is the command above. dsh-compaction-basic registers automatic
listeners only when config.auto is true (default):

- agent/pre-step -> compactIfNeeded(agent, 'pressure', signal) before the
  request is derived; a failure warns and continues the turn.
- agent/request-error -> on CONTEXT_WINDOW_EXCEEDED_CODE, one maximal balanced
  head reduction, then a retry only after the surface replacement generation
  advances.

Trigger and retention defaults (README table and resolveConfig/resolveCompactSpec):

| Field | Default |
|---|---|
| thresholdRatio | 0.8 |
| headroomTokens | 65536 |
| retainRatio | 0.16 of (W - O) |
| retainTokens | none (mutually exclusive with retainRatio) |
| maxTokens | headroomTokens |
| compactionRetries | 1 |
| maxOverflowRetries | 1 |
| auto | true |

thresholdTokens = floor(min(W * thresholdRatio, W - O - headroomTokens));
retainTokens = floor((W - O) * retainRatio), default 0.16. Both must be
positive and retain < threshold or the target refuses configuration.

Manual compactNow selects with retain 0 (everything safe); automatic
pressure keeps the 16% tail. Overflow recovery also uses retain 0.

## What the boundary is

selectCompactableRange(session, measurement, retainTokens):

1. Price the current surface nodes with ctx.tokenMeter.
2. The range starts at surface node 0, except that a system/message at node 0
   is never shadowed: start at the first node that is not a system message.
3. Walk the tail backwards accumulating node tokens until >= retainTokens;
   that index is the first kept node.
4. While that cut is not toolPairingBalancedBefore, walk it forward (keep
   fewer nodes) - a cut may never split an assistant tool call from its
   result.
5. If the kept index is at or before the start, return null (nothing safe).

Tool pairing is computed from content (a tool-call counts +1, a tool/result
-1) over current surface order, with a per-session cache; a missing seq or an
orphan result is corrupt surface state.

## The transaction and what the model receives

compactSurfaceRegion: validate the span and the durable lock, append
compaction/start (log only; this is the lock), summarize, revalidate stability
(whole-surface for automatic, selected-span for manual), reject a summary that
does not shrink, append compaction/summary, append ONE user/message carrying
the checkpoint with surfaceOp { op: 'replace', startSeq, endSeq }, then exactly
one compaction/end. A crash leaves an unmatched start, which is the busy lock
until a later session/end-seed proves it stale.

The replacement message frames the summary as:

    <CHECKPOINT_PREAMBLE>

    <compacted-summary>
    <model text>
    </compacted-summary>

PREAMBLE = 'This is an automatically generated checkpoint condensing an
earlier span of the conversation to free up context. Treat the captured
context as established background and build on it without restating it.
Continue the task directly from the messages that follow, without
acknowledging this checkpoint.'

Only returned text enters the checkpoint (reasoning and tool calls excluded);
image output fails with UNSUPPORTED_CONTENT. The raw summary also rides on
compaction/summary. deriveMessages() renders the checkpoint followed by the
retained nodes; shadowed events stay in the log, so replay is deterministic.

The summarizer request replays the derived system message at node 0, the
header's tools, and the shadowed region's messages byte-for-byte, then appends
one frozen COMPACTION_INSTRUCTION user message (the eight-section Markdown
checkpoint brief with 'Output only the checkpoint text'). That makes the call
a prefix of the last routed request, so the provider cache is reused.

The checkpoint source is { kind: 'compact-checkpoint', compactionId,
sourceCommandId? }. isCompactCheckpointSource is the backend-independent
predicate a consumer uses to recognize condensed history after persistence.

## The transcript row (dsh-client-ui-chat)

Two renderers:

- CompactionItem (automatic key 'compaction', and the landed manual result):
  title = 'Context compacted' for automatic, 'compact' for manual; summary =
  'Compacted {items} history items (~{tokens} tokens)' when both counts are
  present; expandable only when node.summary !== null, and the body renders
  the summary as Markdown.
- GenericCommandCard (the manual command while running and every non-landing
  outcome): title = node.name ('compact'), running summary = 'Compacting
  context…', then the outcome text; data-error paints the summary in the
  error colour; a multiline outcome gets an expandable pre body.

Dictionary (en / zh):

| key | en | zh |
|---|---|---|
| message.compaction | Context compacted | 上下文已压缩 |
| message.compaction.running | Compacting context… | 正在压缩… |
| message.compaction.completed | Compacted {items} history items (~{tokens} tokens) | 已压缩 {items} 条历史记录（约 {tokens} tokens） |
| message.compaction.expand | View compaction summary | 点击查看压缩摘要 |
| message.compaction.unavailable | Compaction summary unavailable | 压缩摘要不可用 |
| message.compaction.commandTitle | compact | compact |

Placement: the marker's anchor is the checkpoint EVENT's seq
(`compactSummary` sets `seq: checkpoint.event.seq`), and the checkpoint
user/message is appended at the end of the log - after the retained tail and
before anything added later. bsh records the transcript length at commit
(`CompactionRecord.after`) and draws the marker after that many messages; a
legacy record draws at the end. An earlier bsh build drew the marker above
`session.compacted`, i.e. above the tail, which put the row somewhere the
reader was not looking right after running the command.

Measured row (MessageItem.module.css / CompactionItem.js): compactionRow has
2px vertical padding; compactionButton is width 100% at height 24px + the
content-font delta, radius-sm, label-tertiary, no background, left aligned;
compactionLeading is 16px + delta square with a 14px + delta glyph and a 6px
right margin; the context icon swaps to a chevron at opacity 1 only while the
button is hovered or focus-visible (both stay at 0 otherwise);
compactionTitle and compactionSummary are secondary size on a 24px + delta
line; compactionSep is a 2x2 caption-coloured dot at radius 1 with 8px
horizontal margins; compactionSummary is nowrap/ellipsised; compactionBody is
padded 4px 0 4px 22px + delta. GenericCommandCard shares that DisclosureRow
geometry, with its own 2x2 separator and error colour.

## Meter, goal, plan, reload

- Meter: the replacement trades the shadowed span for one summary. The
  reported shadowedTokenCount is the sum of the selected nodes' heuristic
  tokens; the checkpoint's own price is counted on the next request.
- Goal and plan: compaction is a SURFACE operation only. dsh-goal and plan
  state are separate durable domains; no compaction path reads or writes
  them, so both survive. bsh mirrors this and pins it with tests.
- Reload: the checkpoint is a logged user/message with a
  compact-checkpoint source; the log replays to the same condensed surface.
  Compaction events are log-only and never surfaceOp events.
- bsh persistence: Session.compacted is a GLOBAL transcript index and
  CompactionRecord.after is a global slot. Startup loads only the log's tail
  (loaded_from), so request_start maps the boundary into the window - otherwise
  a 600-row session reloaded as a 256-row window would either elide the
  retained tail or keep already-folded rows. The summary and the boundary are
  written with the session document; the next turn (and a goal round) rebuild
  the request from them. A document written before this indexing stored a
  window-local boundary; read as global it can only un-fold extra rows (never
  lose retained ones), and the checkpoint still replays.
- Images: dsh-compaction-image-offload owns image selection and records
  omissions on compaction/prune; the seam refuses image output but the
  adapter must resolve or reject request images.

## bsh parity

| Reference behaviour (quoted) | bsh now | Status |
|---|---|---|
| '/compact' takes no arguments; a suffix returns 'Usage: /compact (no arguments)' as an error result | parse_slash_line routes both bare and suffixed lines to compact_command_line; the suffix writes the usage row and notice, never a message | mirrored |
| Busy text names an active compaction or a non-idle agent | streaming gate + a runtime compacting set, same text, error kind | mirrored |
| 'No compactable history yet.' is a SUCCESS result | refusal row with error=false | mirrored |
| Success text reports shadowedSeqs.length and shadowedTokenCount, with a full stop | land_manual_compaction reports replaced rows (the replayed checkpoint is not a row) and before_tokens | mirrored; bsh counts visible transcript rows |
| changed/commit/persistence are distinct codes | not reachable: one synchronous install+save, no second writer, saved errors discarded | deliberately different, documented in shell.rs |
| cancelled is a distinct code | key and rendering exist; no cancel control or signal path is exposed, so the arm is unreachable | deliberately different |
| No provider/model is a summarizer failure ('could not produce a useful summary') | the pre-flight model/credential gate reports slash.compactSummary (same visible text); no request is made | mirrored in text, faster in path |
| Running command row 'Compacting context…' with a command title | a persisted transcript command row, title 'compact', running key msg.compaction.running | mirrored; plain text instead of TextShimmer (no seam) |
| The card carries visually hidden 'Running' / 'Failed' labels for assistive tech | GPUI exposes no accessibility tree in bsh | impossible here (no a11y surface) |
| Outcome text is a hardcoded English payload from the command handler (the zh command package ships only meta.title); only the UI rows are localized | every outcome is localized through the locale, with the reference's own zh wording where it exists | deliberately different, per the task's localization rule; the stored row keeps the language it landed in |
| Landed manual compaction replaces the command card with the marker | land_manual_compaction clears the row; the marker is drawn at the checkpoint event (record.after, after the retained tail), legacy records at the end | mirrored |
| Automatic compaction draws 'Context compacted' with the same counts | automatic Runner compaction emits AutoCompacted; land_automatic_compaction records manual=false, the marker title follows it | mirrored |
| Threshold floor(min(W*0.8, W-O-B)), retain 16% of W-O | bsh has no per-model capacity metadata; the workspace context_budget is the ceiling and retain = 16% of it, at the same pre-request point | deliberately different; documented in state.rs |
| compactionRetries default 1, re-checked at every pre-step, overflow recovery on CONTEXT_WINDOW_EXCEEDED | one automatic attempt per turn, immediately before the first token (Runner::spawn -> Agent::compact_now), proven end-to-end by an_over_threshold_turn_compacts_automatically_before_streaming; no per-step re-check and no overflow hook | deliberately different; the per-step seam lives in the agent loop and bsh has no adapter overflow classification |
| Command outcome lives only in the transcript card | bsh draws the same persisted row AND repeats the line in its status notice, which the reference shell does not have | deliberately different; the status notice is bsh's global error surface for its other commands |
| The auxiliary call replays the exact system/tools/messages prefix, then the frozen instruction, so the provider cache is warm | bsh renders the shadowed span as text and sends a separate completion with the system prompt and no tool schemas; the instruction text is the same constant | deliberately different; the cache-prefix reuse needs a streaming seam bsh's summarizer does not use |
| "prompts you send while it runs are accepted and start after it finishes"; compactNow needs "an idle agent with no waking queued work" | send queues while is_compacting, the manual gate also refuses when has_queued, and the completion drains the inbox | mirrored |
| Summary framing preamble + compacted-summary tags, tags flush against the body | render_checkpoint emits preamble, blank line, `<compacted-summary>`, the summary and the close tag with no separator, byte-identical to the shipped frameSummary; it is replayed as the FIRST message of the next request and priced by the meter | mirrored |
| A summary must be smaller than the span it shadows; an empty answer is a summary failure | compact_now prices the framed checkpoint against the removed span and refuses before install when framed >= span; an empty summary returns an error, not a null outcome | mirrored (agent tests pin both) |
| Checkpoint survives reload through the log | Session.compactions + compact_command are persisted and hydrated by Store::ensure_document; request_transcript rebuilds the checkpoint | mirrored (fixed: hydration previously dropped both) |
| Goal/plan untouched | same; tests pin goal, plan and plan mode across manual and automatic compaction | mirrored |
| The boundary walks back to a tool-pairing balanced cut | bsh's agent-history bridge is text-only: tool calls/results are display rows (ChatMessage.tools) and never become agent messages, so there are no tool units to split. A fold lands between user/assistant text rows | impossible here (no tool-history bridge); stated in caps.rs |
| Image-occurrence pricing and image offload | the summarizer receives text only; an image-only row has no text to count and a folded image is not replayed to the summarizer | impossible here (no image-history seam); stated in caps.rs |
| Overflow recovery (CONTEXT_WINDOW_EXCEEDED, one retry) | bsh surfaces the provider error; no overflow hook | impossible here (no adapter overflow classification seam) |
| Image offload/pruning companions | absent | impossible here (no image-history seam) |

### Parked keys

`slash.compactCancelled`, `slash.compactChanged`, `slash.compactCommit`,
`slash.compactPersistence` and `msg.compaction.unavailable` ship in both
locales because the reference's taxonomy contains them, but bsh cannot reach
them: there is no cancel control, no durable bracket with a second writer, no
surface-changed revalidation (the install is one synchronous step), save
errors are discarded by `save_session`, and an empty summary is refused
before a record is written. They are vocabulary for seams that do not exist
here, not dead code to delete.

## Tests that prove it

- the_next_request_replays_the_stored_checkpoint: the request begins with the
  framed checkpoint, and the meter's message bucket includes its price.
- a_compaction_boundary_maps_the_summarizer_span: removed counts map to the
  stored index, including the synthetic head.
- an_automatic_compaction_records_the_marker_and_keeps_durable_state: automatic
  boundary, manual=false record, checkpoint replay, goal/plan/plan-mode survival.
- the_automatic_threshold_prices_the_request_and_keeps_the_reference_tail:
  below/above the ceiling and the 16% tail.
- an_over_threshold_turn_compacts_automatically_before_streaming (bsh runner,
  local LlmClient mock): the first event is AutoCompacted, the agent session
  starts on the framed checkpoint with the retained tail, and the turn still
  streams its chunks afterwards.
- compact_now_installs_a_model_written_checkpoint (agent): the summarizer seam
  replaces all but the newest message with the model's text framed as a user
  checkpoint.
- compact_now_refuses_an_empty_summary / compact_now_refuses_a_summary_that_does_not_shrink_the_context
  (agent): the two fail-closed gates leave the history untouched.
- the_compact_grammar_is_argument_free: bare, whitespace, suffix and near-miss.
- the_compact_command_row_persists_its_outcome: running -> outcome -> cleared.
- a_compaction_survives_a_reload / a_compact_refusal_survives_a_reload: the
  checkpoint and the command card read back from disk.
- a_paged_session_reloads_its_checkpoint_and_retained_tail: a 600-row session
  restarts as a 256-row tail, keeps the GLOBAL boundary, and the next turn
  carries the checkpoint plus exactly messages 500..600; the meter prices that
  request, not the whole log.
- the_compact_command_card_draws_running_and_failed_outcomes: the running row
  and the failed row are real bounds, with no marker on failure.
- the_compaction_marker_expands_to_the_checkpoint: the marker is drawn at the
  checkpoint event (the scroller child after the retained tail), and a click
  opens the summary body.
- an_automatic_checkpoint_sits_before_the_live_reply: a live turn's empty
  assistant placeholder stays BELOW the marker, so the streamed reply appears
  after the checkpoint row.
- the_typed_compact_command_refuses_arguments_with_its_usage_line: the typed
  suffix is a command, not prose, and appends nothing.
- the_compact_command_without_a_model_reports_the_summary_failure: the
  pre-flight refusal uses the summarizer's own wording.
- the_compact_row_refuses_when_there_is_nothing_to_compact /
  the_compact_row_refuses_while_a_turn_streams: the two gates and their rows.
- compact_outcomes_use_the_reference_wording: every string, both locales,
  pinned to the reference's own copy where it ships one.
