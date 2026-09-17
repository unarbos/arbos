#!/usr/bin/env bash
# Start Cursor's browser sign-in and print the link a human must open.
#
#   cursor-login-link.sh        -> prints https://cursor.com/loginDeepControl?...
#
# How Cursor sign-in works: the app makes a one-time code pair (a "challenge"
# and a "uuid"), opens the link in a browser, and then polls Cursor's server
# until someone signs in on that link. The link only works while THIS app
# instance is still running and waiting. Send the link to Jacob; when he
# signs in on it, the app on the VM becomes signed in.
set -euo pipefail
source "$(dirname "$0")/env.sh"

focus '^parity-proj - Cursor$'
# The "Log In" button on the welcome screen (1600x1000 window at 100,60).
xdotool mousemove 900 593 click 1
sleep 6
id="$(win 'Sign in to Cursor')"
[ -n "$id" ] || { log "browser did not open"; exit 1; }
xdotool windowactivate --sync "$id"
xdotool key ctrl+l; sleep 0.3; xdotool key ctrl+a ctrl+c; sleep 0.5
url="$(xclip -o -selection clipboard)"
echo "$url" | tee /tmp/cursor-login-url.txt
wmctrl -c "Sign in to Cursor"
log "link saved to /tmp/cursor-login-url.txt; app keeps polling while open"
