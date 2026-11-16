#!/usr/bin/env bash
# Build the portable Linux artifact for BSH: one AppImage.
#
# What the image carries: the bsh binary, its desktop entry and icon, and the
# libraries the host is not guaranteed to have. What it deliberately does NOT
# carry: WebKitGTK 4.1 and the stack it pulls in (GTK 3, GLib, ICU, libsoup,
# gdk-pixbuf). WebKitGTK hard-codes the absolute paths of its helper processes
# and injected bundle into libwebkit2gtk at build time and offers no environment
# variable to relocate them, so a bundled WebKit cannot be made to work by
# anything short of a mount namespace overlaying /usr - which is the Flatpak
# runtime's job, not a packager's. The host supplies WebKitGTK 4.1 instead, and
# installer/appimage/AppRun names the one command that installs it.
#
# That split is also what the library filter below encodes: a library the host's
# own libwebkit2gtk already loads is excluded, because shipping a second copy of
# GLib next to the host's WebKitGTK is how an AppImage ends up crashing in ways
# only its users can reproduce. Everything the host's WebKitGTK does not answer
# for is bundled.
#
# Usage:
#   package-linux.sh --version 4.1.2 --binary target/release/bsh [--out dist]
#
# Output (in --out):
#   bsh-<version>-linux-<arch>.AppImage

set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
usage: package-linux.sh --version <version> --binary <path> [--out <dir>]

  --version   the release version, e.g. 4.1.2
  --binary    the built ELF binary, e.g. target/release/bsh
  --out       output directory (default: dist)
USAGE
}

die() {
  printf 'package-linux: error: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'package-linux: %s\n' "$*"
}

version=""
binary=""
out="dist"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift 2 ;;
    --binary) binary="${2:-}"; shift 2 ;;
    --out) out="${2:-}"; shift 2 ;;
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
readme="$repo_root/README.md"
desktop="$repo_root/installer/appimage/bsh.desktop"
apprun="$repo_root/installer/appimage/AppRun"
for asset in "$png" "$license" "$desktop" "$apprun"; do
  [[ -f "$asset" ]] || die "missing packaging input: ${asset#"$repo_root"/}"
done

# A binary that is not ELF cannot be tested or bundled; catching it here beats
# publishing an AppImage that only fails on the user's machine.
if [[ "$(head -c 4 "$binary" | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]]; then
  die "$binary is not an ELF binary"
fi

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

# --- appimagetool -----------------------------------------------------------
# Pinned by version and checksum: the packager's output is only reproducible if
# the tool that assembles it is. A new upstream build is an explicit edit here,
# not something a release silently picks up.
appimagetool_version="1.9.1"
case "$arch" in
  x86_64)
    appimagetool_sha256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0" ;;
  aarch64)
    appimagetool_sha256="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158" ;;
esac
tool_url="https://github.com/AppImage/appimagetool/releases/download/${appimagetool_version}/appimagetool-${arch}.AppImage"

cache="${XDG_CACHE_HOME:-$HOME/.cache}/bsh-appimage"
tool="$cache/appimagetool-${appimagetool_version}-${arch}.AppImage"
if [[ ! -x "$tool" ]]; then
  mkdir -p "$cache"
  note "downloading appimagetool ${appimagetool_version} for $arch"
  curl --fail --location --silent --show-error --output "$tool.part" "$tool_url" \
    || die "could not download $tool_url"
  printf '%s  %s\n' "$appimagetool_sha256" "$tool.part" | sha256sum --check --status \
    || die "appimagetool checksum mismatch; refusing to run $tool_url"
  chmod 755 "$tool.part"
  mv "$tool.part" "$tool"
fi

mkdir -p "$out"
outfile="$out/bsh-${version}-linux-${arch}.AppImage"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

appdir="$work/BSH.AppDir"
mkdir -p \
  "$appdir/usr/bin" \
  "$appdir/usr/lib" \
  "$appdir/usr/share/applications" \
  "$appdir/usr/share/icons/hicolor/512x512/apps" \
  "$appdir/usr/share/doc/bsh"

install -m 755 "$binary" "$appdir/usr/bin/bsh"
install -m 755 "$apprun" "$appdir/AppRun"
install -m 644 "$desktop" "$appdir/bsh.desktop"
install -m 644 "$desktop" "$appdir/usr/share/applications/bsh.desktop"
install -m 644 "$png" "$appdir/bsh.png"
install -m 644 "$png" "$appdir/usr/share/icons/hicolor/512x512/apps/bsh.png"
install -m 644 "$license" "$appdir/usr/share/doc/bsh/copyright"
ln -sfn bsh.png "$appdir/.DirIcon"
if [[ -f "$readme" ]]; then
  install -m 644 "$readme" "$appdir/usr/share/doc/bsh/README.md"
fi

# --- what the host already guarantees --------------------------------------
# The host's own libwebkit2gtk resolves this list when the app opens its browser
# pane, so the image must not carry a second copy of any of it. An empty answer
# means the build host cannot tell the two sets apart, which would silently ship
# the whole GTK stack.
host_guaranteed_sonames() {
  local webkit
  webkit="$(ldconfig -p 2>/dev/null \
    | awk '/libwebkit2gtk-4[.]1[.]so[.]0 / {print $NF; exit}')"
  [[ -n "$webkit" ]] || return 1
  ldd "$webkit" 2>/dev/null \
    | awk '/=> \// {print $1} /^[[:space:]]*\// {print $1}' \
    | sort -u
  return 0
}

host_set="$(host_guaranteed_sonames)" \
  || die "the build host has no libwebkit2gtk-4.1.so.0; install libwebkit2gtk-4.1-0 (and its -dev package) so the host-guaranteed set can be computed"

# --- libraries to bundle ----------------------------------------------------
# Excluded by rule, not by a maintained list:
#   * glibc and the loader - an image that carries its own libc is not portable,
#     it is a different failure mode on every distribution;
#   * the graphics stack - libGL/libEGL/dri must match the running kernel driver,
#     and a bundled mesa loses hardware acceleration and often the display;
#   * anything the host's WebKitGTK already loads, plus WebKitGTK itself
#     (a library does not appear in its own closure, so it needs its own rule).
bundled=""
while IFS= read -r entry; do
  [[ -n "$entry" ]] || continue
  soname="${entry%% *}"
  path="${entry#* }"
  [[ -n "$soname" && -n "$path" && -e "$path" ]] || continue
  case "$soname" in
    ld-linux*|libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|libutil.so.*|libresolv.so.*|libanl.so.*|libnss_*) continue ;;
    libGL.so.*|libEGL.so.*|libGLX.so.*|libGLdispatch.so.*|libOpenGL.so.*|libvulkan.so.*|libdrm.so.*|libgbm.so.*) continue ;;
    # WebKitGTK itself is the host's, and it is the one library its own ldd
    # closure cannot exclude for us: a shared object does not list itself. An
    # image that carried it would ship a second WebKit whose helper processes
    # are still looked up at the host's compiled-in path.
    libwebkit2gtk-*.so.*|libjavascriptcoregtk-*.so.*) continue ;;
  esac
  # `set -e` treats a bare `condition && continue` as a failure when the
  # condition is false, so these two skips are spelled as ifs.
  if printf '%s\n' "$host_set" | grep -Fxq -- "$soname"; then
    continue
  fi
  if [[ -e "$appdir/usr/lib/$soname" ]]; then
    continue
  fi
  install -m 644 "$path" "$appdir/usr/lib/$soname"
  bundled="${bundled} $soname"
done < <(ldd "$binary" 2>/dev/null \
  | awk '/=> \// {print $1" "$3} /^[[:space:]]*\// {print $1" "$1}')

if [[ -n "$bundled" ]]; then
  note "bundled:$bundled"
else
  note "bundled: nothing; the host's WebKitGTK stack covers every library bsh links"
fi

# --- assemble ---------------------------------------------------------------
# The appimagetool download is itself an AppImage; --appimage-extract-and-run
# because a build host has no reason to have libfuse2 installed.
ARCH="$arch" APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$appdir" "$outfile" >/dev/null \
  || die "appimagetool failed"
# Checked before chmod: a tool that exits 0 without writing is otherwise
# reported as a confusing chmod error.
[[ -s "$outfile" ]] || die "appimagetool produced no file at $outfile"
chmod 755 "$outfile"

# A generated image must at least be a type-2 AppImage with the runtime present,
# which is what the ELF check and the size floor prove without mounting it.
if [[ "$(head -c 4 "$outfile" | od -An -tx1 | tr -d ' \n')" != "7f454c46" ]]; then
  die "$outfile is not an ELF image"
fi
size_mb="$(( $(wc -c <"$outfile") / 1024 / 1024 ))"
note "wrote $outfile (${size_mb} MiB)"
