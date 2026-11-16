#!/usr/bin/env bash
# End-to-end validation for the ten runnable BSH extension examples.
# Run on Linux with bubblewrap installed and Python 3 available.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BSH="${BSH_BIN:-$ROOT/target/debug/bsh}"
EXAMPLES=(
  hello
  allinone
  process-echo
  wasm-echo
  extism-echo
  extism-hook
  mcp-echo
  hook-demo
  language-python
  computer-use
)
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if [[ ! -x "$BSH" ]]; then
  echo "BSH binary is missing or not executable: $BSH" >&2
  exit 2
fi
command -v python3 >/dev/null
command -v bwrap >/dev/null

run_ok() {
  local label="$1"
  shift
  echo "==> $label"
  if ! "$@" >"$TMP/stdout" 2>"$TMP/stderr"; then
    cat "$TMP/stdout" >&2 || true
    cat "$TMP/stderr" >&2 || true
    echo "FAILED: $label" >&2
    exit 1
  fi
}

mock_ok() {
  local package="$1" tool="$2" arguments="$3"
  run_ok "mock $package/$tool" "$BSH" ext mock "$ROOT/examples/extensions/$package" "$tool" "$arguments"
}

mock_expected_error() {
  local package="$1" tool="$2" arguments="$3" expected="$4"
  echo "==> mock expected-error $package/$tool"
  if "$BSH" ext mock "$ROOT/examples/extensions/$package" "$tool" "$arguments" >"$TMP/stdout" 2>"$TMP/stderr"; then
    cat "$TMP/stdout"
    echo "Expected ext mock to fail: $package/$tool" >&2
    exit 1
  fi
  if ! grep -Fq "$expected" "$TMP/stderr"; then
    cat "$TMP/stdout" >&2 || true
    cat "$TMP/stderr" >&2 || true
    echo "Expected error text not found: $expected" >&2
    exit 1
  fi
}

# Every example must pass both static manifest validation and contribution-file
# validation before any runtime mock is attempted.
for example in "${EXAMPLES[@]}"; do
  run_ok "validate $example" "$BSH" ext validate "$ROOT/examples/extensions/$example"
  run_ok "test $example" "$BSH" ext test "$ROOT/examples/extensions/$example"
done

# Declarative, process, WASM-component, Extism and MCP runtime smoke tests.
mock_ok hello echo '{"text":"ci"}'

mock_ok allinone describe '{}'
mock_ok allinone echo '{"text":"ci"}'
mock_ok allinone sum '{"a":2,"b":3}'
mock_ok allinone progress '{"steps":1}'
mock_ok allinone events '{"limit":1}'
mock_ok allinone config '{}'
mock_expected_error allinone fail '{"message":"expected-ci-failure"}' 'expected-ci-failure'

mock_ok process-echo echo '{"text":"ci"}'
mock_ok wasm-echo echo '{"text":"ci"}'
mock_ok extism-echo echo '{"text":"ci"}'
mock_ok extism-hook stats '{}'
mock_ok mcp-echo echo '{"text":"ci"}'
mock_ok hook-demo hook-log '{}'
mock_ok language-python diagnose '{"text":"print(1)"}'
mock_ok language-python github-search '{"query":"bsh"}'

# Computer Use is intentionally macOS-only. On Linux, each tool must start
# through the process bridge and return the documented platform refusal before
# any pointer/keyboard/screen side effect is attempted.
computer_use_cases=(
  "screen_size|{}"
  "screenshot|{}"
  "cursor_position|{}"
  "mouse_move|{\"x\":0,\"y\":0}"
  "mouse_click|{\"x\":0,\"y\":0}"
  "mouse_drag|{\"from_x\":0,\"from_y\":0,\"to_x\":1,\"to_y\":1}"
  "scroll|{\"delta_y\":1}"
  "type_text|{\"text\":\"ci\"}"
  "key_press|{\"key\":\"escape\"}"
)
for entry in "${computer_use_cases[@]}"; do
  tool="${entry%%|*}"
  arguments="${entry#*|}"
  echo "==> mock computer-use/$tool (expected macOS-only refusal)"
  "$BSH" ext mock "$ROOT/examples/extensions/computer-use" "$tool" "$arguments" >"$TMP/stdout" 2>"$TMP/stderr" || {
    cat "$TMP/stdout" >&2 || true
    cat "$TMP/stderr" >&2 || true
    exit 1
  }
  if ! grep -Fq "Computer Use currently supports macOS only" "$TMP/stdout"; then
    cat "$TMP/stdout" >&2
    echo "Expected macOS-only refusal was not returned for $tool" >&2
    exit 1
  fi
done

echo "PASS: all ten examples validated and tested; runtime mock coverage completed."
