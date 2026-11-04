#!/usr/bin/env bash
# Does the release gate still decide anything? scripts/release/release-gate.py replayed against
# states main has really been in, including the ones it must refuse and the ones it must not.
#
#   scripts/release/release-gate-selftest.sh        (needs the full history and nothing else)
#
# A guard nobody has watched refuse is not a tested guard, and since 2026-11-04 the reverse is the
# nearer danger: the one-fix-per-release refusal was withdrawn, and a gate that answers "fine" to
# everything passes a suite that only ever checks exit codes for 0. So the cases come in two
# halves that fail in opposite directions.
#
#   Debt is not a refusal. The four shapes this gate used to hold — a Mac fix, an iPhone fix and
#   a dual-artifact fix on a main that owes the matching artifact — now exit 0. Each of them also
#   asserts that the gate still named the debt and still named what the pull request publishes,
#   so deleting the measurement to make them pass fails them instead. Exit 0 alone would be
#   satisfied by a gate that had stopped reading anything at all.
#
#   Everything else it refuses, it still refuses. Nine cases exit 2 on inputs the gate cannot
#   read and on its own shape drifting, and they are untouched by the cadence change.
#
# Every case here asserts the gate's own words, not only its exit code — see case_is. An exit code
# is one of three values shared by twenty-three cases, so on its own it cannot tell a case from the
# case beside it, and a case that cannot do that would pass for a reason its name does not claim.
# scripts/release/release-gate-selftest-audit.sh checks that property mechanically: each case's
# phrases must match that case's output and no other case's.
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
#
# 6f70123be does double duty. It is a release bump, so it is not a Mac change, and it touches
# Cargo.lock and Cargo.toml, which ios.yml lists outright — which makes it a main that owes a
# TestFlight build for a commit no pull request on apps/ios/** has anything to do with. That is
# the exact state #2720 was refused in, replayed below from permanent commits.
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
# Set WHY to the phrases the gate's own message must contain, one per line, all required. An exit
# code alone says the case refused, not that it refused for the reason the case is named after,
# and a refusal that arrives by accident is a case that would still pass with the guard it tests
# removed.
#
# One phrase is often not enough either. Four of the cases below come in two pairs that block on
# the same outstanding artifact and print an identical `::error` line; what separates them is what
# the pull request itself publishes, which is a different line of the same output. A phrase that
# also matches a neighbour cannot tell its case from that neighbour, so each of those four names
# both facts.
has_all() {  # <output> <phrases, one per line>
  local out=$1 p
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    [[ "$out" == *"$p"* ]] || return 1
  done <<< "$2"
}

# release-gate-selftest-audit.sh sets this to a file and reads back what every case actually
# produced, so it can check that no case's phrases match another case's output. It runs the real
# file rather than a copy: a copy of a self-test is one more thing that can drift away from the
# thing it is supposed to describe.
record() {  # <name> <phrases> <output>
  [ -n "${RELEASE_GATE_SELFTEST_RECORD:-}" ] || return 0
  printf '<<<CASE %s\n<<<WHY\n%s\n<<<OUT\n%s\n<<<END\n' "$1" "$2" "$3" >> "$RELEASE_GATE_SELFTEST_RECORD"
}

case_is() {
  local name=$1 want=$2 got=0 out why=${WHY:-}
  shift 2
  out="$(python3 "$gate" "$@" 2>&1)" || got=$?
  record "$name" "$why" "$out"
  if [[ "$got" == "$want" ]] && has_all "$out" "$why"; then
    printf 'ok    %s (exit %s)\n' "$name" "$got"
  else
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s%s, got %s\n%s\n\n' \
      "$name" "$want" "${why:+ saying $(tr '\n' '/' <<< "$why")}" "$got" "$out"
  fi
}

# ── main owes a Mac release ─────────────────────────────────────────────────────────────────────
# main is 83c2b5ebf, the last stable release is v0.6.379, and #2645's Mac fix sits between them.

# This exited 1 until the cadence rule was withdrawn. Three phrases, and each one fails to a
# different defect, which is the point of there being three:
#   exit 0                 — the refusal is gone.
#   publishes ...: YES     — it is gone because the rule is gone, not because the gate decided
#                            this pull request publishes nothing. A gate blind to Mac changes
#                            would exit 0 here too, and this phrase is what separates them.
#   main owes ...          — the measurement survived the refusal. Delete the outstanding read
#                            and the gate still exits 0, still says YES, and fails on this line.
WHY="publishes a stable Mac release: YES — Mac sources changed: crates/arbos-mac
main owes a stable Mac release; 83c2b5ebf
publishes a TestFlight build: no" \
  case_is "a Mac fix merges while main owes a Mac release, and the debt is still named" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

# A pass needs its phrases as much as a refusal does. Exit 0 says the gate let this through, not
# that it let it through for the reason the case is named after: a gate that had decided this pull
# request publishes nothing at all would pass every case below while testing none of them.
WHY="publishes a stable Mac release: no — no Mac path changed
publishes a TestFlight build: no" \
  case_is "a relay-only fix merges while main owes a Mac release" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 0c9e04ede --pr-head f13ab025a

# The trap this gate exists to avoid. v0.6.380's bump commit touches Cargo.lock, which IS a Mac
# path, so a plain path diff calls it a Mac change and this case would wrongly refuse. mac-change.sh
# reads the lockfile and correctly reports no Mac change, so the gate lets it through. The phrase is
# that reason, because a pass arriving any other way is not this case.
WHY="publishes a stable Mac release: no — Cargo.lock moved only our own crates' version numbers" \
  case_is "a release version bump is not a Mac change" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 83c2b5ebf --pr-head 6f70123be

# This and "a Mac fix merges while main owes only a TestFlight build" run the same pull request to
# the same pass. The published Mac build is what differs, so each names it.
WHY="publishes a stable Mac release: YES — Mac sources changed: crates/arbos-mac
last published: 83c2b5eb" \
  case_is "a Mac fix merges once the Mac release has published" 0 \
  --main e445f70ad --mac-released-sha 83c2b5ebf --ios-released-sha e445f70ad \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

# ── main owes a TestFlight build ────────────────────────────────────────────────────────────────
# main is 41847d26b and the last build testers saw was made from 0c7489b91, with #2533's iPhone
# fix between them. The Mac side is clear across that same span, so only the phone blocks.

WHY="publishes a TestFlight build: YES — matches ios.yml's trigger paths: apps/ios/
main owes a TestFlight build; 41847d26b
publishes a stable Mac release: no" \
  case_is "an iPhone fix merges while main owes a TestFlight build, and the debt is still named" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 0c7489b91 --pr-head 41847d26b

# #2720's own state, which is not the case above: there the unbuilt commit IS the pull request's
# own fix, so a gate that confused "main owes" with "this diff" would still look right. Here main
# is the v0.6.380 bump, owing a TestFlight build for Cargo.lock and Cargo.toml, and the pull
# request is an unrelated iPhone fix that shares no file with it. The old gate refused it with
# "Nothing about this pull request needs to change", which was true and was the whole problem.
#
# The reason phrase is the load-bearing one. It names files the pull request does not touch, so
# it can only come from reading main against the last build — the half that had to survive.
WHY="publishes a TestFlight build: YES — matches ios.yml's trigger paths: apps/ios/
why it is unreleased  unreleased on ios.yml's paths: Cargo.lock, Cargo.toml
main owes a TestFlight build; 6f70123be" \
  case_is "an iPhone fix merges while main owes a build for a release bump it never touched" 0 \
  --main 6f70123be --mac-released-sha 6f70123be --ios-released-sha 83c2b5ebf \
  --pr-base 0c7489b91 --pr-head 41847d26b

WHY="publishes a stable Mac release: no
publishes a TestFlight build: YES — matches ios.yml's trigger paths: apps/ios/
last published: 41847d26" \
  case_is "an iPhone fix merges once the TestFlight build has published" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 0c7489b91 --pr-head 41847d26b

WHY="publishes a stable Mac release: YES — Mac sources changed: crates/arbos-mac
last published: 0c7489b9
publishes a TestFlight build: no" \
  case_is "a Mac fix merges while main owes only a TestFlight build" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 49608e2e8 --pr-head 83c2b5ebf

# ── a pull request on both path sets ────────────────────────────────────────────────────────────
# crates/arbos-core is in MAC_PATHS and in ios.yml's filter, so #2604 publishes both artifacts and
# waits for either one that is outstanding.

# Each of these reports the same outstanding artifact as one of the single-path cases above, and
# prints an identical `::notice` line for it. "Dual" is the part that line does not carry: it
# lives in the other publish line, which says YES here and `no` there. Both phrases are needed.
#
# These two also hold the per-artifact scoping in place. Debt is reported for the artifacts the
# pull request publishes, so a gate that started reporting every artifact's debt regardless would
# make the pair indistinguishable, and release-gate-selftest-audit.sh would say so.
WHY="main owes a TestFlight build; 41847d26b
publishes a stable Mac release: YES" \
  case_is "a dual-artifact fix merges while main owes the phone alone" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 0c7489b91 \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

WHY="main owes a stable Mac release; 83c2b5ebf
publishes a TestFlight build: YES" \
  case_is "a dual-artifact fix merges while main owes the Mac alone" 0 \
  --main 83c2b5ebf --mac-released-sha 2402ff996 --ios-released-sha 83c2b5ebf \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

# The same pull request as the two refusals above, so the two publish lines are word for word
# theirs. What makes this case the one it is named after is both artifacts reading clear, so the
# phrases have to reach the lines that say so.
WHY="publishes a stable Mac release: YES — Mac sources changed: crates/arbos-core
clear — no Mac path changed
publishes a TestFlight build: YES — matches ios.yml's trigger paths: crates/arbos-core
clear — nothing on ios.yml's paths has changed since that build" \
  case_is "a dual-artifact fix merges when both artifacts are out" 0 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 9b62b7fcb --pr-head 15d11e89a

# ── a question the gate cannot answer is not an answer ──────────────────────────────────────────
# Exit 2, never 0: "I could not tell" must never read as "nothing is wrong".

# The gate's message names the sha it could not resolve but not which input carried it, so these
# two cases use different unresolvable shas. With one shared placeholder their messages would be
# identical, and each case would pass for the other's reason.
WHY="\`git rev-parse --verify 0000000000000000000000000000000000000000^{commit}\` failed" \
  case_is "an unreadable published build fails closed, not open" 2 \
  --main 41847d26b --mac-released-sha 0c7489b91 \
  --ios-released-sha 0000000000000000000000000000000000000000 \
  --pr-base 0c7489b91 --pr-head 41847d26b

WHY="\`git rev-parse --verify ffffffffffffffffffffffffffffffffffffffff^{commit}\` failed" \
  case_is "an unreadable pull request head fails closed, not open" 2 \
  --main 41847d26b --mac-released-sha 0c7489b91 --ios-released-sha 41847d26b \
  --pr-base 0c7489b91 --pr-head ffffffffffffffffffffffffffffffffffffffff

# A build whose commit is on no branch this checkout has says the two disagree about what main is.
WHY="made from e445f70a, which is not an ancestor of 41847d26" \
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

ordered="$(throwaway)"
python3 - "$root/.github/workflows/release-gate.yml" \
         "$ordered/.github/workflows/release-gate.yml" <<'PY'
import sys
text = open(sys.argv[1], encoding="utf-8").read()
# The screen is still there, but the job does something first. A job that acts before it knows
# whether the run can decide has already spent the decision it was supposed to wait for.
head, _, tail = text.partition("\n  self-test:")
tail = tail.replace(
    "      - name: Can this run decide anything?",
    "      - run: echo work the job had no business doing yet\n"
    "      - name: Can this run decide anything?",
    1,
)
open(sys.argv[2], "w", encoding="utf-8").write(head + "\n  self-test:" + tail)
PY
gate="$ordered/scripts/release/release-gate.py"
WHY="self-test does work before gate-can-decide.sh" \
  case_is "a job that acts before the screen fails closed" 2 "${QUIET[@]}"
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

# screen_is <name> <expected exit> <required phrases, one per line> <env assignments...>
#
# Same contract as WHY on case_is, except the phrases are not optional. All three refusals exit 2
# and print the same sentence about the same two variables; the only thing separating them is the
# values quoted back, so an exit code on its own would let any of them stand in for any other.
screen_is() {
  local name=$1 want=$2 why=$3 got=0 out
  shift 3
  out="$(env "$@" "$here/gate-can-decide.sh" 2>&1)" || got=$?
  record "$name" "$why" "$out"
  if [[ "$got" == "$want" ]] && has_all "$out" "$why"; then
    printf 'ok    %s (exit %s)\n' "$name" "$got"
  else
    failures=$((failures + 1))
    printf 'FAIL  %s: wanted exit %s saying %s, got %s\n%s\n\n' \
      "$name" "$want" "$(tr '\n' '/' <<< "$why")" "$got" "$out"
  fi
}

screen_is "an event carrying no pull request cannot decide" 2 \
  "base ref is '<empty>' and head sha is '<empty>'" GATE_BASE_REF= GATE_HEAD_SHA=
screen_is "a base ref with no head sha cannot decide" 2 \
  "base ref is 'main' and head sha is '<empty>'" GATE_BASE_REF=main GATE_HEAD_SHA=
screen_is "a head sha with no base ref cannot decide" 2 \
  "base ref is '<empty>' and head sha is '41847d26b'" GATE_BASE_REF= GATE_HEAD_SHA=41847d26b
# The pass case needs both lines. A screen that exits 0 having printed one of them, or neither,
# hands the gate step an empty BASE_REF or HEAD_SHA, which is the hole this whole change closes.
screen_is "a pull request can decide" 0 \
  "base_ref=main
head_sha=41847d26b" GATE_BASE_REF=main GATE_HEAD_SHA=41847d26b

if (( failures > 0 )); then
  echo
  echo "$failures release-gate case(s) did not behave as written."
  exit 1
fi
echo
echo "every release-gate case behaved as written, refusals included."
