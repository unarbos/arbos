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

# The pin is READ OUT OF ci.yml, not copied here. This script used to carry its own copy of the
# release and digest under a comment asking a human to keep it in step with the workflow. That is
# a value which is right today and silently wrong the day CI bumps — and the failure is quiet in
# the worst direction: the script keeps installing a compiler the project no longer builds with,
# everything still compiles, and "it builds locally" stops meaning "it builds the way CI builds".
#
# So there is one assertion of the pin in this repository, in the workflow that actually runs it,
# and this derives from it. A ci.yml this cannot parse is an ERROR rather than a fallback to a
# remembered value: zero pins and two pins both refuse. Drift becomes a refusal somebody reads.
#
# The digest in that workflow is the release API's own `assets[].digest` for the asset, not one
# computed from a download, so it is an independent statement of what the file should be. It is
# not a signature, and it attests the asset rather than the LLVM project's intent.
#
# LLVM_RELEASE / LLVM_SHA256 in the environment still win, so a one-off against another release
# needs no edit. Both must then be set together: a release with somebody else's digest fails the
# checksum, which is the right answer but a confusing way to arrive at it.
CI_YML="${CI_YML:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.github/workflows/ci.yml}"

say() { printf '\n==> %s\n' "$*"; }
die() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

read_pin_from_ci() {
  local key="$1" pat="$2" n
  [ -r "$CI_YML" ] || die "cannot read ${CI_YML}, so the LLVM pin is unknown.
     This script derives the pin from the workflow rather than keeping a second copy of it.
     Set ${key} in the environment to override, or point CI_YML at the workflow."
  n="$(grep -cE "^[[:space:]]*${key}:[[:space:]]*${pat}[[:space:]]*$" "$CI_YML")"
  [ "$n" = 1 ] || die "${CI_YML} has ${n} ${key} lines; expected exactly one.
     Zero means the 'clang 21 for webrtc-sys' step was renamed or removed and this script would
     be guessing. More than one means two jobs pin different builds, and picking either would be
     a coin toss. Not falling back to a remembered pin: a second copy of the pin is the thing
     this derivation exists to remove."
  sed -nE "s/^[[:space:]]*${key}:[[:space:]]*(${pat})[[:space:]]*$/\1/p" "$CI_YML"
}

LLVM_RELEASE="${LLVM_RELEASE:-$(read_pin_from_ci LLVM_RELEASE '[0-9][0-9.]*')}"
LLVM_SHA256="${LLVM_SHA256:-$(read_pin_from_ci LLVM_SHA256 '[0-9a-f]{64}')}"
# die() runs inside the command substitutions above, so its exit kills the subshell and not this
# script. Without this postcondition a failed read leaves the pin EMPTY and everything downstream
# carries on: the skip path reports success against a blank version, and the download path builds
# a URL with no release in it. An unset pin must stop the script, not decorate it.
[ -n "$LLVM_RELEASE" ] && [ -n "$LLVM_SHA256" ] \
  || die "the LLVM pin came back empty from ${CI_YML}, so nothing below can be trusted.
     Set LLVM_RELEASE and LLVM_SHA256 together in the environment to override."
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

# Look across the whole host before fetching ~2 GB, and judge what is found against the FLOOR.
#
# Two separate bugs are being avoided here, and they pull in opposite directions. Checking only
# ${LLVM_DIR} misses a usable toolchain installed anywhere else — on 2026-11-06 a worker
# downloaded the asset onto a host that already had a working clang at /usr/lib/llvm-21, merely
# not as the default compiler. And testing `= "$MAJOR"` rejects a host NEWER than the pin and
# sends it to fetch an older compiler it does not need; webrtc-sys asks for "21 or later", so
# equality is the wrong comparison in both directions.
#
# Highest major first, so a host carrying several toolchains gets the newest rather than whatever
# the glob listed first. The default compiler is considered last and on its merits: it is the one
# webrtc-sys already rejected on this repo, so it is the least likely to clear the floor.
find_usable_clang() {
  local c m
  for c in $(ls -d /usr/lib/llvm-*/bin/clang++ 2>/dev/null | sort -rV) "$(command -v clang++ 2>/dev/null)"; do
    [ -n "$c" ] && [ -x "$c" ] || continue
    m="$(clang_major "$c" || true)"
    case "$m" in ''|*[!0-9]*) continue ;; esac
    [ "$m" -ge "$WEBRTC_MIN_CLANG" ] && { printf '%s\n' "$c"; return 0; }
  done
  return 1
}

if found_cxx="$(find_usable_clang)"; then
  LLVM_DIR="$(cd "$(dirname "$found_cxx")/.." && pwd)"
  say "clang $(clang_major "$found_cxx") already at ${found_cxx}, skipping the download"
  # Clearing the floor is what lets the workspace build, and it is a weaker claim than matching
  # CI. Say so, so nobody reports a local build as having used the toolchain CI runs.
  found_ver="$("$found_cxx" --version 2>/dev/null | sed -n '1s/.*clang version \([0-9.]*\).*/\1/p')"
  if [ -n "$found_ver" ] && [ "$found_ver" != "$LLVM_RELEASE" ]; then
    say "note: CI pins ${LLVM_RELEASE}; this is ${found_ver}.
    It clears webrtc-sys' floor of ${WEBRTC_MIN_CLANG}, which is what the build needs, but it is
    not CI's build — don't report a compile here as having used CI's toolchain."
  fi
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
