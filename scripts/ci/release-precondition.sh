#!/usr/bin/env bash
# Decide whether this run may release, from the tag and the manifests.
#
# A release happens only when the ref is a vMAJOR.MINOR.PATCH tag and every
# publishable manifest carries that same version. When either check fails the
# run is skipped - with a warning that names the reason - instead of publishing
# a partial or mismatched set.
#
# Inputs (environment):
#   REF_TYPE       github.ref_type (tag | branch | ...)
#   REF_NAME       github.ref_name
#   EVENT_NAME     github.event_name (push | pull_request | workflow_dispatch)
#   INPUT_VERSION  optional version for a manual dispatch
#
# Outputs on $GITHUB_OUTPUT, and on stdout when unset:
#   should_release=true|false
#   version=<the version, when releasing>
#   reason=<why not, when skipping>

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ref_type="${REF_TYPE:-branch}"
ref_name="${REF_NAME:-}"
event="${EVENT_NAME:-local}"
input_version="${INPUT_VERSION:-}"

cargo_version="$(
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$repo_root/Cargo.toml" \
    | sed -n 's/^version = "\([^"]*\)"$/\1/p' \
    | head -n 1
)"

should_release=false
version=""
reason=""

if [[ "$ref_type" == "tag" ]]; then
  candidate="${ref_name#v}"
  if [[ ! "$ref_name" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
    reason="tag '$ref_name' is not vMAJOR.MINOR.PATCH"
  elif ! detail="$(bash "$repo_root/scripts/ci/check-release-versions.sh" "$candidate" 2>&1)"; then
    reason="the tag does not match the manifests: $detail"
  else
    should_release=true
    version="$candidate"
  fi
elif [[ "$event" == "workflow_dispatch" ]]; then
  candidate="${input_version:-$cargo_version}"
  if ! detail="$(bash "$repo_root/scripts/ci/check-release-versions.sh" "$candidate" 2>&1)"; then
    reason="the requested version does not match the manifests: $detail"
  else
    should_release=true
    version="$candidate"
  fi
else
  reason="not a release tag"
fi

if [[ "$should_release" != true ]]; then
  printf '::warning::skipping the release: %s\n' "$reason"
fi

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    printf 'should_release=%s\n' "$should_release"
    printf 'version=%s\n' "$version"
    printf 'reason=%s\n' "$reason"
  } >> "$GITHUB_OUTPUT"
else
  printf 'should_release=%s\nversion=%s\nreason=%s\n' \
    "$should_release" "$version" "$reason"
fi
