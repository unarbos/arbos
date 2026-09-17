#!/usr/bin/env bash
# Shared settings for the parity scripts. Source this file; do not run it.
#
# Every script in this folder starts with:  source "$(dirname "$0")/env.sh"

export DISPLAY="${DISPLAY:-:1}"

# Where the Project store lives. Captures and docs go under it.
STORE="${STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}"
MEDIA="$STORE/media/parity"          # $MEDIA/cursor and $MEDIA/arbos
PARITY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # this folder

# The small sample project both apps open. Same files for both.
PROJ="${PROJ:-/tmp/parity-proj}"

# Screen size of display :1 on the parity VM.
SCREEN="${SCREEN:-1920x1200}"

# Where the apps live on the VM.
CURSOR_DIR="${CURSOR_DIR:-/tmp/cursor-dl}"                 # AppImage + squashfs-root
ARBOS_SRC="${ARBOS_SRC:-$(cd "$PARITY/../../.." && pwd)}"   # the repository root
ARBOS_BIN="$ARBOS_SRC/desktop/target/debug/arbos-desktop"
KERNEL_BIN="$ARBOS_SRC/target/debug/arbos-kernel"

# Window title patterns (regex for xdotool --name).
CURSOR_WIN='^Cursor Agents$|parity-proj - Cursor$'
ARBOS_WIN='^Arbos$'

mkdir -p "$MEDIA/cursor" "$MEDIA/arbos"

# Print a line with a timestamp. Used by every script.
log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

# Find the first window id whose title matches $1 (a regex).
win() { xdotool search --name "$1" 2>/dev/null | head -1; }

# Bring a window to the front and give it a fixed size and place, so every
# capture has the same geometry. $1 = title regex.
focus() {
  local id; id="$(win "$1")"
  [ -n "$id" ] || { log "no window matching $1"; return 1; }
  xdotool windowactivate --sync "$id"
  xdotool windowsize "$id" 1600 1000
  xdotool windowmove "$id" 100 60
  sleep 0.5
}
