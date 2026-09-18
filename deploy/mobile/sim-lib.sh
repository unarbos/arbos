# Shared helpers for the iPhone loop's simulator scenarios.
#   . "$(dirname "$0")/sim-lib.sh"
#
# The one that matters is `pt`. A screenshot of the loop's simulator is
# 472x1024 pixels; `idb ui tap` works in the device's 393x852 point space.
# So a coordinate measured off a still and passed straight to a tap lands
# about one row low, and the run still looks healthy — stills, timings and
# recordings all arriving — while the taps are in the wrong place. Cycle 46
# opened two wrong projects that way before the header gave it away.
#
# Every hardcoded tap already in these scripts is in points and is correct.
# It is only reading a coordinate off a screenshot that needs converting,
# which is exactly what anyone does when writing a new scenario.

# What the dictation clip says. It lives here because two places need it and
# they must agree: `mac-attach.sh` speaks this sentence to make `note.wav`,
# and `voice-notes-wait.sh` checks the words that came back against it. With
# the sentence written out twice, a scenario can "verify" dictation against
# a sentence the clip no longer says.
NOTE_SAYS=${NOTE_SAYS:-"Please summarise what the workers did today in two sentences."}

# The simulator's screenshot size and the device's point size. Both are the
# iPhone 15 Pro's; change them together if the loop's device changes.
SIM_SHOT_W=${SIM_SHOT_W:-472}
SIM_SHOT_H=${SIM_SHOT_H:-1024}
SIM_PT_W=${SIM_PT_W:-393}
SIM_PT_H=${SIM_PT_H:-852}

# reach_the_list <udid> — get to the projects list, wherever the app woke up.
#
# Since M-338 a cold start comes back to the chat that was in front, so a
# scenario that launches and taps a project row no longer knows what it is
# tapping. Worse, it depends on run order: a fresh install has no front
# project and lands on the list, so the same scenario passes when it runs
# first and taps at a chat when it runs after another. Cycle 115 found
# `refusal-and-transport.sh` had been opening no case at all for fifteen
# cycles that way, and reporting as though it had.
#
# Call this after launch, before tapping a row.
SIM_LIB_DIR=${SIM_LIB_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)}
reach_the_list() {
  local udid=$1 i tree
  for i in 1 2 3; do
    tree=$(python3 "$SIM_LIB_DIR/ui.py" "$udid" dump 2>/dev/null)
    # An empty tree is not "no Back button", it is "I cannot see". Read the
    # first as the second and this returns success from a blind check — the
    # failure every rotted instrument in this loop has had in common.
    if [ -z "$tree" ]; then
      echo "  cannot read the screen — is $SIM_LIB_DIR/ui.py there, and the app up?" >&2
      return 1
    fi
    echo "$tree" | grep -qE "Button +Back" || return 0
    python3 "$SIM_LIB_DIR/ui.py" "$udid" tap "Back" >/dev/null 2>&1
    sleep 3
  done
  echo "  still not on the projects list after three Backs" >&2
  return 1
}

# pt <pixels-on-a-screenshot> -> points for idb, vertical scale.
pt() { python3 -c "print(round($1 * $SIM_PT_H / $SIM_SHOT_H))"; }

# ptx <pixels-on-a-screenshot> -> points, horizontal scale. Separate from
# `pt` because the two ratios are not identical and a tap near an edge can
# miss on the difference.
ptx() { python3 -c "print(round($1 * $SIM_PT_W / $SIM_SHOT_W))"; }

# tap_shot <x-px> <y-px> <udid> — tap where something is on the last still.
tap_shot() { idb ui tap "$(ptx "$1")" "$(pt "$2")" --udid "$3"; }

# page_up <udid> — scroll a list down by one screenful, in points.
#
# Written because two scenarios of mine swiped from y=900 on a screen that is
# 852 points tall. Nothing errors: the gesture lands nowhere, the list does
# not move, and every "page" reads the same rows. Both scenarios then
# reported a list as shorter than it is, and one of those became a filed
# finding about the app disagreeing with itself (M-275, withdrawn).
#
# The screenshot is 1024 px tall and the screen 852 pt, so a y taken off a
# still is always too large — the same pixels-versus-points trap as M-152,
# which `pt` exists for. This wraps the whole gesture so a scenario need not
# get it right twice.
page_up() {
  local from=$(( SIM_PT_H * 80 / 100 ))   # 682 on this device
  local to=$(( SIM_PT_H * 40 / 100 ))     # 341
  idb ui swipe "$(( SIM_PT_W / 2 ))" "$from" "$(( SIM_PT_W / 2 ))" "$to" --duration 0.5 --udid "$1"
}

# page_back <udid> — the other direction: toward the older end of a
# transcript. `page_up` drags the content upward, which walks *towards* the
# newest line; looking for something older with it finds nothing however
# long you try, which is how a search for a worker's `Done` line reported
# the line missing while it sat a screen above.
page_back() {
  local from=$(( SIM_PT_H * 35 / 100 ))
  local to=$(( SIM_PT_H * 85 / 100 ))
  idb ui swipe "$(( SIM_PT_W / 2 ))" "$from" "$(( SIM_PT_W / 2 ))" "$to" --duration 0.5 --udid "$1"
}

# collect_rows <udid> <grep-pattern> <outfile> [ui-script]
#
# Page a scrolling list to its end and write the distinct row labels found.
# Stops when two pages in a row add nothing, and says whether it stopped
# because it converged or because it hit the ceiling — which is the whole
# point. It reports how many pages that took, because "converged after 1
# page" and "converged after 9" are different stories about the same word:
# the first is a list that never scrolled. A fixed number of pages is an assumption about how long the list
# is, and cycle 86 counted 17 rows of a list of 25 that way, which is the
# same shape of error as not scrolling at all (M-287).
collect_rows() {
  local udid=$1 pattern=$2 out=$3 ui=${4:-python3 "$(dirname "${BASH_SOURCE[0]}")/ui.py"}
  local raw="$out.raw" before after still=0 page
  : > "$raw"
  for page in $(seq 1 20); do
    $ui "$udid" dump | grep -E "$pattern" >> "$raw"
    before=$(awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$raw" | sort -u | wc -l)
    page_up "$udid" >/dev/null 2>&1
    sleep 1.2
    $ui "$udid" dump | grep -E "$pattern" >> "$raw"
    after=$(awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$raw" | sort -u | wc -l)
    if [ "$after" -eq "$before" ]; then
      still=$(( still + 1 ))
      [ "$still" -ge 2 ] && { awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$raw" | sort -u > "$out"; echo "converged after $page page(s)"; return 0; }
    else
      still=0
    fi
  done
  awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$raw" | sort -u > "$out"
  echo "hit the 20-page ceiling"
}
