#!/usr/bin/env bash
# Assert every publishable manifest carries one version.
#
# One `vX.Y.Z` tag is meant to release three artifact sets: the BSH
# installers, the npm package (@open1s/jsbos) and the PyPI package (nbos).
# Each pipeline reads its own manifest, so a tag whose number disagrees with
# one manifest would silently publish nothing - or an older version over
# `latest`. This guard fails that push instead.
#
# Usage:
#   check-release-versions.sh [expected-version]

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
expected="${1:-}"

cargo_version="$(
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$repo_root/Cargo.toml" \
    | sed -n 's/^version = "\([^"]*\)"$/\1/p' \
    | head -n 1
)"
npm_version="$(
  sed -n 's/^[[:space:]]*"version": "\([^"]*\)",\{0,1\}$/\1/p' \
    "$repo_root/crates/jsbos/package.json" | head -n 1
)"
pypi_version="$(
  sed -n 's/^version = "\([^"]*\)"$/\1/p' \
    "$repo_root/crates/nbos/pyproject.toml" | head -n 1
)"

fail=0
for entry in \
  "Cargo.toml:$cargo_version" \
  "crates/jsbos/package.json:$npm_version" \
  "crates/nbos/pyproject.toml:$pypi_version"; do
  file="${entry%%:*}"
  value="${entry#*:}"
  if [[ -z "$value" ]]; then
    printf 'check-release-versions: error: no version in %s\n' "$file" >&2
    fail=1
  fi
done

if [[ "$fail" -eq 0 ]]; then
  if [[ "$cargo_version" != "$npm_version" || "$cargo_version" != "$pypi_version" ]]; then
    printf 'check-release-versions: error: manifests disagree: Cargo %s, jsbos %s, nbos %s\n' \
      "$cargo_version" "$npm_version" "$pypi_version" >&2
    fail=1
  fi
fi

if [[ -n "$expected" && "$fail" -eq 0 && "$cargo_version" != "$expected" ]]; then
  printf 'check-release-versions: error: tag v%s does not match the manifests (%s)\n' \
    "$expected" "$cargo_version" >&2
  fail=1
fi

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
printf 'check-release-versions: %s across Cargo, jsbos and nbos\n' "$cargo_version"
