#!/bin/bash
# Type-and-Enter smoke for the built Mac app (P0 #1208: every Enter crashed v0.6.89–v0.6.92).
#   mac-enter-smoke.sh <arbos-mac binary>
# Launches the binary with the fixture, brings it to front, ⌘T (new chat, composer focused), types a
# line, presses Enter and fails when the process is gone afterwards. It first proves it can drive the
# app at all (⌘A ⌘C → the typed text must be on the clipboard); when it cannot — the GUI session
# lacks Accessibility / Automation for this process — it says so and exits 0 with a warning, so the
# deterministic `cargo test -p arbos-mac enter_tests` stays the hard gate and this one adds coverage
# where it can run. Exit 0 = alive (or could not drive), 1 = CRASHED, 2 = could not launch.
set -uo pipefail
BIN=${1:?arbos-mac binary}
export ARBOS_FIXTURE=1 ARBOS_FIXTURE_OPEN=${ARBOS_FIXTURE_OPEN:-Affine} ARBOS_SURFACE=matte ARBOS_UPDATE_AUTO=0
LOG=$(mktemp -t enter-smoke)
TEXT="enter smoke $(date +%s)"
# One GUI app at a time on the shared display (scripts/runner/gui-lock.sh): take the lock, give it back at exit.
GUI_LOCK=/Users/Shared/arbos-run/gui-lock.sh
if [[ -x "$GUI_LOCK" ]]; then
  if ! "$GUI_LOCK" acquire release-smoke 240; then
    echo "::warning title=enter smoke::display busy ($("$GUI_LOCK" status)); skipped — the GPUI enter_tests remain the gate"
    echo "== enter smoke: display busy; skipped"
    exit 0
  fi
  trap '"$GUI_LOCK" release release-smoke >/dev/null 2>&1' EXIT
fi
"$BIN" >"$LOG" 2>&1 &
PID=$!
sleep 8
if ! kill -0 "$PID" 2>/dev/null; then echo "== enter smoke: the app did not stay up after launch"; tail -n 20 "$LOG"; exit 2; fi
# Driving the app needs Automation (System Events) for the runner's node process. The first osascript from
# an ungranted session puts a TCC prompt on the shared display that blocks the job until a human clicks
# Allow (03:5x Oct 22: three such prompts sat on the Mac). So: no keystrokes unless the session was granted
# once by hand and the marker exists — until then this smoke is "launches and stays up" only.
GUI_OK=${ARBOS_SMOKE_GUI_OK_MARKER:-$HOME/.arbos-smoke-gui-ok}
if [[ ! -f "$GUI_OK" ]]; then
  echo "::notice title=enter smoke::launch-only — the app stayed up 8 s; typing is off until the runner session has Accessibility + Automation and $GUI_OK exists (runbook: EC2 Mac hygiene)"
  echo "== enter smoke: ALIVE after launch (typing disabled: no Automation grant marker)"
  kill "$PID" 2>/dev/null; sleep 1; kill -9 "$PID" 2>/dev/null
  exit 0
fi
cannot_drive() {
  echo "::warning title=enter smoke::could not drive the app ($1: this session lacks Accessibility/Automation for the runner); the GPUI enter_tests remain the gate"
  echo "== enter smoke: could not drive the app ($1); skipped"
  kill "$PID" 2>/dev/null; exit 0
}
# osascript blocks forever on a TCC prompt nobody can answer (v0.6.96 release, 22:31: `keystroke`
# hung the job until the process was killed by hand) — every call gets a 10-second cap.
key() {
  osascript -e "tell application \"System Events\" to $1" >/dev/null 2>&1 &
  local p=$! i
  for ((i = 0; i < 100; i++)); do
    kill -0 "$p" 2>/dev/null || { wait "$p" 2>/dev/null; return $?; }
    sleep 0.1
  done
  kill "$p" 2>/dev/null; wait "$p" 2>/dev/null
  return 124
}
key "count processes" || cannot_drive "System Events did not answer within 10 s"
key "tell (first process whose unix id is $PID) to set frontmost to true"; sleep 1
key 'keystroke "t" using command down' || cannot_drive "keystroke blocked"
sleep 1
printf '' | pbcopy
key "keystroke \"$TEXT\"" || cannot_drive "keystroke blocked"
sleep 1
key 'keystroke "a" using command down'; sleep 0.3
key 'keystroke "c" using command down'; sleep 1
if [[ "$(pbpaste 2>/dev/null)" != *"$TEXT"* ]]; then
  cannot_drive "keystrokes did not reach the composer"
fi
# The one keystroke this smoke exists to send is the one whose result must be read: an Enter that
# never arrived leaves the app trivially alive, and "ALIVE after typing + Enter" — which goes into the
# release job's step summary as the evidence that P0 #1208 has not come back — would be printed by a
# run that pressed nothing.
key 'key code 36' || cannot_drive "the Enter keystroke was blocked"   # Enter
sleep 4
if kill -0 "$PID" 2>/dev/null; then
  echo "== enter smoke: ALIVE after typing + Enter"
  kill "$PID" 2>/dev/null; sleep 1; kill -9 "$PID" 2>/dev/null
  exit 0
fi
wait "$PID"; code=$?
echo "::error title=enter smoke::the Mac app CRASHED after typing + Enter (exit $code)"
grep -i "panic\|thread 'main'\|already being updated" "$LOG" | head -5
exit 1
