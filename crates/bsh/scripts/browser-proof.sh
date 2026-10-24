#!/usr/bin/env bash
# Re-run the Browser pane's pixel proof in ONE command.
#
# What this produces, in target/:
#   bsh-obscura-proof.png   the ENGINE's own capture of a styled page, asserted
#                           by the engine test (non-blank, blue heading, red box)
#   bsh-browser-window.png  a screenshot of the REAL GPUI window taken while the
#                           Browser pane is showing that page
#
# What it does NOT do: assert pixels on the GPUI window. The harness cannot
# photograph its own surface - the capture above is evidence for a human to
# inspect, while the automated assertions stay on the engine image and the live
# DOM checks in the engine test. See docs/bsh-browser-substrate.md.
#
# Requirements: macOS (ocu captures through Accessibility), python3, and the
# Obscura binary either on PATH or in BSH_OBSCURA_BIN.
#
# Usage:
#   crates/bsh/scripts/browser-proof.sh
#   BSH_PROOF_SKIP_ENGINE_TEST=1 crates/bsh/scripts/browser-proof.sh   # window only
#   BSH_PROOF_URL=https://www.baidu.com/ crates/bsh/scripts/browser-proof.sh
#
# Environment:
#   BSH_OBSCURA_BIN            the engine binary (default: `command -v obscura`)
#   BSH_OBSCURA_FONT_DIRS      forwarded, for the CJK glyphs
#   BSH_PROOF_SKIP_ENGINE_TEST 1 to skip the engine capture/assertions
#   BSH_PROOF_SECONDS          seconds to wait for the window to settle (6)
#   BSH_PROOF_URL              capture THIS url instead of the local fixture,
#                              e.g. BSH_PROOF_URL=https://www.baidu.com/

set -euo pipefail

# crates/bsh/scripts -> the checkout root is three levels up.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BIN="$ROOT/target/debug/bsh"
TARGET="$ROOT/target"
FIXTURE="$TARGET/bsh-browser-proof-fixture"
HOME_DIR="$TARGET/bsh-browser-proof-home"
WINDOW_PNG="$TARGET/bsh-browser-window.png"
ENGINE_PNG="$TARGET/bsh-obscura-proof.png"
LOG="$TARGET/bsh-browser-proof.log"
SETTLE="${BSH_PROOF_SECONDS:-6}"

fail() { echo "browser-proof: $*" >&2; exit 1; }

command -v python3 >/dev/null || fail "python3 is required"
command -v ocu >/dev/null || fail "ocu (open-computer-use) is required for the window capture"

if [ -z "${BSH_OBSCURA_BIN:-}" ]; then
  BSH_OBSCURA_BIN="$(command -v obscura || true)"
fi
[ -n "${BSH_OBSCURA_BIN:-}" ] && [ -x "$BSH_OBSCURA_BIN" ] \
  || fail "no Obscura binary: set BSH_OBSCURA_BIN or put obscura on PATH"

echo "browser-proof: building the shell"
(cd "$ROOT" && cargo build -p bsh)

# The fixture is the same page the engine test asserts: a blue #4176e6 heading,
# a 200x80 red box, text a page SCRIPT writes, Chinese glyphs, a link, a button
# and an input. Remade every run so the file on disk is the file captured.
# The fixture is remade; the scratch HOME deliberately is NOT wiped, so an
# engine a previous (crashed) proof left behind is reaped by this run the same
# way a real next launch reaps it.
rm -rf "$FIXTURE"
mkdir -p "$FIXTURE" "$HOME_DIR"
cat > "$FIXTURE/index.html" <<'HTML'
<!doctype html><html><head><meta charset="utf-8"><title>Browser proof</title>
<style>
 body{font-family:-apple-system,Helvetica,Arial;margin:0;padding:24px}
 h1{color:#4176e6;font-size:40px;margin:0 0 12px}
 .box{width:200px;height:80px;background:#e04141}
 p{font-size:18px}
</style></head><body>
<h1>Browser proof page</h1>
<div class="box"></div>
<p id="out">script did not run</p>
<p>Chinese: 你好，世界！这是中文渲染测试。</p>
<a href="/next">a link</a> <button>Press me</button> <input placeholder="type here">
<script>document.getElementById('out').textContent='JS wrote this: 1+2='+(1+2);</script>
</body></html>
HTML

if [ -n "${BSH_PROOF_URL:-}" ]; then
  URL="$BSH_PROOF_URL"
  echo "browser-proof: capturing a real URL, fixture server skipped: $URL"
else
  PORT="$(python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
  URL="http://127.0.0.1:$PORT/"
fi

BIN_PID=""
SERVER_PID=""
cleanup() {
  [ -n "$BIN_PID" ] && kill "$BIN_PID" 2>/dev/null || true
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  # SIGTERM does not run the shell's Drop, so its engine is now an orphan.
  # Stop it by the pidfile THIS scratch HOME wrote, after checking its own argv
  # carries that marker: an unrelated Obscura can never match.
  local state="$HOME_DIR/.bos/bsh/browser/engines"
  if [ -d "$state" ]; then
    for file in "$state"/*.pid; do
      [ -e "$file" ] || continue
      local pid marker
      pid="$(sed -n '1p' "$file" | tr -d '[:space:]')"
      marker="$(sed -n '2p' "$file" | tr -d '[:space:]')"
      [ -n "$pid" ] && [ -n "$marker" ] || continue
      if ps -o command= -p "$pid" 2>/dev/null | grep -q -- "$marker"; then
        kill -9 "$pid" 2>/dev/null || true
      fi
    done
  fi
  wait 2>/dev/null || true
}
trap cleanup EXIT

if [ -z "${BSH_PROOF_URL:-}" ]; then
  # Start the fixture server.
  python3 - "$FIXTURE" "$PORT" <<'PY' >/dev/null 2>&1 &
import sys, functools, http.server
fixture, port = sys.argv[1], int(sys.argv[2])
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=fixture)
http.server.ThreadingHTTPServer(("127.0.0.1", port), handler).serve_forever()
PY
  SERVER_PID=$!
  # Keep the shell from printing a job-control "Terminated" line when cleanup
  # stops these; the pids above are still ours to kill.
  disown "$SERVER_PID" 2>/dev/null || true

  # Wait for the fixture.
  for _ in $(seq 1 50); do
    curl -sf "$URL" >/dev/null 2>&1 && break
    sleep 0.1
  done
  curl -sf "$URL" >/dev/null 2>&1 || fail "the fixture server did not answer at $URL"
fi

# Only instances of THIS build are cleared: the pattern is the absolute path.
pkill -f "$BIN" 2>/dev/null || true
sleep 1

echo "browser-proof: opening $URL"
# HOME is overridden for THIS process only: the scratch settings must not be
# inherited by the cargo call at the end (that would move CARGO_HOME).
if [ -n "${BSH_OBSCURA_FONT_DIRS:-}" ]; then
  env HOME="$HOME_DIR" BSH_OBSCURA_BIN="$BSH_OBSCURA_BIN" \
    BSH_OBSCURA_FONT_DIRS="$BSH_OBSCURA_FONT_DIRS" \
    "$BIN" --browser="$URL" >"$LOG" 2>&1 &
else
  env HOME="$HOME_DIR" BSH_OBSCURA_BIN="$BSH_OBSCURA_BIN" \
    "$BIN" --browser="$URL" >"$LOG" 2>&1 &
fi
BIN_PID=$!
disown "$BIN_PID" 2>/dev/null || true

# The engine's pidfile is the signal that the pre-warm actually launched the
# child; the pane's first frame follows it by a few hundred milliseconds.
STATE="$HOME_DIR/.bos/bsh/browser/engines"
for _ in $(seq 1 100); do
  if ls "$STATE"/*.pid >/dev/null 2>&1; then break; fi
  kill -0 "$BIN_PID" 2>/dev/null || fail "bsh exited early; see $LOG"
  sleep 0.1
done
ls "$STATE"/*.pid >/dev/null 2>&1 || fail "the browser engine never started; see $LOG"
sleep "$SETTLE"

echo "browser-proof: capturing the window"
ocu call get_app_state --args '{"app":"bsh","text_limit":200}' > "$TARGET/bsh-browser-ocu.json" 2>"$TARGET/bsh-browser-ocu.err" \
  || fail "ocu could not capture the window (see $TARGET/bsh-browser-ocu.err)"
python3 - "$TARGET/bsh-browser-ocu.json" "$WINDOW_PNG" <<'PY'
import base64, json, sys
payload, out = sys.argv[1], sys.argv[2]
data = json.load(open(payload))
for item in data.get("content", []):
    blob = item.get("data")
    if item.get("type") == "image" or (isinstance(blob, str) and blob.startswith("iVBOR")):
        open(out, "wb").write(base64.b64decode(blob))
        print(f"browser-proof: window image -> {out}")
        break
else:
    raise SystemExit("browser-proof: ocu returned no image payload")
PY

if [ "${BSH_PROOF_SKIP_ENGINE_TEST:-0}" != "1" ]; then
  echo "browser-proof: capturing and asserting the engine image"
  PATH="$(dirname "$BSH_OBSCURA_BIN"):$PATH" \
    cargo test -p bsh --lib a_live_page_answers_with_script_text -- --nocapture
fi

echo
echo "browser-proof: done"
echo "  window evidence: $WINDOW_PNG  (inspect; no pixel assertion here)"
[ "${BSH_PROOF_SKIP_ENGINE_TEST:-0}" != "1" ] && echo "  engine proof:    $ENGINE_PNG  (asserted by the test above)"
echo "  engine log:      $LOG"
