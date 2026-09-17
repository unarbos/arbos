#!/usr/bin/env bash
# Build (if needed) and launch arbos-desktop on display :1 with $PROJ open.
#
#   arbos-launch.sh [--rebuild]  -> Arbos window up; log at /tmp/arbos-app.log
#
# Needs: the arbos repo at $ARBOS_SRC (default: this checkout), the Linux
# system packages the desktop README lists, and a model key. The key comes from
# 1Password (vault Arbos, item OPENROUTER) and Arbos reads it through
# ~/.config/arbos/config.toml, which points at OpenRouter's OpenAI-style API.
set -euo pipefail
source "$(dirname "$0")/env.sh"

if [ "${1:-}" = "--rebuild" ] || [ ! -x "$ARBOS_BIN" ] || [ ! -x "$KERNEL_BIN" ]; then
  log "building arbos-kernel and arbos-desktop (debug); log at /tmp/arbos-build.log"
  # A build that fails must stop the run here, loudly, with its log kept —
  # not leave the previous binary in target/ for the launch below to run as
  # if it were this tree's (rig audit R21; the QA loop's one-line
  # "app build failed" cost it fifteen scenarios and the journey).
  if ! (cd "$ARBOS_SRC" && cargo build -p arbos-kernel) >/tmp/arbos-build.log 2>&1; then
    tail -40 /tmp/arbos-build.log >&2
    log "BUILD FAILED: arbos-kernel (see /tmp/arbos-build.log); nothing launched"
    exit 1
  fi
  if ! (cd "$ARBOS_SRC/desktop" && cargo build) >>/tmp/arbos-build.log 2>&1; then
    tail -40 /tmp/arbos-build.log >&2
    log "BUILD FAILED: arbos-desktop (see /tmp/arbos-build.log; the Linux packages are in desktop/BUILDING.md); nothing launched"
    exit 1
  fi
fi

# The binary about to run must be this tree's. `arbos-desktop --version`
# prints `<version> <build> <sha>[-dirty]`; a sha that is not HEAD's means a
# stale binary (a failed build, an old copy), and the run would measure the
# wrong thing. ARBOS_ALLOW_STALE=1 says so on purpose (a release under test).
tree_sha="$(git -C "$ARBOS_SRC" rev-parse --short=7 HEAD 2>/dev/null || true)"
bin_line="$("$ARBOS_BIN" --version 2>/dev/null | head -n1 || true)"
if [ -n "$tree_sha" ] && [ "${ARBOS_ALLOW_STALE:-0}" != "1" ] && ! grep -q -- "${tree_sha}" <<<"$bin_line"; then
  log "STALE BINARY: $ARBOS_BIN is '$bin_line', tree is $tree_sha — rebuild (--rebuild) or set ARBOS_ALLOW_STALE=1; nothing launched"
  exit 1
fi
log "desktop under test: $bin_line (tree $tree_sha)"

# Model access. OpenRouter speaks the same HTTP API as OpenAI, so Arbos's
# OpenAI client works with it unchanged.
if [ -z "${OPENROUTER_API_KEY:-}" ]; then
  OPENROUTER_API_KEY="$(op item get xbirrctuljw2m6aieoway2szom --vault Arbos --fields label=credential --reveal)"
fi
mkdir -p ~/.config/arbos ~/.config/arbos-desktop
cat > ~/.config/arbos/config.toml <<EOF
model = "${PARITY_MODEL:-google/gemini-2.5-flash}"
api_base = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
EOF

bash "$PARITY/seed-project.sh"

# Tell the desktop which project to show. state.toml is the app's own
# remembered-windows file; writing it before launch skips the folder picker.
cat > ~/.config/arbos-desktop/state.toml <<EOF
projects = ["$PROJ"]
recents = ["$PROJ"]
active = 0
appearance = "light"
reduce_transparency = true
cursor_blink = true
text_size = 13.0
bionic_reading = false
hue = 0.0
chroma = 0.0

[last]

[names]

[dismissed]
EOF

S=arbos-app
tmux -f /exec-daemon/tmux.portal.conf has-session -t "=$S" 2>/dev/null \
  || tmux -f /exec-daemon/tmux.portal.conf new-session -d -s "$S" -c "$ARBOS_SRC/desktop" -- bash -l
# ARBOS_DRIVER_SOCKET turns on the driver: a Unix socket that takes JSON
# requests (click, type, state, screenshot...). Poke at the running app with
#   python3 $PARITY/arbosdriver.py --socket /tmp/arbos-driver.sock ids
rm -f /tmp/arbos-driver.sock
tmux -f /exec-daemon/tmux.portal.conf send-keys -t "$S:0.0" \
  "export DISPLAY=$DISPLAY OPENROUTER_API_KEY='$OPENROUTER_API_KEY' ARBOS_KERNEL_BIN='$KERNEL_BIN' ARBOS_DRIVER_SOCKET=/tmp/arbos-driver.sock; '$ARBOS_BIN' 2>&1 | tee /tmp/arbos-app.log" C-m

for _ in $(seq 1 30); do
  sleep 1
  if [ -n "$(win "$ARBOS_WIN")" ]; then break; fi
done
focus "$ARBOS_WIN"
wmctrl -l
log "Arbos is up"
