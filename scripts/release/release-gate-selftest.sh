#!/usr/bin/env bash
# Does the release gate block? scripts/release/release-gate.py replayed against states main has
# really been in, including the ones it must refuse.
#
#   scripts/release/release-gate-selftest.sh        (needs the full history and nothing else)
#
# A guard nobody has watched refuse is not a tested guard. A run of the gate on a clean main
# proves only that it says yes; these cases make it say no, and say it for the right artifact.
#
# Every commit below is on main's first-parent line and permanent. A merge commit paired with its
# first parent is the same difference as the pull request that produced it, which is what the gate
# reads. Both "already released" values are passed in, so no case reaches the network and no case
# changes answer as main moves on.
#
#   v0.6.379  5395be9c1  release commit; the Mac app was built from its parent 2402ff996
#   v0.6.380  6f70123be  release commit; the Mac app was built from its parent 83c2b5ebf
#   83c2b5ebf #2645, a Mac fix    (base 49608e2e8)
#   f13ab025a #2640, relay only   (base 0c9e04ede)
#   41847d26b #2533, iPhone only  (base 0c7489b91)
#   15d11e89a #2604, crates/arbos-core — on BOTH path sets (base 9b62b7fcb)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
gate="$here/release-gate.py"
failures=0

# case <name> <expected exit> <gate arguments...>
case_is() {
  local name=$1 want=$2 got=0 out
  shift 2
  out="$(python3 "$gate" "$@" 2>&1)" || got=$?
  if [[ "$got" == "$want" ]]; then
    printf 'ok    %s (exit %s)\n' "$name" "$got"
  else
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s, got %s\n%s\n\n' "$name" "$want" "$got" "$out"
  fi
}

# ── main owes a Mac release ─────────────────────────────────────────────────────────────────────
# main is 83c2b5ebf, the last stable release is v0.6.379, and #2645's Mac fix sits between them.

case_is "a Mac fix is refused while main owes a Mac release" 1 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

case_is "a relay-only fix merges while main owes a Mac release" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 0c9e04ede --pr-head f13ab025a

# The trap this gate exists to avoid. v0.6.380's bump commit touches Cargo.lock, which IS a Mac
# path, so a plain path diff calls it a Mac change and this case would wrongly refuse. mac-change.sh
# reads the lockfile and correctly reports no Mac change, so the gate lets it through.
case_is "a release version bump is not a Mac change" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 83c2b5ebf --pr-head 6f70123be

case_is "a Mac fix merges once the Mac release has published" 0 \
  --main e445f70ad --mac-released-sha 83c2b5ebf --ios-released-sha e445f70ad \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

# ── main owes a TestFlight build ────────────────────────────────────────────────────────────────
# main is 41847d26b and the last build testers saw was made from 0c7489b91, with #2533's iPhone
# fix between them. The Mac side is clear across that same span, so only the phone blocks.

case_is "an iPhone fix is refused while main owes a TestFlight build" 1 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 0c7489b91 --pr-head 41847d26b

case_is "an iPhone fix merges once the TestFlight build has published" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 0c7489b91 --pr-head 41847d26b

case_is "a Mac fix merges while main owes only a TestFlight build" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

# ── a pull request on both path sets ────────────────────────────────────────────────────────────
# crates/arbos-core is in MAC_PATHS and in ios.yml's filter, so #2604 publishes both artifacts and
# waits for either one that is outstanding.

case_is "a dual-artifact fix is refused for the phone alone" 1 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

case_is "a dual-artifact fix is refused for the Mac alone" 1 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

case_is "a dual-artifact fix merges when both artifacts are out" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

# ── a question the gate cannot answer is not an answer ──────────────────────────────────────────
# Exit 2, never 0: "I could not tell" must never read as "nothing is wrong".

case_is "an unreadable published build fails closed, not open" 2 \
  --main 41847d26b --mac-released-sha 0c7489b91 \
  --ios-released-sha 0000000000000000000000000000000000000000 \
  --pr-base 0c7489b91 --pr-head 41847d26b

case_is "an unreadable pull request head fails closed, not open" 2 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 0c7489b91 --pr-head 0000000000000000000000000000000000000000

# A build whose commit is on no branch this checkout has says the two disagree about what main is.
case_is "a published build that is not on main fails closed, not open" 2 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha e445f70ad \
  --pr-base 0c7489b91 --pr-head 41847d26b

if (( failures > 0 )); then
  echo
  echo "$failures release-gate case(s) did not behave as written."
  exit 1
fi
echo
echo "every release-gate case behaved as written, refusals included."
