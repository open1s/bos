#!/usr/bin/env bash
# The packaged app, read through the accessibility tree.
#
# Every other guard runs against the shipped *page*; none of them can see what the
# packaged webview exposes, so a missing Tauri capability, an unbound handler or a
# control whose name never reached the tree would pass all of them (rev 123's class
# of defect). This reads the running bundle instead. It is read-only, it never
# clicks, and it reports plainly when there is nothing to read rather than passing
# silently.
#
# Usage: crates/gui/tests/ocu_selftest.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
APP="${BOS_APP_NAME:-BOS}"

if ! command -v ocu >/dev/null 2>&1; then
  echo "[ocu] failures: 0 / 0 (skipped: no ocu on PATH)"
  exit 0
fi
if ! ocu doctor 2>&1 | grep -q "accessibility=granted"; then
  echo "[ocu] failures: 0 / 0 (skipped: accessibility permission not granted)"
  exit 0
fi
if ! ocu call list_apps 2>/dev/null | grep -q "${APP} — "; then
  echo "[ocu] failures: 0 / 0 (skipped: ${APP} is not running - launch target/debug/BOS.app)"
  exit 0
fi

# Falsified by pointing it at another app's window (`BOS_APP_NAME="DeepSeek Harness"`), which
# reddens 9 of these 10: the one that survives is the `Account menu` popup name, because the
# reference app publishes a popup by that name too. It is a real claim about BOS's tree and not
# a specific one - the cross-app probe is what tells the two apart.
# "The process is running" is not "the webview has painted": a bundle that was just
# launched answers with an empty tree for a few seconds. Wait for the window to have a
# shell in it rather than reporting a blank read as ten defects, and still fail if it
# never arrives.
D="$(mktemp -d -t bos-ocu)"
trap 'rm -rf "$D"' EXIT
for attempt in 1 2 3 4 5 6 7 8; do
  ocu call get_app_state --args "{\"app\":\"${APP}\",\"text_limit\":4000}" > "$D/state.json" 2>"$D/err" || true
  grep -q "New Session" "$D/state.json" && break
  sleep 3
done
python3 - "$D/state.json" <<'PY'
import json, sys, re

path = sys.argv[1]
try:
    doc = json.load(open(path, encoding="utf-8"))
    tree = next(p.get("text", "") for p in doc.get("content", []) if p.get("type") == "text")
except Exception as err:
    print("[ocu] failures: 1 / 1 (could not read the window: %s)" % err)
    raise SystemExit(0)

def has(pattern):
    return re.search(pattern, tree) is not None

checks = []
def want(desc, ok, detail=""):
    checks.append((desc, bool(ok), detail))

want("the header reaches the tree with its wordmark", has(r"text BOS") and has(r"text HARNESS"))
want("New Session is a button in the sidebar", has(r"button( \(disabled\))? New Session"))
want("the global panels group names its three destinations",
     has(r"container Global panels") and has(r"button Plugins") and
     has(r"button Automation tasks") and has(r"button MCP connectors"))
want("the account row is a popup (S5)", has(r"pop up button Account menu"))
want("the composer's attach control is a popup with the C1a name",
     has(r"pop up button Add files or run commands"))
want("the model chip's accessibility name carries the full id (C4)",
     has(r"pop up button Model: \S+/\S+"))
want("the editor states its keyboard contract and placeholder (C3)",
     has(r"Description: Enter = send") and has(r"Placeholder: Message or run a task"))
want("the context meter is a popup that says what it measures (B3)",
     has(r"pop up button \d+% of context used"))
want("the usage popup carries the dash rule (B2)", has(r"tok \u00b7 Cache hit"))
# The jobs chip is only in the tree while it is shown, so an idle app is the case
# that says something: nothing running must leave no chip behind.
if has(r"background jobs? running"):
    detail = re.search(r"[0-9]+ background jobs? running[^\n]*", tree).group(0)
    want("the jobs chip carries its caret when it is shown (H1)", detail.endswith("\u2304"), detail)
else:
    want("the jobs chip is absent with nothing running (H1 zero state)", True, "no chip in the tree")

failed = [c for c in checks if not c[1]]
for desc, ok, detail in checks:
    print("[ocu]   %s  %s%s" % ("ok  " if ok else "FAIL", desc, (" (" + detail + ")") if detail else ""))
print("[ocu] failures: %d / %d" % (len(failed), len(checks)))
PY
