> **RECOVERED, INCOMPLETE — this is not the original file.** 8,843 of the original 17,480 bytes, about half. Source: the output of a `cat` of this document captured in the features worker's transcript at 2026-09-13 00:02 UTC; the capture ends part-way through the capture-script listing. Additions up to the last known write (2026-09-13 08:19 UTC) are missing. Owner: `bc-85561744-5b07-5bba-8097-d7eb6858d024`.
>
> The original was lost together with the whole `docs/` directory on 2026-09-16 between 07:43 and 09:01 UTC. Restored by the store-recovery worker `bc-0b112226-cf98-5cab-92c3-2671518dd9b9`. Cause, timeline and the full recovery inventory: `internal/store-docs-loss-2026-09-16.md`.

# Cursor parity process

How we check that the Arbos desktop app looks and feels like Cursor's agent chat, down to every detail. Written 2026-09-12.

## 1. What the parity suite is

The parity suite is a fixed set of prompts, run the same way in two apps, with screen captures of each step.

- App A: Cursor desktop (the reference). Its Agents Window is the target look.
- App B: Arbos desktop (`arbos-desktop`, crate `desktop/` on branch `rust`).

For each prompt we record a short video and a few stills (screenshots) of the chat while the agent works and after it finishes. A person or an agent then compares the two sets, aspect by aspect, and writes every difference down as a gap. Each gap becomes a task.

Jargon used in this doc:

- Composer: the chat text box where you type a prompt.
- Transcript: the scrolling list of messages and status lines above the composer.
- Status line: the small muted text that says what the agent is doing right now ("Exploring 2 files").
- Shimmer: a light band that sweeps across text to show it is live. Cursor uses it on status lines.
- Sub-agent: an agent started by another agent to do one part of the task.
- Background agent: an agent that runs while you do other things; Cursor shows a count of them above the composer.
- Still: a single screenshot (PNG). Recording: a video (MP4).

## 2. The prompt set

The prompts live in one file so both apps get exactly the same text:
`/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/parity/prompts.txt`

| Id | What it exercises | Prompt |
|---|---|---|
| p1-question | Short streamed answer; done state | What does main.py print when run? Answer in one line. |
| p2-multifile | Multi-file edit; diff summary; file chips | Add a mul(a, b) function to math_utils.py and call it from main.py with mul(4, 5). Also list it in README.md. |
| p3-subagents | Sub-agents; their status and highlight; agent strip above the composer | Use parallel sub-agents: one reviews math_utils.py for edge cases, one writes docstrings for every function, one drafts a CHANGELOG.md. Then merge their results. |
| p4-longrun | A long turn; working animation; stop button | Write a detailed 400-word explanation of how Python's import system finds math_utils from main.py, thinking step by step. Then run main.py and show the output. |
| p5-voice | Voice input; words appear in the composer as they arrive | (Speak) "Add a divide function that raises on zero." |
| p6-followup | Send while p4 is running; queued message strip | Also add type hints to every function. |

All prompts run inside one tiny sample project (`/tmp/parity-proj`: `README.md`, `math_utils.py`, `main.py`). `seed-project.sh` resets it before every run so the diff is always the same.

Known wobble: p3 depends on the model choosing to spawn sub-agents. In the hand-driven run it spawned two; in the first `run-suite.sh` run it did the work inline. If a run shows no sub-agents, run p3 again or strengthen the wording ("You must spawn three sub-agents").

## 3. How captures are made

Everything runs on the VM's X display `:1` (1920x1200). Tools, all standard Linux packages:

- `xdotool`: moves the mouse, clicks, types text, and presses keys.
- `scrot`: takes a still of the screen.
- `ffmpeg` with `x11grab`: records the screen to MP4 at 15 frames per second.
- `wmctrl`: lists and closes windows.
- `tmux`: keeps each app running in its own session after the script exits.

Scripts, all under `/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/parity/`. The store does not allow the executable bit, so run each one as `bash <script>`.

| Script | What it does |
|---|---|
| `env.sh` | Shared paths, window names, and helpers (`focus`, `win`, `log`). Sourced by the others. |
| `seed-project.sh` | Resets the sample project to its known state. |
| `cursor-launch.sh` | Downloads the current stable Linux AppImage from Cursor's update API, unpacks it, starts it with the sample project, and dismisses the keyring dialog. |
| `cursor-login-link.sh` | Clicks "Log In", copies the sign-in link out of the browser, and saves it to `/tmp/cursor-login-url.txt`. |
| `arbos-launch.sh` | Builds `arbos-kernel` and `arbos-desktop` (debug) if needed, writes the model config, writes the app state so it opens the sample project, and starts the app. |
| `send-prompt.sh <app> "<text>"` | Clicks the composer and types one prompt. `FIRST=1` targets Arbos's centred empty-chat composer. |
| `shot.sh <app> <name>` | One still to `media/parity/<app>/<name>.png`. |
| `rec.sh start <app> <name>` / `rec.sh stop` | Start and stop an MP4 recording in the same folder. |
| `run-suite.sh <app> [label]` | Runs the whole prompt set and writes captures to `media/parity/<app>/<label>/`. |

Timing inside `run-suite.sh`: for p1-p3 it takes stills at 2, 8, 20, 45, and 90 seconds after Enter, then one at the end. p6 is typed 5 seconds after p4 so it lands while p4 still works. p5 runs only when `arecord -l` finds a microphone; this VM has none.

Window geometry is fixed by `focus`: 1600x1000 at screen position (100,60). Fixed geometry means the composer is always at the same pixel, which is what lets the scripts click without a vision step.

### Fixed capture points per prompt

- After typing, before Enter: the composer with text in it.
- 2 s after Enter: the first working state (status line, spinner, stop button).
- Mid-turn: sub-agents alive, strip above composer, sidebar indicator.
- End: final answer, diff summary, feedback icons, composer back to idle.

### Setting up the VM (one time)

System packages for Arbos on Linux:
`mesa-vulkan-drivers libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev libssl-dev libasound2-dev libwayland-dev libxcb-*-dev` plus `scrot xdotool wmctrl xclip ffmpeg`.
======
#!/usr/bin/env bash
# Run the whole prompt set against one app and capture every step.
#
#   run-suite.sh <app> [run-label]
#
# <app> = cursor | arbos.  [run-label] defaults to today's date. Output goes to
# $MEDIA/<app>/<run-label>/ so runs can be compared side by side.
#
# For each prompt in prompts.txt the script:
#   1. starts an mp4 recording,
#   2. types the prompt and presses Enter,
#   3. takes stills at 2 s, 8 s, 20 s, 45 s, and 90 s,
#   4. stops the recording.
# p5 (voice) is skipped when the VM has no microphone; p6 is sent 5 s after p4
# so it lands while p4 is still working.
#
# Before running: the app must be up (cursor-launch.sh / arbos-launch.sh).
# The script opens a fresh chat itself: Arbos by clicking "New Chat" in the
# sidebar, Cursor with Ctrl+N in the Agents window.
set -euo pipefail
source "$(dirname "$0")/env.sh"

app="$1"; label="${2:-$(date +%F)}"
outdir="$MEDIA/$app/$label"; mkdir -p "$outdir"

# Local copies of shot.sh / rec.sh that write into $outdir.
shot() { scrot -o "$outdir/$1.png"; }
rec_start() {
  ffmpeg -loglevel error -y -f x11grab -framerate 15 -video_size "$SCREEN" -i "$DISPLAY" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p "$outdir/$1.mp4" </dev/null >/dev/null 2>&1 &
  echo $! > /tmp/parity-suite-rec.pid
}
rec_stop() { kill -INT "$(cat /tmp/parity-suite-rec.pid)" 2>/dev/null || true; sleep 2; }

bash "$PARITY/seed-project.sh"

send() { bash "$PARITY/send-prompt.sh" "$app" "$1"; }

# Open a fresh chat so every run starts from the same empty state.
# Arbos: the "New Chat" button at the top of the sidebar (163,139).
case "$app" in
  arbos)  focus "$ARBOS_WIN"; xdotool mousemove 163 139 click 1; sleep 2; first=1 ;;
  cursor) focus "$CURSOR_WIN"; xdotool key ctrl+n; sleep 2; first=0 ;;
esac
shot 00-empty-chat

mapfile -t lines < <(grep -v '^#' "$PARITY/prompts.txt" | grep -v '^$')
declare -A prompt
for l in "${lines[@]}"; do id="${l%%|*}"; text="${l##*|}"; prompt[$id]="$text"; done

for id in p1-question p2-multifile p3-subagents; do
  log "== $id"
  rec_start "$id"; sleep 1
  FIRST="$first" send "${prompt[$id]}"; first=0
  # Cumulative time points: 2, 8, 20, 45, 90 s after Enter.
  at=0; for t in 2 6 12 25 45; do sleep "$t"; at=$((at + t)); shot "$id-t${at}s"; done
  shot "$id-end"; rec_stop
done

log "== p4-longrun + p6-followup"
rec_start p4-longrun-p6-followup; sleep 1
send "${prompt[p4-longrun]}"; sleep 2; shot p4-t2s
sleep 3; send "${prompt[p6-followup]}"; sleep 2; shot p6-queued
for t in 10 20 30; do sleep "$t"; shot "p4-t+${t}s"; done
shot p4-p6-end; rec_stop

if arecord -l 2>/dev/null | grep -q card; then
  log "== p5-voice (microphone found)"
  rec_start p5-voice; sleep 1
  # Both apps: click the mic, wait, click again. Coordinates from send-prompt.sh.
  case "$app" in cursor) xdotool mousemove 1300 1022 click 1 ;; arbos) xdotool mousemove 1344 1022 click 1 ;; esac
  sleep 4; shot p5-recording; xdotool mousemove 1344 1022 click 1; sleep 3; shot p5-end; rec_stop
else
  log "== p5-voice skipped: no microphone on this VM"
fi

ls -la "$outdir"
log "done -> $outdir"
