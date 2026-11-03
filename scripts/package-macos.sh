#!/bin/bash
# Builds and signs the macOS release: OpenSCRIB.app with the command-line
# recorder and the vision helper inside it, plus a .dmg and a .zip.
#
# Copyright (c) 2026 Rovamp Services Pty Ltd. The bundle carries this in its
# Info.plist (tauri.conf.json).
#
#   scripts/package-macos.sh                 build, sign and package
#   scripts/package-macos.sh --skip-build    re-sign and re-package only
#   scripts/package-macos.sh --no-sign       unsigned build, for a quick test
#   scripts/package-macos.sh --notarize <profile>
#
# The signing identity defaults to the only Developer ID Application in the
# keychain; set IDENTITY to choose one. Notarizing needs credentials stored
# once per machine:
#
#   xcrun notarytool store-credentials openscrib \
#       --apple-id <id> --team-id <team> --password <app-specific password>
#
# Cisco's OpenH264 and the AI models are NOT included: OpenH264 must be
# fetched from Cisco by the user (the app does it on first run), and models
# are downloaded on demand from Settings -> Models.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
arch="$(uname -m)"
name="OpenSCRIB-$version-macos-$arch"
out="$root/dist/$name"
app="$out/OpenSCRIB.app"
entitlements="$root/app/src-tauri/entitlements.plist"
# Homebrew's rustup doesn't put cargo on PATH, and npx tauri needs it there.
if ! command -v cargo > /dev/null; then
    rust_arch="${arch/arm64/aarch64}"
    for dir in "$HOME/.cargo/bin" "$HOME/.rustup/toolchains/stable-$rust_arch-apple-darwin/bin"; do
        [ -x "$dir/cargo" ] && { PATH="$dir:$PATH"; break; }
    done
fi
command -v cargo > /dev/null || { echo "cargo is not on PATH" >&2; exit 1; }

skip_build=0
sign=1
notarize_profile=""
while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build) skip_build=1 ;;
        --no-sign) sign=0 ;;
        --notarize) notarize_profile="${2:?--notarize needs a notarytool profile name}"; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

if [ "$sign" = 1 ]; then
    identities="$(security find-identity -v -p codesigning |
        sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p')"
    count="$(printf '%s' "$identities" | grep -c . || true)"
    if [ -z "${IDENTITY:-}" ]; then
        if [ "$count" = 0 ]; then
            echo "No Developer ID Application identity in the keychain." >&2
            echo "Install one, pass IDENTITY=..., or run with --no-sign." >&2
            exit 1
        fi
        # Apple's legacy Developer ID intermediate expires 1 Feb 2027 and caps
        # every certificate it issued to that date, so replacing one leaves two
        # in the keychain. Never guess which to ship with.
        if [ "$count" -gt 1 ]; then
            echo "More than one Developer ID Application identity; pass IDENTITY=..." >&2
            echo "$identities" | sed 's/^/    /' >&2
            exit 1
        fi
        IDENTITY="$identities"
    fi

    # A timestamped signature outlives its certificate, but nothing can be
    # signed with an expired one, so say how long is left.
    expiry="$(security find-certificate -c "$IDENTITY" -p 2>/dev/null |
        openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2)"
    if [ -n "$expiry" ]; then
        left=$(( ($(date -j -f "%b %e %T %Y %Z" "$expiry" +%s 2>/dev/null || echo 0) - $(date +%s)) / 86400 ))
        if [ "$left" -le 0 ]; then
            echo "\"$IDENTITY\" expired on $expiry; issue a new one." >&2
            exit 1
        fi
        [ "$left" -lt 60 ] && echo "Note: the signing certificate expires in $left days ($expiry)."
    fi
fi

if [ "$skip_build" = 0 ]; then
    echo "==> Building the recorder and the vision helper"
    (cd "$root" && cargo build --release -p openscrib-cli -p openscrib-vision)
    echo "==> Building OpenSCRIB.app"
    (cd "$root/app" && npx tauri build --bundles app)

    rm -rf "$out"
    mkdir -p "$out"
    cp -R "$root/target/release/bundle/macos/OpenSCRIB.app" "$app"

    # Both helpers go beside the app binary, where the editor looks for them.
    # "openscrib-cli", not "openscrib": the disk is case-insensitive, so that
    # would collide with the app's own OpenSCRIB binary.
    cp "$root/target/release/openscrib" "$app/Contents/MacOS/openscrib-cli"
    cp "$root/target/release/openscrib-vision" "$app/Contents/MacOS/openscrib-vision"

    cp "$root/LICENSE-MIT" "$out/LICENSE-MIT.txt"
    cat > "$out/README.txt" <<TXT
OpenSCRIB $version - macOS ($arch)
Copyright (c) 2026 Rovamp Services Pty Ltd

Drag OpenSCRIB.app to Applications and open it.

First run:
  1. macOS asks for Screen Recording and Accessibility. Both are needed:
     without them a recording captures nothing. The home screen has the
     buttons, and macOS reopens OpenSCRIB after you grant them.
  2. Click "Get OpenH264" on the home screen to enable screen video. It
     downloads Cisco's free encoder (under 1 MB), as their licence requires.
  3. Narration asks for the microphone the first time you record.

Your files:
  ~/Documents/OpenSCRIB                       recordings and exports
  ~/Library/Application Support/OpenSCRIB     models and OpenH264

Command line:
  /Applications/OpenSCRIB.app/Contents/MacOS/openscrib-cli --help
  (it needs the same two permissions, granted to your terminal)
TXT
    echo "Built $app"
fi

[ -d "$app" ] || { echo "$app doesn't exist; build it first" >&2; exit 1; }

# Cargo's own stripping needs rust-objcopy, which a rustup toolchain without
# libLLVM.dylib can't run, so the release binaries can still carry their debug
# info (~20 MB each). strip does the same job, and has to come before signing.
strip -S -x "$app/Contents/MacOS/"* 2> /dev/null || true

if [ "$sign" = 1 ]; then
    echo "==> Signing with \"$IDENTITY\""
    # Inside out: the nested binaries first, then the bundle itself. Each one
    # gets the hardened runtime and a trusted timestamp, which notarizing
    # requires.
    for bin in "$app/Contents/MacOS/openscrib-cli" "$app/Contents/MacOS/openscrib-vision" "$app"; do
        codesign --force --options runtime --timestamp \
            --entitlements "$entitlements" --sign "$IDENTITY" "$bin"
    done

    echo "==> Verifying"
    codesign --verify --deep --strict --verbose=2 "$app"
    codesign --display --entitlements - "$app" 2>&1 | sed -n '1,4p;/com.apple.security/p'
    # Gatekeeper only accepts a notarized build, so report its answer without
    # failing the package.
    spctl --assess --type exec --verbose=4 "$app" || true
fi

echo "==> Packaging"
mkdir -p "$root/dist"
zip="$root/dist/$name.zip"
rm -f "$zip"
# ditto keeps the signature and the bundle's resource forks; plain zip doesn't.
ditto -c -k --keepParent --sequesterRsrc "$app" "$zip"

dmg="$root/dist/$name.dmg"
rm -f "$dmg"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/"
cp "$out/README.txt" "$out/LICENSE-MIT.txt" "$staging/"
ln -s /Applications "$staging/Applications"
# hdiutil's own size estimate comes out short for a bundle this size and the
# copy then fails with "no space left on device", so size the image here.
size_mb=$(( $(du -sm "$staging" | cut -f1) + 60 ))
hdiutil create -volname "OpenSCRIB $version" -srcfolder "$staging" \
    -ov -format UDZO -size "${size_mb}m" -quiet "$dmg"
if [ "$sign" = 1 ]; then
    codesign --force --timestamp --sign "$IDENTITY" "$dmg"
fi

if [ -n "$notarize_profile" ]; then
    echo "==> Notarizing (this waits for Apple)"
    xcrun notarytool submit "$dmg" --keychain-profile "$notarize_profile" --wait
    xcrun stapler staple "$dmg"
    # Staple the app too, so the zip carries the ticket as well.
    xcrun notarytool submit "$zip" --keychain-profile "$notarize_profile" --wait
    xcrun stapler staple "$app"
    rm -f "$zip"
    ditto -c -k --keepParent --sequesterRsrc "$app" "$zip"
    spctl --assess --type exec --verbose=4 "$app"
fi

du -h "$zip" "$dmg" | sed 's/^/    /'
echo "Done. $([ "$sign" = 1 ] && echo "Signed" || echo "UNSIGNED")$([ -n "$notarize_profile" ] && echo " and notarized")."
[ -n "$notarize_profile" ] || echo "Not notarized: macOS will warn on another Mac until you run --notarize."
