#!/bin/bash
# Linux sweep: one line per Arbos process — is it running a file that has lost its name, and which build is it REALLY?
#   GONE  = readlink /proc/<pid>/exe ends in " (deleted)": the file was replaced or moved under it (both cases; see the note).
#   runs  = the build the running image itself reports (`/proc/<pid>/exe --version` executes the image the process holds,
#           even when its name is gone), NOT kernel.json — a place that once ran a newer kernel keeps a runtime/kernel.json
#           that names the wrong build for an older kernel serving it now (finding 3, 2026-09-17).
#   disk  = mtime of the file now at the start path (argv[0] when absolute, else the exe path), so "gone since" is on the line.
# Home: the store-watch branch (git show origin/store-watch:sweep.sh). Scope: this user's processes unless run as root.
for p in $(pgrep -x arbos-kernel; pgrep -x arbos-hub); do
  exe=$(readlink /proc/$p/exe 2>/dev/null) || continue          # a zombie has no exe: skip
  file=${exe% (deleted)}
  case "$exe" in *"(deleted)") st=GONE;; *) st=ok;; esac
  set -- $(tr '\0' ' ' < /proc/$p/cmdline); role=$2; place=$3
  case "$1" in /*) file=$1;; esac
  runs=$(timeout 5 /proc/$p/exe --version 2>/dev/null | awk '{print $3}')
  [ -n "$runs" ] || runs=$(sed -n 's/.*"git_sha": *"\([0-9a-f]*\)".*/\1/p' "$place/.arbos/runtime/kernel.json" "$place/.arbos/kernel.json" 2>/dev/null | head -1)
  disk=$( [ -f "$file" ] && stat -c %y "$file" | cut -c1-16 || echo missing)
  printf '%-4s pid %-8s since %-16s runs %-12s %s %s  disk:%s\n' "$st" "$p" "$(ps -o lstart= -p $p | awk '{print $2,$3,$4}')" "${runs:--}" "$role" "$place" "$disk"
done
