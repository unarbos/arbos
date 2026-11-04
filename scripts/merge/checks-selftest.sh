#!/usr/bin/env bash
# The required-check gate is the only one there is: main has no branch protection and no ruleset.
# GitHub runs the head's copy of a `pull_request` workflow, so a head that edits a gate would
# otherwise supply the green run this kit reads as PASS. These cases pin the refusal.
#
# The functions are lifted out of checks.sh rather than re-typed: sourcing it would need a GitHub
# token and a repo. If one is renamed or reshaped, the extraction finds nothing and this fails loudly.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/checks.sh"
body="$(sed -n '/^gate_definition_touched()/,/^}/p' "$src")"
[ -n "$body" ] || { echo "FAIL: no gate_definition_touched() in $src"; exit 1; }
eval "$body"
gates="$(sed -n "s/^GATES='\(.*\)'$/\1/p" "$src")"
[ -n "$gates" ] || { echo "FAIL: no GATES in $src"; exit 1; }
gate_wf="${gates%%|*}"

fails=0
check() { # check <expected yes|no> <what> <files...>
  local want=$1 what=$2 got=no
  shift 2
  gate_definition_touched "$gate_wf" "$(printf '%s\n' "$@")" && got=yes
  if [ "$got" = "$want" ]; then
    echo "ok   $what -> $got"
  else
    echo "FAIL $what -> $got, wanted $want"
    fails=$((fails + 1))
  fi
}

check yes "a head that edits the gate itself is refused" \
  "crates/arbos-mac/src/app.rs" "$gate_wf"
check yes "the gate among many files is still found" \
  "a.rs" "$gate_wf" "b.rs" "docs/x.md"
check no "an ordinary diff is judged normally" \
  "crates/arbos-mac/src/app.rs" "crates/arbos-matrix/src/rows.rs"
check no "another workflow is not this gate" \
  ".github/workflows/ci.yml" ".github/workflows/release.yml"
# The match is exact, so a path that merely ends with the gate's name is not the gate. Were it a
# substring match, a file could be named to look like the gate and refuse every merge.
check no "a path that only ends with the gate's name is not the gate" \
  "docs/$gate_wf"
check no "an empty diff touches no gate" ""

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all ok"
