#!/bin/bash
# clang21.sh — the clang gate 5 needs on Linux, installed the way CI installs it.
#
#   clang21.sh check              say whether a usable clang 21 is already here; exit 1 if not
#   clang21.sh pin [ci.yml]       print the release and sha256 CI pins, read out of the workflow
#   clang21.sh install [ci.yml]   fetch, verify and unpack that exact release
#   clang21.sh env                print the CC/CXX a build must be run with
#
# WHY THIS EXISTS. `cargo test --workspace --no-run` fails on this repo under the clang most Linux
# images ship. The message is a version guard inside webrtc-sys' own build.rs, not a compile error:
#
#   clang++ is clang 18, but the hermetic libc++ shipped with this libwebrtc requires clang 21 or
#   later. Install a newer clang and point CC/CXX at it [...]
#
# arbos-mac depends on webrtc-sys through libwebrtc, so every Mac-touching pull request hits this at
# gate 5 and there is no crate to scope around it. On 2026-11-06 a lane worker met the wall on #2873,
# spent ~25 minutes on it, and wrote this so the next one does not spend them again.
#
# WHY THE PIN IS READ FROM CI AND NOT COPIED INTO THIS FILE. Gate 5 asks whether the code compiles
# with the compiler the project ships. A kit that hardcoded 21.1.8 would keep passing after CI moved,
# and gate 5 would quietly become a check against a compiler nobody ships — the same family of fault
# as a test that passes for a reason unrelated to what it names. So the release and the digest are
# parsed out of .github/workflows/ci.yml at run time, and a workflow this cannot parse REFUSES rather
# than falls back to a remembered value. Drift then shows up as a refusal, which someone reads.
#
# WHAT THE CHECKSUM PROVES, EXACTLY. LLVM_SHA256 in ci.yml is, per the comment beside it, the GitHub
# release API's own `assets[].digest` for that file — an independent statement of what the asset
# should be, not a hash someone computed from a download they already trusted. Verifying against it
# is the step worth not skipping: a digest taken from your own download only proves the file did not
# change afterwards. It is still not a signature, and it attests the asset, not the LLVM project's
# intent.
set -u

LLVM_PREFIX_DIR=/usr/lib/llvm-21
WANT_MAJOR=21

# The workflow is the source of truth for the pin, so locate it from the repo this script sits in
# rather than from the caller's cwd, which during a merge is a worktree somewhere else.
default_ci_yml() {
  local here
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  printf '%s/.github/workflows/ci.yml' "$(cd "$here/../.." && pwd)"
}

# read_pin <ci.yml>: echo "<release> <sha256>", or fail with the reason on stderr.
#
# Each key must appear exactly once. Zero means the step was renamed or removed and this script is
# now guessing; more than one means a second job pins a different build and picking either silently
# would be a coin toss. Both refuse.
read_pin() {
  local yml=$1 rel sha nrel nsha
  [ -r "$yml" ] || { echo "clang21: cannot read $yml, so the pin is unknown." >&2; return 2; }
  nrel=$(grep -cE '^\s*LLVM_RELEASE:\s*[0-9]' "$yml")
  nsha=$(grep -cE '^\s*LLVM_SHA256:\s*[0-9a-f]{64}\s*$' "$yml")
  if [ "$nrel" != 1 ] || [ "$nsha" != 1 ]; then
    echo "clang21: REFUSED — $yml has $nrel LLVM_RELEASE and $nsha LLVM_SHA256 lines; expected one of each." >&2
    echo "  Not falling back to a remembered pin: a kit that installs a compiler CI no longer ships" >&2
    echo "  turns gate 5 into a check against a toolchain nobody runs. Fix the parse, or the workflow." >&2
    return 1
  fi
  rel=$(sed -nE 's/^\s*LLVM_RELEASE:\s*([0-9][0-9.]*)\s*$/\1/p' "$yml")
  sha=$(sed -nE 's/^\s*LLVM_SHA256:\s*([0-9a-f]{64})\s*$/\1/p' "$yml")
  [ -n "$rel" ] && [ -n "$sha" ] || { echo "clang21: REFUSED — matched the keys but could not extract values from $yml." >&2; return 1; }
  if [ "${rel%%.*}" != "$WANT_MAJOR" ]; then
    echo "clang21: REFUSED — $yml pins LLVM $rel, major ${rel%%.*}, but this script installs and verifies major $WANT_MAJOR." >&2
    echo "  CI has moved to a different major. Update WANT_MAJOR here deliberately rather than let" >&2
    echo "  this install a compiler it does not check." >&2
    return 1
  fi
  printf '%s %s\n' "$rel" "$sha"
}

# major_of <clang++ path>: what the compiler says it is, asked the way webrtc-sys' build.rs asks.
#
# build.rs decides with `-dM -E` on an empty translation unit, so ask the identical question. A
# toolchain that cannot answer it should fail here, where the log says which fetch produced which
# compiler, rather than a hundred lines into a libc++ header.
major_of() {
  "$1" -dM -E -x c++ /dev/null 2>/dev/null | awk '$2 == "__clang_major__" {print $3}'
}

clang_check() {
  local cxx=$LLVM_PREFIX_DIR/bin/clang++ got
  if [ ! -x "$cxx" ]; then
    echo "clang21: no clang++ at $cxx."
    local sys; sys=$(command -v clang++ 2>/dev/null)
    if [ -n "$sys" ]; then
      echo "  PATH has $sys reporting major $(major_of "$sys" | head -1), which webrtc-sys will reject if it is below $WANT_MAJOR."
    fi
    echo "  Install the pinned one with: $(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/clang21.sh install"
    return 1
  fi
  got=$(major_of "$cxx")
  if [ "$got" != "$WANT_MAJOR" ]; then
    echo "clang21: $cxx reports __clang_major__ ${got:-unreadable}, not $WANT_MAJOR — not usable for gate 5."
    return 1
  fi
  echo "    clang $("$cxx" --version | sed -n '1s/.*clang version \([0-9.]*\).*/\1/p') at $cxx"
  return 0
}

clang_env() {
  printf 'CC=%s/bin/clang\n' "$LLVM_PREFIX_DIR"
  printf 'CXX=%s/bin/clang++\n' "$LLVM_PREFIX_DIR"
}

clang_install() {
  local yml=${1:-$(default_ci_yml)} pin rel sha prefix url tmp got rc=0 attempt
  pin=$(read_pin "$yml") || return $?
  rel=${pin% *}; sha=${pin#* }
  prefix="LLVM-${rel}-Linux-X64"
  url="https://github.com/llvm/llvm-project/releases/download/llvmorg-${rel}/${prefix}.tar.xz"
  echo "==> LLVM $rel, pinned by $yml"

  if clang_check >/dev/null 2>&1; then
    echo "    already installed:"; clang_check; return 0
  fi

  tmp=$(mktemp -d)
  # Three attempts with the same backoff CI uses. The asset is ~2 GB, so a transport failure
  # part-way through is the ordinary case rather than the exotic one.
  for attempt in 1 2 3; do
    rc=0
    curl -fsSL --connect-timeout 20 --max-time 1800 -o "$tmp/llvm.tar.xz" "$url" || rc=$?
    [ "$rc" -eq 0 ] && break
    echo "    attempt $attempt of 3 failed with curl exit $rc"
    [ "$attempt" -lt 3 ] && sleep "$((attempt * 10))"
  done
  if [ "$rc" -ne 0 ]; then
    echo "clang21: could not fetch $url in three attempts (curl exit $rc)." >&2
    echo "  curl 22 means the pin names an asset that is not there; a network code means github.com" >&2
    echo "  is unreachable from this host, which would have broken the checkout too." >&2
    rm -rf "$tmp"; return 1
  fi

  got=$(sha256sum "$tmp/llvm.tar.xz" | cut -d' ' -f1)
  if [ "$got" != "$sha" ]; then
    echo "clang21: REFUSED to unpack — sha256 is $got, not the $sha pinned in $yml." >&2
    echo "  Nothing was installed. See the provenance note at the top of this file for what this" >&2
    echo "  digest does and does not prove before considering an override." >&2
    rm -rf "$tmp"; return 1
  fi
  echo "    sha256 matches the digest CI pins"

  # The same extract list as CI: the driver, the resource directory and libclang, and nothing else.
  # The hermetic libc++ comes from libwebrtc itself, so unpacking the whole 25 GB toolchain would
  # cost disk for files no build here reads.
  sudo mkdir -p "$LLVM_PREFIX_DIR"
  sudo tar -I 'xz -T0 -d' -xf "$tmp/llvm.tar.xz" --strip-components=1 \
    -C "$LLVM_PREFIX_DIR" --wildcards \
    "${prefix}/bin/clang" \
    "${prefix}/bin/clang++" \
    "${prefix}/bin/clang-${WANT_MAJOR}" \
    "${prefix}/lib/clang/${WANT_MAJOR}" \
    "${prefix}/lib/libclang.so*" || { echo "clang21: unpack failed." >&2; rm -rf "$tmp"; return 1; }
  rm -rf "$tmp"

  got=$(major_of "$LLVM_PREFIX_DIR/bin/clang++")
  if [ "$got" != "$WANT_MAJOR" ]; then
    echo "clang21: $prefix unpacked to a compiler reporting __clang_major__ ${got:-unreadable}, not $WANT_MAJOR." >&2
    return 1
  fi
  echo "    $("$LLVM_PREFIX_DIR/bin/clang++" --version | head -1), from $prefix"
  echo
  echo "Gate 5 must then be run with CI's compiler, not the host's:"
  echo "    env \$($(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/clang21.sh env | tr '\n' ' ') cargo test --workspace --locked --no-run"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  case "${1:-check}" in
    check)   clang_check ;;
    pin)     read_pin "${2:-$(default_ci_yml)}" ;;
    install) clang_install "${2:-}" ;;
    env)     clang_env ;;
    *) echo "usage: clang21.sh <check|pin|install|env> [ci.yml]"; exit 2 ;;
  esac
fi
