#!/usr/bin/env bash
# Front door for the parity suite.
#
#   run-suite.sh arbos  [label] [binary-dir]   -> python3 parity_arbos.py (driver socket)
#   run-suite.sh cursor [label]                -> pixel-driven loop below (Cursor has no driver)
#
# [label] defaults to today's date. [binary-dir] holds arbos-desktop and
# arbos-kernel; default is the worktree's target/debug. Output goes to
# $MEDIA/<app>/<label>/.
#
# For Cursor: the app must be up and signed in (cursor-launch.sh, then the
# sign-in link). For each prompt the loop records an mp4, types the prompt,
# and takes stills at 2, 8, 20, 45, and 90 s. The composer click point is a
# pixel constant measured from the public video frames; re-measure after
# sign-in works. p5 (voice) runs only when the VM has a microphone.
set -euo pipefail
source "$(dirname "$0")/env.sh"

app="$1"; label="${2:-$(date +%F)}"

if [ "$app" = arbos ]; then
  bindir="${3:-}"
  desktop="${bindir:+$bindir/arbos-desktop}"; kernel="${bindir:+$bindir/arbos-kernel}"
  : "${desktop:=$ARBOS_BIN}" "${kernel:=$KERNEL_BIN}"
  if [ -z "${OPENROUTER_API_KEY:-}" ]; then
    OPENROUTER_API_KEY="$(op item get xbirrctuljw2m6aieoway2szom --vault Arbos --fields label=credential --reveal)"
    export OPENROUTER_API_KEY
  fi
  exec python3 "$PARITY/parity_arbos.py" --label "$label" --binary "$desktop" --kernel "$kernel"
fi

[ "$app" = cursor ] || { echo "app must be arbos or cursor" >&2; exit 2; }

outdir="$MEDIA/cursor/$label"; mkdir -p "$outdir"
shot() { scrot -o "$outdir/$1.png"; }
rec_start() {
  ffmpeg -loglevel error -y -f x11grab -framerate 15 -video_size "$SCREEN" -i "$DISPLAY" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p "$outdir/$1.mp4" </dev/null >/dev/null 2>&1 &
  echo $! > /tmp/parity-suite-rec.pid
}
rec_stop() { kill -INT "$(cat /tmp/parity-suite-rec.pid)" 2>/dev/null || true; sleep 2; }
send() {
  focus "$CURSOR_WIN"
  xdotool mousemove 900 1022 click 1; sleep 0.4
  xdotool type --delay 12 -- "$1"; sleep 0.3; xdotool key Return
  log "sent to cursor: ${1:0:60}"
}

bash "$PARITY/seed-project.sh"
focus "$CURSOR_WIN"; xdotool key ctrl+n; sleep 2
shot 00-empty-chat

mapfile -t lines < <(grep -v '^#' "$PARITY/prompts.txt" | grep -v '^$')
declare -A prompt
for l in "${lines[@]}"; do id="${l%%|*}"; text="${l##*|}"; prompt[$id]="$text"; done

for id in p1-question p2-multifile p3-subagents; do
  log "== $id"
  rec_start "$id"; sleep 1
  send "${prompt[$id]}"
  at=0; for t in 2 6 12 25 45; do sleep "$t"; at=$((at + t)); shot "$id-t${at}s"; done
  shot "$id-end"; rec_stop
done

log "== p4-longrun + p6-followup"
rec_start p4-longrun-p6-followup; sleep 1
send "${prompt[p4-longrun]}"; sleep 2; shot p4-t2s
sleep 3; send "${prompt[p6-followup]}"; sleep 2; shot p6-queued
at=5; for t in 10 20 30; do sleep "$t"; at=$((at + t)); shot "p4-t${at}s"; done
shot p4-p6-end; rec_stop

if arecord -l 2>/dev/null | grep -q card; then
  log "== p5-voice"
  rec_start p5-voice; sleep 1
  xdotool mousemove 1300 1022 click 1; sleep 4; shot p5-recording
  xdotool mousemove 1300 1022 click 1; sleep 3; shot p5-end; rec_stop
else
  log "== p5-voice skipped: no microphone on this VM"
fi

ls -la "$outdir"
log "done -> $outdir"
