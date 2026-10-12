---
module: gui
tags: [ui, verification, macos, accessibility, computer-use, reference]
problem_type: best-practice
---

# Drive the real window, and read the reference one

## Problem

Two claims about this GUI cannot be checked from a fixture: that the *shipped
bundle* behaves (a missing Tauri capability left the event bridge dead while every
fixture stayed green — rev 123), and that the middle panel still matches the
*reference* the work is measured against. The reference is the DeepSeek Harness
app, whose checkout lives inside an asar archive: the filesystem cannot read it,
so neither `ls` nor ripgrep can describe its layout.

## Approach

Open Computer Use (`open-computer-use` / `ocu`) walks a running app's
accessibility tree and drives it by element, not by coordinates:

```sh
ocu doctor                                                    # permissions
ocu call get_app_state --args '{"app":"DeepSeek Harness"}'    # the reference
ocu call get_app_state --args '{"app":"BOS"}'                 # our window
ocu call click --args '{"app":"BOS","element_index":"20"}'    # after a fresh read
```

The reference dump is the source of truth for wording and structure that no
screenshot settles: it is what showed that the reference's turn foot is a
`disabled button` labelled "Completed in 2h 19m 53s" with a nested "Completed in"`
text, that its header carries a "1 background job running" button and a "More
actions" popup, and that a fenced block is labelled "Code block".

## Verification

`ocu doctor` reports `accessibility=granted, screenRecording=granted` here
(macOS 27.0.1). Reading `~/.bos/gui/log/gui.log` — the file the current build
writes — is the complementary check for the *event* side: a refusal appears there
by topic now, so a launch that logs nothing refused is a window that can hear its
backend.

## Notes

- `element_index` is scoped to one `get_app_state` result; re-read after any
  rebuild or UI change.
- The pointer is global on macOS, so `drag` needs
  `OPEN_COMPUTER_USE_ALLOW_GLOBAL_POINTER_FALLBACKS=1` and the user's explicit
  consent; the semantic tools do not move it.
- The desktop is the user's own session: read freely, click only when asked.
