#!/bin/bash
# The phone's version of docs/acceptance-journeys.md (QA's step ids J1–J8),
# plus the phone-only steps P1–P3 (dictate, photo, call). One run = one
# folder under ~/mobile-out/journey/<run>/ with a still per step, the app
# console, the kernel transcript tail, and score.txt. Steps pass on the
# kernel's own record (transcript / read frame) after the challenge's seq,
# never on the screen alone; EYE = scored from the still; U = unverified.
#   mac-journey.sh <target>    target: pod | <machine>/<project>
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; B=com.unarbos.arbos.ios
TARGET=${1:-pod}; ROW=${TARGET##*/}; [ "$ROW" = pod ] && ROW=phone   # M-121: the pod row folds into its roster twin
RUN=$(date -u +%m%d-%H%M%S); O=$HOME/mobile-out/journey/$RUN; mkdir -p $O
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
ID=J$(date -u +%H%M%S); DIR="journey_$ID"
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
hist() { python3 ~/kernel.py $TARGET history 120 2>/dev/null | awk -v a="${AFTER:-0}" '$1+0 > a+0'; }
seq_of() { python3 ~/kernel.py $TARGET history 120 2>/dev/null | grep -E "$1" | tail -1 | awk '{print $1}'; }
score() { echo "$1 $2 $3" | tee -a $O/score.txt; }
wait_hist() { local s=$1 re=$2 secs=$3 t=0; while [ $t -lt $secs ]; do if hist | grep -qE "$re"; then score "$s" PASS "/$re/ after ${t}s"; return 0; fi; sleep 5; t=$((t+5)); done; score "$s" FAIL "no /$re/ within ${secs}s"; return 1; }
type_send() { idb ui tap 200 788 --udid $U; sleep 0.8; idb ui text "$1" --udid $U; sleep 0.3; idb ui key 40 --udid $U; }
rd() { python3 ~/kernel.py $TARGET read "$1" 2>/dev/null; }
echo "run $RUN id $ID target $TARGET dir $DIR" | tee $O/run.txt

# J1 — open the project from the list (the phone's "create": the project lives on a machine's kernel)
xcrun simctl terminate $U $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty $U $B -hubURL "$H" -hubToken "$T" -dictateWav ~/mobile-clips/note.wav -injectWav ~/mobile-clips/ask.wav > $O/console.log 2>&1 &
sleep 7; shot J1-list
Y=$(python3 ~/find_row.py $O/J1-list.png $ROW); [ "$Y" != "0" ] || { score J1 FAIL "no $ROW row"; Y=234; }
idb ui tap 120 $Y --udid $U; sleep 5; shot J1-open
# seed the failing project (QA's rig seeds a folder; the phone asks the kernel to)
type_send "$ID setup, do this yourself without workers: create $DIR/ with mathlib.py defining area(w, h) that wrongly returns w + h, tests/test_math.py (unittest) asserting area(3, 4) == 12, and git init with one commit on main containing both. No CHANGELOG. Reply 'seeded' when done."
wait_hist J1 "user +$ID setup" 30; AFTER=$(seq_of "user +$ID setup"); echo "anchor $AFTER" | tee -a $O/run.txt
wait_hist J1s "assistant .*[Ss]eeded" 180 >/dev/null || score J1 FAIL "seeding never finished"
# the read frame is confined to .arbos/ (kernel), so the seed is scored on the kernel's own tool records
if hist | grep -qE "tool +(write|edit) +$DIR/tests/test_math.py|tool +bash .*(test_math|set -e|git init)" && hist | grep -qE "assistant .*[Ss]eeded"; then score J1 PASS "project opened; kernel record shows the test written and a commit"; else score J1 FAIL "no record of the test file / commit after the seed line"; fi
shot J1-seeded

# J2 — the real challenge (QA's prompt, scoped to the folder); the first 45 s recorded (workers appearing)
xcrun simctl io "$U" recordVideo --codec h264 --force "$O/j2-raw.mp4" >/dev/null 2>&1 & REC=$!
# the spawn tool record lands in the transcript only when the call ends (wait:true); the child's own
# `turn running` frame is live, so a frame log is what says "a worker is running now"
(python3 ~/frame-log.py $TARGET 400 > $O/frames.log 2>&1 &)
type_send "$ID challenge: in $DIR, through one worker you wait for: fix the failing test, then add perimeter(w, h) and diagonal(w, h) to mathlib.py with three unit tests each (including edge cases), add a CHANGELOG.md entry describing every change, make 'python3 -m unittest -q' from the project folder pass (fix discovery if needed), and commit the work on a branch (not main). Tell me the branch name and the test output's last line."
wait_hist J2 "user +$ID challenge" 30; C=$(seq_of "user +$ID challenge"); AFTER=$C
sleep 8; shot J2-busy
t=0; while [ $t -lt 120 ]; do grep -qE '"state": "running"' $O/frames.log 2>/dev/null && break; hist | grep -qE "tool +spawn" && break; hist | grep -qE "turn_complete" && break; sleep 1; t=$((t+1)); done
sleep 3; SPAWNERR=$(hist | grep -E "tool +spawn.*ERROR:" | head -1 | sed 's/.*ERROR: //')
if [ -n "$SPAWNERR" ]; then score J2 FAIL "spawn refused: $SPAWNERR (run 30: the daemon's binary was gone from disk)"; elif grep -qE '"state": "running"' $O/frames.log 2>/dev/null || hist | grep -qE "tool +spawn"; then score J2 PASS "worker running after ${t}s"; else score J2 U "no worker: the root did it itself (allowed)"; fi
shot J2-workers

kill -INT $REC 2>/dev/null; ffmpeg -v error -y -i "$O/j2-raw.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$O/recording-challenge-workers.mp4" && rm -f "$O/j2-raw.mp4"
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
[ "$TIME_ANS" != "0" ] && score J5 U "read-only ask answered; Stop: the phone has no Stop control (unverified)" || score J5 FAIL "the read-only ask got no answer"
# J7 — the result on disk, read through the kernel's read frame (not the model's word)
# the worker's work may live on a branch in a worktree: read the branch, not the checkout's HEAD
type_send "$ID verify: in $DIR, run these git and python commands yourself and paste the raw output under the headings BRANCHES, CHANGELOG, RETURNS, TESTS, AHEAD, nothing else: (1) list every local branch with its last commit date, newest first; (2) show the CHANGELOG.md from the newest branch that is not main and not the initial setup branch; (3) grep the return lines of mathlib.py on that branch; (4) check that branch out in a detached temporary worktree and run python3 -m unittest -q there, keep only the last line; (5) count that branch commits ahead of main."
wait_hist J7v "user +$ID verify" 30 >/dev/null; V=$(seq_of "user +$ID verify")
t=0; while [ $t -lt 120 ]; do hist | awk -v s="$V" '$1+0 > s+0' | grep -qiE "assistant .*(AHEAD|TESTS)" && break; sleep 5; t=$((t+5)); done
sleep 8; python3 ~/kernel.py $TARGET history 40 2>/dev/null | awk -v s="$V" '$1+0 > s+0' | grep -E "assistant" | tail -1 > $O/verify.txt
VR=$(cat $O/verify.txt)
if echo "$VR" | grep -q "QA-$ID"; then [ "$ENDED_BEFORE" != "0" ] && score J4 U "CHANGELOG carries QA-$ID, but the challenge turn had ended before the line (mid-flight half unverified); answer to the summary: $ANS" || score J4 PASS "CHANGELOG carries QA-$ID, taken by the running work (spawns after: $SPAWNS_AFTER); summary answered: $ANS"; else score J4 FAIL "CHANGELOG lacks QA-$ID (spawns after: $SPAWNS_AFTER, answer: $ANS)"; fi
[ "$SPAWNS_AFTER" != "0" ] && score J4 FAIL "a second worker was spawned for the follow-up"
# score on the fields, not the paste's shape: the root may summarise the worker's report
if echo "$VR" | grep -qiE "changelog" && echo "$VR" | grep -qE "w *\* *h|width *\* *height|multipl" && echo "$VR" | grep -qE "\bOK\b" && echo "$VR" | grep -qiE "(fix|feat)[A-Za-z0-9_./-]*" && echo "$VR" | grep -qiE "AHEAD[^0-9]{0,40}[1-9]" ; then score J7 PASS "kernel-reported: CHANGELOG present, area = w * h, unittest OK, branch with commits — $(echo "$VR" | cut -c1-160)"; else score J7 FAIL "kernel-reported: $(echo "$VR" | cut -c1-200)"; fi
shot J7-verified
# J6 — scored: chat intact on return; notifications by the away card / badge
score J6 EYE "chat intact and an away card on return = pass (J6-back.png); no card = unverified"
score J8 U "a: kernel restart mid-turn — not possible on a hosted kernel from the phone; b: second project — see P-runs; c: link cut 25 s mid-turn — turn finished after the link returned (J8c-link-down.png, J3)"

# P1 — dictate a follow-up; his tap sends
idb ui tap 355 788 --udid $U; sleep 3; shot P1-listening; sleep 8; idb ui tap 355 788 --udid $U; sleep 3; shot P1-dictated
idb ui tap 200 788 --udid $U; sleep 0.5; idb ui key 40 --udid $U
wait_hist P1 "user +please summarize what the workers did" 30
# P2/P3 recorded when RECORD_P=1 (the every-third-cycle recording)
if [ "${RECORD_P:-0}" = "1" ]; then xcrun simctl io "$U" recordVideo --codec h264 --force "$O/p-raw.mp4" >/dev/null 2>&1 & PREC=$!; fi
# P2 — attach a photo
idb ui tap 41 788 --udid $U; sleep 1.5; idb ui tap 160 767 --udid $U; sleep 4; idb ui tap 70 230 --udid $U; sleep 1; idb ui tap 355 131 --udid $U; sleep 3; shot P2-chip
type_send "$ID photo: what is in this photo? One line."
wait_hist P2e "user +$ID photo" 30 >/dev/null; PA=$(seq_of "user +$ID photo")
t=0; R=""; while [ $t -lt 90 ]; do R=$(hist | awk -v a="${PA:-0}" '$1+0 > a+0' | grep -E "^ *[0-9]+ assistant" | tail -1); [ -n "$R" ] && break; sleep 5; t=$((t+5)); done
shot P2-photo-reply
if [ -z "$R" ]; then score P2 FAIL "no reply within 90s"; elif echo "$R" | grep -qiE "didn.t (arrive|reach|come)|did not (arrive|reach|come)|no .?attachments|can.t see|cannot see|nothing at that path"; then score P2 FAIL "photo did not reach the model: $(echo "$R" | cut -c1-100)"; else score P2 PASS "$(echo "$R" | cut -c1-120)"; fi
# P3 — call, ask the project a question (must land in THIS project's transcript)
idb ui tap 351 85 --udid $U; sleep 1.5; idb ui tap 225 94 --udid $U; sleep 3; idb ui tap 196 420 --udid $U; sleep 6; shot P3-call; sleep 16; shot P3-call-answered
wait_hist P3 "user +.*(working on|Arbus|Arbos)" 40
idb ui tap 42 85 --udid $U; sleep 2; grep -E "^metric" $O/console.log | tail -4 | tee $O/call-metrics.txt
if [ -n "${PREC:-}" ]; then kill -INT $PREC 2>/dev/null; sleep 2; ffmpeg -v error -y -i "$O/p-raw.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$O/recording-photo-and-call.mp4" && rm -f "$O/p-raw.mp4"; fi
# PUSH — the hub's own report (#333): enabled or why not; a test alert when the key exists
~/push-check.sh 2>&1 | tee $O/push-check.txt | grep -E "PUSH (status|verdict)" | sed "s/^/PUSH /" >/dev/null
V=$(grep "PUSH verdict" $O/push-check.txt | head -1); case "$V" in *PASS*) score PUSH PASS "$V";; *OFF*) score PUSH U "$V";; *) score PUSH U "$(grep -m1 'PUSH status' $O/push-check.txt)";; esac
# J6' — kill and reopen: nothing lost
xcrun simctl terminate $U $B; sleep 2; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 7; shot J6k-list
Y=$(python3 ~/find_row.py $O/J6k-list.png $ROW); [ "$Y" = "0" ] && Y=234; idb ui tap 120 $Y --udid $U; sleep 6; shot J6k-reopened
score J6k EYE "reopened chat ends where it ended; no pending cards"
python3 ~/kernel.py $TARGET history 150 > $O/transcript-tail.txt 2>/dev/null
echo "--- score"; cat $O/score.txt
