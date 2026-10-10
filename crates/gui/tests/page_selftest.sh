#!/usr/bin/env bash
# Run the checks that need the *shipped page* rather than a fixture.
#
# ui_selftest.html and shell_selftest.html both build their own DOM, so the real
# ui/index.html was never executed by any guard: the dialog landmarks, the
# listbox owning its options, and focus hand-back could not be observed at all.
# This runner assembles a temporary page from the shipped bytes — the markup
# verbatim, with a <base> so its relative script and style URLs resolve, the
# Tauri stub injected ahead of app.js exactly as the transcript harness does,
# and tests/page_checks.js appended last. Nothing else is rewritten.
#
# Usage: crates/gui/tests/page_selftest.sh [chrome-path]
set -euo pipefail
CHROME="${1:-${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}}"
if [ ! -x "$CHROME" ]; then
  echo "page_selftest: no Chrome at '$CHROME' (set CHROME=/path/to/chrome)" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROFILE="$(mktemp -d -t bos-chrome-page)"
# The page needs a .html name: Chrome will not render a file:// URL whose path
# has no extension as a document, so a bare mktemp -t file came back as text and
# the runner reported "page did not finish" for a page that was never parsed.
WORK="$(mktemp -d -t bos-page-selftest)"
PAGE="$WORK/page.html"
OUT="$WORK/dump.html"
trap 'rm -rf "$PROFILE" "$WORK"' EXIT

python3 - "$ROOT" "$HERE" "$PAGE" <<'PY'
import sys

root, here, out = sys.argv[1:4]
html = open(f"{root}/ui/index.html", encoding="utf-8").read()
checks = open(f"{here}/page_checks.js", encoding="utf-8").read()
# An unresolvable promise for every bridge call keeps app.js's own boot inert,
# which is the same trick the transcript harness uses.
stub = (
    "<script>window.__TAURI__={core:{invoke:()=>new Promise(()=>{}),"
    "transformCallback:()=>0},event:{listen:()=>new Promise(()=>{})}};</script>"
)
base = f'<base href="file://{root}/ui/">'
# The <base> must be the FIRST child of <head>: URL resolution happens as each
# element is parsed, so a base appended at the end of the head (where this
# runner first put it) never re-resolves the stylesheet link that precedes it —
# style.css 404'd against the temp dir, the page rendered unstyled, and every
# style-dependent check reported phantom failures. The structural checks never
# needed CSS, so the broken base passed unnoticed for rounds.
assert html.count("<head>") == 1, "the shipped page has one head to extend"
html = html.replace("<head>", "<head>" + base, 1)
html = html.replace("</head>", stub + "</head>", 1)
html = html.replace("</body>", f"<script>{checks}</script></body>", 1)
open(out, "w", encoding="utf-8").write(html)
print(f"page_selftest: assembled {len(html)} bytes from the shipped page")
PY

"$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad \
  --disable-breakpad --no-first-run --no-default-browser-check \
  --disable-dev-shm-usage --window-size=1440,900 \
  --user-data-dir="$PROFILE" --allow-file-access-from-files \
  --dump-dom "file://$PAGE" > "$OUT" 2>/dev/null || true
python3 "$HERE/read_result.py" page "$OUT"
