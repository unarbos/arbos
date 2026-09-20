#!/bin/bash
# COVERS: settings sheet
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
# The app to reinstall is the one the loop just built, not a path some cycle
# left in /tmp. This read a build from the previous day, so every scenario
# after it measured yesterday's app — the source of the separator six cycles
# chased (M-498, M-503).
APP=${APP:-$HOME/mobile-derived/Build/Products/Debug-iphonesimulator/Arbos.app}
[ -d "$APP" ] || { echo "no app at $APP — refusing to reinstall, because doing it"
                   echo "wrong leaves every later scenario measuring a stale build"; exit 1; }
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
# #571 named the round buttons, so the settings disc reads `Settings` where
# it used to read `Gear Shape` — SwiftUI's rendering of the SF Symbol. This
# script tapped the old name and stopped dead on a build that had been
# improved. Prefer the name, fall back to what older builds say.
open_settings() { ui try-tap "Settings" >/dev/null 2>&1 || ui tap "Gear Shape" >/dev/null 2>&1; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
# A row is a project row whatever its status says. Counting only the ones
# reading `Idle` or `Working` is how this scenario reported "a token that
# cannot work empties the list to 1" for weeks: a refused token does not
# remove the rows, it changes what they say — `const, Off` — and rows that
# stopped matching the pattern were counted as gone. A recording of cycle 94
# is what caught it: seven rows before, seven during, seven after.
rows() { ui dump | grep -cE "Button +[a-z0-9-]+, "; }
live_rows() { ui dump | grep -cE "Button +[a-z0-9-]+, (Idle|Working)"; }
statuses() { ui dump | grep -oE "Button +[a-z0-9-]+, [^,]+" | sed -E 's/^Button +[a-z0-9-]+, //' | sort | uniq -c | sort -rn | head -4 | awk '{ printf "%s×%s  ", $1, $2 }'; }

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
# Printed and never read, until now. M-306 measured five sections and three
# `Token saved` fields at cycle 87, and every run since has printed those
# numbers into a log nothing compares. A section quietly lost would have
# been in the output and passed.
SECTIONS=$(ui dump | grep -cE 'Heading')
FIELDS=$(ui dump | grep -c 'Token saved')
echo "  sections: $(ui dump | grep -E 'Heading' | awk '{$1="";$2="";$3="";print}' | tr '\n' ';')"
echo "  section count: $SECTIONS   (M-306 counted 5 at cycle 87)"
echo "  saved-token fields say: $FIELDS   (M-306 counted 3)"
[ "$SECTIONS" = 5 ] || { echo "  CHANGED: $SECTIONS sections, not the 5 of cycle 87 — a section gained or lost"; SHEET_CHANGED=$((${SHEET_CHANGED:-0} + 1)); }
[ "$FIELDS" = 3 ] || { echo "  CHANGED: $FIELDS token fields, not 3 — a credential gained or lost"; SHEET_CHANGED=$((${SHEET_CHANGED:-0} + 1)); }
# The build line is at the foot of a scrolling sheet, so reading the first
# screen finds nothing and says so as if the line were missing. Fourth script
# tonight to count or look at one screen of a scrolling view (M-287, M-303,
# M-304); the cure each time is to move first and read after.
for _ in 1 2 3 4; do page_up "$UDID" >/dev/null 2>&1; sleep 0.8; done
BUILDLINE=$(ui dump | grep -iE 'Arbos, [0-9]' | head -1 | sed 's/^ *//')
echo "  build line: ${BUILDLINE:-MISSING}"
echo "  and under it: $(ui dump | grep -i 'TestFlight build' | head -1 | sed 's/^ *//')"
# The build line is the only place the phone says which build it is running,
# which is the loop's most argued-about fact all day. Its absence must fail
# rather than print "MISSING" into a passing run.
[ -n "$BUILDLINE" ] || { echo "  FAULT: no build line at the foot of the sheet"; SHEET_CHANGED=$((${SHEET_CHANGED:-0} + 1)); }
# On the simulator that line reads `(1)` — the locally built number — under
# the words "The TestFlight build on this phone". True on a phone, and a
# trap here: this line can never answer which TestFlight build Jacob has,
# which is the fact the loop spends most of its messages on.
case "$BUILDLINE" in
  *"(1)"*) echo "  (that is the local build: on the simulator this line cannot say"
           echo "   which TestFlight build anyone has — only a real install can)";;
esac

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
BADLIVE=$(live_rows)
echo "  rows now: $BAD ($BADLIVE still reading Idle or Working)"
echo "  what the rows say: $(statuses)"
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

# The row's claim in one line, so a sweep can read it. It used to say the
# list *empties*; it does not, and never did. A refused token leaves every
# row where it is and changes what each one says.
echo
BADLIVE=${BADLIVE:-0}
if [ "${BAD:-0}" = "${GOOD:-0}" ] && [ "$BADLIVE" -lt "${GOOD:-0}" ] && [ "${SAIDWHY:-no}" = yes ]; then
  echo "VERDICT: a token that cannot work keeps all $BAD rows and marks them off"
  echo "         ($BADLIVE still live), says why, and $RESTORED come back"
  [ "${SHEET_CHANGED:-0}" = 0 ] || echo "         The sheet itself changed: see the ${SHEET_CHANGED} line(s) marked above." 
elif [ "${BAD:-0}" -lt "${GOOD:-0}" ] && [ "${SAIDWHY:-no}" = yes ]; then
  echo "VERDICT: a token that cannot work removed rows — $GOOD before, $BAD after."
  echo "         It said why, and $RESTORED came back. Rows going is a change from"
  echo "         cycle 94, where all seven stayed and turned off."
elif [ "${SAIDWHY:-no}" != yes ]; then
  echo "VERDICT: the rows changed but the screen never said why — the M-206 fault"
else
  echo "VERDICT: unexpected shape — good $GOOD, bad $BAD ($BADLIVE live), restored $RESTORED"
fi
