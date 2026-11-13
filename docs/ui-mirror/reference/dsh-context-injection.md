# DSH context-injection row — reference measurement & BSH alignment

Source: `dsh-client-ui-chat/src/client/chat/ContextInjectionRow.tsx` +
`ContextInjectionRow.module.css` (extracted from `app.asar`; copies beside this
note). Body forms: `ContextBody.tsx`. Wording: the chat bundle's inline locale
(`message.contextInjection`).

## What the reference draws

One **collapsed disclosure per injected context source** (not one grouped row):

- Header: a 24px Tool-calls-rhythm row — leading icon
  (`IconContextInjectionOutlineRegular`, 14px), the title
  `message.contextInjection`, a 2px separator dot (`margin: 0 8px`), then the
  **producer label** (the source), and for a `notice` form one more dot + summary.
- Title wording: **EN "Context injection" / ZH "上下文注入"** (recall is
  "Session recall" / "跨会话召回"; tool changes are "Tools updated").
- Body (expanded): a **bounded code block** — `max-height: 141px`, scrollable,
  `margin: 4px 0 0 22px` (indented under the title), `padding: 10px 16px 12px 12px`,
  `radius-md`, background `--dsw-alias-markdown-code-block`, text
  `400 11px/16px` code face, colour label-tertiary. Model-facing text is bounded
  at 20,000 chars. Form-specific bodies (instructions file list, catalog, snapshot
  sections, recall counts) fall back to this opaque text body when unreadable.
- Expand on row click; collapsed by default.

## How BSH aligns (crates/bsh/src/ui/transcript.rs `inject_row`)

- One disclosure per `ChatMessage.injections` entry (persona / context /
  instructions), rendered **full-width above the prompt** — the reference draws a
  full-width disclosure, and the right-aligned user-bubble stack is shrink-to-fit
  and would collapse its width.
- Title uses `inject.title` = "Context injection" / "上下文注入" (reference bytes).
- Producer label beside the title via `inject.persona/context/instructions`;
  2px separator dot, 24px + delta header rhythm, chevron-on-hover leading.
- Body: `max_h(141px)` scrollport, code-block fill (`theme::new_session_fill`),
  `theme::CODE_FONT` at `theme::CODE_BLOCK_SIZE` (11px) / 16px line, indented
  22px + delta, selectable text. Collapsed by default; `inject_open` keyed by
  (row, slot), cleared on session switch.
- No missing key info: the persona block captures the WHOLE system prompt the
  turn sends (`Agent::injected_system_prompt` = the discovery block + the
  configured persona + live prompt sections + recalled memory), not just
  `config.system_prompt`; the context block carries the full runtime snapshot
  (cwd, current date/time + UTC offset, layout, sandbox, permission, preset) and
  the framed instructions. The current time matches the reference's
  `dsh-time-context`, which injects "now" on every turn.

## Honest deltas

- Icon: BSH's kit has no `IconContextInjectionOutlineRegular`; the row uses
  `CircleDashed` (the compaction marker's icon) as the stand-in.
- Producer label is the injection kind, not the instruction file path / preset
  name (the BSH `ContextInjection` record carries kind + content only).
- Body is the opaque code-block form; the reference's form-specific bodies
  (instruction file list, snapshot sections, catalog) are not modelled.
- Every injected block rides the turn that carried it: the persona is injected
  each turn, so it is recorded on every prompt row (no dedup) — an inject event
  never drops key info the model received.
