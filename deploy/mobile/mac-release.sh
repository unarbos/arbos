#!/bin/bash
# Archive, sign for App Store Connect and upload to TestFlight from the Mac.
# The App Store Connect key sits at ~/.appstoreconnect/private_keys/ (0600)
# and the issuer id in ~/.asc-issuer (0600); neither is ever printed.
#   mac-release.sh <branch> [build-number]
set -uo pipefail
export PATH="/opt/homebrew/bin:$PATH"
BRANCH=${1:?branch}
REPO="$HOME/arbos"
KEY_ID=${ASC_KEY_ID:-ZV82D3ZWRT}
KEY="$HOME/.appstoreconnect/private_keys/AuthKey_$KEY_ID.p8"
ISSUER=$(cat "$HOME/.asc-issuer")
OUT="$HOME/mobile-out/release"; mkdir -p "$OUT"
cd "$REPO" && git fetch -q origin "$BRANCH" && git checkout -q -B "$BRANCH" "origin/$BRANCH"
SHA=$(git rev-parse --short HEAD)
BUILD=${2:-$(git rev-list --count HEAD)}
echo "== $BRANCH $SHA build $BUILD"
cd ios
[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ] && sh scripts/gen-secrets.sh >/dev/null 2>&1 && echo "== secrets baked"
ARCHIVE="$OUT/Arbos-$BUILD.xcarchive"
rm -rf "$ARCHIVE"
xcodebuild archive -project Arbos.xcodeproj -scheme Arbos -configuration Release \
  -destination "generic/platform=iOS" -archivePath "$ARCHIVE" \
  -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
  -authenticationKeyPath "$KEY" -authenticationKeyID "$KEY_ID" -authenticationKeyIssuerID "$ISSUER" \
  CURRENT_PROJECT_VERSION="$BUILD" CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO 2>&1 | tee "$OUT/archive-$BUILD.log" | grep -E "error:|warning: .*(sign|provision)|ARCHIVE (SUCCEEDED|FAILED)"
[ -d "$ARCHIVE" ] || { echo "no archive"; exit 1; }
xcodebuild -exportArchive -archivePath "$ARCHIVE" -exportOptionsPlist ExportOptions.plist \
  -exportPath "$OUT/export-$BUILD" -allowProvisioningUpdates \
  -authenticationKeyPath "$KEY" -authenticationKeyID "$KEY_ID" -authenticationKeyIssuerID "$ISSUER" \
  2>&1 | tee "$OUT/export-$BUILD.log" | grep -E "error:|Upload|EXPORT (SUCCEEDED|FAILED)|status" | head -20
echo "== done: build $BUILD ($SHA)"
