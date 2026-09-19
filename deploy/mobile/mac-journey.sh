#!/bin/bash
# COVERS: journey — the kernel's own build in the record
# COVERS: journey — the phone-only steps P1, P2, P3
# The phone's version of docs/acceptance-journeys.md (QA's step ids J1–J8),
# plus the phone-only steps P1–P3 (dictate, photo, call). One run = one
# folder under ~/mobile-out/journey/<run>/ with a still per step, the app
# console, the kernel transcript tail, and score.txt. Steps pass on the
# kernel's own record (transcript / read frame) after the challenge's seq,
# never on the screen alone; EYE = scored from the still; U = unverified.
#   mac-journey.sh <target>    target: pod | <machine>/<project>
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
# The tools this journey runs come from the checkout beside it, not from
# copies in $HOME. Both existed, they drifted, and the journey was using the
# stale one: `kernel.py` was corrected three times on 09-18 and not one of
# those fixes reached a run, because the run read `"$HERE/kernel.py"`. It also
# meant a rebuilt Mac needed an undocumented "copy these four into $HOME"
# step before the loop's own acceptance test would work.
HERE=$(cd "$(dirname "$0")" && pwd)
. "$(cd "$(dirname "$0")" && pwd)/sim-lib.sh"   # tap_shot: screenshot pixels -> device points
U=B1185668-7488-420F-B12D-4412BAAC7673; B=com.unarbos.arbos.ios
TARGET=${1:-pod}
# A target is "pod" or "<machine>/<project>". Anything else is a mistake made
# at the prompt, and the run used to accept it and fail six steps later with
# "no 157 row on the list" — which reads like the app lost a project rather
# than like a cycle number handed to the wrong argument.
case "$TARGET" in
  pod|*/*) ;;
  *) echo "'$TARGET' cannot be a target. This one takes pod, or <machine>/<project>."
     echo "It does not take a cycle number: the run names its own folder."
     exit 1;;
esac
ROW=${TARGET##*/}; [ "$ROW" = pod ] && ROW=phone   # M-121: the pod row folds into its roster twin
RUN=$(date -u +%m%d-%H%M%S); O=$HOME/mobile-out/journey/$RUN; mkdir -p $O
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
ID=J$(date -u +%H%M%S); DIR="journey_$ID"
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
# Lines this run put there, never the ones before it. With no anchor this
# used to fall back to the whole transcript, and cycle 48 scored a spawn
# refusal that belonged to two runs earlier — in a run where no spawn ever
# happened. An unknown anchor now yields nothing, so a check that needs one
# fails for want of evidence instead of finding somebody else's.
hist() {
  if [ -z "${AFTER:-}" ]; then echo "hist: no anchor set; refusing to read the whole transcript" >&2; return 0; fi
  python3 "$HERE/kernel.py" $TARGET history 120 2>/dev/null | awk -v a="$AFTER" '$1+0 > a+0'
}
seq_of() { python3 "$HERE/kernel.py" $TARGET history 120 2>/dev/null | grep -E "$1" | tail -1 | awk '{print $1}'; }
score() { echo "$1 $2 $3" | tee -a $O/score.txt; }
wait_hist() { local s=$1 re=$2 secs=$3 t=0; while [ $t -lt $secs ]; do if hist | grep -qE "$re"; then score "$s" PASS "/$re/ after ${t}s"; return 0; fi; sleep 5; t=$((t+5)); done; score "$s" FAIL "no /$re/ within ${secs}s"; return 1; }
# Type a line and make sure the whole of it is in the box before sending it.
#
# Cycle 48 lost six of its eight typed lines here and the journey scored the
# app for it. Three faults, all in the one line of shell this replaces.
# The tap point was fixed at 200,788: the composer sits near y=470 with the
# keyboard down, and 200,788 is the space bar with it up. Tapping a text
# box's centre puts the caret in the middle of what is already written, so a
# second line wove itself through the first. And nothing read the field
# back, so whatever had landed when the return key fired is what went.
#
# Counted on a four-line probe, twice each: 6 of 8 lines reached the kernel
# the old way, 8 of 8 the new way.
ui() { python3 ~/arbos/deploy/mobile/ui.py $U "$@"; }
field_len() { local v; v=$(ui field 2>/dev/null); echo ${#v}; }
clear_field() {
  local n
  for _ in 1 2 3 4 5; do
    n=$(field_len); [ "$n" -gt 0 ] || return 0
    ui focus >/dev/null 2>&1; sleep 0.6
    idb ui key-sequence $(for _ in $(seq 1 $((n + 5))); do printf '42 '; done) --udid $U >/dev/null 2>&1
    sleep 0.8
  done
}
type_send() {
  local want=$1 got why
  for _ in 1 2 3; do
    clear_field
    if ! why=$(ui focus 2>&1 >/dev/null); then
      # Name what is in the way. A run that only says "gave up" sends the
      # next cycle looking at the app: this one was iOS's own notifications
      # alert sitting over the chat, which no amount of tapping gets past.
      echo "type_send: no composer to type into — $why" | tee -a $O/run.txt
      ui dump 2>/dev/null | tail -5 | sed 's/^/  on screen: /' | tee -a $O/run.txt
      sleep 1; continue
    fi
    sleep 0.7
    idb ui text "$want" --udid $U >/dev/null 2>&1
    for _ in $(seq 1 40); do [ "$(ui field plain 2>/dev/null)" = "$want" ] && break; sleep 0.25; done
    got=$(ui field plain 2>/dev/null)
    if [ "$got" = "$want" ]; then
      ui tap "Send" >/dev/null 2>&1 || idb ui key 40 --udid $U >/dev/null 2>&1
      return 0
    fi
    echo "type_send: the box held ${#got} of ${#want} characters; clearing and retrying" | tee -a $O/run.txt
  done
  echo "type_send: gave up on a line after three tries" | tee -a $O/run.txt
  return 1
}
rd() { python3 "$HERE/kernel.py" $TARGET read "$1" 2>/dev/null; }
echo "run $RUN id $ID target $TARGET dir $DIR" | tee $O/run.txt
# Which kernel this run is measured against. Asked of the kernel on the
# attach socket, not of the hub: the roster's git_sha is whichever process
# registered last and has named a current build for a node that had been
# running a deleted binary for two days.
python3 "$HERE/kernel.py" $TARGET hello > $O/kernel-version.txt 2>&1 || echo "no hello" > $O/kernel-version.txt
echo "kernel $(head -1 $O/kernel-version.txt)" | tee -a $O/run.txt

# J1 — open the project from the list (the phone's "create": the project lives on a machine's kernel)
xcrun simctl terminate $U $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty $U $B -noAskNotifications 1 -hubURL "$H" -hubToken "$T" -dictateWav ~/mobile-clips/note.wav -injectWav ~/mobile-clips/ask.wav > $O/console.log 2>&1 &
sleep 7
# A cold start comes back to the chat that was in front (M-338), so the
# step called "open the project from the list" may not start on the list at
# all. This run tapped `phone` at y=85 — the chat's own header, not a row —
# and passed, because the chat it woke in happened to be the target. Reach
# the list, so J1 opens a project rather than confirming one was already
# open.
reach_the_list "$U" || score J1 FAIL "could not get to the projects list"
shot J1-list
# By name, not by measuring the still. `find_row.py` knew project names by
# glyph colour and divided by 3 for a screenshot that is 1.2x the point
# size, and cycle 49 opened `pod` twice while believing it had opened a
# fixture project. A run that opens the wrong project scores the wrong one.
ui tap "$ROW" || score J1 FAIL "no $ROW row on the list"
sleep 5; shot J1-open
# seed the failing project (QA's rig seeds a folder; the phone asks the kernel to)
type_send "$ID setup, do this yourself without workers: create $DIR/ with mathlib.py defining area(w, h) that wrongly returns w + h, tests/test_math.py (unittest) asserting area(3, 4) == 12, and git init with one commit on main containing both. No CHANGELOG. Reply 'seeded' when done."
# The anchor is this very line's seq, so this one wait cannot go through
# `hist`: there is nothing to read it against yet, and it scored a FAIL on
# every run for want of an anchor it was about to set. Wait on the seq.
t=0; while [ $t -lt 30 ]; do AFTER=$(seq_of "user +$ID setup"); [ -n "$AFTER" ] && break; sleep 5; t=$((t+5)); done
if [ -n "${AFTER:-}" ]; then score J1 PASS "the setup line reached the kernel after ${t}s, at seq $AFTER"
else score J1 FAIL "the setup line never reached the kernel within 30s"; fi
echo "anchor ${AFTER:-none}" | tee -a $O/run.txt
wait_hist J1s "assistant .*[Ss]eeded" 180 >/dev/null || score J1 FAIL "seeding never finished"
# the read frame is confined to .arbos/ (kernel), so the seed is scored on the kernel's own tool records
if hist | grep -qE "tool +(write|edit) +$DIR/tests/test_math.py|tool +bash .*(test_math|set -e|git init)" && hist | grep -qE "assistant .*[Ss]eeded"; then score J1 PASS "project opened; kernel record shows the test written and a commit"; else score J1 FAIL "no record of the test file / commit after the seed line"; fi
shot J1-seeded

# J2 — the real challenge (QA's prompt, scoped to the folder); the first 45 s recorded (workers appearing)
# A recorder left behind by an interrupted run holds the device, and every
# later `recordVideo` fails with "Host recording is already in progress" —
# silently, because the output went to /dev/null, so cycle 55's first run
# produced no video and only ffmpeg's missing-file error said so.
pkill -INT -f "simctl io.*recordVideo" 2>/dev/null; sleep 2
xcrun simctl io "$U" recordVideo --codec h264 --force "$O/j2-raw.mp4" > "$O/record.log" 2>&1 & REC=$!
sleep 2; grep -q "already in progress" "$O/record.log" 2>/dev/null && echo "recording refused: the device still has a recorder on it" | tee -a $O/run.txt
# the spawn tool record lands in the transcript only when the call ends (wait:true); the child's own
# `turn running` frame is live, so a frame log is what says "a worker is running now"
(python3 "$HERE/frame-log.py" $TARGET 400 > $O/frames.log 2>&1 &)
type_send "$ID challenge: in $DIR, through one worker you wait for: fix the failing test, then add perimeter(w, h) and diagonal(w, h) to mathlib.py with three unit tests each (including edge cases), add a CHANGELOG.md entry describing every change, make 'python3 -m unittest -q' from the project folder pass (fix discovery if needed), and commit the work on a branch (not main). Tell me the branch name and the test output's last line."
wait_hist J2 "user +$ID challenge" 30; C=$(seq_of "user +$ID challenge"); AFTER=$C
# Without the challenge line there is no run to score: everything below would
# be reading the transcript from before it started.
if [ -z "$C" ]; then score J2 FAIL "the challenge never reached the kernel — nothing below this line was exercised"; fi
sleep 8; shot J2-busy
t=0; while [ $t -lt 120 ]; do grep -qE '"state": "running"' $O/frames.log 2>/dev/null && break; hist | grep -qE "tool +spawn" && break; hist | grep -qE "turn_complete" && break; sleep 1; t=$((t+1)); done
sleep 3; SPAWNERR=$(hist | grep -E "tool +spawn.*ERROR:" | head -1 | sed 's/.*ERROR: //')
if [ -z "$C" ]; then :; elif [ -n "$SPAWNERR" ]; then score J2 FAIL "spawn refused: $SPAWNERR"; elif grep -qE '"state": "running"' $O/frames.log 2>/dev/null || hist | grep -qE "tool +spawn"; then score J2 PASS "worker running after ${t}s"; else score J2 U "no worker: the root did it itself (allowed)"; fi
shot J2-workers

kill -INT $REC 2>/dev/null; sleep 4
if [ -s "$O/j2-raw.mp4" ]; then
  ffmpeg -v error -y -i "$O/j2-raw.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$O/recording-challenge-workers.mp4" && rm -f "$O/j2-raw.mp4"
else
  echo "no recording: $(tail -1 "$O/record.log" 2>/dev/null)" | tee -a $O/run.txt
fi
# J4 — follow up mid-flight (as soon as the work is running), then after
type_send "Also add a line to the CHANGELOG saying who asked for this: QA-$ID."
wait_hist J4m "user +Also add a line to the CHANGELOG" 30 >/dev/null; F=$(seq_of "user +Also add a line")
# J5 — steer + read-only ask while it works (the phone has no Stop: interrupt is U)
type_send "Use British spelling in the CHANGELOG."
wait_hist J5s "user +Use British spelling" 30 >/dev/null
type_send "What time is it, roughly? One line."
wait_hist J5q "user +What time is it" 30 >/dev/null
shot J5-asked
# J8c — drop the link mid-turn (the phone owns this): 25 s without 443
echo "block drop out quick proto tcp from any to any port 443" | sudo pfctl -ef - 2>/dev/null; echo "$(date -u +%H:%M:%S) link cut"
sleep 12; shot J8c-link-down; sleep 13; sudo pfctl -d 2>/dev/null; sudo pfctl -F all 2>/dev/null; echo "$(date -u +%H:%M:%S) link back"
# J6 — background while it works, come back
idb ui button HOME --udid $U; sleep 40; shot J6-home; xcrun simctl launch $U $B >/dev/null 2>&1; sleep 4; shot J6-back
# Read it rather than look at it. Both halves of J6 are in the tree: the chat
# is intact if it still has a Back and some transcript, and the away card
# says "While you were away" when anything arrived. This was scored EYE for
# its whole life, which made an acceptance run depend on somebody opening a
# screenshot — and the standing order asks for counted evidence.
J6_BACK=$(python3 "$HERE/ui.py" $U dump 2>/dev/null | grep -cE "Button +Back")
J6_ROWS=$(python3 "$HERE/ui.py" $U dump 2>/dev/null | grep -cE "StaticText")
J6_CARD=$(python3 "$HERE/ui.py" $U dump 2>/dev/null | grep -c "While you were away")
# J3 — watch it work honestly: the challenge turn ends; no two identical assistant lines in a row; no status-as-prose
wait_hist J3 "turn_complete" 300 >/dev/null
sleep 5; hist > $O/after-challenge.txt
DUP=$(grep -E "^ *[0-9]+ assistant" $O/after-challenge.txt | awk '{$1="";$2=""; print}' | uniq -d | wc -l | tr -d ' ')
STATUS=$(grep -cE "^ *[0-9]+ assistant +status[ \"]" $O/after-challenge.txt | tr -d ' ')
[ "$DUP" = "0" ] && [ "$STATUS" = "0" ] && score J3 PASS "turn ended; no repeated assistant line; no status-as-prose" || score J3 FAIL "dup=$DUP status-as-prose=$STATUS"
shot J3-done
# J4 (cont.) — was the mid-flight line taken by the running work, not a second worker?
SPAWNS_AFTER=$(awk -v f="$F" '$1+0 > f+0' $O/after-challenge.txt | grep -cE "tool +spawn" | tr -d ' ')
ENDED_BEFORE=$(awk -v c="$C" -v f="$F" '$1+0 > c+0 && $1+0 < f+0' $O/after-challenge.txt | grep -c turn_complete | tr -d ' ')
type_send "Summarise what you changed in two lines."
wait_hist J4a "user +Summarise what you changed" 30 >/dev/null; S=$(seq_of "user +Summarise")
t=0; while [ $t -lt 90 ]; do hist | awk -v s="$S" '$1+0 > s+0' | grep -qE "assistant" && break; sleep 5; t=$((t+5)); done
ANS=$(hist | awk -v s="$S" '$1+0 > s+0' | grep -cE "assistant" | tr -d ' ')
# J5 — the read-only ask was answered
TIME_ANS=$(hist | awk -v f="$F" '$1+0 > f+0' | grep -ciE "assistant .*([0-9]{1,2}:[0-9]{2}|o.clock|UTC|morning|afternoon|evening|around|about)" | tr -d ' ')
# The Stop half of J5 is not exercised here, and for eleven runs this line
# said the phone had no Stop control at all. It has one — the composer's
# stop square, scored at cycle 40 (M-130) and labelled `Stop` in the tree —
# and it is absent at this moment only because the turn has already ended.
# QA imports these verdicts, so a scorer that states an app fact must be
# right about it or say nothing.
[ "$TIME_ANS" != "0" ] && score J5 U "read-only ask answered; Stop not exercised by this step (the control exists: composer stop square, M-130)" || score J5 FAIL "the read-only ask got no answer"
# J7 — the result on disk, read through the kernel's read frame (not the model's word)
# the worker's work may live on a branch in a worktree: read the branch, not the checkout's HEAD
type_send "$ID verify: in $DIR, run these git and python commands yourself and paste the raw output under the headings BRANCHES, CHANGELOG, RETURNS, TESTS, AHEAD, nothing else: (1) list every local branch with its last commit date, newest first; (2) show the CHANGELOG.md from the newest branch that is not main and not the initial setup branch; (3) grep the return lines of mathlib.py on that branch; (4) check that branch out in a detached temporary worktree and run python3 -m unittest -q there, keep only the last line; (5) count that branch commits ahead of main."
wait_hist J7v "user +$ID verify" 30 >/dev/null; V=$(seq_of "user +$ID verify")
t=0; while [ $t -lt 120 ]; do hist | awk -v s="$V" '$1+0 > s+0' | grep -qiE "assistant .*(AHEAD|TESTS)" && break; sleep 5; t=$((t+5)); done
sleep 8; python3 "$HERE/kernel.py" $TARGET history 40 2>/dev/null | awk -v s="$V" '$1+0 > s+0' | grep -E "assistant" | tail -1 > $O/verify.txt
VR=$(cat $O/verify.txt)
if echo "$VR" | grep -q "QA-$ID"; then [ "$ENDED_BEFORE" != "0" ] && score J4 U "CHANGELOG carries QA-$ID, but the challenge turn had ended before the line (mid-flight half unverified); answer to the summary: $ANS" || score J4 PASS "CHANGELOG carries QA-$ID, taken by the running work (spawns after: $SPAWNS_AFTER); summary answered: $ANS"; else score J4 FAIL "CHANGELOG lacks QA-$ID (spawns after: $SPAWNS_AFTER, answer: $ANS)"; fi
[ "$SPAWNS_AFTER" != "0" ] && score J4 FAIL "a second worker was spawned for the follow-up"
# score on the fields, not the paste's shape: the root may summarise the worker's report
if echo "$VR" | grep -qiE "changelog" && echo "$VR" | grep -qE "w *\* *h|width *\* *height|multipl" && echo "$VR" | grep -qE "\bOK\b" && echo "$VR" | grep -qiE "(fix|feat)[A-Za-z0-9_./-]*" && echo "$VR" | grep -qiE "AHEAD[^0-9]{0,40}[1-9]" ; then score J7 PASS "kernel-reported: CHANGELOG present, area = w * h, unittest OK, branch with commits — $(echo "$VR" | cut -c1-160)"; else score J7 FAIL "kernel-reported: $(echo "$VR" | cut -c1-200)"; fi
shot J7-verified
# J6 — scored: chat intact on return; notifications by the away card / badge
# The card is the half that can legitimately be absent: nothing may have
# arrived in those forty seconds. The chat being intact is not optional.
if [ "${J6_BACK:-0}" = 0 ] || [ "${J6_ROWS:-0}" -lt 2 ]; then
  score J6 FAIL "came back to no chat — Back x${J6_BACK:-0}, ${J6_ROWS:-0} text row(s) (J6-back.png)"
elif [ "${J6_CARD:-0}" != 0 ]; then
  score J6 PASS "chat intact on return (${J6_ROWS} text rows) and an away card was waiting"
else
  score J6 U "chat intact on return (${J6_ROWS} text rows); no away card, which is only a fault if something arrived while away (J6-back.png)"
fi
score J8 U "a: kernel restart mid-turn — not possible on a hosted kernel from the phone; b: second project — see P-runs; c: link cut 25 s mid-turn — turn finished after the link returned (J8c-link-down.png, J3)"

# P1 — dictate a follow-up; his tap sends.
# By label, for the reason type_send is: the composer row is not at y=788,
# and the button on its right changes from Microphone to Up the moment there
# are words to send, so a fixed point hits whichever happens to be there.
# The one button on the right of the composer is three buttons in turn:
# Microphone, then Stop while it listens, then Up once there are words to
# send. A second tap on "Microphone" does not stop it, because by then there
# is no Microphone there — which is why this step sent nothing for months.
ui tap "Microphone" || score P1 FAIL "no microphone button on the composer"
sleep 3; shot P1-listening; sleep 8
ui tap "Stop" >/dev/null 2>&1 || echo "P1: no Stop button; dictation may not have started" | tee -a $O/run.txt
sleep 3; shot P1-dictated
heard=$(ui field plain 2>/dev/null)
if [ -z "$heard" ]; then score P1 FAIL "dictation put nothing in the composer"; else
  echo "P1 heard: $heard" | tee -a $O/run.txt
  ui tap "Send" || score P1 FAIL "dictated words in the box but no send button: $(ui dump | awk '$2 > 740 && $2 < 830')"
fi
# Matched on the words the recogniser does not get to choose. The clip says
# "summarise" and iOS hears "summaries"; the check wanted "summarize", so P1
# failed for months on a spelling while its line was reaching the kernel
# every time (run 33, seq 2326). What the step is testing is that dictation
# reaches the kernel at all, not how iOS renders one word.
wait_hist P1 "user +.*what the workers did" 30
# P2/P3 recorded when RECORD_P=1 (the every-third-cycle recording)
if [ "${RECORD_P:-0}" = "1" ]; then xcrun simctl io "$U" recordVideo --codec h264 --force "$O/p-raw.mp4" >/dev/null 2>&1 & PREC=$!; fi
# P2 — attach a photo.
# The picker itself is another process, so `describe-all` cannot see inside
# it and the taps within it have to be points. What must not be left to luck
# is getting *out*: a picker still open swallows everything after it, and in
# run 32 it swallowed the photo line and the whole call step — P3's stills
# are of the photo grid. So the way in is by name, the way out is checked.
ui tap "Add" || score P2 FAIL "no attachment button on the composer"
sleep 1.5
ui tap "Photo Library" || score P2 FAIL "no Photo Library in the attachment menu"
sleep 4
# Measured off `P2-chip.png` in pixels and converted, because that is the
# only way to find anything in here and a coordinate read off a still is
# 1.2x the point size on this device. The old tap for the photo was at
# y=230 points, which is the "Private Access to Photos" banner, not the
# grid — so nothing was ever selected and the tick stayed disabled.
tap_shot 78 470 "$U"; sleep 1      # a photo in the grid, below the privacy banner
tap_shot 426 157 "$U"; sleep 3     # the picker's tick, which is what closes it
SWIPED=0
for _ in 1 2 3; do
  ui field >/dev/null 2>&1 && break
  SWIPED=1
  echo "P2: the picker is still up; swiping it away" | tee -a $O/run.txt
  idb ui swipe 196 300 196 850 --duration 0.4 --udid $U; sleep 2
done
if ! ui field >/dev/null 2>&1; then
  score P2 FAIL "the photo picker would not close; nothing after this was exercised"
elif [ "$SWIPED" = 1 ]; then
  # The tick is what closes the picker. Having to swipe means it was never
  # pressed, so nothing was attached and the question below is about no
  # photo at all — which the model will answer anyway, plausibly.
  score P2 FAIL "no photo attached: the picker had to be dismissed by hand, so the tick was missed"
fi
shot P2-chip
# Asking "what is in this photo" invites a plausible answer whether or not
# one arrived, and P2 scored on the reply not sounding like a refusal — so
# for the months the picker was silently attaching nothing, it passed. The
# prompt now gives the model an exact sentence for the negative, and the
# check below reads the subject.
type_send "$ID photo: name the subject of the attached photo and its main colour, in one short sentence. If no image reached you, say exactly: no image reached me."
wait_hist P2e "user +$ID photo" 30 >/dev/null; PA=$(seq_of "user +$ID photo")
t=0; R=""; while [ $t -lt 90 ]; do R=$(hist | awk -v a="${PA:-0}" '$1+0 > a+0' | grep -E "^ *[0-9]+ assistant" | tail -1); [ -n "$R" ] && break; sleep 5; t=$((t+5)); done
shot P2-photo-reply
if [ -z "$R" ]; then score P2 FAIL "no reply within 90s"
elif echo "$R" | grep -qiE "no image reached me|didn.t (arrive|reach|come)|did not (arrive|reach|come)|no .?attachments|can.t see|cannot see|nothing at that path"; then score P2 FAIL "photo did not reach the model: $(echo "$R" | cut -c1-100)"
elif echo "$R" | grep -qiE "flower|blossom|petal|magenta|pink|bloom|waterfall|leaf|leaves|green"; then score P2 PASS "the model named the picture: $(echo "$R" | cut -c1-120)"
else score P2 U "a reply that names neither the picture nor a refusal: $(echo "$R" | cut -c1-140)"; fi
# P3 — call, ask the project a question (must land in THIS project's transcript)
ui menu || score P3 FAIL "no overflow menu in the chat header"
sleep 1.5
ui tap "Call $ROW" || score P3 FAIL "no 'Call $ROW' in the chat menu"
sleep 3
idb ui tap 196 420 --udid $U          # the orb: the call waits for a tap on it
sleep 6; shot P3-call; sleep 16; shot P3-call-answered
wait_hist P3 "user +.*(working on|Arbus|Arbos)" 40
ui tap "End call" >/dev/null 2>&1 || idb ui tap 42 85 --udid $U
sleep 2; grep -E "^metric" $O/console.log | tail -4 | tee $O/call-metrics.txt
if [ -n "${PREC:-}" ]; then kill -INT $PREC 2>/dev/null; sleep 2; ffmpeg -v error -y -i "$O/p-raw.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$O/recording-photo-and-call.mp4" && rm -f "$O/p-raw.mp4"; fi
# PUSH — the hub's own report (#333): enabled or why not; a test alert when the key exists
"$HERE/push-check.sh" 2>&1 | tee $O/push-check.txt | grep -E "PUSH (status|verdict)" | sed "s/^/PUSH /" >/dev/null
V=$(grep "PUSH verdict" $O/push-check.txt | head -1); case "$V" in *PASS*) score PUSH PASS "$V";; *OFF*) score PUSH U "$V";; *) score PUSH U "$(grep -m1 'PUSH status' $O/push-check.txt)";; esac
# J6' — kill and reopen: nothing lost
xcrun simctl terminate $U $B; sleep 2; xcrun simctl launch $U $B -noAskNotifications 1 -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 7
reach_the_list "$U" || score J6k FAIL "could not get to the projects list after the relaunch"
shot J6k-list
ui tap "$ROW" || score J6k FAIL "no $ROW row after the relaunch"
sleep 6; shot J6k-reopened
# Counted, for the same reason as J6. "Ends where it ended" is the chat
# showing its tail: a composer to type into and no away card left unread.
J6K_FIELD=$(python3 "$HERE/ui.py" $U dump 2>/dev/null | grep -cE "TextField")
J6K_CARD=$(python3 "$HERE/ui.py" $U dump 2>/dev/null | grep -c "While you were away")
if [ "${J6K_FIELD:-0}" = 0 ]; then
  score J6k FAIL "the reopened chat has no composer (J6k-reopened.png)"
elif [ "${J6K_CARD:-0}" != 0 ]; then
  score J6k U "reopened with a composer, but an away card is still showing — it may have arrived after the reopen (J6k-reopened.png)"
else
  score J6k PASS "reopened at its end: a composer to type into and no card left pending"
fi
python3 "$HERE/kernel.py" $TARGET history 150 > $O/transcript-tail.txt 2>/dev/null
# Again, now the run is over: a kernel replaced under a run has happened
# here, and a run that measured two builds must say so rather than pick one.
python3 "$HERE/kernel.py" $TARGET hello > $O/kernel-version-end.txt 2>&1 || true
echo "--- score"; cat $O/score.txt
echo "--- kernel"; head -1 $O/kernel-version.txt
# The app's own build. This was never set, so every run record ever written
# says "main@unknown": the journey named the kernel it tested and could not
# name the app. The checkout's HEAD is the app's build only while the
# installed binary is not older than the sources, so that is asked too,
# rather than assumed — the same question mac-cycle.sh asks after it installs.
if [ -z "${APP_BUILD:-}" ]; then
  REPO=$(cd "$HERE/../.." && pwd)
  BR=$(git -C "$REPO" rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)
  SHA=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)
  APP_BUILD="$BR@$SHA"
  INSTALLED=$(xcrun simctl get_app_container "$U" "$B" app 2>/dev/null)
  if [ -n "$INSTALLED" ]; then
    REF=$(mktemp) && touch -r "$INSTALLED/Arbos" "$REF"
    NEWER=$(find "$REPO/ios" -name '*.swift' -newer "$REF" -print -quit 2>/dev/null)
    rm -f "$REF"
    [ -n "$NEWER" ] && APP_BUILD="$APP_BUILD (the app on the device is older than $(basename "$NEWER"))"
  else
    APP_BUILD="$APP_BUILD (could not date the installed app)"
  fi
fi
echo "--- app   $APP_BUILD"
python3 "$HERE/journey-record.py" "$O" "$TARGET" "$APP_BUILD" "${RUN_NOTES:-}"

# What this run covers, named from its own declaration, so whoever writes the
# ledger credits every row it exercised. Cycle 157 ran this and credited one
# of its two rows; the rotation then sent cycle 169 to re-run a row that had
# passed twelve cycles earlier, because a row's age is the last cycle that
# *named* it.
echo
echo "coverage rows this run exercised:"
grep "^# COVERS:" "$0" | sed 's/^# COVERS: */  /'
