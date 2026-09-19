#!/bin/bash
# COVERS: the loop's own machine
#
# Can this machine actually run the harness?
#
#   check-machine.sh
#
# `check-tools.sh` asks whether the scripts reach for tools inside the
# checkout, and checksums them. That is about the harness being replaceable.
# It cannot tell you whether the machine under it has what those tools need,
# and a rented Mac is replaced often enough for that to matter.
#
# It already bit. At cycle 152 `style-pair.py` died on the Mac with
# `ModuleNotFoundError: No module named 'PIL'`, mid-cycle, and the work went
# on by copying the screenshots to another machine and comparing them there.
# That workaround is still how that row gets measured, which means the tool
# lives here and runs somewhere else.
#
# The Python list is read out of the harness's own imports rather than typed
# here. A list typed here would be right on the day it was written and would
# then quietly rot as tools gained dependencies — which is the same failure
# as the one above, one layer up.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
MISSING=0
ok()   { printf '  %-22s %s\n' "$1" "$2"; }
gone() { printf '  %-22s MISSING — %s\n' "$1" "$2"; MISSING=$((MISSING + 1)); }

echo "commands the harness runs:"
for c in xcrun simctl idb ffmpeg python3 git; do
  case "$c" in
    simctl) xcrun simctl help >/dev/null 2>&1 && ok "$c" "through xcrun" || gone "$c" "no simulator control at all";;
    *) if command -v "$c" >/dev/null 2>&1; then
         ok "$c" "$(command -v "$c")"
       else
         gone "$c" "$( [ "$c" = ffmpeg ] && echo 'no recordings can be shrunk' || echo 'nothing that uses it can run' )"
       fi;;
  esac
done

echo
echo "python modules the harness imports:"
# Read from the tools themselves, so a new dependency appears here the day it
# is added rather than the day someone remembers to add it.
MODS=$(grep -ohE "^(import|from) [a-zA-Z_][a-zA-Z0-9_]*" "$HERE"/*.py \
       | awk '{print $2}' | sort -u)
[ -n "$MODS" ] || { echo "  read no imports out of $HERE/*.py — the reader is broken,"
                    echo "  and every line here would be an empty pass"; exit 1; }
for m in $MODS; do
  if python3 -c "import $m" >/dev/null 2>&1; then
    ok "$m" "importable"
  else
    gone "$m" "$(grep -lE "^(import|from) $m\b" "$HERE"/*.py | xargs -n1 basename | tr '\n' ' ')cannot run"
  fi
done

echo
echo "what the harness reads and writes:"
for p in "$HOME/arbos/.git" "$HOME/mobile-out" "$HOME/mobile-docs" "$HOME/mobile-clips"; do
  if [ -e "$p" ]; then
    ok "$(basename "$p")" "$(du -sh "$p" 2>/dev/null | awk '{print $1}')"
  else
    gone "$(basename "$p")" "the loop keeps its $( [ "$p" = "$HOME/mobile-docs" ] && echo 'only surviving ledger copy' || echo 'work' ) here"
  fi
done
CLIPS=$(ls "$HOME"/mobile-clips/*.wav 2>/dev/null | wc -l | tr -d ' ')
[ "$CLIPS" -gt 0 ] && ok "voice clips" "$CLIPS wav file(s)" || gone "voice clips" "no call or dictation scenario can speak"
# Named, never printed: the scenarios read the hub token out of it.
[ -f "$HOME/arbos/ios/Arbos/Secrets.plist" ] && ok "Secrets.plist" "present (not read here)" \
  || gone "Secrets.plist" "no scenario can reach the hub"

echo
echo "room to work:"
FREE=$(df -g "$HOME" 2>/dev/null | awk 'NR==2 {print $4}')
BOOTED=$(xcrun simctl list devices booted 2>/dev/null | grep -c "Booted")
[ "${FREE:-0}" -ge 10 ] && ok "disk free" "${FREE}G" || gone "disk free" "only ${FREE:-?}G — a build needs several"
[ "${BOOTED:-0}" -ge 1 ] && ok "booted simulator" "$BOOTED" || gone "booted simulator" "nothing to drive"

echo
if [ "$MISSING" = 0 ]; then
  echo "VERDICT: this machine can run the whole harness."
else
  echo "VERDICT: $MISSING thing(s) missing. Each line above says what stops working,"
  echo "         so a scenario that fails for one of these reasons is not the app."
  exit 1
fi
