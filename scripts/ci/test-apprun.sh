#!/usr/bin/env bash
# Exercise installer/appimage/AppRun without building an image.
#
# The release job proves the image starts on clean distributions. This runs on
# every change and pins what breaks silently: the glibc floor, the
# missing-library message (one command per package manager), the passthrough to
# bsh, and --integrate/--uninstall.
#
# Usage: test-apprun.sh

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
apprun="$repo_root/installer/appimage/AppRun"
[[ -f "$apprun" ]] || { printf 'test-apprun: error: missing %s\n' "$apprun" >&2; exit 1; }

failures=0
fail() {
  printf 'test-apprun: error: %s\n' "$*" >&2
  failures=1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# --- a scratch AppDir -------------------------------------------------------
appdir="$work/AppDir"
mkdir -p "$appdir/usr/bin" "$work/stub" "$work/os"
install -m 755 "$apprun" "$appdir/AppRun"
cat > "$appdir/usr/bin/bsh" <<'EOF'
#!/bin/sh
printf 'exec:%s\n' "$*"
EOF
chmod 755 "$appdir/usr/bin/bsh"
: > "$appdir/bsh.png"

# --- stubs for the two host queries AppRun makes ----------------------------
cat > "$work/stub/getconf" <<'EOF'
#!/bin/sh
printf 'glibc %s\n' "${FAKE_GLIBC-2.39}"
EOF
chmod 755 "$work/stub/getconf"

# Every dependency resolves unless FAKE_MISSING names sonames, which is how the
# "host without WebKitGTK" case is reproduced without a container.
cat > "$work/stub/ldd" <<'EOF'
#!/bin/sh
for soname in ${FAKE_MISSING:-}; do
  printf '\t%s => not found\n' "$soname"
done
printf '\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x0)\n'
EOF
chmod 755 "$work/stub/ldd"

run() {
  PATH="$work/stub:$PATH" "$appdir/AppRun" "$@"
}

# --- 1. the app's own command line is passed through ------------------------
output="$(run --help)" || fail "--help did not reach bsh"
[[ "$output" == "exec:--help" ]] || fail "bsh received '$output' instead of --help"

output="$(run ext list)" || fail "ext did not reach bsh"
[[ "$output" == "exec:ext list" ]] || fail "bsh received '$output' instead of ext list"

# --- 2. --integrate refuses a path it cannot keep --------------------------
set +e
error="$(APPIMAGE= XDG_DATA_HOME="$work/data" run --integrate 2>&1)"
status=$?
set -e
[[ "$status" -eq 2 ]] || fail "--integrate without APPIMAGE exited $status, want 2"
grep -Fq "needs the .AppImage file itself" <<<"$error" \
  || fail "--integrate did not explain why it refused"

# --- 3. --integrate writes a menu entry, --uninstall removes it ------------
appimage="$work/bsh-4.1.2-linux-x86_64.AppImage"
: > "$appimage"
APPIMAGE="$appimage" XDG_DATA_HOME="$work/data" run --integrate > /dev/null \
  || fail "--integrate failed"
desktop="$work/data/applications/bsh.desktop"
[[ -f "$desktop" ]] || fail "--integrate did not write $desktop"
grep -Fq "Exec=\"$appimage\" %U" "$desktop" \
  || fail "the menu entry does not run the image where it sits"
grep -Fq 'Icon=bsh' "$desktop" || fail "the menu entry has no icon"
[[ -f "$work/data/icons/hicolor/512x512/apps/bsh.png" ]] || fail "--integrate did not copy the icon"

APPIMAGE="$appimage" XDG_DATA_HOME="$work/data" run --uninstall > /dev/null \
  || fail "--uninstall failed"
[[ ! -e "$desktop" ]] || fail "--uninstall left $desktop behind"
[[ ! -e "$work/data/icons/hicolor/512x512/apps/bsh.png" ]] || fail "--uninstall left the icon behind"

# --- 4. the glibc floor ----------------------------------------------------
set +e
error="$(FAKE_GLIBC=2.28 run --help 2>&1)"
status=$?
set -e
[[ "$status" -eq 1 ]] || fail "glibc 2.28 exited $status, want 1"
grep -Fq 'needs glibc 2.35 or newer' <<<"$error" || fail "the glibc message is missing"

set +e
error="$(FAKE_GLIBC= run --help 2>&1)"
status=$?
set -e
[[ "$status" -eq 1 ]] || fail "a non-glibc host exited $status, want 1"
grep -Fq 'does not use glibc' <<<"$error" || fail "the musl message is missing"

# --- 5. the missing-library message names the right package ----------------
check_missing() {
  local id="$1" like="$2" expected="$3" error status
  printf 'ID=%s\nID_LIKE=%s\n' "$id" "$like" > "$work/os/fixture"
  set +e
  error="$(BSH_OS_RELEASE="$work/os/fixture" FAKE_MISSING=libwebkit2gtk-4.1.so.0 run --help 2>&1)"
  status=$?
  set -e
  [[ "$status" -eq 1 ]] || fail "$id: a host without WebKitGTK exited $status, want 1"
  grep -Fq 'missing library: libwebkit2gtk-4.1.so.0' <<<"$error" \
    || fail "$id: the missing library was not named"
  grep -Fq "$expected" <<<"$error" || fail "$id: did not print '$expected'"
}

check_missing ubuntu debian 'sudo apt install libwebkit2gtk-4.1-0'
check_missing fedora '' 'sudo dnf install webkit2gtk4.1'
check_missing arch '' 'sudo pacman -S webkit2gtk-4.1'
check_missing opensuse suse 'sudo zypper install libwebkit2gtk-4_1-0'
check_missing void '' 'install the package that provides libwebkit2gtk-4.1.so.0'

# --- 6. an unknown library still gets a sentence, not silence --------------
set +e
error="$(BSH_OS_RELEASE="$work/os/fixture" FAKE_MISSING='libweird.so.9' run --help 2>&1)"
set -e
grep -Fq 'install the package that provides libweird.so.9' <<<"$error" \
  || fail "an unknown soname produced no instruction"

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi
printf 'test-apprun: ok\n'
