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
# This does not judge pass or fail. It prints each scenario's own verdict
# line, and flags the ones that produced none — a scenario that reached no
# conclusion is the shape a rotted check takes.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; shift || true

DEFAULT=(
  list-composer.sh
  list-search-filter-refresh.sh
  cold-start-and-history.sh
  settings-and-a-bad-token.sh
  chip-and-send-arrow.sh
  voice-notes-wait.sh
  the-core-chat-path.sh
  call-pulled-down.sh
  worker-chat-open-and-back.sh
  pill-count-vs-sheet.sh
  notifications.sh
)
SCENARIOS=("$@")
[ ${#SCENARIOS[@]} -eq 0 ] && SCENARIOS=("${DEFAULT[@]}")

OUT="$HOME/mobile-out/$CYCLE/sweep"; mkdir -p "$OUT"
printf '%-34s %s\n' "scenario" "what it concluded"
printf '%-34s %s\n' "--------" "------------------"

SILENT=0
for name in "${SCENARIOS[@]}"; do
  log="$OUT/${name%.sh}.log"
  bash "$HERE/scenarios/$name" "$CYCLE" > "$log" 2>&1
  # A scenario's conclusion is a VERDICT line if it has one, else the last
  # line that reads like a finding rather than a path.
  verdict=$(grep -E "^ *VERDICT" "$log" | tail -1 | sed 's/^ *VERDICT: *//')
  if [ -z "$verdict" ]; then
    verdict=$(grep -vE "^ *$|stills in|logs in|still in" "$log" | tail -1 | sed 's/^ *//')
    [ -n "$verdict" ] && verdict="(no verdict) $verdict"
  fi
  if [ -z "$verdict" ]; then
    verdict="NOTHING — no verdict and no output"
    SILENT=$((SILENT + 1))
  fi
  printf '%-34s %s\n' "${name%.sh}" "$(echo "$verdict" | cut -c1-90)"
done

echo
echo "$SILENT of ${#SCENARIOS[@]} reached no conclusion at all."
echo "logs in $OUT — read any scenario whose verdict surprises you, especially a"
echo "confident one, since a rotted check is confident by construction."
