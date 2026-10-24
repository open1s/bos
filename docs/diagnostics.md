# Diagnosing a flicker or relayout report

BSH ships an off-by-default, per-frame trace. A flicker report is otherwise
undiagnosable: a screenshot shows the pane, not how often it repainted or what
moved. This trace answers exactly that.

## Capture

Run the app from the build directory with the trace pointed at a file:

```sh
pkill -f "target/debug/bsh"
BSH_FRAME_TRACE=/tmp/bsh-frames.log ./target/debug/bsh 2> /tmp/bsh-stderr.log
```

The value `1` (or an empty value) writes to `$TMPDIR/bsh-frames-<pid>.log`
and prints the resolved path on stderr. Any other value is the path. The
environment is read ONCE at launch; unset is the default and opens nothing.

Reproduce the flicker, quit the app, and attach the trace file to the report.
If the pane settles, the file stops growing: no line appears until the next
painted frame.

## One line per frame

```text
frame=812 dt_ms=16.7 tab=chat streaming=yes offset=-1282.0 max=1282.0 content=2102.0 rows=32 source=stream-delta
```

- `frame` - the process's monotonic frame number.
- `dt_ms` - wall time since the previous painted frame, in milliseconds.
- `tab` - `chat`, `trajectory` or `audit`.
- `streaming` - yes while any session's turn is streaming.
- `offset` - the transcript scroller's offset (negative going down).
- `max` - the transcript scroller's maximum offset (positive).
- `content` - scrollable content height: max offset plus the viewport.
- `rows` - transcript child count: the timestamp row plus the messages.
- `source` - the last instrumented request that asked for the frame:
  `elapsed-clock`, `stream-delta`, `stream-ended`, `tail`,
  `wheel-ease`, `jobs-clock`, `subagent-clock`, or `unknown`
  for a click, a hover, or any path that was not instrumented.

A line whose `source` is `elapsed-clock` and whose `offset`/`max`/`content` are
identical to the line before it is a repaint that changed nothing except the
running line's seconds; that is what the diagnostic exists to expose.

## Bound

One file is capped at 20,000 lines. When the cap is reached the next line says
so and the trace stops, so leaving it on cannot fill the disk. Re-run with the
same path to truncate and start over.
