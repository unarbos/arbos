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

# A pull request that publishes nothing and a main that owes nothing: the quiet case the two
# drift guards below are bolted onto, so that only the drifted input can be what fails them.
QUIET=(--main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b
       --pr-base 0c7489b91 --pr-head 41847d26b)

# case <name> <expected exit> <gate arguments...>
#
# Set WHY to a phrase the gate's own message must contain. An exit code alone says the case
# refused, not that it refused for the reason the case is named after, and a refusal that arrives
# by accident is a case that would still pass with the guard it tests removed.
case_is() {
  local name=$1 want=$2 got=0 out why=${WHY:-}
  shift 2
  out="$(python3 "$gate" "$@" 2>&1)" || got=$?
  if [[ "$got" == "$want" && ( -z "$why" || "$out" == *"$why"* ) ]]; then
    printf 'ok    %s (exit %s)\n' "$name" "$got"
  else
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s%s, got %s\n%s\n\n' \
      "$name" "$want" "${why:+ saying \"$why\"}" "$got" "$out"
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

# ── the gate's own inputs drifting is also a refusal ────────────────────────────────────────────
# Both of these would be silent misses rather than wrong answers: a pull request the gate never
# judges, or judges against a list that no longer describes what ships. Neither may pass.

# Exactly the state this change fixed: arbos-client and arbos-matrix are linked into Arbos.app and
# were missing from the Mac path list, so a fix in either shipped with no release cut for it.
ARBOS_MAC_PATHS="crates/arbos-mac crates/arbos-core crates/arbos-update Cargo.lock scripts/bundle-mac.sh site/assets/logo" \
  WHY="the Mac binary links arbos-client, arbos-matrix" \
  case_is "a Mac path list that is not the binary's closure fails closed" 2 "${QUIET[@]}"

# The two cases below each break one thing in a throwaway tree, so the real one is never edited.
#
# Directories are linked rather than copied, and that detail is load-bearing: the coverage check
# asks the filesystem whether a watched path is a directory, so a tree missing `site` invents a
# `site/assets/logo` gap that has nothing to do with the drift on trial. Both cases would then
# have passed with the guard they test removed. WHY is what caught it, and is why these cases
# assert the message rather than the exit code alone.
root="$(cd "$here/../.." && pwd)"
throwaways=()
trap 'rm -rf "${throwaways[@]}"' EXIT

throwaway() {
  local d
  d="$(mktemp -d)"
  throwaways+=("$d")
  mkdir -p "$d/scripts/release" "$d/.github/workflows"
  cp "$here"/*.py "$here"/*.sh "$d/scripts/release/"
  ln -s "$root/crates" "$d/crates"
  ln -s "$root/site" "$d/site"
  cp "$root/.github/workflows/ios.yml" "$d/.github/workflows/"
  echo "$d"
}

# A trigger list that has fallen behind a predicate: the gate would never start for an iOS-only
# pull request, which is a miss rather than a wrong answer, and silent.
drift="$(throwaway)"
grep -v '"apps/ios/\*\*"' "$root/.github/workflows/release-gate.yml" \
  > "$drift/.github/workflows/release-gate.yml"
gate="$drift/scripts/release/release-gate.py"
WHY="touching apps/ios/** (from .github/workflows/ios.yml) would never start this gate" \
  case_is "a trigger list that misses a predicate's path fails closed" 2 "${QUIET[@]}"
gate="$here/release-gate.py"

# A job that has lost the screen. GitHub concludes a run `success` when every job is
# success-or-skipped, so one such job clears the gate on a head nobody judged — which is exactly
# what `workflow_dispatch` used to do here.
shape="$(throwaway)"
python3 - "$root/.github/workflows/release-gate.yml" \
         "$shape/.github/workflows/release-gate.yml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
# Drop the self-test job's screen, leaving the gate job's in place: one unscreened job is enough.
head, _, tail = text.partition("\n  self-test:")
tail = re.sub(r"      - name: Can this run decide anything\?\n(?:        .*\n)+", "", tail, count=1)
open(sys.argv[2], "w", encoding="utf-8").write(head + "\n  self-test:" + tail)
PY
gate="$shape/scripts/release/release-gate.py"
WHY="self-test never runs gate-can-decide.sh" \
  case_is "a job that could pass without deciding fails closed" 2 "${QUIET[@]}"
gate="$here/release-gate.py"

# The other two shapes the same assertion watches for: a job that can be skipped, and a step whose
# failure is swallowed. Both end as a run GitHub calls `success` with no verdict inside it.
skippable="$(throwaway)"
sed 's|^  self-test:|  self-test:\n    if: github.event_name == '"'"'pull_request'"'"'|' \
  "$root/.github/workflows/release-gate.yml" > "$skippable/.github/workflows/release-gate.yml"
gate="$skippable/scripts/release/release-gate.py"
WHY="self-test has an \`if:\`" \
  case_is "a job that can be skipped fails closed" 2 "${QUIET[@]}"
gate="$here/release-gate.py"

swallowed="$(throwaway)"
sed 's|^      - run: scripts/release/release-gate-selftest.sh|      - run: scripts/release/release-gate-selftest.sh\n        continue-on-error: true|' \
  "$root/.github/workflows/release-gate.yml" > "$swallowed/.github/workflows/release-gate.yml"
gate="$swallowed/scripts/release/release-gate.py"
WHY="self-test has a step with \`continue-on-error\`" \
  case_is "a step whose failure is swallowed fails closed" 2 "${QUIET[@]}"
gate="$here/release-gate.py"

# ── the screen itself ───────────────────────────────────────────────────────────────────────────
# Every job's first step. An event that hands the gate no pull request must exit 2, never 0:
# `workflow_dispatch` supplied neither value, and the fallback that filled them in is what turned
# a run that judged nothing into a success on a real head.

screen_is() {
  local name=$1 want=$2 got=0
  shift 2
  env "$@" "$here/gate-can-decide.sh" >/dev/null 2>&1 || got=$?
  if [[ "$got" == "$want" ]]; then
    printf 'ok    %s (exit %s)\n' "$name" "$got"
  else
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s, got %s\n' "$name" "$want" "$got"
  fi
}

screen_is "an event carrying no pull request cannot decide" 2 GATE_BASE_REF= GATE_HEAD_SHA=
screen_is "a base ref with no head sha cannot decide" 2 GATE_BASE_REF=main GATE_HEAD_SHA=
screen_is "a head sha with no base ref cannot decide" 2 GATE_BASE_REF= GATE_HEAD_SHA=41847d26b
screen_is "a pull request can decide" 0 GATE_BASE_REF=main GATE_HEAD_SHA=41847d26b

if (( failures > 0 )); then
  echo
  echo "$failures release-gate case(s) did not behave as written."
  exit 1
fi
echo
echo "every release-gate case behaved as written, refusals included."
