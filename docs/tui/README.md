# BSH TUI — Design Documentation

> Status: Proposed design baseline · Related: [Issue #452](https://github.com/open1s/bos/issues/452)

This directory defines the product requirements, architecture and UI/UX contract for the BSH terminal interface. It is a design-only deliverable: no runtime behavior or implementation is changed by these documents.

## Documents

| Document | Purpose |
|---|---|
| [RFC: BSH TUI Architecture](rfc.md) | Host/client boundaries, application contract, event delivery, recovery, security and Ratatui architecture |
| [UI/UX Specification](ui-ux.md) | Information architecture, screens, interaction rules, responsive layouts, accessibility and visual language |
| [Product Requirements & Delivery Backlog](requirements.md) | Normative requirements, acceptance criteria, phased delivery and story-sized work items |
| [Visual Concept](assets/bsh-tui-workspace.svg) | Editable vector concept of the primary workspace |

## Accepted design inputs

- Runtime Host: **standalone**
- Execution scope: **local-first**
- Event Journal: **durable**
- TUI rendering: **Ratatui**
- Product model: **one authoritative Runtime, multiple independent interfaces**
- ADR selections from Issue #452: ADR-01 Adaptive, ADR-02 Sidebar, ADR-03 Modal, ADR-04 Inline, ADR-05 Ratatui.

These are design inputs, not claims that the current code already provides the required Host, Journal, IPC or TUI contracts. All implementation dependencies must be verified by a code Gap Analysis.

## Design principles

1. Host owns domain state, execution, authorization and durable facts; interfaces are clients.
2. A client disconnect never implicitly cancels or transfers ownership of a Job.
3. UI state is a projection, never the source of truth for execution or security decisions.
4. Durable facts are replayable; ephemeral progress is bounded and may be coalesced.
5. Narrow terminals remain usable through progressive disclosure, not horizontal clipping.
6. The same application semantics must be reusable by GUI, TUI, Web and Mobile.

## Scope boundary

This package specifies the TUI experience and the minimum shared contracts it depends on. It does not authorize changing BSH runtime code, selecting an IPC wire format without a separate contract review, or claiming feature availability based on the mockup.
