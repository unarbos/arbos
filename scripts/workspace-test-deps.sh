#!/usr/bin/env bash
# System dependencies for `cargo test --workspace` on a bare Linux box. One-time cost ~13 min.
#
# Fontconfig is NOT the blocker, and that is the belief this script replaces. The received
# wisdom is that the workspace suite cannot run on a cloud VM "because of missing system
# fontconfig", and that the way round it is to test one crate at a time with `-p <crate>`. Both
# halves are wrong:
#
#   * fontconfig is one of TEN apt packages arbos-mac/GPUI needs. On its own it fixes nothing.
#   * THE BLOCKER IS CLANG. `webrtc-sys` ships a hermetic libc++ that needs clang >= 21, and
#     Ubuntu 24.04 carries clang 18 with no apt path to 21. Nothing to do with fonts.
#
# Install both and the workspace suite runs and passes. So `-p <crate>` is a workaround for a
# solved problem; don't verify a cross-crate change through a single-crate run because of it.
#
# This is what CI already does — the `Linux libraries for arbos-mac (GPUI)` and
# `clang 21 for webrtc-sys` steps of .github/workflows/ci.yml. Keep the LLVM pin below in step
# with that workflow.
#
#   scripts/workspace-test-deps.sh                                 # install, verify, print the exports
#   scripts/workspace-test-deps.sh cargo test --workspace --locked # install, then run that with CC/CXX set
#   scripts/workspace-test-deps.sh --ci                            # install, then run CI's three rust steps
#
# Idempotent: re-running skips the LLVM download when clang 21 is already unpacked.
set -euo pipefail

# github.com/llvm/llvm-project/releases/tag/llvmorg-21.1.8, the last 21.x. The digest is the
# release API's own `assets[].digest` for this asset, not one computed from a download, so it is
# an independent statement of what the file should be.
LLVM_RELEASE="${LLVM_RELEASE:-21.1.8}"
LLVM_SHA256="${LLVM_SHA256:-b3b7f2801d15d50736acea3c73982994d025b01c2f035b91ae3b49d1b575732b}"
MAJOR="${LLVM_RELEASE%%.*}"
LLVM_DIR="/usr/lib/llvm-${MAJOR}"

# What webrtc-sys requires, as a floor. Deliberately NOT derived from LLVM_RELEASE: a check
# against the version you asked for passes whatever you asked for, so `LLVM_RELEASE=18.0.0`
# would sail through and hand the build a clang the hermetic libc++ rejects — the very failure
# this script exists to catch early.
WEBRTC_MIN_CLANG=21

# GPUI's X11/Wayland/audio/GL/font stack. fontconfig is in here and is not the interesting one.
GPUI_PKGS=(
  libxkbcommon-dev
  libxkbcommon-x11-dev
  libwayland-dev
  libxcb-xkb-dev
  libasound2-dev
  libssl-dev
  libgl1-mesa-dev
  libfontconfig-dev
  libstdc++-14-dev
  libglib2.0-dev
)

say() { printf '\n==> %s\n' "$*"; }
die() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  command -v sudo >/dev/null 2>&1 || die "not root and no sudo; re-run as root"
  SUDO="sudo"
fi

# Ask the compiler the same question webrtc-sys's build.rs asks (`-dM -E` over an empty
# translation unit), so a disagreement surfaces here rather than inside a libc++ header.
clang_major() {
  local cxx="$1"
  [ -x "$cxx" ] || return 1
  "$cxx" -dM -E -x c++ /dev/null 2>/dev/null |
    awk '$2 == "__clang_major__" { print $3 }' | head -1
}

# ---- 1. the GPUI packages ----------------------------------------------------------------------

say "apt packages for arbos-mac/GPUI"
$SUDO apt-get update -qq
$SUDO apt-get install -y -qq "${GPUI_PKGS[@]}"

# ---- 2. clang 21 for webrtc-sys, which is the part that actually blocks the build ---------------

if [ "$(clang_major "${LLVM_DIR}/bin/clang++" || true)" = "$MAJOR" ]; then
  say "clang ${MAJOR} already at ${LLVM_DIR}, skipping the download"
else
  prefix="LLVM-${LLVM_RELEASE}-Linux-X64"
  url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_RELEASE}/${prefix}.tar.xz"
  say "downloading ${prefix} (~1 GB; most of the one-time cost)"

  rc=0
  for attempt in 1 2 3; do
    rc=0
    curl -fsSL --connect-timeout 20 --max-time 900 -o /tmp/llvm.tar.xz "$url" || rc=$?
    if [ "$rc" -eq 0 ]; then
      break
    fi
    echo "attempt ${attempt} of 3 failed with curl exit ${rc}"
    if [ "$attempt" -lt 3 ]; then
      sleep "$((attempt * 10))"
    fi
  done
  # curl 22 means the pin names an asset that is not there — check the release page. Any other
  # code means github.com is unreachable from this host.
  [ "$rc" -eq 0 ] || die "could not download ${url} in three attempts (curl exit ${rc})"

  echo "${LLVM_SHA256}  /tmp/llvm.tar.xz" | sha256sum -c -

  say "unpacking to ${LLVM_DIR}"
  $SUDO mkdir -p "$LLVM_DIR"
  $SUDO tar -I 'xz -T0 -d' -xf /tmp/llvm.tar.xz --strip-components=1 \
    -C "$LLVM_DIR" --wildcards \
    "${prefix}/bin/clang" \
    "${prefix}/bin/clang++" \
    "${prefix}/bin/clang-${MAJOR}" \
    "${prefix}/lib/clang/${MAJOR}" \
    "${prefix}/lib/libclang.so*"
  rm -f /tmp/llvm.tar.xz
fi

# ---- 3. fail here, loudly, rather than 600 lines into a libc++ header --------------------------

CC="${LLVM_DIR}/bin/clang"
CXX="${LLVM_DIR}/bin/clang++"
export CC CXX

got="$(clang_major "$CXX" || true)"
if [ -z "$got" ]; then
  die "${CXX} cannot answer __clang_major__, so it is not a usable C++ compiler.
     Remove ${LLVM_DIR} and re-run to download it again."
fi
if [ "$got" -lt "$WEBRTC_MIN_CLANG" ]; then
  die "${CXX} reports clang ${got}, but webrtc-sys needs ${WEBRTC_MIN_CLANG} or later.
     Left alone, the build fails ~600 lines into webrtc-sys-*/build.rs with
     'the hermetic libc++ shipped with this libwebrtc requires clang 21 or later' —
     the real cause that gets mistaken for the fontconfig story.
     LLVM_RELEASE is ${LLVM_RELEASE}; point it at a ${WEBRTC_MIN_CLANG}.x or newer release,
     or remove ${LLVM_DIR} and re-run to download the pinned default."
fi
say "clang ${got} at ${CXX} — webrtc-sys will build"

# ---- 4. hand off --------------------------------------------------------------------------------

if [ "${1:-}" = "--ci" ]; then
  say "CI's three rust steps, in CI's order"
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  exit 0
fi

if [ "$#" -gt 0 ]; then
  say "running: $*"
  exec "$@"
fi

cat <<EOF

Done. A script cannot set these in your shell, so copy them:

  export CC=${CC}
  export CXX=${CXX}

Then:

  cargo test --workspace --locked
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo fmt --all --check

Or let this script hold them for you:

  scripts/workspace-test-deps.sh --ci                     # all three, in CI's order
  scripts/workspace-test-deps.sh cargo test -p arbos-agentd

EOF
