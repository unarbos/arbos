#!/bin/bash
# The settings sheet, and what the app says when its hub token is wrong.
#
#   settings-and-a-bad-token.sh <cycle>
#
# Destructive on purpose: it saves a token that cannot work, so the recovery
# is a reinstall, which restores what the build was compiled with. Nothing
# here ever reads or prints the real token.
#
# The question worth asking is not only "do the rows go Off" — cycle 37
# established that — but whether the screen says *why*. A client that knows
# it was refused and shows a blank list is inventing a silence.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"   # page_up
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/settings"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
APP=/tmp/dd/Build/Products/Debug-iphonesimulator/Arbos.app
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
# #571 named the round buttons, so the settings disc reads `Settings` where
# it used to read `Gear Shape` — SwiftUI's rendering of the SF Symbol. This
# script tapped the old name and stopped dead on a build that had been
# improved. Prefer the name, fall back to what older builds say.
open_settings() { ui tap "Settings" >/dev/null 2>&1 || ui tap "Gear Shape" >/dev/null 2>&1; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
rows() { ui dump | grep -cE "Button +[a-z0-9-]+, (Idle|Working)"; }

fresh() {
  xcrun simctl uninstall "$UDID" $B 2>/dev/null
  xcrun simctl install "$UDID" "$APP"
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
  sleep 8
  # Every number below is a count of list rows. A reinstall clears the front
  # project so this is usually a no-op — usually is not a reason to omit it.
  reach_the_list "$UDID" || exit 1
}

# A reinstall does NOT undo a saved token: it is in the Keychain, which
# outlives the app on iOS. Cycle 60's first run stranded the simulator that
# way and had to be rescued by hand. The token is read from the build into a
# shell variable and typed straight in; it is never echoed, and the only
# exposure is this machine's process list while idb runs.
put_the_real_token_back() {
  local tok y
  tok=$(plutil -extract hubToken raw -o - "$HOME/arbos/ios/Arbos/Secrets.plist" | tr -d '\n')
  [ -n "$tok" ] || { echo "  no hub token in the build to restore from"; return 1; }
  echo "  restoring the build's token (${#tok} characters, not shown)"
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 7
  open_settings; sleep 3
  y=$(ui dump | awk '$3 == "SecureTextField" || $3 == "TextField" { print $2 }' | sort -n | tail -1)
  idb ui tap 196 "$y" --udid "$UDID"; sleep 1
  idb ui text "$tok" --udid "$UDID" >/dev/null 2>&1; sleep 2
  ui tap "Done" >/dev/null; sleep 8
}

echo "== a good build, for the baseline =="
fresh
GOOD=$(rows)
echo "  rows: $GOOD"
shot 01-rows-with-the-real-token

echo
echo "== the sheet itself =="
open_settings || { echo "  no settings button under either name"; exit 1; }
sleep 3; shot 02-settings
echo "  sections: $(ui dump | grep -E 'Heading' | awk '{$1="";$2="";$3="";print}' | tr '\n' ';')"
echo "  saved-token fields say: $(ui dump | grep -c 'Token saved')"
# The build line is at the foot of a scrolling sheet, so reading the first
# screen finds nothing and says so as if the line were missing. Fourth script
# tonight to count or look at one screen of a scrolling view (M-287, M-303,
# M-304); the cure each time is to move first and read after.
for _ in 1 2 3 4; do page_up "$UDID" >/dev/null 2>&1; sleep 0.8; done
echo "  build line: $(ui dump | grep -iE 'Arbos, [0-9]' | head -1 | sed 's/^ *//')"
echo "  and under it: $(ui dump | grep -i 'TestFlight build' | head -1 | sed 's/^ *//')"

echo
echo "== save a hub token that cannot work =="
# The field is the one under the Mesh hub heading; it is found by walking the
# text fields, because a secure field carries no value to match on.
HUBTOK=$(ui dump | awk '$3 == "SecureTextField" || $3 == "TextField" { print $2 }' | sort -n | tail -1)
ui dump | awk -v y="$HUBTOK" '$2 == y { print "  typing into the field at y=" y }'
idb ui tap 196 "$HUBTOK" --udid "$UDID"; sleep 1
idb ui text "not-a-real-token-cycle-$CYCLE" --udid "$UDID"; sleep 1
shot 03-bad-token-typed
ui tap "Done" >/dev/null || echo "  no Done button"
sleep 6; shot 04-list-after-a-bad-token
BAD=$(rows)
echo "  rows now: $BAD"
echo "  what the screen says:"
SAIDWHY=no
ui dump | grep -qi "token refused\|not connected\|could not" && SAIDWHY=yes
ui dump | grep -vE "Button +[a-z0-9-]+, (Idle|Working)" | grep -E "StaticText" | head -6 | sed 's/^/    /'

echo
echo "== put it back =="
put_the_real_token_back
RESTORED=$(rows)
echo "  rows after restoring the token: $RESTORED"
shot 05-rows-restored
echo "stills in $OUT"

# The row's claim in one line, so a sweep can read it: the list empties when
# the token cannot work, says why, and comes back when it can.
echo
if [ "${BAD:-0}" -lt "${GOOD:-0}" ] && [ "$RESTORED" = "${GOOD:-0}" ] && [ "${SAIDWHY:-no}" = yes ]; then
  echo "VERDICT: a token that cannot work empties the list to $BAD, says why, and $RESTORED come back"
elif [ "${SAIDWHY:-no}" != yes ]; then
  echo "VERDICT: the list changed but the screen never said why — the M-206 fault"
else
  echo "VERDICT: unexpected shape — good $GOOD, bad $BAD, restored $RESTORED"
fi
