#!/usr/bin/env bash
# Build arbos-ffi for Apple device + simulator, generate Swift bindings, pack one
# xcframework, and install into packages/ArbosFFI for the local Swift package.
# Run on a Mac with Xcode and the iOS SDK.
#
# Usage (from repo root):
#   ./scripts/build-xcframework.sh
#   OUT_DIR=dist/arbos-ffi ./scripts/build-xcframework.sh
#
# Versioning: ship the xcframework under the same release tag as the Mac build
# (workspace package version). See docs/crdt-sync.md § "arbos-ffi / xcframework".
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Where cargo writes: honour CARGO_TARGET_DIR. The gate exports it (rust-gate.sh builds into
# ~/arbos-rust-gate-target) and this script read $ROOT/target regardless, so every FFI change
# went through the gate with a stale libarbos_ffi.a and stale Swift bindings (2026-10-23,
# #1540: "RoomAction has no member healCheck" against a lib built at 19:13).
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT_DIR="${OUT_DIR:-$TARGET_DIR/xcframework}"
PKG_DIR="$ROOT/packages/ArbosFFI"
FFI_CRATE=arbos-ffi
LIB_NAME=arbos_ffi
# Keep lockstep with workspace.dependencies.uniffi (=0.32.2).
UNIFFI_VERSION="$(
  python3 - <<'PY'
import pathlib, re
text = pathlib.Path("Cargo.toml").read_text()
m = re.search(r'^uniffi\s*=\s*"=?([^"]+)"', text, re.M)
print(m.group(1) if m else "0.32.2")
PY
)"

DEVICE_TARGET=aarch64-apple-ios
SIM_TARGET=aarch64-apple-ios-sim
# Gate / generic simulator destinations include x86_64; the sim slice ships both.
SIM_X86_TARGET=x86_64-apple-ios

echo "==> rustup targets (${DEVICE_TARGET}, ${SIM_TARGET}, ${SIM_X86_TARGET})"
rustup target add "$DEVICE_TARGET" "$SIM_TARGET" "$SIM_X86_TARGET"

echo "==> uniffi-bindgen ${UNIFFI_VERSION}"
# UNIFFI_TOOLS_ROOT: a CI cache keeps the binary outside target/, which rust-cache prunes.
TOOLS_ROOT="${UNIFFI_TOOLS_ROOT:-$TARGET_DIR/uniffi-tools}"
BINDGEN="$TOOLS_ROOT/bin/uniffi-bindgen"
if [[ ! -x "$BINDGEN" ]] || ! "$BINDGEN" --version 2>/dev/null | grep -q "$UNIFFI_VERSION"; then
  cargo install uniffi --version "$UNIFFI_VERSION" --features cli \
    --root "$TOOLS_ROOT" --locked --force
fi

echo "==> cargo build -p ${FFI_CRATE} (device + simulator arm64 + simulator x86_64)"
cargo build -p "$FFI_CRATE" --release --locked --target "$DEVICE_TARGET"
cargo build -p "$FFI_CRATE" --release --locked --target "$SIM_TARGET"
cargo build -p "$FFI_CRATE" --release --locked --target "$SIM_X86_TARGET"

DEVICE_LIB="$TARGET_DIR/${DEVICE_TARGET}/release/lib${LIB_NAME}.a"
SIM_ARM_LIB="$TARGET_DIR/${SIM_TARGET}/release/lib${LIB_NAME}.a"
SIM_X86_LIB="$TARGET_DIR/${SIM_X86_TARGET}/release/lib${LIB_NAME}.a"
test -f "$DEVICE_LIB"
test -f "$SIM_ARM_LIB"
test -f "$SIM_X86_LIB"

# Name must stay lib${LIB_NAME}.a: xcodebuild -create-xcframework keeps the
# basename inside the xcframework, and the iOS gate looks for that file.
UNIVERSAL_SIM_DIR="$TARGET_DIR/arbos-ffi-ios-sim-universal"
rm -rf "$UNIVERSAL_SIM_DIR"
mkdir -p "$UNIVERSAL_SIM_DIR"
UNIVERSAL_SIM_LIB="$UNIVERSAL_SIM_DIR/lib${LIB_NAME}.a"
echo "==> lipo simulator (${SIM_TARGET} + ${SIM_X86_TARGET})"
lipo -create "$SIM_ARM_LIB" "$SIM_X86_LIB" -output "$UNIVERSAL_SIM_LIB"
SIM_ARCHS="$(lipo -archs "$UNIVERSAL_SIM_LIB")"
case " $SIM_ARCHS " in
  *" arm64 "*) ;;
  *)
    echo "error: universal simulator library must include arm64 (lipo -archs: ${SIM_ARCHS})" >&2
    exit 1
    ;;
esac
case " $SIM_ARCHS " in
  *" x86_64 "*) ;;
  *)
    echo "error: universal simulator library must include x86_64 (lipo -archs: ${SIM_ARCHS})" >&2
    exit 1
    ;;
esac
DEVICE_ARCHS="$(lipo -archs "$DEVICE_LIB")"
if [[ "$DEVICE_ARCHS" != "arm64" ]]; then
  echo "error: device library must be only arm64 (lipo -archs: ${DEVICE_ARCHS})" >&2
  exit 1
fi
SIM_LIB="$UNIVERSAL_SIM_LIB"

GEN_DIR="$OUT_DIR/generated"
rm -rf "$OUT_DIR"
mkdir -p "$GEN_DIR"

echo "==> uniffi-bindgen generate (Swift)"
# Prefer the simulator staticlib for metadata extraction on Apple Silicon Macs.
"$BINDGEN" generate --library "$SIM_ARM_LIB" --language swift --out-dir "$GEN_DIR"

SWIFT_OUT="$GEN_DIR/${LIB_NAME}.swift"
# UniFFI may emit <module>.swift or arbos_ffi.swift depending on namespace.
if [[ ! -f "$SWIFT_OUT" ]]; then
  SWIFT_OUT="$(find "$GEN_DIR" -maxdepth 1 -name '*.swift' | head -n1)"
fi
test -n "$SWIFT_OUT" && test -f "$SWIFT_OUT"

HEADERS_DIR="$OUT_DIR/headers"
mkdir -p "$HEADERS_DIR"
# Modulemap + umbrella header from bindgen sit next to the .swift file.
cp "$GEN_DIR"/*.h "$HEADERS_DIR/" 2>/dev/null || true
cp "$GEN_DIR"/*.modulemap "$HEADERS_DIR/" 2>/dev/null || true
if [[ ! -f "$HEADERS_DIR/module.modulemap" ]]; then
  # Some UniFFI versions name it ${LIB_NAME}.modulemap
  for mm in "$HEADERS_DIR"/*.modulemap; do
    [[ -f "$mm" ]] || continue
    cp "$mm" "$HEADERS_DIR/module.modulemap"
    break
  done
fi

XC_DIR="$OUT_DIR/ArbosFFI.xcframework"
echo "==> xcodebuild -create-xcframework"
rm -rf "$XC_DIR"
xcodebuild -create-xcframework \
  -library "$DEVICE_LIB" -headers "$HEADERS_DIR" \
  -library "$SIM_LIB" -headers "$HEADERS_DIR" \
  -output "$XC_DIR"

cp "$SWIFT_OUT" "$OUT_DIR/ArbosFFI.swift"

# Install into the local Swift package (gitignored products).
echo "==> install packages/ArbosFFI"
mkdir -p "$PKG_DIR/Sources/ArbosFFI"
rm -rf "$PKG_DIR/ArbosFFIRust.xcframework"
cp -R "$XC_DIR" "$PKG_DIR/ArbosFFIRust.xcframework"
cp "$OUT_DIR/ArbosFFI.swift" "$PKG_DIR/Sources/ArbosFFI/ArbosFFI.swift"

# Rough size note for docs / release notes.
DEVICE_SIZE=$(wc -c <"$DEVICE_LIB" | tr -d ' ')
echo "==> done"
echo "    xcframework: $PKG_DIR/ArbosFFIRust.xcframework"
echo "    swift:       $PKG_DIR/Sources/ArbosFFI/ArbosFFI.swift"
echo "    device .a:   ${DEVICE_SIZE} bytes (~$((DEVICE_SIZE / 1024 / 1024)) MiB before strip/link)"
echo "    sim arches:  $(lipo -archs "$SIM_LIB")"
