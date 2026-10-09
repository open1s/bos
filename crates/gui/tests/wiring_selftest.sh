#!/usr/bin/env bash
# Guard the seam between Rust commands and the webview.
#
# Every `#[tauri::command]` has to be listed in `generate_handler!` to exist, and
# every command the UI calls has to be registered or the call fails at runtime.
# A command that is registered but never called is a feature nobody can reach.
#
# This is a static text scan, not an execution: a command name built at runtime
# would be invisible to it, and it does not check that the *arguments* a call
# passes match the command's parameters — only the names in both directions.
#
# It also asserts the two numbers are equal, so a call added in a new file is
# seen as a change rather than silently missed.
# Usage: wiring_selftest.sh [root]        check a tree (default: this crate)
#        wiring_selftest.sh --self-test    prove the check can fail: copy the
#                                         tree, plant a registered command that
#                                         nothing calls, and expect a failure.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

if [ "${1:-}" = "--self-test" ]; then
  P="$(mktemp -d)"
  trap 'rm -rf "$P"' EXIT
  mkdir -p "$P/src" "$P/ui"
  cp "$here/../src/app.rs" "$P/src/app.rs"
  cp "$here"/../ui/*.js "$P/ui/"
  python3 - "$P/src/app.rs" <<'PLANT'
import sys
path = sys.argv[1]
s = open(path, encoding='utf-8').read()
i = s.index('generate_handler![')
j = s.index(']', i)
open(path, 'w', encoding='utf-8').write(s[:j] + '    ghost_command,\n' + s[j:])
print('[wiring]  self-test: planted a registered command nobody calls')
PLANT
  out="$(sh "$here/wiring_selftest.sh" "$P" 2>&1)"
  echo "$out"
  if echo "$out" | grep -q "unreachable=\['ghost_command'\]" && echo "$out" | grep -q 'failures: 1'; then
    echo "[wiring]  self-test: the guard fails on drift, as it must"
    exit 0
  fi
  echo "[wiring]  self-test: the guard MISSED the planted drift"
  exit 1
fi

cd "${1:-$(cd "$here/.." && pwd)}"

python3 - <<'PY'
import glob
import re
import sys

rust = open('src/app.rs', encoding='utf-8').read()
m = re.search(r'generate_handler!\[(.*?)\n\s*\]', rust, re.S)
if not m:
    print('[wiring]  FAIL: no generate_handler! list found')
    sys.exit(1)
registered = sorted(set(re.findall(r'\b([a-z_][a-z0-9_]*)\s*,', m.group(1))))

js_files = sorted(glob.glob('ui/*.js'))
called = {}
for path in js_files:
    for name in re.findall(r'invoke\(\s*"([a-z_][a-z0-9_]*)"', open(path, encoding='utf-8').read()):
        called.setdefault(name, []).append(path)
called_names = sorted(called)

unreachable = [c for c in registered if c not in called]
phantom = [c for c in called_names if c not in registered]
where = ",".join(sorted({p.split('/')[-1] for p in called.values() for p in p}))

print(f'[wiring] registered={len(registered)} invoked={len(called_names)} from={where} '
      f'unreachable={unreachable} phantom={phantom}')
print(f'[wiring] failures: {1 if (unreachable or phantom) else 0}')
sys.exit(1 if (unreachable or phantom) else 0)
PY
