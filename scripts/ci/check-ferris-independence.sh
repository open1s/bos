#!/usr/bin/env bash
# The SDK is independent: no BSH, agent or GPUI in the normal dependency graph.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

for crate in ferris-core ferris-runtime ferris-sdk; do
  deps="$(cargo tree -p "$crate" -e normal --prefix none | awk '{print $1}' | sort -u)"
  for forbidden in bsh agent gpui gpui-kit react; do
    if grep -qx "$forbidden" <<<"$deps"; then
      echo "independence failure: $crate depends on $forbidden" >&2
      exit 1
    fi
  done
done

cargo run -q -p ferris-smoke
echo "ferris independence: ok"
