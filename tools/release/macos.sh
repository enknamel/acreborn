#!/bin/zsh
# Build, bundle, sign, notarize and staple a macOS release of acswarm.
#
#   tools/release/macos.sh [VERSION] [--notarize] [--universal]
#
# Produces dist/acswarm-VERSION-macos.dmg holding acswarm.app (the
# viewer), an Applications shortcut to drag it onto, and the
# command-line tools beside it. Signing uses the
# "Developer ID Application" identity in the login keychain (set
# SIGN_ID to pick one). Notarization needs credentials stored once with
#   xcrun notarytool store-credentials acswarm --apple-id YOU@EXAMPLE \
#       --team-id TEAMID --password APP-SPECIFIC-PASSWORD
# (an app-specific password from appleid.apple.com), then --notarize.
set -euo pipefail
cd "$(dirname "$0")/../.."
export PATH="$HOME/.cargo/bin:$PATH"

VERSION=${1:-$(git describe --tags --always --dirty 2>/dev/null || echo 0.0.0)}
NOTARIZE=0; UNIVERSAL=0; UNSIGNED=0
for a in "$@"; do
  [[ $a == --notarize ]] && NOTARIZE=1
  [[ $a == --universal ]] && UNIVERSAL=1
  [[ $a == --unsigned ]] && UNSIGNED=1
done
SIGN_ID=${SIGN_ID:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}
if (( ! UNSIGNED )); then
  [[ -n $SIGN_ID ]] || { echo "no Developer ID Application identity in the keychain (or pass --unsigned)"; exit 1; }
fi
echo "version $VERSION, signing as: ${SIGN_ID:-(unsigned)}"

BINS=(acviewer acbot acclient aclauncher)
if (( UNIVERSAL )); then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --target $t $(printf -- '-p %s ' $BINS)
  done
  mkdir -p target/universal
  for b in $BINS; do
    lipo -create -output target/universal/$b \
      target/aarch64-apple-darwin/release/$b target/x86_64-apple-darwin/release/$b
  done
  BIN_DIR=target/universal
else
  cargo build --release $(printf -- '-p %s ' $BINS)
  BIN_DIR=target/release
fi

DIST=dist/acswarm-$VERSION-macos
rm -rf "$DIST" && mkdir -p "$DIST"
APP=$DIST/acswarm.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
sed "s/VERSION/$VERSION/g" tools/release/Info.plist > "$APP/Contents/Info.plist"
cp "$BIN_DIR/acviewer" "$APP/Contents/MacOS/acviewer"
for b in acbot acclient aclauncher; do cp "$BIN_DIR/$b" "$DIST/$b"; done
cp -R scripts "$DIST/scripts"
cp LICENSE README.md "$DIST/"

# Sign: every Mach-O, inside out, with the hardened runtime and a
# secure timestamp (both needed by notarization).
if (( UNSIGNED )); then
  echo "unsigned build (Gatekeeper will need a right-click Open on other Macs)"
else
  sign() { codesign --force --options runtime --timestamp \
             --entitlements tools/release/entitlements.plist --sign "$SIGN_ID" "$1"; }
  sign "$APP/Contents/MacOS/acviewer"
  sign "$APP"
  for b in acbot acclient aclauncher; do sign "$DIST/$b"; done
  codesign --verify --deep --strict --verbose=2 "$APP"
  spctl --assess --type execute --verbose=2 "$APP" || echo "(spctl needs notarization to pass; see --notarize)"
fi

# Notarize the app first (via a zip) so the ticket can be stapled to it
# before it goes into the disk image; then notarize and staple the image.
if (( NOTARIZE )); then
  ZIP=dist/acswarm-$VERSION-app.zip
  rm -f "$ZIP" && ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile acswarm --wait
  xcrun stapler staple "$APP"
  rm -f "$ZIP"
  spctl --assess --type execute --verbose=2 "$APP"
fi

DMG=dist/acswarm-$VERSION-macos.dmg
rm -f "$DMG"
ln -sfn /Applications "$DIST/Applications"
hdiutil create -volname "acswarm $VERSION" -srcfolder "$DIST" -fs HFS+ -format UDZO -ov "$DMG"
rm -f "$DIST/Applications"
if (( ! UNSIGNED )); then
  codesign --force --timestamp --sign "$SIGN_ID" "$DMG"
fi
if (( NOTARIZE )); then
  xcrun notarytool submit "$DMG" --keychain-profile acswarm --wait
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi
echo "release: $DMG"
