#!/bin/bash
# COVERS: projects list — faces, rows, sections
# What each row of the projects list actually says.
#
#   list-rows.sh <cycle>
#
# The row was last read at cycle 69 and its claims have been carried in the
# coverage ledger as prose ever since: every row names a state; a project
# whose kernel the mesh owns carries an age; a project whose machine is gone
# keeps its row and says so; and an absent project never reads as "now".
# Prose is not a check, so this counts them off the accessibility tree.
#
# It also refuses a row whose label carries a piece that is only
# punctuation. The list draws " · " between the state and the machine, and
# as a `Text` of its own SwiftUI hands it to VoiceOver as an element — so
# `phone` read "phone, Idle, dot, home, 16m". That is the same family as
# the SF Symbol names in `check-names.sh`: a thing drawn for the eye,
# spoken to the ear because nobody told it not to be.
#
# Nothing here uses a fixture. The claims are about what the live hub
# reports for real projects, and a fixture would only prove the fixture.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/list-rows"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
# The app comes back to the chat that was in front, so reach the list.
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 3; }
ui dump > "$OUT/tree.txt"
xcrun simctl io "$UDID" screenshot "$OUT/01-list.png" >/dev/null 2>&1

# A row is a Button whose label is `<name>, <state>…`. The chrome — Search,
# Settings, Read — carries no comma, which is what separates them here.
grep -E "Button +[A-Za-z.][A-Za-z0-9._-]*," "$OUT/tree.txt" > "$OUT/rows.txt"
ROWCOUNT=$(wc -l < "$OUT/rows.txt" | tr -d ' ')
echo "rows on screen: $ROWCOUNT"
# "0 of 0 rows name a state" is not a pass, it is a check that found no
# rows. Cycle 142 made the rows GenericElements instead of Buttons, this
# grep matched none of them, and five runs in a row reported every row
# clean while measuring nothing at all.
if [ "$ROWCOUNT" = 0 ]; then
  echo
  echo "VERDICT: none — no project rows matched at all. Either the list is"
  echo "         empty or rows stopped being Buttons; either way nothing"
  echo "         below would have been about a row."
  exit 1
fi
echo
cat "$OUT/rows.txt" | sed 's/^/  /'
echo

JUDGE=$(mktemp -t listrows)
cat > "$JUDGE" <<'PY'
import re, sys

STATES = ("Idle", "Working", "Restart needed", "is asleep", "is off",
          "Connecting", "Starting")
AGE = re.compile(r"^\d+(s|m|h|d)$")

rows, bad_state, bad_age, punct, aged = [], [], [], [], []
for line in sys.stdin.read().splitlines():
    label = line.split(None, 3)[3].strip() if len(line.split(None, 3)) > 3 else ""
    if not label:
        continue
    parts = [p.strip() for p in label.split(",")]
    name, rest = parts[0], parts[1:]
    rows.append(name)
    if not any(s in " ".join(rest) for s in STATES):
        bad_state.append(label)
    # A piece that is only punctuation is a separator that reached the ear.
    for p in rest:
        if p and not re.search(r"[A-Za-z0-9]", p):
            punct.append(f"{name}: {p!r}")
    for p in rest:
        if AGE.match(p):
            aged.append(f"{name} {p}")
        # "now" and "0m" are what an absent project must never claim.
        if p.lower() in ("now", "0m", "0s"):
            bad_age.append(f"{name}: {p}")

print(f"  rows:            {len(rows)}")
print(f"  with a state:    {len(rows) - len(bad_state)} of {len(rows)}")
print(f"  carrying an age: {len(aged)}" + (f" — {', '.join(aged)}" if aged else ""))
for label in bad_state:
    print(f"  NO STATE:        {label}")
for p in punct:
    print(f"  PUNCTUATION:     {p}")
for b in bad_age:
    print(f"  READS AS NOW:    {b}")
faults = len(bad_state) + len(punct) + len(bad_age)
print(f"FAULTS {faults}")
PY
trap 'rm -f "$JUDGE"' EXIT

# Prove the judge can fail before believing it when it passes. Two of these
# are rows this app really drew, and the third is the shape the ledger says
# must never appear.
SELF=$(printf '%s\n' \
  " 196  530  Button       phone, Idle,  · , home, 16m" \
  " 196  233  Button       nameless-row" \
  " 196  307  Button       ghost, Idle, now" | python3 "$JUDGE" | grep -c "^  \(NO STATE\|PUNCTUATION\|READS AS NOW\)")
if [ "$SELF" != 3 ]; then
  echo "the judge failed its own self-test ($SELF of 3 known-bad rows caught)."
  echo "Not running: a check that cannot fail cannot pass either."
  exit 1
fi
echo "judge self-test: caught 3 of 3 known-bad rows"
echo

RESULT=$(python3 "$JUDGE" < "$OUT/rows.txt")
echo "$RESULT"
FAULTS=$(echo "$RESULT" | sed -n 's/^FAULTS //p')
echo
echo "stills and tree in $OUT"
echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: every row names a state, ages read as ages, and nothing"
  echo "         speaks a separator"
else
  # Keep the scene. The separator fault comes and goes: it read six of six
  # dotted during the cycle-161 sweep and six of six clean an hour later from
  # the same commit, with the label provably in effect both times (a probe
  # string reached the tree). Three fixes have each held once. What has been
  # missing every time is what the screen looked like at the moment it fired,
  # so the next occurrence is guessed at from a one-line flag.
  WHEN=$OUT/fault-$(date -u +%H%M%S)
  mkdir -p "$WHEN"
  python3 "$HERE/../ui.py" "$UDID" dump > "$WHEN/tree.txt" 2>&1
  xcrun simctl io "$UDID" screenshot "$WHEN/screen.png" >/dev/null 2>&1
  {
    echo "rows on screen:   $(grep -cE 'Button +[a-z0-9-]+, ' "$WHEN/tree.txt")"
    echo "section headers:  $(grep -cE 'Button +(Working|Read)$' "$WHEN/tree.txt")"
    echo "rows reading Working: $(grep -cE 'Button +[a-z0-9-]+, Working' "$WHEN/tree.txt")"
    echo "the app on the device:"
    ls -l "$(xcrun simctl get_app_container "$UDID" com.unarbos.arbos.ios app 2>/dev/null)/Arbos" 2>/dev/null
    echo "the tree's project rows:"
    grep -E 'Button +[a-z0-9-]+, ' "$WHEN/tree.txt"
  } > "$WHEN/state.txt" 2>&1
  echo "  the screen at the moment it fired is in $WHEN"
  echo "  ($(sed -n 2p "$WHEN/state.txt"), $(sed -n 3p "$WHEN/state.txt"))"
  echo
  echo "VERDICT: $FAULTS fault(s) above — a row that says nothing, a separator"
  echo "         that reached the ear, or an age that claims 'now'"
fi
