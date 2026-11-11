#!/bin/bash
# One GUI app at a time on the EC2 Mac. The main display is shared by every worker's screenshot rig and
# by the release smoke; on 2026-10-22 03:08 two VNC clicks meant for one fixture app landed in another
# worker's app sitting at the same window position. Every script that launches an app on the display,
# clicks, types or captures takes this lock first and releases it when its app is gone.
#
#   gui-lock.sh acquire <owner> [timeout-s]   wait up to timeout-s (default 300) for the display; 0 = got it,
#                                              1 = still busy (owner and age printed), 2 = bad arguments
#   gui-lock.sh release <owner>                release if <owner> holds it (anyone may release a stale lock)
#   gui-lock.sh status                         print the holder, or "free"
#
# Lock file: /Users/Shared/arbos-run/gui.lock — "owner=<name> pid=<pid> since=<ISO>". A lock is stale when
# its pid (the calling script, $PPID — or GUI_LOCK_PID when set) is gone or it is older than GUI_LOCK_STALE_S
# (default 900 s); stale locks are taken over.
# Installed as /Users/Shared/arbos-run/gui-lock.sh (keep in step with scripts/runner/gui-lock.sh).
set -uo pipefail
LOCK=${GUI_LOCK_FILE:-/Users/Shared/arbos-run/gui.lock}
STALE_S=${GUI_LOCK_STALE_S:-900}
cmd=${1:-}; owner=${2:-}

holder() { sed -n 's/.*owner=\([^ ]*\).*/\1/p' "$LOCK" 2>/dev/null | head -n 1; }
holder_pid() { sed -n 's/.*pid=\([0-9]*\).*/\1/p' "$LOCK" 2>/dev/null | head -n 1; }
# BSD stat on the shared Mac; GNU stat keeps the existing shell usable on Linux.
gui_stat() {
  local value
  value=$(stat -f "$1" "$3" 2>/dev/null) && [[ "$value" =~ ^[0-9]+$ ]] && { echo "$value"; return; }
  value=$(stat -c "$2" "$3" 2>/dev/null) && [[ "$value" =~ ^[0-9]+$ ]] && { echo "$value"; return; }
  return 1
}
age_s() {
  local m
  m=$(gui_stat %m %Y "$LOCK") || { echo 0; return; }
  echo $(( $(date +%s) - m ))
}
stale() {
  [ -f "$LOCK" ] || return 0
  local p; p=$(holder_pid)
  # ps, not kill -0: another user's live PID must not appear dead on EPERM.
  if [ -n "$p" ] && ! ps -p "$p" >/dev/null 2>&1; then return 0; fi
  [ "$(age_s)" -gt "$STALE_S" ]
}

# Runtime-tagged states; only this pure function advances a GUI lease attempt.
# Snapshot: Free | Stale | Mine | Other. Terminal replies cannot become grants later.
gui_transition() {
  case "$1:$2" in
    Free:Acquire|Stale:Acquire|Mine:Acquire) echo Taking ;;
    Other:Acquire) echo Waiting ;;
    Taking:Stored) echo Granted ;;
    Taking:Failed|Taking:OwnerGone|Releasing:Failed) echo Failed ;;
    Free:Release) echo Free ;;
    Mine:Release|Stale:Release) echo Releasing ;;
    Other:Release) echo Denied ;;
    Releasing:Removed) echo Released ;;
    Free:Cancelled|Stale:Cancelled|Mine:Cancelled|Other:Cancelled|Waiting:Cancelled|Taking:Cancelled|Releasing:Cancelled|Granted:Cancelled) echo Cancelled ;;
    Granted:*|Released:*|Denied:*|Failed:*|Cancelled:*) echo "$1" ;;
    *) return 2 ;;
  esac
}

gui_snapshot() {
  if [ ! -f "$LOCK" ]; then echo Free
  elif stale; then echo Stale
  elif [ "$(holder)" = "$owner" ]; then echo Mine
  else echo Other; fi
}

# Permanent mutex inode, separate from the user-visible lease file that release removes.
# The kernel releases this short lock on process exit; no PID/age-based mutex reclamation.
prepare_gui_mutex() {
  mkdir -p "$(dirname "$LOCK")" || return 1
  (umask 000; : >> "$LOCK.transition-lock")
}
gui_lock_fd() {
  local wait=${1:-0}
  if command -v lockf >/dev/null 2>&1; then
    if (( wait )); then lockf -s 9; else lockf -s -t 0 9; fi
  elif command -v flock >/dev/null 2>&1; then
    if (( wait )); then flock -x 9; else flock -n 9 || return 75; fi
  else return 69; fi
}

# Owned write identity is captured before rename, so deferred signals can roll
# back precisely that inode. A hard-link backup preserves a renewal's exact record/mtime.
GUI_CLAIM=(None)
GUI_PRIOR_DIR=""
gui_rollback() {
  local inode="" temp="" current
  case "${GUI_CLAIM[0]}" in
    Writing) temp=${GUI_CLAIM[1]}; inode=${GUI_CLAIM[2]} ;;
    Published) inode=${GUI_CLAIM[1]} ;;
    None) return 0 ;;
  esac
  current=$(gui_stat %i %i "$LOCK" || true)
  if [[ -n "$inode" && "$current" == "$inode" ]]; then
    if [[ -n "$GUI_PRIOR_DIR" && -f "$GUI_PRIOR_DIR/lease" ]]; then
      mv -f "$GUI_PRIOR_DIR/lease" "$LOCK" || return 1
    else rm -f "$LOCK" || return 1; fi
  fi
  [[ -z "$temp" ]] || rm -f "$temp"
  GUI_CLAIM=(None)
}
gui_rollback_locked() {
  gui_lock_fd 1 || return 1
  local gui_mutex_held=1
  gui_rollback
} 9< "$LOCK.transition-lock"

gui_cleanup() {
  local rc=$1
  if (( rc != 0 )) && [[ "${GUI_CLAIM[0]}" != None ]]; then
    if [[ "${gui_mutex_held:-0}" == 1 ]]; then
      gui_rollback || { echo "gui: lease rollback failed" >&2; return 1; }
    elif ! gui_rollback_locked; then echo "gui: lease rollback failed" >&2; return 1; fi
  fi
  [[ -z "$GUI_PRIOR_DIR" ]] || rm -rf "$GUI_PRIOR_DIR"
  return 0
}
gui_signal() {
  local signal=$1
  trap '' TERM INT HUP
  state=$(gui_transition "${state:-Free}" Cancelled) || state=Failed
  gui_cleanup 143
  trap - EXIT
  trap - "$signal"
  kill -s "$signal" "$$"  # preserve the original GUI command's signal exit status
  exit 143
}

take() {
  local temp="$LOCK.$$" inode
  if [[ -f "$LOCK" ]]; then
    GUI_PRIOR_DIR=$(mktemp -d "$LOCK.previous-$$-XXXXXX") || return 1
    ln "$LOCK" "$GUI_PRIOR_DIR/lease" || return 1
  fi
  printf 'owner=%s pid=%s since=%s\n' "$owner" "${GUI_LOCK_PID:-$PPID}" "$(date -u +%FT%TZ)" > "$temp" || return 1
  inode=$(gui_stat %i %i "$temp") || { rm -f "$temp"; return 1; }
  GUI_CLAIM=(Writing "$temp" "$inode")
  mv -f "$temp" "$LOCK" || return 1
  GUI_CLAIM=(Published "$inode")
  chmod 666 "$LOCK" 2>/dev/null || true
}

# Effects run in the requester shell, not $(...). Function-scoped FD9 is restored
# before any caller waits; signal cleanup can see ownership before a deferred trap.
gui_attempt() {
  local event=$1 rc=0 pid
  gui_lock_fd "$([[ "$event" == Release ]] && echo 1 || echo 0)" || rc=$?
  case "$rc" in 0) ;; 75) state=Waiting; return ;; *) state=Failed; return ;; esac
  local gui_mutex_held=1
  state=$(gui_transition "$(gui_snapshot)" "$event") || { state=Failed; return; }
  case "$state" in
    Taking)
      pid=${GUI_LOCK_PID:-$PPID}
      if ! [[ "$pid" =~ ^[0-9]+$ ]] || ! ps -p "$pid" >/dev/null 2>&1; then
        state=$(gui_transition "$state" OwnerGone)
      elif take; then
        if ps -p "$pid" >/dev/null 2>&1; then state=$(gui_transition "$state" Stored)
        else gui_rollback; state=$(gui_transition "$state" OwnerGone); fi
      else gui_rollback; state=$(gui_transition "$state" Failed); fi ;;
    Releasing)
      if rm -f "$LOCK"; then state=$(gui_transition "$state" Removed)
      else state=$(gui_transition "$state" Failed); fi ;;
  esac
} 9< "$LOCK.transition-lock"

[[ "${BASH_SOURCE[0]}" == "$0" ]] || return 0

case "$cmd" in
  acquire)
    [ -n "$owner" ] || { echo "usage: gui-lock.sh acquire <owner> [timeout-s]" >&2; exit 2; }
    trap 'gui_cleanup "$?"' EXIT
    trap 'gui_signal TERM' TERM
    trap 'gui_signal INT' INT
    trap 'gui_signal HUP' HUP
    prepare_gui_mutex || { echo "gui: cannot lock display" >&2; exit 1; }
    timeout=${3:-300}; t0=$(date +%s)
    while :; do
      gui_attempt Acquire || state=Failed
      case "$state" in
        Granted) echo "gui: locked by $owner"; exit 0 ;;
        Failed) echo "gui: cannot lock display" >&2; exit 1 ;;
      esac
      if [ $(( $(date +%s) - t0 )) -ge "$timeout" ]; then
        echo "gui: busy — held by $(holder) (pid $(holder_pid), $(age_s)s)"; exit 1
      fi
      sleep 5
    done ;;
  release)
    [ -n "$owner" ] || { echo "usage: gui-lock.sh release <owner>" >&2; exit 2; }
    [ -f "$LOCK" ] || exit 0
    prepare_gui_mutex || { echo "gui: cannot release display" >&2; exit 1; }
    gui_attempt Release || state=Failed
    case "$state" in
      Free) exit 0 ;;
      Released) echo "gui: released by $owner"; exit 0 ;;
      Denied) echo "gui: not yours — held by $(holder)"; exit 1 ;;
      *) echo "gui: cannot release display" >&2; exit 1 ;;
    esac ;;
  status)
    if [ -f "$LOCK" ]; then
      if stale; then echo "stale: $(cat "$LOCK") ($(age_s)s)"; else echo "held: $(cat "$LOCK") ($(age_s)s)"; fi
    else echo free; fi ;;
  *) echo "usage: gui-lock.sh acquire <owner> [timeout-s] | release <owner> | status" >&2; exit 2 ;;
esac
