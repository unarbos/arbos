#!/usr/bin/env bash
# Take one still of display :1.
#
#   shot.sh <app> <name>        -> $MEDIA/<app>/<name>.png
#
# <app> is "cursor" or "arbos". <name> is a short kebab-case label.
# Uses scrot (a small X11 screenshot tool). Prints the file path.
set -euo pipefail
source "$(dirname "$0")/env.sh"

app="$1"; name="$2"
out="$MEDIA/$app/$name.png"
scrot -o "$out"
sleep 0.3
ls -la "$out" >&2
echo "$out"
