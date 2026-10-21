#!/bin/bash
# iOS pre-merge gate, run on the EC2 Mac by the feedback poller over ssh:
#   ios-gate.sh <branch> <sha>
# Fetches the branch into ~/arbos-gate (its own clone, never the audit clone), checks out <sha>
# detached, and builds the iPhone app twice (Release, then Debug; generic simulator, no signing). Prints the
# compiler errors; exit 0 = both compile, 1 = one does not, 2 = infrastructure trouble (fetch/checkout).
set -uo pipefail
branch=$1; sha=$2
cd ~/arbos-gate || exit 2
git fetch -q origin "+refs/heads/${branch}:refs/remotes/origin/${branch}" || { echo "fetch of ${branch} failed"; exit 2; }
git checkout -q --detach "$sha" || { echo "checkout of ${sha} failed"; exit 2; }
# Arbos embeds ArbosFFI, which links the gitignored xcframework from
# scripts/build-xcframework.sh. Build it here so Release/Debug fail on Rust
# problems with a clear log, not only inside xcodebuild. The ArbosFFI target
# also has a Run Script that builds when the artifact is missing/stale for this
# commit (covers a gate Mac still running an older copy of this script).
if [[ -x scripts/build-xcframework.sh ]]; then
  echo "== xcframework: building ArbosFFI for ${sha}"
  ./scripts/build-xcframework.sh || { echo "== xcframework: FAILS"; exit 1; }
fi
cd apps/ios/Arbos || exit 2
# TestFlight archives the Release configuration: a `#if DEBUG` type used unconditionally compiles in
# Debug and breaks the TestFlight job (2026-10-19, 6d64c2ee). Build Release first (that is what ships),
# then Debug (the simulator rig and the fixtures run Debug). Separate derived data keeps both incremental.
rc=0
for config in Release Debug; do
  log=$(mktemp -t ios-gate)
  xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration "$config" \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-$config" \
    CODE_SIGNING_ALLOWED=NO build >"$log" 2>&1
  crc=$?
  echo "== $config: $([ "$crc" -eq 0 ] && echo compiles || echo FAILS)"
  grep -E "error:|BUILD (SUCCEEDED|FAILED)" "$log" | grep -v "warning:" | sed 's#/Users/[^/]*/arbos-gate/##' | head -40
  rm -f "$log"
  [ "$crc" -eq 0 ] || rc=1
done
exit $rc
