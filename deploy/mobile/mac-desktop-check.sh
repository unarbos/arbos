#!/bin/bash
# Desktop checks on the AWS Mac for #265: (1) `make bundle` passes, and
# genuinely fails on a forced build-number mismatch; (2) the status bar's
# unreachable state, asserted on element ids through the driver, run in
# the console user's GUI session.
#   mac-desktop-check.sh <ref>      e.g. origin/main
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
source ~/.cargo/env
REF=${1:-origin/main}
OUT="$HOME/mobile-out/desktop-check"; mkdir -p "$OUT"
cd ~/arbos && git fetch -q origin && git checkout -q -B check "$REF" && git log --oneline -1 | tee "$OUT/sha.txt"
BUILD=$(git rev-list --count HEAD)

echo "== 1a. kernel + desktop release build, then make bundle (expect pass)"
cargo build --release -p arbos-kernel 2>&1 | tail -1
cd desktop
make bundle > "$OUT/bundle-pass.log" 2>&1; rc=$?
tail -4 "$OUT/bundle-pass.log"
APP=target/bundle.noindex/Arbos.app
stamped=$($APP/Contents/MacOS/Arbos --version 2>/dev/null | cut -d' ' -f2)
claimed=$(plutil -extract CFBundleVersion raw -o - $APP/Contents/Info.plist 2>/dev/null)
echo "RESULT bundle-pass rc=$rc stamped=$stamped claimed=$claimed expected=$BUILD"

echo "== 1b. force a mismatch: binary stamped 999, bundle claims $BUILD (expect fail)"
ARBOS_BUILD=999 cargo build --release 2>&1 | tail -1
# The drift the check exists for: a binary from a cached build that says one
# number while the plist says another. `-o build` keeps make from rebuilding
# (which would restamp the binary), so the 999 binary meets a plist of $BUILD.
make -o build bundle > "$OUT/bundle-mismatch.log" 2>&1; rc2=$?
grep -E "bundle:|error|make:" "$OUT/bundle-mismatch.log" | tail -4
echo "RESULT bundle-mismatch rc=$rc2 (nonzero = the check fired)"

echo "== 1c. back to normal (expect pass)"
make bundle > "$OUT/bundle-pass2.log" 2>&1; rc3=$?
echo "RESULT bundle-pass2 rc=$rc3 stamped=$($APP/Contents/MacOS/Arbos --version | cut -d' ' -f2)"

echo "== 2. status bar unreachable state (GUI session of arbosgui)"
chmod -R o+rX ~/arbos/desktop/target/release/arbos-desktop ~/arbos/desktop/driver 2>/dev/null
chmod o+x ~ ~/arbos ~/arbos/desktop ~/arbos/desktop/target ~/arbos/desktop/target/release 2>/dev/null
cp ~/mobile-out/desktop-check/bar_check.py /Users/Shared/bar_check.py 2>/dev/null
sudo launchctl asuser "$(id -u arbosgui)" sudo -u arbosgui env HOME=/Users/arbosgui \
  ARBOS_DESKTOP_BIN="$HOME/arbos/desktop/target/release/arbos-desktop" \
  ARBOS_UPDATE_CHANNEL=dev HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 \
  PYTHONPATH="$HOME/arbos/desktop/driver" \
  /usr/bin/python3 /Users/Shared/bar_check.py "$OUT" 2>&1 | tail -20
