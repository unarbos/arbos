#!/bin/bash
# Run the loop's scenarios and report what each one concluded.
#
#   sweep.sh <cycle> [scenario ...]     # all the quick ones by default
#
# Why this exists. Today the loop found, one cycle at a time, that its own
# checks rot: a gesture that stopped reaching the screen, a page count
# mistaken for the end of a list, a counting rule invalidated by its data, a
# pattern blind to an animated glyph, a tap on a name the app stopped using,
# and — the sharpest one — a check whose whole premise was a fact about the
# gateway that later changed (M-343).
#
# Every one was found by accident, while looking at something else. A
# scenario that no longer measures what it claims goes on passing quietly,
# so the way to find the next one is to run them all and read the verdicts
# together rather than wait to trip over them.
#
# Before chasing a fault: check whether its fix is already written and simply
# unmerged. The sweep runs whatever is checked out, which is `main` unless a
# cycle says otherwise, and a scenario whose repair sits in an open PR keeps
# reporting the thing that was fixed. `tool-fold` did exactly that for
# thirteen cycles — cycle 162 rewrote it four ways over, that work stayed in
# an open PR, and every sweep since printed the fault it had already
# disproved. Cycle 173 then blamed the wrong cause for it.
#
# This does not judge pass or fail. It prints each scenario's own verdict
# line, and flags the ones that produced none — a scenario that reached no
# conclusion is the shape a rotted check takes.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; shift || true

# Everything self-contained enough to run unattended. A check that is not
# here only runs when somebody remembers it, and cycle 133 found four that
# nobody had for seventeen cycles — three passed and one had been broken the
# whole time.
DEFAULT=(
  # First, because every scenario below that sends anything goes through the
  # helper this one checks. If typing is broken, the rest of the sweep reports
  # on lines that were never typed.
  typing-into-the-composer.sh
  list-composer.sh
  list-search-filter-refresh.sh
  list-rows.sh
  list-sections.sh
  list-faces.sh
  cold-start-and-history.sh
  chat-opens-at-the-end.sh
  settings-and-a-bad-token.sh
  chip-and-send-arrow.sh
  attach-a-file.sh
  voice-notes-wait.sh
  the-core-chat-path.sh
  stop-a-turn.sh
  tool-fold.sh
  call-pulled-down.sh
  call-menu.sh
  worker-chat-open-and-back.sh
  worker-chat-shape.sh
  style-pair.sh
  chat-overflow-menu.sh
  pill-count-vs-sheet.sh
  notifications.sh
  sleeping-machine.sh
  refusal-and-transport.sh
)
# Deliberately out, with the reason, so the next person does not have to
# work out whether it was an oversight:
#
#   barge-in.sh, orb-phases.sh, one-breath-one-answer.sh
#     each drives a real call with injected audio; three to five minutes
#     apiece and they need the live gateway to be answering.
#   several-workers.sh, worker-while-it-works.sh
#     watch a worker for two minutes by design.
#   what-a-returning-user-sees.sh
#     sleeps 120 s twice on purpose — it is about coming back later.
#   type-send-measured.sh
#     needs a composer already open; it says so and refuses otherwise.
#   photo-reaches-the-model.sh
#     drives the system photo picker by coordinate, which is the one thing
#     here that a different simulator would break silently.
#   call-face.sh
#     opens a call per project and samples the glyph under the orb; three
#     calls is three to four minutes, and the question it measures is filed
#     as a decision rather than a fault, so it runs when that decision is
#     being made.
#   call-text-in-chat.sh, first-words-on-a-slow-connect.sh
#     both drive a real call, like the three above. Named here because they
#     were out with no reason written down, which is the state this list
#     exists to prevent — cycle 161 found four such, and two of them
#     (tool-fold, call-menu) turned out to be plain oversights.
#   everything named after a cycle — set37*, rec37*, c39, call37, fold37,
#   mac-cycle7*, mac-cycle8*, mac-cycle13, mac-cycle37, mac-c37b, j7check,
#   fixj7 — and the two mac-ask-restart scripts, mac-stall and mac-stop
#     one-offs written for a particular cycle, several reading paths that
#     cycle wrote. They are kept as a record of how something was measured
#     once, not as checks. If one of them is worth running again it needs
#     rewriting to stand on its own first.
#   connect-times-by-engine.sh
#     reads the whole log history rather than the app; run it when the
#     question is about trend, not about today.
SCENARIOS=("$@")
[ ${#SCENARIOS[@]} -eq 0 ] && SCENARIOS=("${DEFAULT[@]}")

OUT="$HOME/mobile-out/$CYCLE/sweep"; mkdir -p "$OUT"
printf '%-34s %s\n' "scenario" "what it concluded"
printf '%-34s %s\n' "--------" "------------------"

SILENT=0; NOVERDICT=0
for name in "${SCENARIOS[@]}"; do
  log="$OUT/${name%.sh}.log"
  bash "$HERE/scenarios/$name" "$CYCLE" > "$log" 2>&1
  # A scenario's conclusion is a VERDICT line if it has one, else the last
  # line that reads like a finding rather than a path.
  verdict=$(grep -E "^ *VERDICT" "$log" | tail -1 | sed 's/^ *VERDICT: *//')
  if [ -z "$verdict" ]; then
    verdict=$(grep -vE "^ *$|stills in|logs in|still in" "$log" | tail -1 | sed 's/^ *//')
    [ -n "$verdict" ] && { verdict="(no verdict) $verdict"; NOVERDICT=$((NOVERDICT + 1)); }
  fi
  if [ -z "$verdict" ]; then
    verdict="NOTHING — no verdict and no output"
    SILENT=$((SILENT + 1))
  fi
  printf '%-34s %s\n' "${name%.sh}" "$(echo "$verdict" | cut -c1-90)"
done

echo
# Three scenarios printed "(no verdict) no phone row" while this line said
# "0 of 11 reached no conclusion at all" — it was counting only the ones
# that printed nothing whatsoever. A run that says something and concludes
# nothing is the case worth counting, because it is the one that looks fine.
echo "$SILENT of ${#SCENARIOS[@]} printed nothing at all."
echo "$NOVERDICT of ${#SCENARIOS[@]} printed output but reached no verdict — read those first."
echo "logs in $OUT — read any scenario whose verdict surprises you, especially a"
echo "confident one, since a rotted check is confident by construction."

# The rows this sweep just exercised, read from the scenarios themselves. The
# ledger ages a row by the last cycle that named it, and a sweep names
# nothing — so anything it covers grows old on paper while being run every
# time (M-484).
echo
echo "coverage rows this sweep exercised:"
for name in "${SCENARIOS[@]}"; do
  grep -h "^# COVERS:" "$HERE/scenarios/$name" 2>/dev/null
done | sed 's/^# COVERS: */  /' | sort -u
