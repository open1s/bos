#!/usr/bin/env bash
# Build a Debian package and a portable tarball for Linux.
#
# The .deb is the installer: it carries the binary, a desktop entry, the icon
# and the licence, and its Depends field is derived from the release binary's
# own shared-library list (so it names the GTK/WebKit/Soup stack actually
# linked, not a hand-written guess that drifts). The tarball is the fallback
# for distributions without dpkg, and carries an install.sh for a per-user
# install under ~/.local.
#
# Usage:
#   package-linux.sh --version 3.0.3 --binary target/release/bsh [--out dist] [--no-deb] [--no-rpm]
#
# Outputs (in --out):
#   bsh_<deb-version>_<arch>.deb          (when dpkg-deb is available)
#   bsh-<version>-linux-<arch>.rpm        (when rpmbuild is available)
#   bsh-<version>-linux-<arch>.tar.gz

set -euo pipefail

usage() {
  sed -n '3,17p' "${BASH_SOURCE[0]}" | sed 's/^#\{0,1\} \{0,1\}//'
}

die() {
  printf 'package-linux: error: %s\n' "$*" >&2
  exit 1
}

version=""
binary=""
out="dist"
build_deb=true
build_rpm=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift 2 ;;
    --binary) binary="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
    --no-deb) build_deb=false; shift ;;
    --no-rpm) build_rpm=false; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ -n "$version" ]] || die "--version is required"
[[ -n "$binary" ]] || die "--binary is required"
[[ -f "$binary" ]] || die "binary not found: $binary"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
png="$repo_root/crates/bsh/src/assets/bos-harness-icon.png"
license="$repo_root/LICENSE"
[[ -f "$png" ]] || die "icon not found: $png"
[[ -f "$license" ]] || die "licence not found: $license"

if [[ "$build_deb" == true ]] && ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "package-linux: dpkg-deb not found; skipping the .deb" >&2
  build_deb=false
fi

if [[ "$build_rpm" == true ]] && ! command -v rpmbuild >/dev/null 2>&1; then
  echo "package-linux: rpmbuild not found; skipping the .rpm" >&2
  build_rpm=false
fi

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64"; deb_arch="amd64" ;;
  aarch64|arm64) arch="aarch64"; deb_arch="arm64" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

# Debian compares a tilde below the empty string, so 3.0.3-rc.1 becomes
# 3.0.3~rc.1 and sorts before 3.0.3 rather than after it.
# sed, not ${version/-/~}: bash on Ubuntu expands the tilde in the
# substitution's replacement to $HOME (measured: 0.0.0/home/runnerci).
deb_version="$(printf '%s' "$version" | sed 's/-/~/')"

mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

stage="$work/root"
mkdir -p "$stage/DEBIAN" \
  "$stage/usr/bin" \
  "$stage/usr/share/applications" \
  "$stage/usr/share/icons/hicolor/512x512/apps" \
  "$stage/usr/share/doc/bsh"

install -m 755 "$binary" "$stage/usr/bin/bsh"
install -m 644 "$png" "$stage/usr/share/icons/hicolor/512x512/apps/bsh.png"
install -m 644 "$license" "$stage/usr/share/doc/bsh/copyright"

cat > "$stage/usr/share/applications/bsh.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=BSH
GenericName=BOS Harness
Comment=The BOS agent workspace
Exec=bsh %U
Icon=bsh
Terminal=false
Categories=Development;Utility;
StartupWMClass=bsh
Keywords=BOS;agent;AI;terminal;harness;
DESKTOP

# Derive Depends from the binary itself. dpkg -S maps each resolved .so to the
# package that owns it, which is the same list apt would need to run it. The
# two explicit entries cover libraries the WebView stack can dlopen rather
# than link, which ldd would miss.
collect_depends() {
  local lib real owner packages=""
  while IFS= read -r lib; do
    [[ -n "$lib" ]] || continue
    real="$(readlink -f "$lib" 2>/dev/null || printf '%s' "$lib")"
    # A library outside dpkg's database (a PPA, /usr/local, a vendored copy)
    # must not abort packaging, hence the trailing || true.
    owner="$(dpkg -S "$real" 2>/dev/null \
      | awk -F: '/^[a-z0-9][a-z0-9+.-]*:/ { print $1; exit }' || true)"
    [[ -n "$owner" ]] && packages="$packages $owner"
  done < <(ldd "$binary" 2>/dev/null \
    | awk '/=> \//{ print $3 } /^[[:space:]]*\//{ print $1 }' \
    | sort -u)

  packages="$packages libwebkit2gtk-4.1-0 libgtk-3-0"
  printf '%s\n' $packages \
    | sed 's/:.*//' \
    | grep -v -E -- '-dev$|^gcc-|^binutils$|^g\+\+-' \
    | sort -u \
    | paste -sd, -
}

depends="libc6"
if [[ "$build_deb" == true ]]; then
  derived="$(collect_depends)"
  [[ -n "$derived" ]] && depends="$derived"
fi

installed_kb="$(du -sk "$stage/usr" | cut -f1)"

cat > "$stage/DEBIAN/control" <<CONTROL
Package: bsh
Version: $deb_version
Section: utils
Priority: optional
Architecture: $deb_arch
Maintainer: BrickOS Team <open1s@users.noreply.github.com>
Installed-Size: $installed_kb
Depends: $depends
Homepage: https://github.com/open1s/bos
Description: BOS Harness - native desktop agent workspace
 BSH is the native desktop shell of BOS: agent sessions, a Ghostty terminal,
 a browser pane, an editor and the extension platform in one GPUI window.
CONTROL

tarball="$out/bsh-$version-linux-$arch.tar.gz"
tar_root="$work/bsh-$version-linux-$arch"
mkdir -p "$tar_root/share/applications" \
  "$tar_root/share/icons/hicolor/512x512/apps"
install -m 755 "$binary" "$tar_root/bsh"
install -m 644 "$stage/usr/share/applications/bsh.desktop" \
  "$tar_root/share/applications/bsh.desktop"
install -m 644 "$png" \
  "$tar_root/share/icons/hicolor/512x512/apps/bsh.png"
install -m 644 "$license" "$tar_root/LICENSE"
if [[ -f "$repo_root/README.md" ]]; then
  install -m 644 "$repo_root/README.md" "$tar_root/README.md"
fi

cat > "$tar_root/install.sh" <<'INSTALL'
#!/bin/sh
# Install BSH for the current user. PREFIX overrides ~/.local.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
prefix="${PREFIX:-$HOME/.local}"
mkdir -p "$prefix/bin" "$prefix/share/applications" \
  "$prefix/share/icons/hicolor/512x512/apps"
install -m755 "$here/bsh" "$prefix/bin/bsh"
install -m644 "$here/share/applications/bsh.desktop" \
  "$prefix/share/applications/bsh.desktop"
install -m644 "$here/share/icons/hicolor/512x512/apps/bsh.png" \
  "$prefix/share/icons/hicolor/512x512/apps/bsh.png"
printf 'installed %s\n' "$prefix/bin/bsh"
INSTALL
chmod 755 "$tar_root/install.sh"

tar -czf "$tarball" -C "$work" "$(basename "$tar_root")"
printf 'package-linux: wrote %s\n' "$tarball"

if [[ "$build_deb" == true ]]; then
  deb="$out/bsh_${deb_version}_${deb_arch}.deb"
  dpkg-deb --root-owner-group --build "$stage" "$deb" >/dev/null
  dpkg-deb --info "$deb" >/dev/null
  printf 'package-linux: wrote %s\n' "$deb"
  printf 'package-linux: Depends: %s\n' "$depends"
fi

if [[ "$build_rpm" == true ]]; then
  # RPM splits the version at the last hyphen, so a prerelease suffix belongs in
  # Release (0.rc.1) and sorts correctly there.
  rpm_version="${version%%[-+]*}"
  rpm_release="1"
  if [[ "$version" == *-* ]]; then
    rpm_release="0.${version#*-}"
    rpm_release="${rpm_release//[^A-Za-z0-9.]/.}"
  fi

  topdir="$work/rpm"
  mkdir -p "$topdir/BUILD" "$topdir/RPMS" "$topdir/SOURCES" "$topdir/SPECS" "$topdir/SRPMS"
  spec="$topdir/SPECS/bsh.spec"

  cat > "$spec" <<SPEC
Name: bsh
Version: $rpm_version
Release: $rpm_release
Summary: BOS Harness - native desktop agent workspace
License: MIT
URL: https://github.com/open1s/bos
BuildArch: $arch
AutoReqProv: yes

%description
BSH is the native desktop shell of BOS: agent sessions, a Ghostty terminal,
a browser pane, an editor and the extension platform in one GPUI window.

%install
install -D -m 755 "$stage/usr/bin/bsh" %{buildroot}/usr/bin/bsh
install -D -m 644 "$stage/usr/share/applications/bsh.desktop" %{buildroot}/usr/share/applications/bsh.desktop
install -D -m 644 "$stage/usr/share/icons/hicolor/512x512/apps/bsh.png" %{buildroot}/usr/share/icons/hicolor/512x512/apps/bsh.png
install -D -m 644 "$stage/usr/share/doc/bsh/copyright" %{buildroot}/usr/share/doc/bsh/copyright

%files
/usr/bin/bsh
/usr/share/applications/bsh.desktop
/usr/share/icons/hicolor/512x512/apps/bsh.png
/usr/share/doc/bsh/copyright
SPEC

  log="$work/rpmbuild.log"
  if ! rpmbuild --define "_topdir $topdir" --define "_build_id_links none" \
      --target "$arch" -bb "$spec" >"$log" 2>&1; then
    cat "$log" >&2
    die "rpmbuild failed"
  fi

  built_rpm="$(find "$topdir/RPMS" -type f -name '*.rpm' -print -quit)"
  [[ -n "$built_rpm" ]] || die "rpmbuild produced no .rpm"
  rpm_path="$out/bsh-$version-linux-$arch.rpm"
  cp "$built_rpm" "$rpm_path"
  printf 'package-linux: wrote %s\n' "$rpm_path"
  if command -v rpm >/dev/null 2>&1; then
    # AutoReqProv resolves sonames into distro package requirements; print them
    # so the release log shows what the package actually asks for.
    printf 'package-linux: RPM requires: %s\n' \
      "$(rpm -qp --requires "$rpm_path" | paste -sd, -)"
  fi
fi

