#!/usr/bin/env bash
# End-to-end verification: several BSH federation nodes on ONE host.
#
# Covers the profile data plane (identity, peer store, locks), pairing, the
# live roster, two separate processes finding each other over TCP, the same
# profile refused twice, and several nodes hosted by one process.
#
#   cargo build -p bsh --bin bsh
#   BIN=target/debug/bsh scripts/verify-federation-local.sh
#
# Exits non-zero if any check fails. Uses a scratch BSH_HOME, so the reader's
# own nodes are never touched.

set -uo pipefail

BIN=${BIN:-target/debug/bsh}
ROOT=$(mktemp -d "${TMPDIR:-/tmp}/bsh-fed-verify.XXXXXX") || exit 1
export BSH_HOME="$ROOT"
PIDS=()

cleanup() {
    for pid in ${PIDS[@]:-}; do
        [ -n "$pid" ] && kill "$pid" 2>/dev/null
    done
    rm -rf "$ROOT"
}
trap cleanup EXIT

pass=0
fail=0
ok()  { echo "PASS  $1"; pass=$((pass + 1)); }
bad() { echo "FAIL  $1"; fail=$((fail + 1)); }

wait_for() { # seconds, command...
    local deadline=$((SECONDS + $1))
    shift
    until eval "$*"; do
        [ $SECONDS -ge $deadline ] && return 1
        sleep 1
    done
    return 0
}

echo "== profiles and pairing =="
if "$BIN" federation status >/dev/null 2>&1; then ok "default profile created"; else bad "default profile created"; fi
if "$BIN" federation --profile work status >/dev/null 2>&1; then ok "work profile created"; else bad "work profile created"; fi

paired=$("$BIN" federation pair work --grant builtin/read_file 2>&1)
if grep -q "^paired " <<<"$paired"; then ok "pair enrolls both profiles"; else bad "pair enrolls both profiles: $paired"; fi
if grep -q "grant: work may invoke builtin/read_file" <<<"$paired"; then ok "grant is directional"; else bad "grant is directional: $paired"; fi
if "$BIN" federation status | grep -q "peers 1"; then ok "default store trusts work"; else bad "default store trusts work"; fi
if "$BIN" federation --profile work status | grep -q "peers 1"; then ok "work store trusts default"; else bad "work store trusts default"; fi

echo "== two processes, two profiles =="
"$BIN" federation serve >"$ROOT/serve-default.log" 2>&1 &
PIDS+=($!)
sleep 2
"$BIN" federation serve --profile work >"$ROOT/serve-work.log" 2>&1 &
PIDS+=($!)

if wait_for 30 "grep -q 'peers=[1-9]' '$ROOT/serve-default.log' && grep -q 'peers=[1-9]' '$ROOT/serve-work.log'"; then
    ok "both processes discover a peer over TCP"
else
    bad "both processes discover a peer over TCP"
    echo "--- default ---"; cat "$ROOT/serve-default.log"
    echo "--- work ---"; cat "$ROOT/serve-work.log"
fi

roster=$("$BIN" federation profiles 2>&1)
if grep -qE '^default[[:space:]]+running' <<<"$roster"; then ok "roster: default running"; else bad "roster: default running: $roster"; fi
if grep -qE '^work[[:space:]]+running' <<<"$roster"; then ok "roster: work running"; else bad "roster: work running: $roster"; fi
if grep -q "profiles 2" <<<"$roster"; then ok "roster counts both profiles"; else bad "roster counts both profiles: $roster"; fi

if "$BIN" federation serve --profile work >/dev/null 2>&1; then
    bad "a second process on a running profile is refused"
else
    ok "a second process on a running profile is refused"
fi

kill "${PIDS[0]}" "${PIDS[1]}" 2>/dev/null
wait "${PIDS[0]}" "${PIDS[1]}" 2>/dev/null
PIDS=()

echo "== several nodes in one process =="
"$BIN" federation serve --profile a --profile b >"$ROOT/serve-ab.log" 2>&1 &
PIDS+=($!)
if wait_for 20 "grep -q '^\[a\]' '$ROOT/serve-ab.log' && grep -q '^\[b\]' '$ROOT/serve-ab.log'"; then
    ok "one process runs two nodes"
else
    bad "one process runs two nodes"
    cat "$ROOT/serve-ab.log"
fi
roster=$("$BIN" federation profiles 2>&1)
if grep -qE '^a[[:space:]]+running' <<<"$roster" && grep -qE '^b[[:space:]]+running' <<<"$roster"; then
    ok "roster: a and b running"
else
    bad "roster: a and b running: $roster"
fi
kill "${PIDS[0]}" 2>/dev/null
wait "${PIDS[0]}" 2>/dev/null
PIDS=()

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
