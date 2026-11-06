#!/usr/bin/env bash
# Selftest for workspace-test-deps.sh. No network, no sudo, no apt, ~0.1s.
#
# WHY THIS EXISTS. The script has a CONSUMER, and the consumer is not a person. The Cloud Agent
# environment build provisions clang by calling this script and then reading the compiler paths
# back out of the two `export` lines it prints at the end. It copies nothing else — not the URL,
# the digest, the extract list, the floor, nor even /usr/lib/llvm-21 — so those two lines are the
# whole interface, and an edit that reformats the trailer breaks a component in another repository
# with nothing here going red.
#
# That is #2890's lesson one layer down: a second reader depends on a value this produces, and the
# only thing standing between a later edit and a silent break is a test that names the dependency.
# So this asserts the handoff as a contract rather than trusting the trailer to stay put.
#
# The rest of the cases cover the parts that decide WHICH compiler a build gets: the pin must come
# from ci.yml or not at all, and the floor must accept at and above itself while refusing below.
set -u

D="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$D/workspace-test-deps.sh"
fail=0
ok()  { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; fail=1; }

[ -f "$SCRIPT" ] || { echo "no workspace-test-deps.sh beside this selftest"; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

# apt and sudo are stubbed so the real control flow runs without touching the host. Nothing here
# installs, downloads or elevates; a case that needed any of those would not belong in a lint job.
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' > "$T/bin/apt-get"; chmod +x "$T/bin/apt-get"
printf '#!/bin/sh\nexec "$@"\n' > "$T/bin/sudo";  chmod +x "$T/bin/sudo"

# A stub toolchain answers the one question the script asks of a compiler — build.rs's `-dM -E`
# over an empty translation unit — and carries both drivers, because the script requires the C
# driver beside the C++ one.
stub() {
  mkdir -p "$T/llvm-$1/bin"
  for drv in clang clang++; do
    { printf '#!/bin/sh\n'
      printf 'case "$1" in --version) echo "clang version %s.0.0"; exit 0;; esac\n' "$2"
      printf 'echo "#define __clang_major__ %s"\n' "$2"; } > "$T/llvm-$1/bin/$drv"
    chmod +x "$T/llvm-$1/bin/$drv"
  done
}

run() { PATH="$T/bin:$PATH" CLANG_SEARCH_GLOB="$T/llvm-*/bin/clang++" bash "$SCRIPT" 2>&1; }

echo "workspace-test-deps — the pin, the floor, and the handoff:"

# ---- the handoff the environment build reads -----------------------------------------------
# The assertion is about the COMPILERS, not the lines. Grepping for the two `export` lines would
# pass when the script prints a path to a compiler that does not exist — which is exactly the
# "provisioned the toolchain and then could not find it" failure this case is here to catch. A
# check that cannot fail for the reason it names is not a check, so the paths are run.
#
# The floor is written out here rather than read from the script. It is the constraint being
# guarded, and a guard derived from the thing it guards passes for the same reason its subject
# does. 21 is webrtc-sys' requirement — the external authority — not this repository's opinion.
FLOOR=21

stub 21 21
out="$(run)"
cc="$(printf '%s\n' "$out"  | sed -n 's/^  export CC=\(.*\)$/\1/p')"
cxx="$(printf '%s\n' "$out" | sed -n 's/^  export CXX=\(.*\)$/\1/p')"
if [ -z "$cc" ] || [ -z "$cxx" ]; then
  bad "the export trailer the environment build parses yielded no paths (CC='$cc' CXX='$cxx')"
elif [ ! -x "$cc" ] || [ ! -x "$cxx" ]; then
  bad "an exported path is not executable, so the handoff names a compiler that is not there: CC='$cc' CXX='$cxx'"
else
  ok "the handoff names two executables, not just two well-formed lines"
  ccmaj="$("$cc"   -dM -E -x c++ /dev/null 2>/dev/null | awk '$2 == "__clang_major__" { print $3 }')"
  cxxmaj="$("$cxx" -dM -E -x c++ /dev/null 2>/dev/null | awk '$2 == "__clang_major__" { print $3 }')"
  if [ -n "$ccmaj" ] && [ "$ccmaj" -ge "$FLOOR" ] 2>/dev/null \
  && [ -n "$cxxmaj" ] && [ "$cxxmaj" -ge "$FLOOR" ] 2>/dev/null; then
    ok "both exported compilers answer __clang_major__ at or above $FLOOR"
  else
    bad "an exported compiler does not clear the floor: CC major '${ccmaj:-unreadable}', CXX major '${cxxmaj:-unreadable}', floor $FLOOR"
  fi
fi

# ---- the floor -------------------------------------------------------------------------------
# Above the floor must be accepted. Testing equality against the pin would reject a host NEWER
# than CI and send it to download an older compiler it does not need.
stub 99 99
case "$(run)" in
  *"llvm-99"*) ok "a major above the floor is used rather than rejected" ;;
  *) bad "a clang above the floor was not selected" ;;
esac

# Below the floor must not be accepted. Refusing 18 means falling through to the install, so this
# case has to reach the download path WITHOUT downloading: curl is stubbed to produce a file that
# fails the checksum, which stops the script on its next line. Without the stub this case fetches
# 1.88 GiB and takes the backoff sleeps with it — measured, because the first version of this file
# did exactly that and left the asset in /tmp.
rm -rf "$T"/llvm-*
stub 18 18
printf '#!/bin/sh\nwhile [ $# -gt 0 ]; do case "$1" in -o) shift; echo stub > "$1";; esac; shift; done\nexit 0\n' > "$T/bin/curl"
chmod +x "$T/bin/curl"
sysmaj="$(clang++ -dM -E -x c++ /dev/null 2>/dev/null | awk '$2 == "__clang_major__" { print $3 }')"
if [ -n "$sysmaj" ] && [ "$sysmaj" -ge 21 ] 2>/dev/null; then
  ok "skipped the below-floor case: this host's own clang++ is $sysmaj and would decide it"
else
  case "$(run)" in
    *"downloading"*) ok "clang 18 alone is refused and the script goes to install, which is webrtc-sys' own answer" ;;
    *) bad "clang 18 alone was accepted as usable" ;;
  esac
fi
rm -f "$T/bin/curl" /tmp/llvm.tar.xz

# ---- the pin ----------------------------------------------------------------------------------
# One assertion of the pin in this repository, in the workflow that runs it. A ci.yml that cannot
# be parsed must refuse rather than fall back to a remembered value.
rm -rf "$T"/llvm-*
stub 21 21
if CI_YML=/dev/null run >/dev/null 2>&1; then
  bad "a ci.yml with no pin still produced a run"
else
  ok "a ci.yml with no pin refuses rather than falling back"
fi

printf '          LLVM_RELEASE: 21.1.8\n          LLVM_RELEASE: 21.1.7\n' > "$T/two.yml"
if CI_YML="$T/two.yml" run >/dev/null 2>&1; then
  bad "two pins in one workflow resolved to one anyway"
else
  ok "two pins in one workflow refuse rather than pick one"
fi

# Non-vacuous: the repository's real workflow must parse, so the parser cannot drift off the file
# it actually has to read while the fixtures above stay green.
real="$D/../.github/workflows/ci.yml"
if [ -r "$real" ]; then
  case "$(CI_YML="$real" run)" in
    *"ERROR"*) bad "the repository's own ci.yml does not parse" ;;
    *) ok "the repository's own ci.yml parses to a usable pin" ;;
  esac
fi

echo
if [ "$fail" -eq 0 ]; then echo "workspace-test-deps selftest: PASS"; else echo "workspace-test-deps selftest: FAIL"; fi
exit "$fail"
