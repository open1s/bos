#!/usr/bin/env bash
# Narrow-window guard. There is no screenshot baseline here on purpose: the
# question this answers is numeric (does anything leave the viewport?) and a
# number is checkable by anyone, including a reviewer without eyes on the pixels.
set -u
cd "$(dirname "$0")/.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if [ ! -x "$CHROME" ]; then echo "[layout] chrome not found at $CHROME"; exit 2; fi
WIDTHS="${*:-1440 900 560 420}"
P="$(mktemp -d)"
trap 'rm -rf "$P" ui/.layout_probe.html' EXIT
python3 - "$PWD/ui/index.html" "$PWD/ui/.layout_probe.html" "$PWD/tests/layout_probe.js" <<'PY'
import sys
src, dst, probe = sys.argv[1], sys.argv[2], sys.argv[3]
html = open(src, encoding="utf-8").read()
shim = ('<script>window.__TAURI__ = { core: { invoke: () => new Promise(() => {}) },'
        ' event: { listen: () => () => {} } };</script>\n')
# The shim goes before app.js: without it the first Tauri call throws and the
# wiring below it (the drawer handler among it) never runs, so a probe that skips
# this measures a page whose buttons do nothing.
marker = "<script src=\"app.js\"></script>"
assert marker in html, "app.js tag"
html = html.replace(marker, shim + marker, 1)
tag = "<script src=\"../tests/layout_probe.js\"></script>"
assert "</body>" in html
open(dst, "w", encoding="utf-8").write(html.replace("</body>", tag + "\n</body>", 1))
PY
fail=0
for W in $WIDTHS; do
  "$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad --disable-breakpad \
    --no-first-run --no-default-browser-check --disable-dev-shm-usage --hide-scrollbars \
    --user-data-dir="$P/u$W" --allow-file-access-from-files --window-size=$W,900 \
    --dump-dom "file://$PWD/ui/.layout_probe.html" > "$P/dom$W.html" 2>/dev/null
  line="$(python3 - "$P/dom$W.html" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
if not m:
    print("no result element — the page did not finish")
    raise SystemExit
d = json.loads(m.group(1))
b = d["boxes"]
def box(k):
    v = b.get(k)
    return "none" if not v else f"{v[0]}..{v[0]+v[2]}({v[2]}x{v[3]})"
print(f'width={d["viewport"]} doc={d["docClientW"]}/{d["docScrollW"]} '
      f'sidebar={box("sidebar")} main={box("main")} composer={box("composer")} '
      f'statusbar={box("statusbar")} offscreen=[{",".join(d["offscreen"])}] '
      f'sidebarSqueezed={d.get("sidebarSqueezed")} rows={d.get("sidebarRows")} '
      f'modalCard={d["modal"]["card"]} modalOutside=[{",".join(d["modal"]["outside"])}] '
      f'drawer={d.get("drawer")} modalClipped=[{",".join(d["modal"]["clipped"])}] scrolledX={d.get("scrolledX")} widest=[{",".join(d.get("widest", []))}] leftmost=[{",".join(d.get("leftmost", []))}]')
PY
)"
  echo "[layout] $line"
  echo "$line" | grep -q "doc=[0-9]*/[0-9]*" || fail=1
  echo "$line" | grep -q "scrolledX=0" || { echo "[layout]  WIDE: the window can be panned sideways"; fail=1; }
  echo "$line" | grep -q "offscreen=\[\]" || { echo "[layout]  WIDE: elements leave the viewport"; fail=1; }
  echo "$line" | grep -q "modalOutside=\[\]" || { echo "[layout]  WIDE: the settings modal leaves the viewport"; fail=1; }
  if echo "$line" | grep -q "'present': True"; then
    echo "$line" | grep -Eq "'opened': True, 'closedLeft': -[0-9]+, 'openLeft': 0, 'openWidth': 2[0-9][0-9], 'expanded': 'true', 'reclosed': False" \
      || { echo "[layout]  WIDE: the drawer does not open, or does not close"; fail=1; }
  fi
  if [ "$(echo "$line" | sed -n 's/.*width=\([0-9]*\).*/\1/p')" -gt 720 ]; then
    echo "$line" | grep -q "'present': False" || { echo "[layout]  WIDE: the drawer shows on a wide window"; fail=1; }
  fi
  echo "$line" | grep -q "sidebarSqueezed=False" || { echo "[layout]  WIDE: the chat list is squeezed into an unreadable box"; fail=1; }
done
echo "[layout] failures: $fail"
exit $fail
