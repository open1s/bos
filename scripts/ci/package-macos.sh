#!/usr/bin/env bash
# Build BSH.app, a compressed .dmg and a .app.zip for macOS.
#
# This is the release counterpart of crates/bsh/make-dev-app.sh: the same
# minimal bundle, but versioned from the release tag, signed, and (when
# notary credentials are present) notarized and stapled. Signing matters even
# without a Developer ID: an arm64 macOS binary must carry at least an ad-hoc
# signature or the kernel refuses to execute it.
#
# Usage:
#   package-macos.sh --version 3.0.3 --binary target/release/bsh [--out dist]
#
# Environment:
#   APPLE_SIGNING_IDENTITY   "Developer ID Application: ..." identity. When
#                            empty, the bundle is ad-hoc signed.
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                            all three set => submit the app for notarization
#                            and staple the ticket into the bundle.
#
# Outputs (in --out):
#   bsh-<version>-macos-<arch>.app.zip
#   bsh-<version>-macos-<arch>.dmg

set -euo pipefail

usage() {
  sed -n '3,26p' "${BASH_SOURCE[0]}" | sed 's/^#\{0,1\} \{0,1\}//'
}

die() {
  printf 'package-macos: error: %s\n' "$*" >&2
  exit 1
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
icon="$repo_root/crates/bsh/src/assets/bos-harness-icon.icns"
entitlements="$repo_root/installer/bsh.entitlements"
[[ -f "$icon" ]] || die "icon not found: $icon"

for tool in codesign ditto hdiutil plutil; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool missing: $tool"
done

# CFBundleShortVersionString is 1-3 period-separated integers, so a
# prerelease or build-metadata suffix lives in the artifact name, not here.
core_version="${version%%[-+]*}"

case "$(uname -m)" in
  arm64) arch="arm64" ;;
  x86_64) arch="x86_64" ;;
  *) die "unsupported host architecture: $(uname -m)" ;;
esac

mkdir -p "$out"
# Everything staged lives outside --out so the output directory holds only the
# artifacts that get uploaded.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/BSH.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

install -m 755 "$binary" "$app/Contents/MacOS/bsh"
install -m 644 "$icon" "$app/Contents/Resources/bos-harness-icon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>BSH</string>
  <key>CFBundleDisplayName</key><string>BOS Harness</string>
  <key>CFBundleExecutable</key><string>bsh</string>
  <key>CFBundleIdentifier</key><string>com.bos.bsh</string>
  <key>CFBundleIconFile</key><string>bos-harness-icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$core_version</string>
  <key>CFBundleVersion</key><string>$core_version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 open1s</string>
</dict>
</plist>
PLIST

plutil -lint "$app/Contents/Info.plist" >/dev/null

identity="${APPLE_SIGNING_IDENTITY:-}"
if [[ -n "$identity" ]]; then
  [[ -f "$entitlements" ]] || die "entitlements not found: $entitlements"
  codesign --force --timestamp --options runtime \
    --entitlements "$entitlements" \
    --sign "$identity" "$app"
else
  # Ad-hoc: no identity, no hardened runtime, but a valid code signature.
  codesign --force --sign - "$app"
fi
codesign --verify --deep --strict --verbose=2 "$app"

zip_path="$out/bsh-$version-macos-$arch.app.zip"
dmg_path="$out/bsh-$version-macos-$arch.dmg"
rm -f "$zip_path" "$dmg_path"

notarize=false
if [[ -n "${APPLE_ID:-}" && -n "${APPLE_TEAM_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" ]]; then
  [[ -n "$identity" ]] || die "notarization needs APPLE_SIGNING_IDENTITY"
  notarize=true
fi

if [[ "$notarize" == true ]]; then
  # Submit a zip of the bundle, then staple the ticket into the .app itself so
  # both the dmg and the zip carry it. One submission covers both artifacts.
  submit_zip="$work/BSH-notarize.zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$submit_zip"
  echo "notarizing BSH.app (this waits for Apple)"
  xcrun notarytool submit "$submit_zip" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" \
    --wait
  rm -f "$submit_zip"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  # spctl talks to Gatekeeper; the stapled ticket makes this work offline, but
  # a runner's Gatekeeper cache can be cold, so a refusal is a warning here.
  if ! spctl --assess --type execute --verbose=2 "$app"; then
    echo "package-macos: warning: spctl assessment failed after stapling" >&2
  fi
else
  echo "package-macos: warning: no notary credentials; shipping a signed but unnotarized app" >&2
fi

# Zip after stapling so the ticket travels with the bundle.
if ! ditto -c -k --sequesterRsrc --keepParent "$app" "$zip_path"; then
  echo "package-macos: ditto failed; free space: $(df -h "$out" | tail -1)" >&2
  exit 1
fi

stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/BSH.app"
ln -s /Applications "$stage/Applications"

# hdiutil writes some failures on stdout, so -quiet plus a redirect made a
# failed dmg look like a silent exit 1. Keep the output and report it.
if ! output="$(hdiutil create -volname "BSH $core_version" \
    -srcfolder "$stage" -ov -format UDZO "$dmg_path" 2>&1)"; then
  echo "package-macos: hdiutil create failed: $output" >&2
  echo "package-macos: free space: $(df -h "$out" | tail -1)" >&2
  exit 1
fi
if ! output="$(hdiutil verify "$dmg_path" 2>&1)"; then
  echo "package-macos: hdiutil verify failed: $output" >&2
  exit 1
fi
echo "package-macos: dmg verified"

if [[ -n "$identity" ]]; then
  codesign --force --timestamp --sign "$identity" "$dmg_path"
fi

printf 'package-macos: wrote %s\n' "$dmg_path"
printf 'package-macos: wrote %s\n' "$zip_path"
