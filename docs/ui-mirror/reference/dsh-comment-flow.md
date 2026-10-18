# Comment flow (measured from a screen recording)

Source: PixPin_2026-10-19_12-54-11.mp4 (17 s), frames at 1/4/7/10/13/16 s.

## The operation, step by step

1. **Select lines.** Dragging over the gutter/code highlights whole lines
   (f10: lines 39-43 on a light blue wash).
2. **A + appears in the left margin** at the selection's first line: a blue
   circle, ~18-20 px, with a white plus, sitting just left of the code column
   (f10, f4). It is the only affordance; nothing in the toolbar changes.
3. **Pressing + opens the composer inline, under the selection** - not centred,
   not in a side panel (f4, f13). A white rounded card (radius ~10, thin border,
   soft shadow), ~470 px wide, left-aligned with the code column:
   - title `Comment`, 13 px, semibold;
   - a wrapping textarea, ~110 px tall, placeholder `Add context for this change`;
   - footer: the anchor on the left (`line 34`, or `lines 39-43`), and
     `Cancel` / `Comment` on the right. `Comment` is filled and dark only once
     there is text - grey/disabled before (f4 vs f13).
4. **Pressing Comment commits it into the document flow.** The composer becomes a
   compact white card inserted between the commented block and the line after it
   (f10: card between line 34 and line 35; f16: card between line 43 and line 44).
   The card holds the text and, under it, the caption `line 34` / `lines 39-43`
   with a `...` menu at the right (f10, f13, f16).
5. **Comments persist and coexist.** f16 shows two: one anchored to line 34 and
   one to lines 39-43, each its own card, each with its caption and menu. The
   code below each card continues from the next line.

## What that means structurally

- A comment is a **document annotation with a line range**, rendered IN the code
  stream: the text below it shifts down. It is not a side panel or a floating
  overlay, which is what bsh draws today.
- The anchor is the SELECTION (a range), labelled `line N` for one line and
  `lines A-B` for several.
- The composer and the committed comment are the SAME box in the same place:
  committing swaps the textarea for the text plus caption.
- The `...` menu is the comment's own actions (the recording does not open it;
  bsh's drawer already carries delete, so delete is the measured minimum).

## bsh's state against it

| step | bsh |
|---|---|
| select lines, + in the margin | done: `comment_plus` follows `selected_range` |
| composer under the selection, `Comment` disabled until text | done: card with title, wrapping field, `line N` / `lines A-B`, Cancel / Comment |
| comment committed INTO the flow, above the next line | **not done**: the comment lands in a drawer list, and the card is an overlay |
| caption + `...` menu per comment | partial: the drawer row carries the caption and a delete control |

## The gap, precisely

Interleaving cards into the code requires bsh to render the document itself,
line by line, instead of handing it to the kit `Editor` - the editor paints one
text block and offers no seam for a child between two lines. Note that
`range_to_bounds` gives the pixels for a range, so an absolute overlay can sit at
the right y for every comment; what it cannot do is push the code down, which is
what makes the reference's cards read as part of the file.

