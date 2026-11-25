#!/usr/bin/env bash
# Build Arbos.app for macOS (D42): universal binary (arm64 + x86_64), Info.plist
# with the version and build number, Developer ID signing with the hardened
# runtime, notarization + stapling, and the two release artifacts:
#
#   dist/Arbos-<version>-macos-universal.dmg      what people download
#   dist/Arbos-<version>-macos-universal.tar.gz   what the in-app updater installs
#
# Every step it has no credentials for is skipped with a line saying so, so
# the script also runs on a Mac with no certificates (ad-hoc signed, local use).
#
#   scripts/bundle-mac.sh                                   # universal, ad-hoc
#   ARBOS_BUILD=212 scripts/bundle-mac.sh                   # version 0.1.0.212
#   ARBOS_SIGN_IDENTITY="Developer ID Application: …" scripts/bundle-mac.sh
#   NOTARY_KEY_PATH=AuthKey.p8 NOTARY_KEY_ID=… NOTARY_ISSUER_ID=…   # App Store Connect API key
#   ARBOS_NOTARY_PROFILE=arbos-notary                       # or a notarytool keychain profile
#   ARBOS_TARGETS=aarch64-apple-darwin scripts/bundle-mac.sh   # single arch (faster)
#
# The in-app updater (crates/arbos-update) verifies the tarball's Ed25519
# signature from the appcast, not Apple's; `arbos-updatectl appcast` signs it.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "bundle-mac.sh: macOS only (lipo, codesign, notarytool)." >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"

APP_NAME="Arbos"
BIN="arbos-mac"
BASE="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD="${ARBOS_BUILD:-$(git rev-list --count HEAD 2>/dev/null || echo 1)}"
if [[ ! "$BUILD" =~ ^[0-9]+$ ]]; then
  echo "ARBOS_BUILD must be a decimal build number, got '$BUILD'" >&2
  exit 1
fi
# Edge builds are `<base>.<build>`; a stable (tag) build is just `<base>`, matching the tag,
# the appcast item, and what publish-release.sh looks for.
if [[ "${ARBOS_DEFAULT_UPDATE_CHANNEL:-edge}" == "stable" ]]; then
  VERSION="$BASE"
else
  VERSION="$BASE.$BUILD"
fi
TARGETS="${ARBOS_TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}"
ARCH_LABEL="universal"
if [[ "$TARGETS" != *" "* ]]; then
  case "$TARGETS" in
    aarch64-*) ARCH_LABEL="arm64" ;;
    x86_64-*) ARCH_LABEL="x86_64" ;;
  esac
fi
DIST="${ARBOS_DIST:-$ROOT/dist}"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION-macos-$ARCH_LABEL.dmg"
TARBALL="$DIST/$APP_NAME-$VERSION-macos-$ARCH_LABEL.tar.gz"

echo "==> building $BIN $VERSION for: $TARGETS"
export ARBOS_BUILD="$BUILD"   # compiled in: update::current_version()
export ARBOS_DEFAULT_UPDATE_CHANNEL="${ARBOS_DEFAULT_UPDATE_CHANNEL:-edge}"   # compiled in: update::feed_url()
BINS=()
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
for t in $TARGETS; do
  rustup target add "$t" >/dev/null
  cargo build --release --locked -p "$BIN" --target "$t"
  BINS+=("$TARGET_DIR/$t/release/$BIN")
done

mkdir -p "$DIST"
rm -rf "$APP" "$DMG" "$TARBALL"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [[ ${#BINS[@]} -gt 1 ]]; then
  lipo -create "${BINS[@]}" -output "$APP/Contents/MacOS/$BIN"
else
  cp "${BINS[0]}" "$APP/Contents/MacOS/$BIN"
fi
chmod 755 "$APP/Contents/MacOS/$BIN"
# CFBundleShortVersionString is the Cargo version; CFBundleVersion the build
# number; the binary reports both as 0.1.0.<build> (the appcast's version).
sed -e "s/__VERSION__/$BASE/" -e "s/__BUILD__/$BUILD/" \
  "crates/arbos-mac/macos/Info.plist" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
if [[ -f crates/arbos-mac/macos/Arbos.icns ]]; then
  cp crates/arbos-mac/macos/Arbos.icns "$APP/Contents/Resources/"
fi
# The binary embeds the Inter subsets (notification avatars, chart PNGs). OFL 1.1 clause 2
# asks every distributed copy to carry the licence, so the app bundle gets its own copy
# (crates/arbos-core/assets/README.md).
cp crates/arbos-core/assets/Inter-LICENSE.txt "$APP/Contents/Resources/"
echo "==> $APP"

SIGNING="ad-hoc"
if [[ -n "${ARBOS_SIGN_IDENTITY:-}" ]]; then
  echo "==> signing with: $ARBOS_SIGN_IDENTITY"
  # Hardened runtime + secure timestamp: both are notarization requirements.
  # One executable, so no --deep (Apple deprecated it).
  codesign --force --options runtime --timestamp \
    --entitlements crates/arbos-mac/macos/entitlements.plist \
    --sign "$ARBOS_SIGN_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
  case "$ARBOS_SIGN_IDENTITY" in
    "Developer ID Application:"*) SIGNING="developer-id" ;;
    *) SIGNING="other" ;;
  esac
else
  echo "==> no ARBOS_SIGN_IDENTITY: ad-hoc signing (not distributable)"
  codesign --force --sign - "$APP"
fi

# notarize <path>: submit and wait; read the verdict from the JSON because a
# rejection can still exit 0. Anything but Accepted fails the build.
NOTARY_ARGS=()
if [[ -n "${NOTARY_KEY_PATH:-}" && -n "${NOTARY_KEY_ID:-}" && -n "${NOTARY_ISSUER_ID:-}" ]]; then
  NOTARY_ARGS=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
elif [[ -n "${ARBOS_NOTARY_PROFILE:-}" ]]; then
  NOTARY_ARGS=(--keychain-profile "$ARBOS_NOTARY_PROFILE")
fi
NOTARIZE=false
if [[ ${#NOTARY_ARGS[@]} -gt 0 && "$SIGNING" == "developer-id" ]]; then
  NOTARIZE=true
elif [[ ${#NOTARY_ARGS[@]} -gt 0 ]]; then
  echo "==> notary credentials given but the identity is not a Developer ID Application certificate; Apple would refuse, skipping notarization"
else
  echo "==> no notary credentials (NOTARY_KEY_PATH/ID/ISSUER_ID or ARBOS_NOTARY_PROFILE): notarization skipped"
fi
NOTARY_SUBMISSION_ID=""
notarize() {
  local result status
  result="$(xcrun notarytool submit "$1" "${NOTARY_ARGS[@]}" --wait --output-format json)"
  printf '%s\n' "$result" >&2
  NOTARY_SUBMISSION_ID="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
  status="$(printf '%s' "$result" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')"
  if [[ "$status" != "Accepted" ]]; then
    echo "notarization of $(basename "$1") ended with status '${status:-unknown}'" >&2
    if [[ -n "$NOTARY_SUBMISSION_ID" ]]; then
      xcrun notarytool log "$NOTARY_SUBMISSION_ID" "${NOTARY_ARGS[@]}" "$DIST/notarization-$(basename "$1").log" >&2 || true
    fi
    return 1
  fi
}
# staple <path>: the ticket fetch goes to Apple's CDN and now and then drops
# (stapler exit 68). Eight attempts over ~5 minutes; then ship unstapled
# (Gatekeeper fetches the ticket online) and say so.
STAPLED=true
staple() {
  local attempt pause
  for attempt in 1 2 3 4 5 6 7 8; do
    if xcrun stapler staple "$1"; then
      return 0
    fi
    [[ $attempt -lt 8 ]] || break
    case $attempt in 1) pause=10 ;; 2) pause=20 ;; 3) pause=40 ;; *) pause=60 ;; esac
    echo "stapler attempt $attempt failed; retrying in ${pause}s" >&2
    sleep "$pause"
  done
  echo "::warning title=Not stapled::Apple accepted $(basename "$1") (submission ${NOTARY_SUBMISSION_ID:-unknown}) but the ticket could not be fetched; shipping unstapled." >&2
  STAPLED=false
  if [[ -n "$NOTARY_SUBMISSION_ID" ]]; then
    xcrun notarytool log "$NOTARY_SUBMISSION_ID" "${NOTARY_ARGS[@]}" "$DIST/notarization-$(basename "$1").log" >&2 || true
  fi
  return 0
}

if $NOTARIZE; then
  # Staple the bundle BEFORE tarring it: the updater swaps the .app with no
  # dmg involved, so the tarball copy must carry its own ticket.
  ZIP="$DIST/arbos-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  echo "==> notarizing the app"
  notarize "$ZIP"
  rm -f "$ZIP"
  staple "$APP"
fi

echo "==> $TARBALL"
tar -czf "$TARBALL" -C "$DIST" "$APP_NAME.app"

echo "==> $DMG"
DMG_SRC="$DIST/dmg-src"
rm -rf "$DMG_SRC" && mkdir -p "$DMG_SRC/.background"
ditto "$APP" "$DMG_SRC/$APP_NAME.app"
ln -s /Applications "$DMG_SRC/Applications"
cp crates/arbos-mac/macos/dmg-background.png "$DMG_SRC/.background/background.png"
cp crates/arbos-mac/macos/dmg-background@2x.png "$DMG_SRC/.background/background@2x.png"
# The window layout (dark background, 128 px icons, app left / Applications
# right, 660x400) is a prebuilt .DS_Store generated once with the `ds_store`
# + `mac_alias` Python libraries (see crates/arbos-mac/macos/README-dmg.md),
# so no Finder session or Apple Events are needed on the build machine. The
# background alias inside it points at /Volumes/Arbos/.background/background.png,
# which is why the volume name must stay "Arbos".
cp crates/arbos-mac/macos/dmg.DS_Store "$DMG_SRC/.DS_Store"
MOUNT_DIR="/Volumes/$APP_NAME"
# Nothing here mounts the image, but a busy volume of the same name (someone running the
# app from a mounted DMG on this machine) would make any later verification step lay out
# the wrong volume; refuse rather than guess.
if mount | grep -q " on $MOUNT_DIR "; then
  echo "error: $MOUNT_DIR is mounted and busy; eject it before building the DMG" >&2
  exit 1
fi
hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$DMG_SRC" -ov -format UDZO -fs HFS+ \
  -imagekey zlib-level=9 "$DMG"
rm -rf "$DMG_SRC"
if [[ "$SIGNING" != "ad-hoc" ]]; then
  codesign --force --timestamp --sign "$ARBOS_SIGN_IDENTITY" "$DMG"
fi
if $NOTARIZE; then
  echo "==> notarizing the dmg"
  notarize "$DMG"
  staple "$DMG"
fi

# A summary other scripts read (scripts/release/local-release.sh, the workflow).
cat > "$DIST/build-info.env" <<EOF
ARBOS_VERSION=$VERSION
ARBOS_BASE_VERSION=$BASE
ARBOS_BUILD=$BUILD
ARBOS_ARCH=$ARCH_LABEL
ARBOS_SIGNING=$SIGNING
ARBOS_NOTARIZED=$NOTARIZE
ARBOS_STAPLED=$STAPLED
ARBOS_DMG=$DMG
ARBOS_TARBALL=$TARBALL
EOF
echo "done: version $VERSION, signing $SIGNING, notarized $NOTARIZE, stapled $STAPLED"
