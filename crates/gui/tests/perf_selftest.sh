#!/usr/bin/env bash
# The performance guard: what the shipped page costs, with budgets.
#
# The other guards answer whether a behavior exists. This one answers what it
# costs, because two of this UI's central claims were comments rather than
# facts: "coalesce streaming repaints to one per animation frame" and "the
# streaming repaint is the hot path". Neither could regress loudly, and the M1
# numbers in the requirements doc came from a fixture, not from the page the
# user runs.
#
# It runs the shipped page with tests/live/bridge.js as its backend, asked for a
# heavy workspace through window.__PERF_SCALE__ before the app boots (many
# chats, one very long transcript), and then measures against budgets in
# tests/perf/perf_checks.js. rAF is replaced by a queue there, so "one frame per
# burst" is counted exactly rather than inferred from wall time.
#
# Like the other page guards this runs wherever Chrome runs; it is the
# performance half of the same macOS-reachable loop (tauri-driver has no macOS
# backend, see e2e/README.md).
#
# Usage: crates/gui/tests/perf_selftest.sh [chrome-path]
set -euo pipefail
CHROME="${1:-${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}}"
if [ ! -x "$CHROME" ]; then
  echo "perf_selftest: no Chrome at '$CHROME' (set CHROME=/path/to/chrome)" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROFILE="$(mktemp -d -t bos-chrome-perf)"
WORK="$(mktemp -d -t bos-perf-selftest)"
PAGE="$WORK/page.html"
OUT="$WORK/dump.html"
trap 'rm -rf "$PROFILE" "$WORK"' EXIT

# The scale is stated here so the check names can quote it, and it is set before
# the bridge loads so the app's very first paint happens at that size.
SESSIONS="${PERF_SESSIONS:-300}"
MESSAGES="${PERF_MESSAGES:-5000}"

python3 - "$ROOT" "$HERE" "$PAGE" "$SESSIONS" "$MESSAGES" <<'PY'
import sys

root, here, out, sessions, messages = sys.argv[1:6]
html = open(f"{root}/ui/index.html", encoding="utf-8").read()
bridge = open(f"{here}/live/bridge.js", encoding="utf-8").read()
checks = open(f"{here}/perf/perf_checks.js", encoding="utf-8").read()
base = f'<base href="file://{root}/ui/">'
scale = (
    "<script>window.__PERF_T0=performance.now();"
    f"window.__PERF_SCALE__={{sessions:{sessions},messages:{messages}}};</script>"
)
# The <base> must be the first child of <head> (URL resolution happens as each
# element is parsed), the scale must be set before the bridge reads it, and the
# bridge must exist before app.js reads __TAURI__.event at module load.
assert html.count("<head>") == 1, "the shipped page has one head to extend"
html = html.replace("<head>", "<head>" + base + scale, 1)
html = html.replace("</head>", f"<script>{bridge}</script></head>", 1)
html = html.replace("</body>", f"<script>{checks}</script></body>", 1)
open(out, "w", encoding="utf-8").write(html)
print(f"perf_selftest: assembled {len(html)} bytes, scale {sessions} chats / {messages} messages")
PY

# --virtual-time-budget fast-forwards the bridge's timers so a search that
# debounces still lands, and --run-all-compositor-stages-before-draw keeps the
# frame-driven repaints honest.
"$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad \
  --disable-breakpad --no-first-run --no-default-browser-check \
  --disable-dev-shm-usage --window-size=1440,900 \
  --run-all-compositor-stages-before-draw --virtual-time-budget=20000 \
  --user-data-dir="$PROFILE" --allow-file-access-from-files \
  --dump-dom "file://$PAGE" > "$OUT" 2>/dev/null || true
python3 "$HERE/read_result.py" perf "$OUT"
