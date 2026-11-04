#!/usr/bin/env bash
# Set one release version across every carrier a vX.Y.Z tag releases.
#
# The BSH installers, the npm package (@open1s/jsbos) and the PyPI package
# (nbos) are published by three pipelines that each read their own manifest.
# Bumping one by hand is how a tag ends up releasing only part of the set, so
# this script writes the version everywhere and then asserts agreement.
# It only edits files: commit and tag deliberately, as a release.
#
# Usage:
#   bump-release-version.sh 1.1.1

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version="${1:?usage: bump-release-version.sh <x.y.z>}"

die() {
  printf 'bump-release-version: error: %s\n' "$*" >&2
  exit 1
}

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] \
  || die "'$version' is not a SemVer version"

export VERSION="$version"

cargo_toml="$repo_root/Cargo.toml"
jsbos_json="$repo_root/crates/jsbos/package.json"
jsbos_js="$repo_root/crates/jsbos/jsbos.js"
jsbos_readme="$repo_root/crates/jsbos/README.md"
nbos_pyproject="$repo_root/crates/nbos/pyproject.toml"
nbos_readme="$repo_root/crates/nbos/README.md"

# The cargo workspace version lives at the top of its own table.
awk -v v="$version" '
  /^\[workspace\.package\]$/ { in_block = 1 }
  in_block && !done && /^version = / {
    print "version = \"" v "\""
    done = 1
    next
  }
  { print }
' "$cargo_toml" > "$cargo_toml.tmp" && mv "$cargo_toml.tmp" "$cargo_toml"

# The package's own version is the first top-level one.
perl -pi -e 'if (!$done && s/^  "version": "[^"]*",/  "version": "$ENV{VERSION}",/) { $done = 1 }' "$jsbos_json"

# Same for the PEP 621 project table.
awk -v v="$version" '
  BEGIN { done = 0 }
  /^version = / && !done {
    print "version = \"" v "\""
    done = 1
    next
  }
  { print }
' "$nbos_pyproject" > "$nbos_pyproject.tmp" && mv "$nbos_pyproject.tmp" "$nbos_pyproject"

# The generated napi loader compares each optional platform package against
# the version it was generated with.
perl -pi -e "s/!== '[0-9][^']*'/!== '\$ENV{VERSION}'/g; s/expected [0-9][^ ]* but/expected \$ENV{VERSION} but/g" "$jsbos_js"

perl -pi -e "s/^(# @open1s\/jsbos — v).*/\1\$ENV{VERSION}/" "$jsbos_readme"
perl -pi -e "s/^(# nbos — v).*/\1\$ENV{VERSION}/" "$nbos_readme"

# The lock records every workspace member's version; cargo is the authority.
if command -v cargo >/dev/null 2>&1; then
  if ! cargo metadata --offline --format-version 1 >/dev/null 2>&1; then
    printf 'bump-release-version: warning: cargo could not refresh Cargo.lock offline; run cargo metadata yourself\n' >&2
  fi
else
  printf 'bump-release-version: warning: cargo not on PATH; Cargo.lock is stale\n' >&2
fi

bash "$repo_root/scripts/ci/check-release-versions.sh" "$version"

cat <<EOF

bump-release-version: wrote $version. Next, commit and tag it:

  jj describe -m "release: BSH $version"
  jj new
  jj tag set v$version -r @-
  jj git push && jj git push --tag v$version

The tag starts all three pipelines: BSH installers, npm @open1s/jsbos and
PyPI nbos.
EOF
