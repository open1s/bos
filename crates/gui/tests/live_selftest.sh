#!/usr/bin/env bash
# The live-page harness: the shipped page against a stateful, asynchronous
# backend double.
#
# Every other guard in this directory either builds its own DOM
# (ui_selftest.html, shell_selftest.html) or runs the shipped page against a
# bridge that answers in one settled microtask (page_selftest.sh). The
# behaviors that make the app an app — a turn that streams in over time and
# ends, a queued follow-up flushing on finish, an approval that must be
# answered before a tool runs, a failed send rolling back with the prompt
# restored — exist only against a live backend, so nothing could see them.
# This runner assembles the shipped page with tests/live/bridge.js as its
# bridge (state across commands, events on timers, failure on demand) and
# appends tests/live/live_checks.js, then measures what a user would see.
#
# Like page_selftest.sh this runs anywhere Chrome runs; tauri-driver itself has
# no macOS backend (see e2e/README.md), and this is the macOS-reachable half of
# that suite's intent.
#
# Usage: crates/gui/tests/live_selftest.sh [chrome-path]
set -euo pipefail
CHROME="${1:-${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}}"
if [ ! -x "$CHROME" ]; then
  echo "live_selftest: no Chrome at '$CHROME' (set CHROME=/path/to/chrome)" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROFILE="$(mktemp -d -t bos-chrome-live)"
WORK="$(mktemp -d -t bos-live-selftest)"
PAGE="$WORK/page.html"
OUT="$WORK/dump.html"
trap 'rm -rf "$PROFILE" "$WORK"' EXIT

python3 - "$ROOT" "$HERE" "$PAGE" <<'PY'
import sys

root, here, out = sys.argv[1:4]
html = open(f"{root}/ui/index.html", encoding="utf-8").read()
bridge = open(f"{here}/live/bridge.js", encoding="utf-8").read()
checks = open(f"{here}/live/live_checks.js", encoding="utf-8").read()
base = f'<base href="file://{root}/ui/">'
# The <base> must be the FIRST child of <head> (URL resolution happens as each
# element is parsed), and the bridge must exist before app.js reads
# __TAURI__.event at module load — the same two placements page_selftest.sh
# argues for at length.
assert html.count("<head>") == 1, "the shipped page has one head to extend"
html = html.replace("<head>", "<head>" + base, 1)
html = html.replace("</head>", f"<script>{bridge}</script></head>", 1)
html = html.replace("</body>", f"<script>{checks}</script></body>", 1)
open(out, "w", encoding="utf-8").write(html)
print(f"live_selftest: assembled {len(html)} bytes from the shipped page")
PY

# --virtual-time-budget fast-forwards the bridge's scripted timers until the
# checks have run; --run-all-compositor-stages-before-draw keeps the rAF-driven
# streaming repaints honest instead of dropping frames the checks wait on.
"$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad \
  --disable-breakpad --no-first-run --no-default-browser-check \
  --disable-dev-shm-usage --window-size=1440,900 \
  --run-all-compositor-stages-before-draw --virtual-time-budget=15000 \
  --user-data-dir="$PROFILE" --allow-file-access-from-files \
  --dump-dom "file://$PAGE" > "$OUT" 2>/dev/null || true
python3 "$HERE/read_result.py" live "$OUT"
