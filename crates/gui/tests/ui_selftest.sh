#!/usr/bin/env bash
# Headless selftests for crates/gui/ui — the *shipped* frontend in Chrome.
#
#   ui_selftest.html     transcript render contract + repaint timings (n rows)
#   shell_selftest.html  shell contract: panes, dividers and clamping, theme,
#                        typography, work-details, task state machine, document
#                        panel (including that document text stays inert)
#
# Chrome's own sandbox cannot initialise under the DSH file sandbox, hence
# --no-sandbox; both pages are local files and load no remote content. The
# 1440x900 window keeps the shell out of its narrow-window regimes so clamping
# is exercised at its normal size.
#
# Usage: crates/gui/tests/ui_selftest.sh [message-count] [chrome-path]
set -euo pipefail
N="${1:-5000}"
CHROME="${2:-${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}}"
if [ ! -x "$CHROME" ]; then
  echo "ui_selftest: no Chrome at '$CHROME' (set CHROME=/path/to/chrome)" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
PROFILE="$(mktemp -d -t bos-chrome)"
OUT="$(mktemp -t bos-ui-selftest)"
trap 'rm -rf "$PROFILE" "$OUT"' EXIT

run_page() {
  local page="$1" url="$2" rc=0
  "$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad \
    --disable-breakpad --no-first-run --no-default-browser-check \
    --disable-dev-shm-usage --window-size=1440,900 \
    --user-data-dir="$PROFILE" --allow-file-access-from-files \
    --dump-dom "$url" > "$OUT" 2>/dev/null || true
  python3 "$HERE/read_result.py" "$page" "$OUT" || rc=1
}

status=0
run_page ui "file://$HERE/ui_selftest.html?n=$N" || status=1
run_page shell "file://$HERE/shell_selftest.html" || status=1
exit "$status"
