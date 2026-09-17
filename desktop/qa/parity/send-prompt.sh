#!/usr/bin/env bash
# Type one prompt into the focused app's chat box and press Enter.
#
#   send-prompt.sh <app> "<text>"
#
# <app> = cursor | arbos. The script clicks the chat box first. The click
# point is the bottom-centre of the 1600x1000 window placed at (100,60),
# which is where both apps keep their composer (the chat text box).
set -euo pipefail
source "$(dirname "$0")/env.sh"

app="$1"; text="$2"
case "$app" in
  cursor) focus "$CURSOR_WIN" ;;
  arbos)  focus "$ARBOS_WIN" ;;
  *) echo "app must be cursor or arbos" >&2; exit 2 ;;
esac

# Composer position (x,y) on screen, for a 1600x1000 window at (100,60).
# Arbos: an empty chat shows the composer mid-screen (1031,568); after the
# first message it sits at the bottom (1031,1022). Pass FIRST=1 for an
# empty chat. Cursor: bottom-centre of the transcript column (measured from
# the public video frames; re-measure once sign-in works).
case "$app" in
  cursor) xdotool mousemove 900 1022 click 1 ;;
  arbos)  PYTHONPATH=/workspace/desktop/driver python3 -c "from arbosdriver import Arbos; Arbos('/tmp/arbos-driver.sock').connect().click('composer-field')" ;;
esac
sleep 0.4
xdotool type --delay 12 -- "$text"
sleep 0.3
xdotool key Return
log "sent to $app: ${text:0:60}"
