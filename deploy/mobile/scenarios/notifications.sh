#!/bin/bash
# COVERS: notifications (`notify`, push)
# The notification path, end to end: a reply that arrives while the phone is
# elsewhere.
#
#   notifications.sh <cycle> [target-row]
#
# What it exercises: the permission ask, the banner on the Home Screen, the
# badge on the icon, the tap that lands in the right project, and the away
# card in the chat on return.
#
# The app is reinstalled first, because a notification decision is kept per
# install and a scripted run that once tapped "Don't Allow" can never see a
# banner again — it will report "no banner" for ever and look like a bug in
# the app. `-noAskNotifications` is deliberately NOT passed here: this is
# the one scenario that wants the ask.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/notifications"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }

say() { echo "$(date -u +%H:%M:%S) $*"; }

xcrun simctl uninstall "$UDID" $B 2>/dev/null
# The app to reinstall is the one the loop just built, not a path some cycle
# left in /tmp. This read /tmp/dd/Build/Products/..., a build from the
# previous day, so every scenario after this one in the sweep measured
# yesterday's app — which is where the separator that six cycles chased kept
# coming from (M-498, M-503). Proven: fresh build reads "phone, Idle, home",
# this scenario runs, and the next read is "phone, Idle, · , home" with the
# binary dated a day earlier.
DERIVED=${DERIVED:-$HOME/mobile-derived/Build/Products/Debug-iphonesimulator/Arbos.app}
APP=${APP:-$DERIVED}
[ -d "$APP" ] || { echo "no app at $APP — refusing to run against whatever is"
                   echo "already installed, which is how a stale build gets measured"; exit 1; }
xcrun simctl install "$UDID" "$APP"
xcrun simctl launch --console-pty "$UDID" $B > "$OUT/console.log" 2>&1 &
sleep 8
reach_the_list "$UDID" || exit 1
ui tap "$ROW" || { say "no $ROW row"; exit 1; }
sleep 4

# The ask comes on the first message, not on launch.
# Short on purpose. The banner is posted from the live socket, and iOS only
# keeps a backgrounded app alive for about half a minute, so a reply that
# takes longer never arrives at all until push is on. Asking for twenty
# numbers is how cycle 58's first run got a badge and no banner.
LINE="Reply with the single word OK and nothing else."
ui focus >/dev/null || { say "no composer"; exit 1; }
sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 40); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
ui tap "Send" || { say "no send button"; exit 1; }
sleep 3
shot 01-sent

say "allowing notifications if iOS asks"
for _ in 1 2 3; do
  if ui tap "Allow" >/dev/null 2>&1; then say "allowed"; break; fi
  sleep 2
done
shot 02-after-the-ask

say "away we go"
# A banner shows for a few seconds and then withdraws itself, so a poll that
# screenshots after noticing it has already missed it. Cycle 58's first run
# reported "no banner" for exactly that reason, with the badge sitting on
# the icon the whole time. Record the wait, and burst stills through it.
pkill -INT -f "simctl io.*recordVideo" 2>/dev/null; sleep 2
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/away-raw.mp4" > "$OUT/record.log" 2>&1 & REC=$!
idb ui button HOME --udid "$UDID"
# Taken at once, not after a wait: the banner arrives about two seconds in,
# so a "before" shot four seconds later already has the banner in it, and
# 03 and 04 came out byte-identical. Just long enough for the Home Screen to
# settle.
sleep 0.6; shot 03-home-before-the-banner
sleep 3.4
mkdir -p "$OUT/burst"
BANNER=""
for t in $(seq 1 60); do
  xcrun simctl io "$UDID" screenshot "$OUT/burst/$(printf %03d "$t").png" >/dev/null 2>&1
  # The banner belongs to SpringBoard, so `describe-all` on the app cannot
  # see it — the app's own console is what says it was posted, and the
  # burst is what shows it. A detector that looked in the tree reported
  # "no banner" through a run where one was plainly on the screen.
  if [ -z "$BANNER" ] && grep -q "^banner .*: posted" "$OUT/console.log" 2>/dev/null; then
    BANNER=$((t * 2)); say "banner posted about ${BANNER}s after going away"
    grep -E "^notify |^banner " "$OUT/console.log" | tail -3 | sed 's/^/  /'
    # The console says "posted" before SpringBoard has drawn it, so the
    # still is taken after a beat rather than copied from the burst frame
    # that noticed — that frame is reliably the one just before it appears.
    sleep 1.5; shot 04-home-with-the-banner
    sleep 3   # let a person read it before the tap
    break
  fi
  sleep 2
done
[ -n "$BANNER" ] || { say "no banner in 120s — check the console for 'notify'"; shot 04-home-no-banner; }

say "tapping the banner" 
ui tap "Arbos" >/dev/null 2>&1 || idb ui tap 196 120 --udid "$UDID"
sleep 5; shot 05-tapped-into-the-project
kill -INT $REC 2>/dev/null; sleep 4
if [ -s "$OUT/away-raw.mp4" ]; then
  ffmpeg -v error -y -i "$OUT/away-raw.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$OUT/recording-notification-away-and-back.mp4" && rm -f "$OUT/away-raw.mp4"
else
  say "no recording: $(tail -1 "$OUT/record.log" 2>/dev/null)"
fi
echo "--- where it landed ---"
LANDED=$(ui dump 2>/dev/null | head -6)
echo "$LANDED"

# The row's four claims in one line, so a sweep can read it. The banner
# itself belongs to SpringBoard and is invisible to `describe-all` (M-194),
# so its evidence is the app's own console line, not the tree.
echo
UNSEEN=$(grep -oE "unseen=[0-9]+" "$OUT/console.log" 2>/dev/null | tail -1)
INPROJECT=no
echo "$LANDED" | grep -qE "Button +Back" && INPROJECT=yes
if [ -z "$BANNER" ]; then
  echo "VERDICT: no banner was posted within the window — the rest says nothing"
elif [ "$INPROJECT" = yes ]; then
  echo "VERDICT: banner ~${BANNER}s after going away (${UNSEEN:-no badge count}), and the tap landed in the project"
else
  echo "VERDICT: banner ~${BANNER}s after going away, but the tap did not land in a project"
fi
