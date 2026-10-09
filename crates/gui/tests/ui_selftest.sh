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
  python3 - "$page" "$OUT" <<'PY' || rc=1
import html as H, json, re, sys

page, path = sys.argv[1], sys.argv[2]
raw = open(path, encoding="utf-8", errors="replace").read()
m = re.search(r'<pre id="result">(.*?)</pre>', raw, re.S)
if not m:
    print(f"[{page}] result block missing (script threw at load?)")
    raise SystemExit(1)
d = json.loads(H.unescape(m.group(1)))
for k, v in d.items():
    if k not in ("checks", "failures", "total", "page"):
        print(f"[{page}] {k}: {v}")
checks = d.get("checks")
rows = (
    [(k, str(v).startswith("PASS")) for k, v in checks.items()]
    if isinstance(checks, dict)
    else [(c["name"], bool(c["ok"])) for c in (checks or [])]
)
for name, ok in rows:
    print(f"[{page}] " + ("  ok  " if ok else " FAIL ") + name)
failures = int(d.get("failures") or 0)
print(f"[{page}] failures: {failures} / {len(rows)}\n")
raise SystemExit(1 if failures else 0)
PY
  return "$rc"
}

status=0
run_page ui "file://$HERE/ui_selftest.html?n=$N" || status=1
run_page shell "file://$HERE/shell_selftest.html" || status=1
exit "$status"
