# BSH TUI — UI/UX Specification

- **Status:** Proposed design baseline
- **Related:** [Issue #452](https://github.com/open1s/bos/issues/452)
- **Primary reference:** [Workspace concept](assets/bsh-tui-workspace.svg)

## 1. Product experience

BSH TUI is an operational workspace for supervising Agent work, not a terminal chat wrapper. The user should be able to inspect what is running, understand why an action is waiting, approve or reject a precisely described action, follow delegation and recover context without typing commands for every operation.

Primary UX goals:
- Keep conversation and execution context together.
- Make Jobs and Agents independently discoverable.
- Surface security decisions before side effects.
- Preserve state across resize, navigation and reconnect.
- Keep keyboard-only operation first-class.
- Avoid overwhelming small terminals.

## 2. Information architecture

Primary destinations:
1. **Conversations** — sessions, message timeline, composer and execution cards.
2. **Jobs** — queued/running/waiting/completed/failed/interrupted work.
3. **Agents** — parent/child delegation tree, status, assignment and outputs.
4. **Tools** — available tool definitions and execution history.
5. **Capabilities** — granted/requested capabilities and scope.
6. **Activity** — chronological operational events and progress.
7. **Audit** — security-sensitive decisions and durable audit facts.

Secondary destinations:
- **Inspector** — context-sensitive detail for selected message, job, agent, tool or approval.
- **Files** — artifacts and file references.
- **Settings** — keymap, theme, layout, connection and local preferences.

Navigation is persistent when width permits. On narrow terminals use a command palette and back stack; all screens remain addressable without a mouse.

## 3. Workspace anatomy

The primary workspace has three logical regions:

- **Sidebar:** navigation, workspace identity, connection status and optional pinned destinations.
- **Main:** active screen, timeline or focused task view.
- **Inspector:** contextual details, timeline, input/output, authorization and diagnostics.
- **Composer:** persistent input area on Conversation screen.
- **Status bar:** Host connection, active mode, selected context and essential shortcuts.

The visual concept shows a three-column wide layout. It is a composition reference, not a literal pixel specification.

## 4. Adaptive behavior

Use progressive disclosure. Never make the main timeline or composer horizontally scroll as the default response to a narrow terminal.

| Width | Behavior |
|---|---|
| < 80 cols | Single screen; sidebar becomes command palette/back navigation; Inspector becomes detail screen |
| 80–99 cols | Compact sidebar + main; Inspector opens as modal |
| 100–119 cols | Sidebar + main; Inspector hidden by default and opened on demand |
| 120–159 cols | Sidebar + main + Inspector |
| ≥ 160 cols | Three-column layout with additional metadata and wider content |

At height < 24 rows, reduce secondary chrome, collapse nonessential status labels and prioritize active content. At very small heights, switch to a focused list/detail screen.

Resize invariants:
- Preserve active destination, selected entity and composer draft.
- Keep cursor position and text selection stable where terminal capabilities permit.
- Recompute scroll bounds without jumping to unrelated content.
- Do not silently dismiss a pending approval or lose its identity.

## 5. Conversation timeline

Render typed semantic blocks; never infer domain meaning from markdown appearance.

Supported block kinds:
- User message
- Assistant message
- Reasoning summary (policy-filtered; never expose hidden/internal reasoning)
- Tool invocation and tool result
- Delegation / agent handoff
- Approval request and decision
- Error / recovery notice
- System event and execution status

Each block should expose author, timestamp, status and relevant correlation identity where useful. Long outputs are collapsible but retain copy/export and Inspector access. Tool input/output should be structured and redacted according to authorization policy.

Streaming behavior:
- Clearly distinguish streaming text from committed message state.
- Coalesce high-frequency token updates for rendering.
- Keep message completion and tool transition boundaries visible.
- Show reconnect/replay state without duplicating messages.
- On a replay gap, refresh from a Host snapshot and visibly indicate that history was resynchronized.

## 6. Composer

Composer requirements:
- Multi-line editing with Unicode and wide-character correctness.
- Paste support for multiline text.
- Cursor movement, selection, deletion and undo where supported.
- Draft preservation during navigation and resize.
- Explicit send behavior; no accidental send from newline insertion.
- Command and mention completion with discoverable suggestions.
- Clear distinction between a slash command and ordinary message text.

Default key behavior:
- Enter: send when composer is in single-line submit mode; in multiline mode use configured submit chord.
- Shift+Enter: insert newline.
- Ctrl+Enter: optional explicit submit shortcut, configurable by platform.
- Tab: cycle completion/focus according to current mode; never silently submit.
- Escape: close completion first, then exit transient mode; preserve draft.
- Paste: insert text as data; pasted control sequences must not execute as key commands.

Exact defaults must be validated across macOS, Linux and Windows terminals before implementation.

## 7. Interaction modes and keymap

Modes are visible and discoverable:
- **Normal:** navigate and invoke actions.
- **Insert:** edit composer or editable field.
- **Command:** command palette / slash-command entry.
- **Search:** search current screen or history.
- **Inspect:** move focus into contextual details.
- **Dialog:** modal interaction with focus containment.

Baseline navigation:
- `j/k` or arrows: move selection in lists.
- `g/G`: top/bottom in scrollable lists where enabled.
- `Enter`: open/select.
- `Esc`: back/close transient UI.
- `/`: search.
- `?`: keymap help.
- `Ctrl+P`: command palette (configurable).
- `Tab/Shift+Tab`: move focus among panes/controls.
- `Ctrl+C`: must not be overloaded to cancel an active Host Job without a confirmation/explicit command.

A command palette provides fuzzy navigation and action discovery. Every destructive or security-sensitive command requires contextual confirmation.

## 8. Inline execution cards and Inspector

Inline execution card shows:
- Tool/agent identity and invocation state.
- Short purpose and safe summary of arguments.
- Queued/running/waiting approval/completed/failed/interrupted state.
- Duration and result summary when available.
- Explicit action to inspect full details.

Inspector tabs:
- Overview
- Input
- Output
- Timeline
- Authorization
- Diagnostics

Inspector content must be permission-filtered and redact secrets. Collapsing an inline card changes only presentation state. It cannot cancel execution or delete output.

## 9. Jobs screen

Jobs are first-class resources independent of the currently open Conversation.

List filters: All, Active, Waiting, Completed, Failed, Interrupted. Each row shows Job ID, short goal, owning Session, current state, age/duration and responsible Agent. Detail view shows lifecycle timeline, dependencies, cancellation availability, artifacts and recovery guidance.

Never show a Cancel action unless the Host confirms the Job is cancellable in its current state. Cancel is a request, not an immediate state mutation; wait for Host confirmation.

## 10. Agents screen

Show delegation as a navigable tree:
- Agent identity and role.
- Parent/child relationship.
- Assigned task / goal summary.
- State: planning, working, waiting, completed, failed or idle.
- Last activity and output reference.

Do not imply that an Agent is executing merely because it exists or is online. Distinguish availability, assignment and active execution.

## 11. Approval modal

Approval is a high-priority, focused modal. It must show:
- Requesting Agent and Tool.
- Exact action description and canonical action digest/reference.
- Target resource and workspace.
- Requested capability and scope.
- Risk classification and whether the action is destructive.
- Expiration and current request status.
- Available choices (Approve once, Deny, optionally scoped grant only if the Host contract supports it).

Default focus must not be on Approve. The user must be able to inspect details before deciding. Expired, stale, duplicate or already-decided requests are read-only and explain why. The UI displays success only after Host acknowledgement.

## 12. Capabilities, Activity and Audit

Capabilities screen separates available, granted, requested, denied and expired capabilities. Show scope and principal/agent binding; never imply a tool is authorized merely because it is listed.

Activity is operational and chronological; Audit is security-sensitive and durable. They are not interchangeable. Audit records should identify actor, action, target, decision, timestamp, outcome and correlation reference, with secrets excluded.

## 13. Visual language

- Dark obsidian/navy foundation with restrained cyan, green, amber and violet accents.
- Neutral surfaces and clear contrast; avoid neon on every element.
- Color is never the only status signal: pair color with text/icon/state label.
- Distinguish focus, selection, hover (if supported), disabled and destructive states.
- Use consistent borders, spacing, alignment and density.
- Support terminal 16-color fallback and monochrome terminals.
- Respect terminal theme where practical; allow light/dark variants only after contrast validation.

## 14. Accessibility and terminal compatibility

- Keyboard-only navigation for every action.
- Visible focus indicator and predictable focus order.
- No color-only meaning.
- Screen-reader/terminal accessibility labels where supported by terminal ecosystem.
- Unicode width handling for CJK, emoji and combining marks.
- Correct handling of bracketed paste, resize, terminal suspend/resume and alternate screen restoration.
- Graceful fallback for terminals without mouse, true color or advanced key reporting.
- Never leave terminal echo/raw mode altered after panic or process exit.

## 15. Empty, error and recovery states

Every screen must define:
- Empty state with a next action.
- Loading state with non-blocking navigation where safe.
- Offline/Host unavailable state.
- Stale cached data state.
- Permission denied state.
- Validation/conflict state.
- Recovery/replay state.
- Partial/unknown operation outcome.

A timeout is presented as “outcome unknown — checking Host” rather than “failed”. Recovery guidance must be actionable and must not recommend blind retry for unknown external side effects.

## 16. UX acceptance checklist

- [ ] Every destination is reachable by keyboard.
- [ ] At 80 columns the user can navigate and complete core tasks without horizontal clipping.
- [ ] Composer draft survives resize and screen changes.
- [ ] Tool invocation/result are distinguishable from assistant prose.
- [ ] Job remains visible after leaving its originating conversation.
- [ ] Approval shows exact target, scope, action and expiry.
- [ ] Approval result is Host-confirmed, not optimistic.
- [ ] Disconnect does not imply Job cancellation.
- [ ] Reconnect does not duplicate committed timeline items.
- [ ] Stale/offline projections are clearly marked.
- [ ] CJK and wide Unicode text align correctly.
- [ ] Terminal modes are restored after normal exit and panic.
