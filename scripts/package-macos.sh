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

# Every Developer ID certificate for a team carries the same common name, so a
# replacement sits in the keychain beside its predecessor and only the SHA-1
# hash tells them apart. Choose by hash, and prefer a certificate from the G2
# sub-CA: the previous sub-CA expires on 1 Feb 2027 and caps everything it
# issued to that date, whatever the certificate itself says.
#
# Sets IDENTITY (the hash codesign is given), IDENTITY_NAME, IDENTITY_SUBCA and
# IDENTITY_DAYS.
pick_identity() {
    local held dir pem fp issuer end g2 best_end=0 best_g2=-1 found=0
    held="$(security find-identity -v -p codesigning |
        sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Developer ID Application:.*/\1/p')"
    [ -n "$held" ] || return 1

    dir="$(mktemp -d)"
    security find-certificate -a -c "Developer ID Application" -p 2> /dev/null |
        awk -v d="$dir" '/BEGIN CERTIFICATE/ { n++ } n { print > (d "/" n ".pem") }'
    for pem in "$dir"/*.pem; do
        [ -f "$pem" ] || continue
        fp="$(openssl x509 -in "$pem" -noout -fingerprint -sha1 2> /dev/null |
            cut -d= -f2 | tr -d ':')"
        # Only a certificate whose private key is on this machine can sign.
        case "$held" in *"$fp"*) ;; *) continue ;; esac
        issuer="$(openssl x509 -in "$pem" -noout -issuer)"
        case "$issuer" in *OU=G2*) g2=1 ;; *) g2=0 ;; esac
        end="$(date -j -f "%b %e %T %Y %Z" \
            "$(openssl x509 -in "$pem" -noout -enddate | cut -d= -f2)" +%s 2> /dev/null || echo 0)"
        if [ "$g2" -gt "$best_g2" ] || { [ "$g2" = "$best_g2" ] && [ "$end" -gt "$best_end" ]; }; then
            IDENTITY="$fp"
            IDENTITY_NAME="$(openssl x509 -in "$pem" -noout -subject |
                sed -n 's/.*CN=\(Developer ID Application: [^/]*\).*/\1/p')"
            IDENTITY_SUBCA="$([ "$g2" = 1 ] && echo G2 || echo "previous (expires 1 Feb 2027)")"
            IDENTITY_DAYS=$(( (end - $(date +%s)) / 86400 ))
            best_end="$end"
            best_g2="$g2"
            found=1
        fi
    done
    rm -rf "$dir"
    [ "$found" = 1 ]
}

# Check the credentials before building: a submission that cannot authenticate
# is worth finding out about now, not in twenty minutes.
if [ -n "$notarize_profile" ]; then
    if ! xcrun notarytool history --keychain-profile "$notarize_profile" > /dev/null 2>&1; then
        echo "notarytool can't use the profile \"$notarize_profile\"." >&2
        echo "Store it once with an app-specific password from appleid.apple.com:" >&2
        echo "    xcrun notarytool store-credentials $notarize_profile \\" >&2
        echo "        --apple-id <your Apple ID> --team-id NP2J294726 --password <app-specific password>" >&2
        echo "or with an App Store Connect key: --key AuthKey_XXX.p8 --key-id XXX --issuer <uuid>" >&2
        exit 1
    fi
fi

if [ "$sign" = 1 ]; then
    if [ -n "${IDENTITY:-}" ]; then
        IDENTITY_NAME="$IDENTITY"
        IDENTITY_SUBCA="as given"
        IDENTITY_DAYS=""
    elif ! pick_identity; then
        echo "No Developer ID Application identity in this keychain." >&2
        echo "Install one, pass IDENTITY=<name or hash>, or run with --no-sign." >&2
        exit 1
    fi

    # A timestamped signature keeps validating after its certificate expires,
    # but nothing new can be signed with an expired one.
    if [ -n "${IDENTITY_DAYS:-}" ]; then
        if [ "$IDENTITY_DAYS" -le 0 ]; then
            echo "That certificate expired $(( -IDENTITY_DAYS )) days ago; issue a new one from the G2 sub-CA." >&2
            exit 1
        fi
        if [ "$IDENTITY_SUBCA" != "G2" ]; then
            echo "Note: signing with a certificate from Apple's previous sub-CA, which stops"
            echo "      issuing and verifying new signatures on 1 Feb 2027 ($IDENTITY_DAYS days)."
            echo "      Replace it from the G2 sub-CA; this build stays valid, it is timestamped."
        elif [ "$IDENTITY_DAYS" -lt 60 ]; then
            echo "Note: the signing certificate expires in $IDENTITY_DAYS days."
        fi
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

# This script once lost its build step to a careless edit and then signed and
# shipped the previous build without a word, so never sign a bundle that is
# older than the code it came from.
newest_src="$(find "$root/crates" "$root/app/src" "$root/app/src-tauri/src" \
    \( -name '*.rs' -o -name '*.ts' -o -name '*.svelte' \) -exec stat -f '%m' {} + | sort -rn | head -1)"
built="$(stat -f %m "$app/Contents/MacOS/openscrib-app")"
if [ "$built" -lt "${newest_src:-0}" ]; then
    echo "$app was built $(date -r "$built" '+%d %b %H:%M'), before the code last changed." >&2
    if [ "$skip_build" = 0 ]; then
        echo "The build step did not run. That is a bug in this script, not in the build." >&2
        exit 1
    fi
    echo "Carrying on because --skip-build was asked for." >&2
fi

# Cargo's own stripping needs rust-objcopy, which a rustup toolchain without
# libLLVM.dylib can't run, so the release binaries can still carry their debug
# info (~20 MB each). strip does the same job, and has to come before signing.
strip -S -x "$app/Contents/MacOS/"* 2> /dev/null || true

if [ "$sign" = 1 ]; then
    echo "==> Signing with \"$IDENTITY_NAME\" [$IDENTITY, sub-CA $IDENTITY_SUBCA]"
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
    echo "==> Notarizing (Apple usually answers in a few minutes)"
    # One submission is enough for both artefacts: Apple records the disk
    # image and every signed binary inside it, and a ticket is looked up by
    # code hash, so the .app can be stapled from the same submission.
    reply="$(mktemp)"
    xcrun notarytool submit "$dmg" --keychain-profile "$notarize_profile" --wait 2>&1 |
        tee "$reply"
    # notarytool reports a rejection in its output rather than its exit code,
    # so read the status it printed.
    if [ "$(sed -n 's/^ *status: *\(.*\)$/\1/p' "$reply" | tail -1)" != "Accepted" ]; then
        id="$(sed -n 's/^ *id: *\([0-9a-f-]\{36\}\)$/\1/p' "$reply" | head -1)"
        if [ -n "$id" ]; then
            echo "--- Apple's notarization log ---" >&2
            xcrun notarytool log "$id" --keychain-profile "$notarize_profile" >&2 || true
        fi
        echo "Notarizing failed; the log says which binary Apple objected to." >&2
        exit 1
    fi

    # Stapling writes the ticket into the artefacts, so they pass Gatekeeper
    # on a Mac that is offline or has never seen them.
    xcrun stapler staple "$dmg"
    xcrun stapler staple "$app"
    rm -f "$zip"
    ditto -c -k --keepParent --sequesterRsrc "$app" "$zip"
    xcrun stapler validate "$app"
    # The check that matters: what another Mac will decide on first launch.
    spctl --assess --type exec --verbose=4 "$app"
fi

du -h "$zip" "$dmg" | sed 's/^/    /'
echo "Done. $([ "$sign" = 1 ] && echo "Signed" || echo "UNSIGNED")$([ -n "$notarize_profile" ] && echo " and notarized")."
[ -n "$notarize_profile" ] || echo "Not notarized: macOS will warn on another Mac until you run --notarize."
