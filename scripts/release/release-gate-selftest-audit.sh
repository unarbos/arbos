#!/usr/bin/env bash
# Can each release-gate case tell itself apart from the case beside it?
#
#   scripts/release/release-gate-selftest-audit.sh      (needs the full history and nothing else)
#
# scripts/release/release-gate-selftest.sh checks that every case produces the exit code it should.
# That is not enough on its own. Three exit codes are shared by twenty-three cases, so an exit code
# can only ever say "this case refused", never "this case refused for the reason it is named
# after". A case that cannot distinguish itself from its neighbour passes whenever the neighbour
# would, which means it is evidence about the pair and not about either one.
#
# So every case also asserts phrases from the gate's own output, and this file checks that those
# phrases do the work they were added for: each case's phrases must appear in that case's output
# and in no other case's. One phrase is often not enough. Four of the gate's cases come in two
# pairs that block on the same outstanding artifact and print an identical `::error` line; what
# separates them is what the pull request itself publishes, on a different line of the same
# output.
#
# This and the mutation hunt catch different halves of the same defect. Breaking the thing a case
# names finds a case that is missing or hollow. This finds a case that is present and firing but
# whose assertion is too weak to be about its own subject. A suite can be clean under one and
# wrong under the other: the gate's cases survived thirteen mutations while ten of them still
# asserted nothing but an exit code.
#
# Exit 0 every case is discriminating. Exit 1 at least one is not, and it is named.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rec="$(mktemp)"
trap 'rm -f "$rec"' EXIT

RELEASE_GATE_SELFTEST_RECORD="$rec" "$here/release-gate-selftest.sh" > /dev/null

python3 - "$rec" <<'PY'
import re, sys

blob = open(sys.argv[1], encoding="utf-8").read()
cases = [
    (m.group(1), [p for p in m.group(2).split("\n") if p.strip()], m.group(3))
    for m in re.finditer(r"<<<CASE (.*?)\n<<<WHY\n(.*?)\n<<<OUT\n(.*?)\n<<<END\n", blob, re.S)
]
if not cases:
    sys.exit("the self-test recorded no cases at all, so this audit checked nothing")

bad = []
for name, phrases, _ in cases:
    if not phrases:
        bad.append(f"{name}: asserts no phrase, so only its exit code is checked")
        continue
    others = [n for n, _, out in cases if n != name and all(p in out for p in phrases)]
    if others:
        bad.append(f"{name}: its phrases also match {', '.join(others)}")

for line in bad:
    print(f"AMBIGUOUS  {line}")
if bad:
    print(f"\n{len(bad)} of {len(cases)} release-gate case(s) cannot be told from a neighbour.")
    sys.exit(1)
print(f"all {len(cases)} release-gate cases assert phrases that match no other case.")
PY
