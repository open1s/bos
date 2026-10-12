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
  # The dump's path must not be rebuilt from $W further down: the guard reassigns
  # W to the width the page *reports* (the drawer checks want the layout width),
  # and at a 420px window the page reports the 500px its stylesheet clamps to — so
  # a later "$P/dom$W.html" names a file that was never written (rev 124).
  DOM="$P/dom$W.html"
  "$CHROME" --headless=new --no-sandbox --disable-gpu --disable-crashpad \
    --no-first-run --no-default-browser-check --disable-dev-shm-usage --hide-scrollbars \
    --user-data-dir="$P/u$W" --allow-file-access-from-files --window-size=$W,900 \
    --dump-dom "file://$PWD/ui/.layout_probe.html" > "$DOM" 2>/dev/null
  line="$(python3 - "$DOM" <<'PY'
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
      f'painted=[{",".join(d.get("painted", []))}] '
      f'drawer={d.get("drawer")} modalClipped=[{",".join(d["modal"]["clipped"])}] scrolledX={d.get("scrolledX")} widest=[{",".join(d.get("widest", []))}] leftmost=[{",".join(d.get("leftmost", []))}]')
PY
)"
  attach="$(python3 - "$DOM" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result-attach"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
if not m:
    print("attach=missing")
    raise SystemExit
d = json.loads(m.group(1))
print("attach=" + json.dumps(d))
PY
)"
  echo "[layout] $line"
  echo "[layout] $attach"
  echo "$line" | grep -q "doc=[0-9]*/[0-9]*" || fail=1
  echo "$line" | grep -q "scrolledX=0" || { echo "[layout]  WIDE: the window can be panned sideways"; fail=1; }
  echo "$line" | grep -q "offscreen=\[\]" || { echo "[layout]  WIDE: elements leave the viewport"; fail=1; }
  echo "$line" | grep -q "modalOutside=\[\]" || { echo "[layout]  WIDE: the settings modal leaves the viewport"; fail=1; }
  # The ☰ is shown by the layout *state*, not a width: at 721-1023px the column is a
  # 56px rail whose chat list is hidden, so the door must be there and must bring the
  # list back. Requiring its absence above 720px is what let that corner ship — the
  # band that needed the door most was the one the guard forbade it in (rev 120).
  W="$(echo "$line" | sed -n 's/.*width=\([0-9]*\).*/\1/p')"
  if [ "$W" -gt 1023 ]; then
    echo "$line" | grep -q "'present': False" || { echo "[layout]  WIDE: the ☰ must not be offered where the column has room to stay open"; fail=1; }
  else
    echo "$line" | grep -Eq "'present': True, 'opened': True, 'mode': 'drawer'" \
      || { echo "[layout]  WIDE: the ☰ must be offered where the column folds, and must open it"; fail=1; }
    echo "$line" | grep -q "'closedList': False" || { echo "[layout]  WIDE: a folded column at rest must not show a chat list it cannot fit"; fail=1; }
    echo "$line" | grep -q "'openList': True" || { echo "[layout]  WIDE: the drawer must bring back the chat list it exists for"; fail=1; }
    echo "$line" | grep -Eq "'openLeft': 0, 'openWidth': 3[0-9][0-9], 'expanded': 'true', 'reclosed': False" \
      || { echo "[layout]  WIDE: the drawer does not open over the centre, or does not close"; fail=1; }
    echo "$line" | grep -q "'armedBefore': False, 'armedOpen': False, 'armedAfter': False" \
      || { echo "[layout]  WIDE: opening the drawer must not arm a rail the window cannot justify"; fail=1; }
    if [ "$W" -le 720 ]; then
      echo "$line" | grep -Eq "'closedLeft': -[0-9]+" || { echo "[layout]  WIDE: a parked drawer must hang off the left edge"; fail=1; }
    else
      echo "$line" | grep -q "'closedLeft': 0" || { echo "[layout]  WIDE: the rail must stay in flow above 720px"; fail=1; }
    fi
  fi
  echo "$line" | grep -q "sidebarSqueezed=False" || { echo "[layout]  WIDE: the chat list is squeezed into an unreadable box"; fail=1; }
  echo "$line" | grep -q "painted=\[\]" || { echo "[layout]  WIDE: something in the UI paints a shadow, a blur or a gradient"; fail=1; }
  # The attach strip is only visible with images staged, which the empty-window
  # measures above never produce — these are its own assertions.
  echo "$attach" | grep -q '"present": true' || { echo "[layout]  WIDE: the attach probe did not run"; fail=1; }
  echo "$attach" | grep -q '"chips": 2' || { echo "[layout]  WIDE: staging two images must draw two chips"; fail=1; }
  echo "$attach" | grep -q '"sendEnabledImagesOnly": true' || { echo "[layout]  WIDE: staged images alone must enable Send"; fail=1; }
  echo "$attach" | grep -q '"overlapsInput": false' || { echo "[layout]  WIDE: the attach strip covers the composer input"; fail=1; }
  echo "$attach" | grep -q '"insideViewport": true' || { echo "[layout]  WIDE: the attach strip leaves the viewport"; fail=1; }
  echo "$attach" | grep -q '"paletteCoversStrip": true' || { echo "[layout]  WIDE: the palette does not cover the strip it shares an anchor with"; fail=1; }
  echo "$attach" | grep -q '"stripBackAfterPalette": true' || { echo "[layout]  WIDE: the attach strip does not come back after the palette closes"; fail=1; }
  echo "$attach" | grep -q '"chipsAfterRemove": 1' || { echo "[layout]  WIDE: the chip's remove control does not remove its own chip"; fail=1; }
  echo "$attach" | grep -q '"hiddenWhenEmpty": true' || { echo "[layout]  WIDE: the empty strip must hide and disable Send again"; fail=1; }
  tok="$(python3 - "$DOM" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result-tokens"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
if not m:
    print("tokens=missing")
    raise SystemExit
print("tokens=" + json.dumps(json.loads(m.group(1))))
PY
)"
  echo "[layout] $tok"
  # §3B.6/§3B.7 — the measured reference numbers. Each one names itself, so a
  # drift says which value moved rather than that "the tokens changed".
  echo "$tok" | grep -q '"accent": "#4176e6"' || { echo "[layout]  §3B: accent is not #4176e6"; fail=1; }
  echo "$tok" | grep -q '"sidebarBg": "#e8e9eb"' || { echo "[layout]  §3B: the sidebar background is not #e8e9eb"; fail=1; }
  echo "$tok" | grep -q '"selected": "#dddee1"' || { echo "[layout]  §3B: a selected row is not #dddee1"; fail=1; }
  echo "$tok" | grep -q '"text": "#0f1115"' || { echo "[layout]  §3B: primary text is not #0f1115"; fail=1; }
  echo "$tok" | grep -q '"dim": "#6b7280"' || { echo "[layout]  §3B: muted text is not #6b7280"; fail=1; }
  echo "$tok" | grep -q '"tokenSidebarW": 348' || { echo "[layout]  §3B: the sidebar token is not 348px"; fail=1; }
  echo "$tok" | grep -q '"rowH": 44' || { echo "[layout]  §3B: a session row is not on a 44px pitch"; fail=1; }
  echo "$tok" | grep -q '"rowFont": 15' || { echo "[layout]  §3B: a session row does not read at 15px"; fail=1; }
  echo "$tok" | grep -q '"sidebarFont": 15' || { echo "[layout]  §3B: the sidebar does not inherit the 15px row type"; fail=1; }
  # W1 — the badge's top is stated as 200px below the tab divider, so the check
  # states that *difference*: a head that changes height moves the badge with it.
  badge="$(python3 - "$DOM" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result-tokens"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
t = json.loads(m.group(1))
out = []
if t.get("badgeW") is None or abs(t["badgeW"] - 265) > 2:
    out.append(f"the working badge is not 265px wide (w={t.get('badgeW')})")
if t.get("badgeRightInset") is None or abs(t["badgeRightInset"] - 27) > 2:
    out.append(f"the working badge is not 27px from the column's right edge (inset={t.get('badgeRightInset')})")
if t.get("badgeTop") is None or t.get("headH") is None or abs((t["badgeTop"] - t["headH"]) - 200) > 2:
    out.append(f"the working badge is not 200px below the tab divider (top={t.get('badgeTop')}, headH={t.get('headH')})")
print("\n".join(out))
PY
)"
  if [ -n "$badge" ]; then echo "$badge" | while IFS= read -r l; do echo "[layout]  §3B: $l"; done; fail=1; fi
  # C1b — the send circle: 42×42, 16 from the composer's right edge, 12 from its
  # bottom. Its numbers are named so a drift says which one moved.
  send="$(python3 - "$DOM" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result-tokens"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
t = json.loads(m.group(1))
out = []
if t.get("sendW") != 42 or t.get("sendH") != 42:
    out.append(f"the send button is not a 42x42 circle (w={t.get('sendW')}, h={t.get('sendH')})")
if t.get("sendRightInset") is None or abs(t["sendRightInset"] - 16) > 2:
    out.append(f"the send button is not 16px from the composer's right edge (inset={t.get('sendRightInset')})")
if t.get("sendBottomInset") is None or abs(t["sendBottomInset"] - 12) > 2:
    out.append(f"the send button is not 12px from the composer's bottom (inset={t.get('sendBottomInset')})")
print("\n".join(out))
PY
)"
  if [ -n "$send" ]; then echo "$send" | while IFS= read -r l; do echo "[layout]  §3B: $l"; done; fail=1; fi
  doc="$(python3 - "$DOM" <<'PY'
import json, re, sys
raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result-doc"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
if not m:
    print("doc=missing")
    raise SystemExit
print("doc=" + json.dumps(json.loads(m.group(1))))
PY
)"
  echo "[layout] $doc"
  echo "$doc" | grep -q '"present": true' || { echo "[layout]  WIDE: the document toggle is not on the page"; fail=1; }
  if echo "$doc" | grep -q '"visible": false'; then
    # The head hides its controls once the window is this narrow; a hidden button
    # is 0-wide at the origin, and that is the layout's choice rather than a bar.
    echo "[layout]  WIDE: the document toggle is hidden at this width (the head's own choice, not a placement bug)"
  else
    echo "$doc" | grep -q '"inRow": true' || { echo "[layout]  WIDE: the document toggle is not inside the head's control row — hung on the column it paints a full-width block under the tabs"; fail=1; }
    # The right edge is information, not a rule: the bug this guards is a control
    # that became a bar under the tabs, and that is inRow/notABar.
    echo "$doc" | grep -q '"atRight": true' || echo "[layout]  WIDE: the document toggle is not on the control row's far right"
    echo "$doc" | grep -q '"notABar": true' || { echo "[layout]  WIDE: the document toggle is as wide as the row — a button has become a bar"; fail=1; }
    echo "$doc" | grep -q '"aboveTabs": true' || { echo "[layout]  WIDE: the document toggle sits below the tabs, not in the head"; fail=1; }
  fi

done
echo "[layout] failures: $fail"
exit $fail
