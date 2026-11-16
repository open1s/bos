#!/usr/bin/env bash
# Resolve the version a release build carries, and whether it may publish.
#
# The tag is the source of truth: a `v3.0.3` tag builds 3.0.3. The workspace
# Cargo.toml must say the same thing, because the binary reports
# `CARGO_PKG_VERSION` in the About pane - a package whose number disagrees with
# the tag is a support problem, not a cosmetic one. A manual dispatch may pass
# an expected version instead, but it is checked against the manifest the same
# way, so the two can never drift.
#
# Inputs (environment):
#   EVENT_NAME     push | workflow_dispatch (anything else is a local dry run)
#   TAG            the ref name of a tag push, e.g. v3.0.3
#   INPUT_VERSION  optional expected version for a manual dispatch
#   INPUT_PUBLISH  "true" to publish, "false" (default) to build only
#
# Outputs: version=, publish=, prerelease= on $GITHUB_OUTPUT when set,
# otherwise on stdout so the script is usable by hand.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die() {
  printf 'resolve-release-version: error: %s\n' "$*" >&2
  exit 1
}

cargo_version="$(
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$repo_root/Cargo.toml" \
    | sed -n 's/^version = "\([^"]*\)"$/\1/p' \
    | head -n 1
)"
[[ -n "$cargo_version" ]] || die "no workspace.package.version found in Cargo.toml"

event="${EVENT_NAME:-local}"
tag="${TAG:-}"
input_version="${INPUT_VERSION:-}"
publish="${INPUT_PUBLISH:-false}"

if [[ "$event" == "push" ]]; then
  [[ "$tag" == v* ]] || die "tag '$tag' does not start with 'v'"
  version="${tag#v}"
  # A tag push is an explicit request to publish; that is what tags are for.
  publish="true"
else
  version="${input_version:-$cargo_version}"
fi

# SemVer 2.0: MAJOR.MINOR.PATCH with an optional -prerelease and/or +build.
# The stricter check is deliberate: it keeps the number usable in an MSI
# ProductVersion, a Debian Version and an Apple CFBundleShortVersionString.
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  die "'$version' is not a SemVer version (expected MAJOR.MINOR.PATCH[-prerelease][+build])"
fi

if [[ "$version" != "$cargo_version" ]]; then
  die "release version $version does not match workspace version $cargo_version.
Bump [workspace.package] version in Cargo.toml (and the changelog) before tagging."
fi

prerelease=false
[[ "$version" == *-* ]] && prerelease=true

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    printf 'version=%s\n' "$version"
    printf 'publish=%s\n' "$publish"
    printf 'prerelease=%s\n' "$prerelease"
  } >> "$GITHUB_OUTPUT"
else
  printf 'version=%s\n' "$version"
  printf 'publish=%s\n' "$publish"
  printf 'prerelease=%s\n' "$prerelease"
fi
