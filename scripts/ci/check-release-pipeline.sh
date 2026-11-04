#!/usr/bin/env bash
# Structural checks for the release pipeline.
#
# The workspace workflow once stopped parsing entirely and nobody noticed,
# because a broken workflow is silent until the day it is needed. This runs on
# every change (see .github/workflows/rust.yml) and checks the cheap things:
# the YAML parses, every shell packager is syntactically valid, the assets the
# packagers read exist, and the release job still expects the artifact names the
# packagers emit.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
status=0

fail() {
  printf 'check-release-pipeline: error: %s\n' "$*" >&2
  status=1
}

note() {
  printf 'check-release-pipeline: %s\n' "$*"
}

# --- Shell syntax -----------------------------------------------------------
for script in "$repo_root"/scripts/ci/*.sh; do
  bash -n "$script" || fail "shell syntax: ${script#"$repo_root"/}"
done

# --- Workflow YAML parses ---------------------------------------------------
if python3 -c 'import yaml' >/dev/null 2>&1; then
  if ! python3 - "$repo_root" <<'PY'
import glob
import sys

import yaml

root = sys.argv[1]
failed = False
for path in sorted(glob.glob(root + "/.github/workflows/*.yml")):
    try:
        yaml.safe_load(open(path))
    except Exception as error:  # noqa: BLE001 - report the parser's own message
        print(f"check-release-pipeline: error: {path}: {error}", file=sys.stderr)
        failed = True
raise SystemExit(1 if failed else 0)
PY
  then
    fail "a workflow file does not parse"
  fi
elif command -v ruby >/dev/null 2>&1; then
  ruby -ryaml -e 'ARGV.each { |path| YAML.load_file(path) }' \
    "$repo_root"/.github/workflows/*.yml \
    || fail "a workflow file does not parse"
else
  note "warning: neither PyYAML nor Ruby is available; skipped the YAML parse"
fi

# --- Inputs the packagers read ---------------------------------------------
for asset in \
  "crates/bsh/src/assets/bos-harness-icon.icns" \
  "crates/bsh/src/assets/bos-harness-icon.ico" \
  "crates/bsh/src/assets/bos-harness-icon.png" \
  "installer/bsh.wxs" \
  "installer/bsh.entitlements" \
  "docs/bsh-release.md"; do
  [[ -f "$repo_root/$asset" ]] || fail "missing release asset: $asset"
done

# --- Release job expects what the packagers emit ---------------------------
release="$repo_root/.github/workflows/release.yml"
for name in \
  'bsh-${VERSION}-macos-arm64.dmg' \
  'bsh-${VERSION}-macos-x86_64.dmg' \
  'bsh-${VERSION}-macos-arm64.app.zip' \
  'bsh-${VERSION}-linux-x86_64.tar.gz' \
  'bsh_${deb_version}_amd64.deb' \
  'bsh-${VERSION}-linux-x86_64.rpm' \
  'bsh-${VERSION}-windows-x64.zip' \
  'bsh-${VERSION}-windows-x64.msi'; do
  grep -Fq "\"$name\"" "$release" || fail "release job no longer verifies $name"
done

# --- The scripts the workflow calls exist ----------------------------------
for script in \
  resolve-release-version.sh \
  package-macos.sh \
  package-linux.sh \
  package-windows.ps1 \
  release-notes.sh; do
  [[ -f "$repo_root/scripts/ci/$script" ]] || fail "missing packager: $script"
done

if [[ "$status" -ne 0 ]]; then
  exit 1
fi
note "release pipeline structure is valid"
